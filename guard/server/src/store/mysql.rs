use base_db::sqlx::MySqlPool;

use crate::core::{GuardError, GuardResult};
use crate::store::command::HttpCommandClaim;
use crate::store::migration::{
    INTEGRATIONS_V2_COMPATIBILITY_SQL, MYSQL_0003, MYSQL_0003_COLUMNS, MYSQL_0003_INDEXES,
    MYSQL_0004_COLUMNS, MYSQL_0004_INDEXES, MYSQL_MIGRATIONS,
};
use crate::store::model::{OutboxRecord, OutboxRow, outbox_from_row};

#[derive(Debug, Clone)]
pub struct MysqlStore {
    pool: MySqlPool,
}

impl MysqlStore {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }

    pub(crate) fn pool(&self) -> &MySqlPool {
        &self.pool
    }

    pub async fn migrate(&self) -> GuardResult<()> {
        base_db::migration::run_mysql_migrations(&self.pool, &MYSQL_MIGRATIONS[..1])
            .await
            .map_err(database_error)?;
        base_db::sqlx::query(INTEGRATIONS_V2_COMPATIBILITY_SQL)
            .execute(&self.pool)
            .await
            .map_err(database_error)?;
        migrate_integrations_v3(&self.pool).await?;
        migrate_command_idempotency_v4(&self.pool).await?;
        base_db::migration::run_mysql_migrations(&self.pool, &MYSQL_MIGRATIONS[..6])
            .await
            .map_err(database_error)?;
        migrate_mqtt_runtime_schema_cleanup_v8(&self.pool).await?;
        migrate_integration_schema_consolidation_v9(&self.pool).await
    }

    pub async fn due_outbox(&self, now_ms: i64, limit: usize) -> GuardResult<Vec<OutboxRecord>> {
        let rows = base_db::sqlx::query_as::<_, OutboxRow>("SELECT outbox_id,event_id,integration_id,mapping_id,destination_kind,destination,payload,state,attempts,next_attempt_at_ms,last_error,created_at_ms,updated_at_ms,expires_at_ms FROM guard_outbox WHERE state IN ('PENDING','RETRY_WAIT') AND next_attempt_at_ms <= ? ORDER BY next_attempt_at_ms,outbox_id LIMIT ?")
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

    pub async fn delete_outbox(&self, outbox_id: &str) -> GuardResult<()> {
        base_db::sqlx::query("DELETE FROM guard_outbox WHERE outbox_id=?")
            .bind(outbox_id)
            .execute(&self.pool)
            .await
            .map_err(database_error)?;
        Ok(())
    }

    pub async fn cleanup_dead_outbox(
        &self,
        older_than_ms: i64,
        max_per_integration: usize,
    ) -> GuardResult<u64> {
        let mut removed = base_db::sqlx::query(
            "DELETE FROM guard_outbox WHERE state='DEAD' AND updated_at_ms <= ?",
        )
        .bind(older_than_ms)
        .execute(&self.pool)
        .await
        .map_err(database_error)?
        .rows_affected();
        let rows = base_db::sqlx::query_as::<_, (String, String)>(
            "SELECT outbox_id,integration_id FROM guard_outbox WHERE state='DEAD' ORDER BY integration_id,updated_at_ms DESC,outbox_id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        let mut counts = std::collections::HashMap::<String, usize>::new();
        for (outbox_id, integration_id) in rows {
            let count = counts.entry(integration_id).or_default();
            *count = count.saturating_add(1);
            if *count > max_per_integration {
                removed = removed.saturating_add(
                    base_db::sqlx::query("DELETE FROM guard_outbox WHERE outbox_id=?")
                        .bind(outbox_id)
                        .execute(&self.pool)
                        .await
                        .map_err(database_error)?
                        .rows_affected(),
                );
            }
        }
        Ok(removed)
    }

    pub async fn insert_outbox_records(&self, records: &[OutboxRecord]) -> GuardResult<()> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        for record in records {
            insert_outbox_mysql(&mut tx, record).await?;
        }
        tx.commit().await.map_err(database_error)?;
        Ok(())
    }

    pub async fn insert_mapped_outbox_records(&self, records: &[OutboxRecord]) -> GuardResult<()> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        let now_ms = records
            .iter()
            .map(|record| record.created_at_ms)
            .max()
            .unwrap_or_default();
        base_db::sqlx::query("DELETE FROM guard_integration_delivery WHERE expires_at_ms <= ?")
            .bind(now_ms)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        for record in records {
            let expires_at_ms = record
                .expires_at_ms
                .unwrap_or_else(|| record.created_at_ms.saturating_add(259_200_000));
            let claimed = base_db::sqlx::query("INSERT IGNORE INTO guard_integration_delivery(event_id,mapping_id,expires_at_ms,created_at_ms) VALUES (?,?,?,?)")
                .bind(&record.event_id).bind(&record.mapping_id).bind(expires_at_ms).bind(record.created_at_ms)
                .execute(&mut *tx).await.map_err(database_error)?;
            if claimed.rows_affected() > 0 {
                insert_outbox_mysql(&mut tx, record).await?;
            }
        }
        tx.commit().await.map_err(database_error)?;
        Ok(())
    }

    pub async fn outbox_records(&self, limit: usize) -> GuardResult<Vec<OutboxRecord>> {
        let rows = base_db::sqlx::query_as::<_, OutboxRow>(
            "SELECT outbox_id,event_id,integration_id,mapping_id,destination_kind,destination,payload,state,attempts,next_attempt_at_ms,last_error,created_at_ms,updated_at_ms,expires_at_ms FROM guard_outbox ORDER BY created_at_ms DESC,outbox_id LIMIT ?",
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

    pub async fn describe_claimed_command(
        &self,
        command_id: &str,
        integration_id: &str,
        action: &str,
        now_ms: i64,
    ) -> GuardResult<()> {
        let result = base_db::sqlx::query("UPDATE guard_command SET integration_id=?,operation_id=?,action=?,state='CLAIMED',updated_at_ms=? WHERE command_id=?")
            .bind(integration_id)
            .bind(command_id)
            .bind(action)
            .bind(now_ms)
            .bind(command_id)
            .execute(&self.pool)
            .await
            .map_err(database_error)?;
        if result.rows_affected() == 0 {
            return Err(GuardError::NotFound(format!("command {command_id}")));
        }
        Ok(())
    }

    pub async fn complete_command(
        &self,
        command_id: &str,
        state: &str,
        now_ms: i64,
    ) -> GuardResult<()> {
        let result = base_db::sqlx::query(
            "UPDATE guard_command SET state=?,updated_at_ms=? WHERE command_id=?",
        )
        .bind(state)
        .bind(now_ms)
        .bind(command_id)
        .execute(&self.pool)
        .await
        .map_err(database_error)?;
        if result.rows_affected() == 0 {
            return Err(GuardError::NotFound(format!("command {command_id}")));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn claim_http_command(
        &self,
        command_id: &str,
        integration_id: &str,
        operation_id: &str,
        action: &str,
        request_hash: &str,
        expires_at_ms: i64,
        now_ms: i64,
    ) -> GuardResult<HttpCommandClaim> {
        let mut tx = self.pool.begin().await.map_err(database_error)?;
        base_db::sqlx::query("DELETE FROM guard_command WHERE expires_at_ms < ?")
            .bind(now_ms)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        let existing = base_db::sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                String,
                String,
                Option<i64>,
                Option<Vec<u8>>,
            ),
        >("SELECT integration_id,operation_id,action,request_hash,state,http_status,response_body FROM guard_command WHERE command_id=?")
        .bind(command_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?;
        if let Some((
            stored_integration,
            stored_operation,
            stored_action,
            stored_hash,
            state,
            status,
            body,
        )) = existing
        {
            tx.rollback().await.map_err(database_error)?;
            if stored_integration != integration_id
                || stored_action != action
                || stored_hash != request_hash
            {
                return Err(GuardError::Conflict(
                    "request id was already used with different request content".to_string(),
                ));
            }
            return if state == "COMPLETED" {
                Ok(HttpCommandClaim::Completed {
                    operation_id: stored_operation,
                    status: u16::try_from(status.unwrap_or(500)).unwrap_or(500),
                    response_body: body.unwrap_or_default(),
                })
            } else {
                Ok(HttpCommandClaim::Pending {
                    operation_id: stored_operation,
                })
            };
        }
        base_db::sqlx::query("INSERT INTO guard_command(command_id,integration_id,operation_id,action,state,request_hash,expires_at_ms,created_at_ms,updated_at_ms) VALUES (?,?,?,?,?,?,?,?,?)")
            .bind(command_id)
            .bind(integration_id)
            .bind(operation_id)
            .bind(action)
            .bind("CLAIMED")
            .bind(request_hash)
            .bind(expires_at_ms)
            .bind(now_ms)
            .bind(now_ms)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        Ok(HttpCommandClaim::Claimed {
            command_id: command_id.to_string(),
            operation_id: operation_id.to_string(),
        })
    }

    pub async fn complete_http_command(
        &self,
        command_id: &str,
        status: u16,
        response_body: &[u8],
        now_ms: i64,
    ) -> GuardResult<()> {
        let result = base_db::sqlx::query("UPDATE guard_command SET state='COMPLETED',http_status=?,response_body=?,updated_at_ms=? WHERE command_id=? AND state='CLAIMED'")
            .bind(i64::from(status))
            .bind(response_body)
            .bind(now_ms)
            .bind(command_id)
            .execute(&self.pool)
            .await
            .map_err(database_error)?;
        if result.rows_affected() == 0 {
            return Err(GuardError::Conflict(
                "HTTP idempotency command is not claimable".to_string(),
            ));
        }
        Ok(())
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
        let row = base_db::sqlx::query_as::<_, OutboxRow>("SELECT outbox_id,event_id,integration_id,mapping_id,destination_kind,destination,payload,state,attempts,next_attempt_at_ms,last_error,created_at_ms,updated_at_ms,expires_at_ms FROM guard_outbox WHERE outbox_id=?")
            .bind(outbox_id).fetch_optional(&self.pool).await.map_err(database_error)?
            .ok_or_else(|| GuardError::NotFound(format!("outbox {outbox_id}")))?;
        outbox_from_row(row)
    }
    pub async fn list_user_profiles(&self) -> GuardResult<Vec<crate::auth::UserProfile>> {
        let rows = base_db::sqlx::query_as::<_, (String, String, String, i64, Option<i64>, i64, i64)>(
            "SELECT username,role,nickname,enabled,expires_at_ms,created_at_ms,updated_at_ms FROM guard_user ORDER BY username",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        rows.into_iter()
            .map(
                |(
                    username,
                    role,
                    nickname,
                    enabled,
                    expires_at_ms,
                    created_at_ms,
                    updated_at_ms,
                )| {
                    Ok(crate::auth::UserProfile {
                        username,
                        role: crate::auth::Role::parse(&role)?,
                        nickname,
                        enabled: enabled != 0,
                        expires_at_ms,
                        created_at_ms,
                        updated_at_ms,
                    })
                },
            )
            .collect()
    }

    pub async fn load_user(&self, username: &str) -> GuardResult<Option<crate::auth::UserAccount>> {
        let row = base_db::sqlx::query_as::<_, (String, String, String, String, Option<i64>)>(
            "SELECT username,role,nickname,password_hash,expires_at_ms FROM guard_user WHERE username=? AND enabled=1",
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await
        .map_err(database_error)?;
        row.map(|(username, role, nickname, hash, expires_at_ms)| {
            Ok(crate::auth::UserAccount::with_nickname_and_expiration(
                username,
                crate::auth::Role::parse(&role)?,
                nickname,
                hash,
                expires_at_ms,
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
        access: crate::auth::UserAccess,
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
        let enabled = if access.enabled { 1_i64 } else { 0_i64 };
        let nickname = nickname
            .map(str::trim)
            .map(str::to_string)
            .or_else(|| existing_nickname.clone())
            .unwrap_or_default();
        match (existing_nickname.is_some(), password_hash) {
            (true, Some(hash)) => {
                base_db::sqlx::query("UPDATE guard_user SET role=?,password_hash=?,nickname=?,enabled=?,expires_at_ms=?,updated_at_ms=? WHERE username=?")
                    .bind(role.as_str())
                    .bind(hash)
                    .bind(&nickname)
                    .bind(enabled)
                    .bind(access.expires_at_ms)
                    .bind(now_ms)
                    .bind(username)
                    .execute(&mut *tx)
                    .await
                    .map_err(database_error)?;
            }
            (true, None) => {
                base_db::sqlx::query(
                    "UPDATE guard_user SET role=?,nickname=?,enabled=?,expires_at_ms=?,updated_at_ms=? WHERE username=?",
                )
                .bind(role.as_str())
                .bind(&nickname)
                .bind(enabled)
                .bind(access.expires_at_ms)
                .bind(now_ms)
                .bind(username)
                .execute(&mut *tx)
                .await
                .map_err(database_error)?;
            }
            (false, Some(hash)) => {
                base_db::sqlx::query("INSERT INTO guard_user(username,role,password_hash,nickname,enabled,expires_at_ms,created_at_ms,updated_at_ms) VALUES (?,?,?,?,?,?,?,?)")
                    .bind(username)
                    .bind(role.as_str())
                    .bind(hash)
                    .bind(&nickname)
                    .bind(enabled)
                    .bind(access.expires_at_ms)
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
        let rows = base_db::sqlx::query_as::<_, (String, String, String, String, Option<i64>)>(
            "SELECT username,role,nickname,password_hash,expires_at_ms FROM guard_user WHERE enabled=1 ORDER BY username",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(database_error)?;
        rows.into_iter()
            .map(|(username, role, nickname, hash, expires_at_ms)| {
                Ok(crate::auth::UserAccount::with_nickname_and_expiration(
                    username,
                    crate::auth::Role::parse(&role)?,
                    nickname,
                    hash,
                    expires_at_ms,
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

async fn migrate_integrations_v3(pool: &MySqlPool) -> GuardResult<()> {
    let existing = base_db::sqlx::query_scalar::<_, String>(
        "SELECT name FROM _base_db_migrations WHERE version=3",
    )
    .fetch_optional(pool)
    .await
    .map_err(database_error)?;
    if existing.is_some() {
        return Ok(());
    }

    base_db::sqlx::raw_sql(MYSQL_0003)
        .execute(pool)
        .await
        .map_err(database_error)?;
    for &(table, column, definition) in MYSQL_0003_COLUMNS {
        let exists = base_db::sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME=? AND COLUMN_NAME=?)",
        )
        .bind(table)
        .bind(column)
        .fetch_one(pool)
        .await
        .map_err(database_error)?;
        if exists == 0 {
            base_db::sqlx::query(base_db::sqlx::AssertSqlSafe(format!(
                "ALTER TABLE {table} ADD COLUMN {definition}"
            )))
            .execute(pool)
            .await
            .map_err(database_error)?;
        }
    }
    for &(table, index, statement) in MYSQL_0003_INDEXES {
        let exists = base_db::sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM information_schema.STATISTICS WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME=? AND INDEX_NAME=?)",
        )
        .bind(table)
        .bind(index)
        .fetch_one(pool)
        .await
        .map_err(database_error)?;
        if exists == 0 {
            base_db::sqlx::query(statement)
                .execute(pool)
                .await
                .map_err(database_error)?;
        }
    }
    let applied_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    base_db::sqlx::query(
        "INSERT INTO _base_db_migrations(version,name,applied_at_ms) VALUES (3,'guard_integrations',?)",
    )
    .bind(i64::try_from(applied_at_ms).unwrap_or(i64::MAX))
    .execute(pool)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn migrate_command_idempotency_v4(pool: &MySqlPool) -> GuardResult<()> {
    let existing = base_db::sqlx::query_scalar::<_, String>(
        "SELECT name FROM _base_db_migrations WHERE version=4",
    )
    .fetch_optional(pool)
    .await
    .map_err(database_error)?;
    if existing.is_some() {
        return Ok(());
    }

    for &(table, column, definition) in MYSQL_0004_COLUMNS {
        let exists = base_db::sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME=? AND COLUMN_NAME=?)",
        )
        .bind(table)
        .bind(column)
        .fetch_one(pool)
        .await
        .map_err(database_error)?;
        if exists == 0 {
            base_db::sqlx::query(base_db::sqlx::AssertSqlSafe(format!(
                "ALTER TABLE {table} ADD COLUMN {definition}"
            )))
            .execute(pool)
            .await
            .map_err(database_error)?;
        }
    }
    for &(table, index, statement) in MYSQL_0004_INDEXES {
        let exists = base_db::sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM information_schema.STATISTICS WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME=? AND INDEX_NAME=?)",
        )
        .bind(table)
        .bind(index)
        .fetch_one(pool)
        .await
        .map_err(database_error)?;
        if exists == 0 {
            base_db::sqlx::query(statement)
                .execute(pool)
                .await
                .map_err(database_error)?;
        }
    }
    let applied_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    base_db::sqlx::query(
        "INSERT INTO _base_db_migrations(version,name,applied_at_ms) VALUES (4,'guard_command_idempotency',?)",
    )
    .bind(i64::try_from(applied_at_ms).unwrap_or(i64::MAX))
    .execute(pool)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn migrate_mqtt_runtime_schema_cleanup_v8(pool: &MySqlPool) -> GuardResult<()> {
    let existing = base_db::sqlx::query_scalar::<_, String>(
        "SELECT name FROM _base_db_migrations WHERE version=8",
    )
    .fetch_optional(pool)
    .await
    .map_err(database_error)?;
    if existing.is_some() {
        return Ok(());
    }

    let column_exists = base_db::sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME='guard_integration_mqtt' AND COLUMN_NAME='protocol_version')",
    )
    .fetch_one(pool)
    .await
    .map_err(database_error)?;
    if column_exists != 0 {
        base_db::sqlx::query("ALTER TABLE guard_integration_mqtt DROP COLUMN protocol_version")
            .execute(pool)
            .await
            .map_err(database_error)?;
    }
    let applied_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    base_db::sqlx::query(
        "INSERT INTO _base_db_migrations(version,name,applied_at_ms) VALUES (8,'guard_mqtt_runtime_schema_cleanup',?)",
    )
    .bind(i64::try_from(applied_at_ms).unwrap_or(i64::MAX))
    .execute(pool)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn migrate_integration_schema_consolidation_v9(pool: &MySqlPool) -> GuardResult<()> {
    let existing = base_db::sqlx::query_scalar::<_, String>(
        "SELECT name FROM _base_db_migrations WHERE version=9",
    )
    .fetch_optional(pool)
    .await
    .map_err(database_error)?;
    if let Some(existing) = existing {
        if existing != "guard_integration_schema_consolidation" {
            return Err(GuardError::Conflict(format!(
                "migration version 9 is registered as {existing}"
            )));
        }
        return Ok(());
    }

    let integration_slot_table_exists = base_db::sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM information_schema.TABLES WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME='guard_integration_slot')",
    )
    .fetch_one(pool)
    .await
    .map_err(database_error)?;
    let slot_column_nullable = base_db::sqlx::query_scalar::<_, String>(
        "SELECT IS_NULLABLE FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME='guard_integration' AND COLUMN_NAME='slot'",
    )
    .fetch_optional(pool)
    .await
    .map_err(database_error)?;
    if slot_column_nullable.is_none() {
        base_db::sqlx::query("ALTER TABLE guard_integration ADD COLUMN slot VARCHAR(32) NULL")
            .execute(pool)
            .await
            .map_err(database_error)?;
    } else if integration_slot_table_exists != 0 && slot_column_nullable.as_deref() == Some("NO") {
        for table in ["guard_mqtt_runtime_revision", "guard_mqtt_runtime_state"] {
            let constraints = base_db::sqlx::query_scalar::<_, String>(
                "SELECT CONSTRAINT_NAME FROM information_schema.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME=? AND COLUMN_NAME='slot' AND REFERENCED_TABLE_NAME='guard_integration'",
            )
            .bind(table)
            .fetch_all(pool)
            .await
            .map_err(database_error)?;
            for constraint in constraints {
                if !constraint
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
                {
                    return Err(GuardError::Conflict(format!(
                        "unsafe MySQL foreign key name {constraint}"
                    )));
                }
                base_db::sqlx::query(base_db::sqlx::AssertSqlSafe(format!(
                    "ALTER TABLE {table} DROP FOREIGN KEY {constraint}"
                )))
                .execute(pool)
                .await
                .map_err(database_error)?;
            }
        }
        base_db::sqlx::query(
            "ALTER TABLE guard_integration MODIFY COLUMN slot VARCHAR(32) NULL DEFAULT NULL",
        )
        .execute(pool)
        .await
        .map_err(database_error)?;
    }
    if integration_slot_table_exists != 0 {
        base_db::sqlx::query(
            "UPDATE guard_integration AS integration LEFT JOIN guard_integration_slot AS integration_slot ON integration_slot.integration_id=integration.integration_id SET integration.slot=integration_slot.slot",
        )
        .execute(pool)
        .await
        .map_err(database_error)?;
    }

    let slot_index_exists = base_db::sqlx::query_scalar::<_, i64>(
        "SELECT EXISTS(SELECT 1 FROM information_schema.STATISTICS WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME='guard_integration' AND INDEX_NAME='idx_guard_integration_slot')",
    )
    .fetch_one(pool)
    .await
    .map_err(database_error)?;
    if slot_index_exists == 0 {
        base_db::sqlx::query(
            "CREATE UNIQUE INDEX idx_guard_integration_slot ON guard_integration(slot)",
        )
        .execute(pool)
        .await
        .map_err(database_error)?;
    }

    for table in ["guard_mqtt_runtime_revision", "guard_mqtt_runtime_state"] {
        let constraints = base_db::sqlx::query_scalar::<_, String>(
            "SELECT CONSTRAINT_NAME FROM information_schema.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME=? AND COLUMN_NAME='slot' AND REFERENCED_TABLE_NAME='guard_integration_slot'",
        )
        .bind(table)
        .fetch_all(pool)
        .await
        .map_err(database_error)?;
        for constraint in constraints {
            if !constraint
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
            {
                return Err(GuardError::Conflict(format!(
                    "unsafe MySQL foreign key name {constraint}"
                )));
            }
            base_db::sqlx::query(base_db::sqlx::AssertSqlSafe(format!(
                "ALTER TABLE {table} DROP FOREIGN KEY {constraint}"
            )))
            .execute(pool)
            .await
            .map_err(database_error)?;
        }
    }

    for (table, constraint) in [
        (
            "guard_mqtt_runtime_revision",
            "fk_guard_mqtt_revision_integration_slot",
        ),
        (
            "guard_mqtt_runtime_state",
            "fk_guard_mqtt_state_integration_slot",
        ),
    ] {
        let foreign_key_exists = base_db::sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM information_schema.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME=? AND COLUMN_NAME='slot' AND REFERENCED_TABLE_NAME='guard_integration' AND REFERENCED_COLUMN_NAME='slot')",
        )
        .bind(table)
        .fetch_one(pool)
        .await
        .map_err(database_error)?;
        if foreign_key_exists == 0 {
            base_db::sqlx::query(base_db::sqlx::AssertSqlSafe(format!(
                "ALTER TABLE {table} ADD CONSTRAINT {constraint} FOREIGN KEY (slot) REFERENCES guard_integration(slot)"
            )))
            .execute(pool)
            .await
            .map_err(database_error)?;
        }
    }

    for table in ["guard_integration_mqtt", "guard_integration_slot"] {
        let table_exists = base_db::sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM information_schema.TABLES WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME=?)",
        )
        .bind(table)
        .fetch_one(pool)
        .await
        .map_err(database_error)?;
        if table_exists != 0 {
            base_db::sqlx::query(base_db::sqlx::AssertSqlSafe(format!("DROP TABLE {table}")))
                .execute(pool)
                .await
                .map_err(database_error)?;
        }
    }

    let applied_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    base_db::sqlx::query(
        "INSERT INTO _base_db_migrations(version,name,applied_at_ms) VALUES (9,'guard_integration_schema_consolidation',?)",
    )
    .bind(i64::try_from(applied_at_ms).unwrap_or(i64::MAX))
    .execute(pool)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn insert_outbox_mysql(
    tx: &mut base_db::sqlx::Transaction<'_, base_db::sqlx::MySql>,
    record: &OutboxRecord,
) -> GuardResult<()> {
    base_db::sqlx::query("INSERT INTO guard_outbox(outbox_id,event_id,integration_id,mapping_id,destination_kind,destination,payload,state,attempts,next_attempt_at_ms,last_error,created_at_ms,updated_at_ms,expires_at_ms) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(&record.outbox_id).bind(&record.event_id).bind(&record.integration_id).bind(&record.mapping_id).bind(record.destination_kind.as_str()).bind(&record.destination)
        .bind(&record.payload).bind(record.state.as_str()).bind(i64::from(record.attempts)).bind(record.next_attempt_at_ms)
        .bind(&record.last_error).bind(record.created_at_ms).bind(record.updated_at_ms).bind(record.expires_at_ms)
        .execute(&mut **tx).await.map_err(database_error)?;
    Ok(())
}

fn database_error(error: impl std::fmt::Display) -> GuardError {
    GuardError::Conflict(format!("outbox database operation failed: {error}"))
}
