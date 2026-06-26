use base_db::sqlx::SqlitePool;

use crate::core::{GuardError, GuardResult};
use crate::store::migration::MIGRATIONS;
use crate::store::model::{EventRecord, OutboxRecord, OutboxRow, outbox_from_row};

#[derive(Debug, Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn migrate(&self) -> GuardResult<()> {
        base_db::migration::run_sqlite_migrations(&self.pool, MIGRATIONS)
            .await
            .map_err(database_error)
    }

    pub async fn insert_event_with_outbox(
        &self,
        event: &EventRecord,
        records: &[OutboxRecord],
    ) -> GuardResult<bool> {
        validate_records(event, records)?;
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let existing = base_db::sqlx::query_scalar::<_, String>(
            "SELECT event_id FROM guard_event WHERE event_id = ?",
        )
        .bind(&event.event_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?;
        if existing.is_some() {
            tx.rollback().await.map_err(database_error)?;
            return Ok(false);
        }
        base_db::sqlx::query(
            "INSERT INTO guard_event(event_id, topic, priority, payload) VALUES (?, ?, ?, ?)",
        )
        .bind(&event.event_id)
        .bind(&event.topic)
        .bind(i64::from(event.priority))
        .bind(&event.payload)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        for record in records {
            insert_outbox_sqlite(&mut tx, record).await?;
        }
        tx.commit().await.map_err(database_error)?;
        Ok(true)
    }

    pub async fn due_outbox(&self, now_ms: i64, limit: usize) -> GuardResult<Vec<OutboxRecord>> {
        let rows = base_db::sqlx::query_as::<_, OutboxRow>("SELECT outbox_id,event_id,destination_kind,destination,payload,state,attempts,next_attempt_at_ms,last_error,created_at_ms,updated_at_ms FROM guard_outbox WHERE state IN ('PENDING','RETRY_WAIT') AND next_attempt_at_ms <= ? ORDER BY next_attempt_at_ms,outbox_id LIMIT ?")
            .bind(now_ms).bind(i64::try_from(limit).unwrap_or(i64::MAX)).fetch_all(&self.pool).await.map_err(database_error)?;
        rows.into_iter().map(outbox_from_row).collect()
    }

    pub async fn update_outbox(&self, record: &OutboxRecord) -> GuardResult<()> {
        let result = base_db::sqlx::query("UPDATE guard_outbox SET state=?,attempts=?,next_attempt_at_ms=?,last_error=?,updated_at_ms=? WHERE outbox_id=?")
            .bind(record.state.as_str()).bind(i64::from(record.attempts)).bind(record.next_attempt_at_ms)
            .bind(&record.last_error).bind(record.updated_at_ms).bind(&record.outbox_id)
            .execute(&self.pool).await.map_err(database_error)?;
        if result.rows_affected() == 0 {
            return Err(GuardError::NotFound(format!("outbox {}", record.outbox_id)));
        }
        Ok(())
    }

    pub async fn outbox_records(&self, limit: usize) -> GuardResult<Vec<OutboxRecord>> {
        let rows = base_db::sqlx::query_as::<_, OutboxRow>(
            "SELECT outbox_id,event_id,destination_kind,destination,payload,state,attempts,next_attempt_at_ms,last_error,created_at_ms,updated_at_ms FROM guard_outbox ORDER BY created_at_ms DESC,outbox_id LIMIT ?",
        )
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        rows.into_iter().map(outbox_from_row).collect()
    }

    pub async fn claim_command(
        &self,
        command_id: &str,
        expires_at_ms: i64,
        now_ms: i64,
    ) -> GuardResult<bool> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        base_db::sqlx::query("DELETE FROM guard_command WHERE expires_at_ms < ?")
            .bind(now_ms)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        let existing = base_db::sqlx::query_scalar::<_, String>(
            "SELECT command_id FROM guard_command WHERE command_id = ?",
        )
        .bind(command_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?;
        if existing.is_some() {
            tx.rollback().await.map_err(database_error)?;
            return Ok(false);
        }
        base_db::sqlx::query(
            "INSERT INTO guard_command(command_id,expires_at_ms,created_at_ms) VALUES (?,?,?)",
        )
        .bind(command_id)
        .bind(expires_at_ms)
        .bind(now_ms)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        Ok(true)
    }

    pub async fn recover_stale_sending(
        &self,
        stale_before_ms: i64,
        now_ms: i64,
    ) -> GuardResult<u64> {
        let result = base_db::sqlx::query(
            "UPDATE guard_outbox SET state='RETRY_WAIT',next_attempt_at_ms=?,last_error='delivery interrupted before completion',updated_at_ms=? WHERE state='SENDING' AND updated_at_ms <= ?",
        )
        .bind(now_ms)
        .bind(now_ms)
        .bind(stale_before_ms)
        .execute(&self.pool)
        .await
        .map_err(database_error)?;
        Ok(result.rows_affected())
    }

    pub async fn retry_dead_outbox(
        &self,
        outbox_id: &str,
        now_ms: i64,
    ) -> GuardResult<OutboxRecord> {
        let result = base_db::sqlx::query("UPDATE guard_outbox SET state='PENDING',attempts=0,next_attempt_at_ms=?,last_error=NULL,updated_at_ms=? WHERE outbox_id=? AND state='DEAD'")
            .bind(now_ms).bind(now_ms).bind(outbox_id).execute(&self.pool).await.map_err(database_error)?;
        if result.rows_affected() == 0 {
            return Err(GuardError::Conflict(format!(
                "outbox {outbox_id} is not dead"
            )));
        }
        self.get_outbox(outbox_id).await
    }

    pub async fn get_outbox(&self, outbox_id: &str) -> GuardResult<OutboxRecord> {
        let row = base_db::sqlx::query_as::<_, OutboxRow>("SELECT outbox_id,event_id,destination_kind,destination,payload,state,attempts,next_attempt_at_ms,last_error,created_at_ms,updated_at_ms FROM guard_outbox WHERE outbox_id=?")
            .bind(outbox_id).fetch_optional(&self.pool).await.map_err(database_error)?
            .ok_or_else(|| GuardError::NotFound(format!("outbox {outbox_id}")))?;
        outbox_from_row(row)
    }
    pub async fn list_user_profiles(&self) -> GuardResult<Vec<crate::auth::UserProfile>> {
        let rows = base_db::sqlx::query_as::<_, (String, String, String, i64, i64, i64)>(
            "SELECT username,role,nickname,enabled,created_at_ms,updated_at_ms FROM guard_user ORDER BY username",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        rows.into_iter()
            .map(
                |(username, role, nickname, enabled, created_at_ms, updated_at_ms)| {
                    Ok(crate::auth::UserProfile {
                        username,
                        role: crate::auth::Role::parse(&role)?,
                        nickname,
                        enabled: enabled != 0,
                        created_at_ms,
                        updated_at_ms,
                    })
                },
            )
            .collect()
    }

    pub async fn load_user(&self, username: &str) -> GuardResult<Option<crate::auth::UserAccount>> {
        let row = base_db::sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT username,role,nickname,password_hash FROM guard_user WHERE username=? AND enabled=1",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?;
        row.map(|(username, role, nickname, hash)| {
            Ok(crate::auth::UserAccount::with_nickname(
                username,
                crate::auth::Role::parse(&role)?,
                nickname,
                hash,
            ))
        })
        .transpose()
    }

    pub async fn upsert_user(
        &self,
        username: &str,
        role: crate::auth::Role,
        password_hash: Option<&str>,
        nickname: Option<&str>,
        enabled: bool,
        now_ms: i64,
    ) -> GuardResult<()> {
        if username.trim().is_empty() {
            return Err(GuardError::InvalidConfig(
                "username is required".to_string(),
            ));
        }
        if let Some(hash) = password_hash {
            crate::auth::UserAccount::new(username, role, hash).validate_password_hash()?;
        }
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let existing_nickname = base_db::sqlx::query_scalar::<_, String>(
            "SELECT nickname FROM guard_user WHERE username=?",
        )
        .bind(username)
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?;
        let enabled = if enabled { 1_i64 } else { 0_i64 };
        let nickname = nickname
            .map(str::trim)
            .map(str::to_string)
            .or_else(|| existing_nickname.clone())
            .unwrap_or_default();
        match (existing_nickname.is_some(), password_hash) {
            (true, Some(hash)) => {
                base_db::sqlx::query("UPDATE guard_user SET role=?,password_hash=?,nickname=?,enabled=?,updated_at_ms=? WHERE username=?")
                    .bind(role.as_str())
                    .bind(hash)
                    .bind(&nickname)
                    .bind(enabled)
                    .bind(now_ms)
                    .bind(username)
                    .execute(&mut *tx)
                    .await
                    .map_err(database_error)?;
            }
            (true, None) => {
                base_db::sqlx::query(
                    "UPDATE guard_user SET role=?,nickname=?,enabled=?,updated_at_ms=? WHERE username=?",
                )
                .bind(role.as_str())
                .bind(&nickname)
                .bind(enabled)
                .bind(now_ms)
                .bind(username)
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
            }
            (false, Some(hash)) => {
                base_db::sqlx::query("INSERT INTO guard_user(username,role,password_hash,nickname,enabled,created_at_ms,updated_at_ms) VALUES (?,?,?,?,?,?,?)")
                    .bind(username)
                    .bind(role.as_str())
                    .bind(hash)
                    .bind(&nickname)
                    .bind(enabled)
                    .bind(now_ms)
                    .bind(now_ms)
                    .execute(&mut *tx)
                    .await
                    .map_err(database_error)?;
            }
            (false, None) => {
                tx.rollback().await.map_err(database_error)?;
                return Err(GuardError::InvalidConfig(
                    "password is required for new UI users".to_string(),
                ));
            }
        }
        tx.commit().await.map_err(database_error)?;
        Ok(())
    }

    pub async fn load_users(&self) -> GuardResult<Vec<crate::auth::UserAccount>> {
        let rows = base_db::sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT username,role,nickname,password_hash FROM guard_user WHERE enabled=1 ORDER BY username",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        rows.into_iter()
            .map(|(username, role, nickname, hash)| {
                Ok(crate::auth::UserAccount::with_nickname(
                    username,
                    crate::auth::Role::parse(&role)?,
                    nickname,
                    hash,
                ))
            })
            .collect()
    }

    pub async fn bootstrap_admin(&self, username: &str, password_hash: &str) -> GuardResult<bool> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let count = base_db::sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM guard_user")
            .fetch_one(&mut *tx)
            .await
            .map_err(database_error)?;
        if count != 0 {
            tx.rollback().await.map_err(database_error)?;
            return Ok(false);
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64;
        base_db::sqlx::query("INSERT INTO guard_user(username,role,password_hash,enabled,created_at_ms,updated_at_ms) VALUES (?,?,?,?,?,?)")
            .bind(username)
            .bind(crate::auth::Role::Admin.as_str())
            .bind(password_hash)
            .bind(1_i64)
            .bind(now_ms)
            .bind(now_ms)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        Ok(true)
    }
}

async fn insert_outbox_sqlite(
    tx: &mut base_db::sqlx::Transaction<'_, base_db::sqlx::Sqlite>,
    record: &OutboxRecord,
) -> GuardResult<()> {
    base_db::sqlx::query("INSERT INTO guard_outbox(outbox_id,event_id,destination_kind,destination,payload,state,attempts,next_attempt_at_ms,last_error,created_at_ms,updated_at_ms) VALUES (?,?,?,?,?,?,?,?,?,?,?)")
        .bind(&record.outbox_id).bind(&record.event_id).bind(record.destination_kind.as_str()).bind(&record.destination)
        .bind(&record.payload).bind(record.state.as_str()).bind(i64::from(record.attempts)).bind(record.next_attempt_at_ms)
        .bind(&record.last_error).bind(record.created_at_ms).bind(record.updated_at_ms)
        .execute(&mut **tx).await.map_err(database_error)?;
    Ok(())
}

fn validate_records(event: &EventRecord, records: &[OutboxRecord]) -> GuardResult<()> {
    if records.iter().any(|record| {
        record.event_id != event.event_id
            || record.outbox_id.is_empty()
            || record.destination.is_empty()
    }) {
        return Err(GuardError::InvalidConfig(
            "outbox records must match event and have ids/destinations".to_string(),
        ));
    }
    Ok(())
}

fn database_error(error: impl std::fmt::Display) -> GuardError {
    GuardError::Conflict(format!("outbox database operation failed: {error}"))
}
