use base_db::{
    dbx::{
        DatabasePoolConfig,
        sqlitex::{SqliteConnectionConfig, build_sqlite_pool},
    },
    sqlx::{Row, SqlitePool},
};
use gmv_protocol::steward::v1::{DeliveryReceipt, DeliveryState, InventorySnapshot};
use prost::Message;
use std::{path::Path, time::Duration};

#[derive(Clone)]
pub struct StateStore {
    pool: SqlitePool,
}

impl StateStore {
    pub async fn open(path: &Path) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let pool = build_sqlite_pool(
            SqliteConnectionConfig::new(path),
            DatabasePoolConfig {
                max_size: 4,
                min_idle: Some(1),
                connection_timeout: Duration::from_secs(8),
                ..DatabasePoolConfig::default()
            },
        )?;
        let store = Self { pool };
        store.initialize().await?;
        Ok(store)
    }

    async fn initialize(&self) -> Result<(), base_db::sqlx::Error> {
        for statement in [
            "CREATE TABLE IF NOT EXISTS delivery (assignment_id TEXT PRIMARY KEY, artifact_id TEXT NOT NULL, revision TEXT NOT NULL, sha256 TEXT NOT NULL, state INTEGER NOT NULL, stable_error_code TEXT NOT NULL, observed_at_epoch_ms INTEGER NOT NULL)",
            "CREATE TABLE IF NOT EXISTS receipt_outbox (message_id TEXT PRIMARY KEY, assignment_id TEXT NOT NULL, payload BLOB NOT NULL, created_at_epoch_ms INTEGER NOT NULL, sent_at_epoch_ms INTEGER)",
            "CREATE TABLE IF NOT EXISTS command_journal (command_id TEXT PRIMARY KEY, command_type INTEGER NOT NULL, state INTEGER NOT NULL, result BLOB NOT NULL, completed_at_epoch_ms INTEGER NOT NULL)",
            "CREATE TABLE IF NOT EXISTS inventory_observation (singleton INTEGER PRIMARY KEY CHECK (singleton = 1), payload BLOB NOT NULL, observed_at_epoch_ms INTEGER NOT NULL)",
        ] {
            base_db::sqlx::query(statement).execute(&self.pool).await?;
        }
        Ok(())
    }

    pub async fn notice_delivery(
        &self,
        assignment_id: &str,
        artifact_id: &str,
        revision: &str,
        sha256: &str,
        observed_at_epoch_ms: i64,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let existing = base_db::sqlx::query(
            "SELECT artifact_id, revision, sha256, state FROM delivery WHERE assignment_id = ?",
        )
        .bind(assignment_id)
        .fetch_optional(&self.pool)
        .await?;
        if let Some(row) = existing {
            let matches = row.try_get::<String, _>("artifact_id")? == artifact_id
                && row.try_get::<String, _>("revision")? == revision
                && row.try_get::<String, _>("sha256")? == sha256;
            if !matches {
                return Err(
                    "assignment identity was reused with different artifact content".into(),
                );
            }
            let state = row.try_get::<i32, _>("state")?;
            return Ok(matches!(
                DeliveryState::try_from(state).unwrap_or(DeliveryState::Unspecified),
                DeliveryState::Noticed
                    | DeliveryState::Downloading
                    | DeliveryState::Downloaded
                    | DeliveryState::Verified
            ));
        }
        base_db::sqlx::query(
            "INSERT INTO delivery (assignment_id, artifact_id, revision, sha256, state, stable_error_code, observed_at_epoch_ms) VALUES (?, ?, ?, ?, ?, '', ?)",
        )
        .bind(assignment_id)
        .bind(artifact_id)
        .bind(revision)
        .bind(sha256)
        .bind(DeliveryState::Noticed as i32)
        .bind(observed_at_epoch_ms)
        .execute(&self.pool)
        .await?;
        Ok(true)
    }

    pub async fn record_receipt(
        &self,
        message_id: &str,
        receipt: &DeliveryReceipt,
    ) -> Result<(), base_db::sqlx::Error> {
        let mut transaction = self.pool.begin().await?;
        base_db::sqlx::query(
            "UPDATE delivery SET state = ?, stable_error_code = ?, observed_at_epoch_ms = ? WHERE assignment_id = ?",
        )
        .bind(receipt.state)
        .bind(&receipt.stable_error_code)
        .bind(receipt.observed_at_epoch_ms)
        .bind(&receipt.assignment_id)
        .execute(&mut *transaction)
        .await?;
        base_db::sqlx::query(
            "INSERT OR IGNORE INTO receipt_outbox (message_id, assignment_id, payload, created_at_epoch_ms) VALUES (?, ?, ?, ?)",
        )
        .bind(message_id)
        .bind(&receipt.assignment_id)
        .bind(receipt.encode_to_vec())
        .bind(receipt.observed_at_epoch_ms)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await
    }

    pub async fn pending_receipts(
        &self,
        limit: i64,
    ) -> Result<Vec<(String, DeliveryReceipt)>, Box<dyn std::error::Error + Send + Sync>> {
        let rows = base_db::sqlx::query(
            "SELECT message_id, payload FROM receipt_outbox WHERE sent_at_epoch_ms IS NULL ORDER BY created_at_epoch_ms LIMIT ?",
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let message_id = row.try_get::<String, _>("message_id")?;
                let payload = row.try_get::<Vec<u8>, _>("payload")?;
                Ok((message_id, DeliveryReceipt::decode(payload.as_slice())?))
            })
            .collect()
    }

    pub async fn mark_receipt_sent(
        &self,
        message_id: &str,
        sent_at_epoch_ms: i64,
    ) -> Result<(), base_db::sqlx::Error> {
        base_db::sqlx::query("UPDATE receipt_outbox SET sent_at_epoch_ms = ? WHERE message_id = ?")
            .bind(sent_at_epoch_ms)
            .bind(message_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn save_inventory(
        &self,
        inventory: &InventorySnapshot,
    ) -> Result<(), base_db::sqlx::Error> {
        base_db::sqlx::query(
            "INSERT INTO inventory_observation (singleton, payload, observed_at_epoch_ms) VALUES (1, ?, ?) ON CONFLICT(singleton) DO UPDATE SET payload = excluded.payload, observed_at_epoch_ms = excluded.observed_at_epoch_ms",
        )
        .bind(inventory.encode_to_vec())
        .bind(inventory.observed_at_epoch_ms)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn duplicate_assignment_is_idempotent_but_digest_change_is_rejected() {
        let path = std::env::temp_dir().join(format!("steward-state-{}.db", uuid::Uuid::now_v7()));
        let store = StateStore::open(&path).await.unwrap();
        assert!(
            store
                .notice_delivery("a1", "bundle", "r1", "abc", 1)
                .await
                .unwrap()
        );
        assert!(
            store
                .notice_delivery("a1", "bundle", "r1", "abc", 2)
                .await
                .unwrap()
        );
        store
            .record_receipt(
                "receipt-1",
                &DeliveryReceipt {
                    assignment_id: "a1".to_string(),
                    artifact_id: "bundle".to_string(),
                    revision: "r1".to_string(),
                    state: DeliveryState::Staged as i32,
                    observed_at_epoch_ms: 3,
                    ..DeliveryReceipt::default()
                },
            )
            .await
            .unwrap();
        let pending = store.pending_receipts(10).await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, "receipt-1");
        store.mark_receipt_sent("receipt-1", 4).await.unwrap();
        assert!(store.pending_receipts(10).await.unwrap().is_empty());
        assert!(
            !store
                .notice_delivery("a1", "bundle", "r1", "abc", 4)
                .await
                .unwrap()
        );
        assert!(
            store
                .notice_delivery("a1", "bundle", "r1", "def", 5)
                .await
                .is_err()
        );
        store.close().await;
        std::fs::remove_file(path).unwrap();
    }
}
