use crate::core::{GuardError, GuardResult};
use crate::integration::model::{
    CredentialPurpose, CredentialStatus, Integration, IntegrationAudit, IntegrationCredential,
    IntegrationHttpConfig, IntegrationMapping, IntegrationMqttConfig, IntegrationTransport,
};
#[cfg(feature = "db-mysql")]
use crate::store::mysql::MysqlStore;
#[cfg(feature = "db-sqlite")]
use crate::store::sqlite::SqliteStore;

type IntegrationRow = (
    String,
    String,
    String,
    i64,
    i64,
    i64,
    String,
    Option<i64>,
    i64,
    String,
    i64,
    i64,
);
type CredentialRow = (
    String,
    String,
    String,
    String,
    String,
    i64,
    String,
    i64,
    Option<i64>,
    Option<i64>,
    String,
    i64,
    i64,
);
type MappingRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
    i64,
);
type AuditRow = (
    String,
    Option<String>,
    String,
    String,
    String,
    String,
    String,
    i64,
);

macro_rules! impl_integration_store {
    ($store:ty) => {
        impl $store {
            pub async fn list_integrations(&self) -> GuardResult<Vec<Integration>> {
                let rows = base_db::sqlx::query_as::<_, IntegrationRow>(
                    "SELECT integration_id,name,transport,inbound_enabled,outbound_enabled,enabled,scopes,expires_at_ms,config_version,created_by,created_at_ms,updated_at_ms FROM guard_integration ORDER BY created_at_ms DESC,integration_id",
                )
                .fetch_all(self.pool())
                .await
                .map_err(database_error)?;
                rows.into_iter().map(integration_from_row).collect()
            }

            pub async fn get_integration(
                &self,
                integration_id: &str,
            ) -> GuardResult<Option<Integration>> {
                base_db::sqlx::query_as::<_, IntegrationRow>(
                    "SELECT integration_id,name,transport,inbound_enabled,outbound_enabled,enabled,scopes,expires_at_ms,config_version,created_by,created_at_ms,updated_at_ms FROM guard_integration WHERE integration_id=?",
                )
                .bind(integration_id)
                .fetch_optional(self.pool())
                .await
                .map_err(database_error)?
                .map(integration_from_row)
                .transpose()
            }

            pub async fn upsert_integration(&self, value: &Integration) -> GuardResult<()> {
                let scopes = base::serde_json::to_string(&value.scopes)
                    .map_err(|error| GuardError::InvalidConfig(error.to_string()))?;
                let result = base_db::sqlx::query("UPDATE guard_integration SET name=?,transport=?,inbound_enabled=?,outbound_enabled=?,enabled=?,scopes=?,expires_at_ms=?,config_version=?,updated_at_ms=? WHERE integration_id=?")
                    .bind(&value.name).bind(value.transport.as_str()).bind(i64::from(value.inbound_enabled)).bind(i64::from(value.outbound_enabled))
                    .bind(i64::from(value.enabled)).bind(&scopes).bind(value.expires_at_ms).bind(value.config_version).bind(value.updated_at_ms)
                    .bind(&value.integration_id).execute(self.pool()).await.map_err(database_error)?;
                if result.rows_affected() == 0 {
                    base_db::sqlx::query("INSERT INTO guard_integration(integration_id,name,transport,inbound_enabled,outbound_enabled,enabled,scopes,expires_at_ms,config_version,created_by,created_at_ms,updated_at_ms) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)")
                        .bind(&value.integration_id).bind(&value.name).bind(value.transport.as_str()).bind(i64::from(value.inbound_enabled)).bind(i64::from(value.outbound_enabled))
                        .bind(i64::from(value.enabled)).bind(scopes).bind(value.expires_at_ms).bind(value.config_version).bind(&value.created_by)
                        .bind(value.created_at_ms).bind(value.updated_at_ms).execute(self.pool()).await.map_err(database_error)?;
                }
                Ok(())
            }

            pub async fn list_integration_credentials(
                &self,
                integration_id: &str,
            ) -> GuardResult<Vec<IntegrationCredential>> {
                let rows = base_db::sqlx::query_as::<_, CredentialRow>(
                    "SELECT credential_id,access_key,integration_id,purpose,secret_ciphertext,key_version,status,not_before_ms,expires_at_ms,revoked_at_ms,created_by,created_at_ms,updated_at_ms FROM guard_integration_credential WHERE integration_id=? ORDER BY created_at_ms DESC,credential_id",
                )
                .bind(integration_id)
                .fetch_all(self.pool())
                .await
                .map_err(database_error)?;
                rows.into_iter().map(credential_from_row).collect()
            }

            pub async fn find_integration_credential(
                &self,
                access_key: &str,
            ) -> GuardResult<Option<IntegrationCredential>> {
                base_db::sqlx::query_as::<_, CredentialRow>(
                    "SELECT credential_id,access_key,integration_id,purpose,secret_ciphertext,key_version,status,not_before_ms,expires_at_ms,revoked_at_ms,created_by,created_at_ms,updated_at_ms FROM guard_integration_credential WHERE access_key=?",
                )
                .bind(access_key)
                .fetch_optional(self.pool())
                .await
                .map_err(database_error)?
                .map(credential_from_row)
                .transpose()
            }

            pub async fn insert_integration_credential(
                &self,
                value: &IntegrationCredential,
            ) -> GuardResult<()> {
                base_db::sqlx::query("INSERT INTO guard_integration_credential(credential_id,access_key,integration_id,purpose,secret_ciphertext,key_version,status,not_before_ms,expires_at_ms,revoked_at_ms,created_by,created_at_ms,updated_at_ms) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)")
                    .bind(&value.credential_id).bind(&value.access_key).bind(&value.integration_id).bind(value.purpose.as_str()).bind(&value.secret_ciphertext)
                    .bind(value.key_version).bind(value.status.as_str()).bind(value.not_before_ms).bind(value.expires_at_ms).bind(value.revoked_at_ms)
                    .bind(&value.created_by).bind(value.created_at_ms).bind(value.updated_at_ms).execute(self.pool()).await.map_err(database_error)?;
                Ok(())
            }

            pub async fn revoke_integration_credential(
                &self,
                credential_id: &str,
                now_ms: i64,
            ) -> GuardResult<()> {
                let result = base_db::sqlx::query("UPDATE guard_integration_credential SET status='REVOKED',revoked_at_ms=?,updated_at_ms=? WHERE credential_id=? AND status='ACTIVE'")
                    .bind(now_ms).bind(now_ms).bind(credential_id).execute(self.pool()).await.map_err(database_error)?;
                if result.rows_affected() == 0 {
                    return Err(GuardError::NotFound(format!("active credential {credential_id}")));
                }
                Ok(())
            }

            pub async fn get_integration_http_config(
                &self,
                integration_id: &str,
            ) -> GuardResult<Option<IntegrationHttpConfig>> {
                let row = base_db::sqlx::query_as::<_, (String, Option<String>, i64, String, String, i64, i64, i64, i64)>(
                    "SELECT integration_id,callback_url,callback_timeout_ms,private_network_policy,private_network_allowlist,max_attempts,event_ttl_ms,max_response_bytes,updated_at_ms FROM guard_integration_http WHERE integration_id=?",
                ).bind(integration_id).fetch_optional(self.pool()).await.map_err(database_error)?;
                row.map(http_config_from_row).transpose()
            }

            pub async fn upsert_integration_http_config(
                &self,
                value: &IntegrationHttpConfig,
            ) -> GuardResult<()> {
                let allowlist = base::serde_json::to_string(&value.private_network_allowlist)
                    .map_err(|error| GuardError::InvalidConfig(error.to_string()))?;
                let result = base_db::sqlx::query("UPDATE guard_integration_http SET callback_url=?,callback_timeout_ms=?,private_network_policy=?,private_network_allowlist=?,max_attempts=?,event_ttl_ms=?,max_response_bytes=?,updated_at_ms=? WHERE integration_id=?")
                    .bind(&value.callback_url).bind(value.callback_timeout_ms).bind(&value.private_network_policy).bind(&allowlist).bind(value.max_attempts).bind(value.event_ttl_ms)
                    .bind(value.max_response_bytes).bind(value.updated_at_ms).bind(&value.integration_id).execute(self.pool()).await.map_err(database_error)?;
                if result.rows_affected() == 0 {
                    base_db::sqlx::query("INSERT INTO guard_integration_http(integration_id,callback_url,callback_timeout_ms,private_network_policy,private_network_allowlist,max_attempts,event_ttl_ms,max_response_bytes,updated_at_ms) VALUES (?,?,?,?,?,?,?,?,?)")
                        .bind(&value.integration_id).bind(&value.callback_url).bind(value.callback_timeout_ms).bind(&value.private_network_policy).bind(allowlist).bind(value.max_attempts)
                        .bind(value.event_ttl_ms).bind(value.max_response_bytes).bind(value.updated_at_ms).execute(self.pool()).await.map_err(database_error)?;
                }
                Ok(())
            }

            pub async fn get_integration_mqtt_config(
                &self,
                integration_id: &str,
            ) -> GuardResult<Option<IntegrationMqttConfig>> {
                let row = base_db::sqlx::query_as::<_, (String, String, String, String, String, String, i64)>(
                    "SELECT integration_id,protocol_version,allowed_actions,command_topic,result_topic,event_topic_prefix,updated_at_ms FROM guard_integration_mqtt WHERE integration_id=?",
                ).bind(integration_id).fetch_optional(self.pool()).await.map_err(database_error)?;
                row.map(mqtt_config_from_row).transpose()
            }

            pub async fn upsert_integration_mqtt_config(
                &self,
                value: &IntegrationMqttConfig,
            ) -> GuardResult<()> {
                let actions = base::serde_json::to_string(&value.allowed_actions)
                    .map_err(|error| GuardError::InvalidConfig(error.to_string()))?;
                let result = base_db::sqlx::query("UPDATE guard_integration_mqtt SET protocol_version=?,allowed_actions=?,command_topic=?,result_topic=?,event_topic_prefix=?,updated_at_ms=? WHERE integration_id=?")
                    .bind(&value.protocol_version).bind(&actions).bind(&value.command_topic).bind(&value.result_topic).bind(&value.event_topic_prefix)
                    .bind(value.updated_at_ms).bind(&value.integration_id).execute(self.pool()).await.map_err(database_error)?;
                if result.rows_affected() == 0 {
                    base_db::sqlx::query("INSERT INTO guard_integration_mqtt(integration_id,protocol_version,allowed_actions,command_topic,result_topic,event_topic_prefix,updated_at_ms) VALUES (?,?,?,?,?,?,?)")
                        .bind(&value.integration_id).bind(&value.protocol_version).bind(actions).bind(&value.command_topic).bind(&value.result_topic)
                        .bind(&value.event_topic_prefix).bind(value.updated_at_ms).execute(self.pool()).await.map_err(database_error)?;
                }
                Ok(())
            }

            pub async fn list_integration_mappings(
                &self,
                integration_id: &str,
            ) -> GuardResult<Vec<IntegrationMapping>> {
                let rows = base_db::sqlx::query_as::<_, MappingRow>(
                    "SELECT mapping_id,integration_id,direction,source_type,schema_version,destination_kind,destination,payload_profile,enabled,created_at_ms,updated_at_ms FROM guard_integration_mapping WHERE integration_id=? ORDER BY created_at_ms DESC,mapping_id",
                ).bind(integration_id).fetch_all(self.pool()).await.map_err(database_error)?;
                Ok(rows.into_iter().map(mapping_from_row).collect())
            }

            pub async fn upsert_integration_mapping(
                &self,
                value: &IntegrationMapping,
            ) -> GuardResult<()> {
                let result = base_db::sqlx::query("UPDATE guard_integration_mapping SET direction=?,source_type=?,schema_version=?,destination_kind=?,destination=?,payload_profile=?,enabled=?,updated_at_ms=? WHERE mapping_id=? AND integration_id=?")
                    .bind(&value.direction).bind(&value.source_type).bind(&value.schema_version).bind(&value.destination_kind).bind(&value.destination)
                    .bind(&value.payload_profile).bind(i64::from(value.enabled)).bind(value.updated_at_ms).bind(&value.mapping_id).bind(&value.integration_id)
                    .execute(self.pool()).await.map_err(database_error)?;
                if result.rows_affected() == 0 {
                    base_db::sqlx::query("INSERT INTO guard_integration_mapping(mapping_id,integration_id,direction,source_type,schema_version,destination_kind,destination,payload_profile,enabled,created_at_ms,updated_at_ms) VALUES (?,?,?,?,?,?,?,?,?,?,?)")
                        .bind(&value.mapping_id).bind(&value.integration_id).bind(&value.direction).bind(&value.source_type).bind(&value.schema_version)
                        .bind(&value.destination_kind).bind(&value.destination).bind(&value.payload_profile).bind(i64::from(value.enabled))
                        .bind(value.created_at_ms).bind(value.updated_at_ms).execute(self.pool()).await.map_err(database_error)?;
                }
                Ok(())
            }

            pub async fn append_integration_audit(
                &self,
                value: &IntegrationAudit,
            ) -> GuardResult<()> {
                base_db::sqlx::query("INSERT INTO guard_integration_audit(audit_id,integration_id,actor,action,target_id,outcome,detail_summary,created_at_ms) VALUES (?,?,?,?,?,?,?,?)")
                    .bind(&value.audit_id).bind(&value.integration_id).bind(&value.actor).bind(&value.action).bind(&value.target_id)
                    .bind(&value.outcome).bind(&value.detail_summary).bind(value.created_at_ms).execute(self.pool()).await.map_err(database_error)?;
                base_db::sqlx::query("DELETE FROM guard_integration_audit WHERE created_at_ms < ?")
                    .bind(value.created_at_ms.saturating_sub(180 * 24 * 60 * 60 * 1000))
                    .execute(self.pool()).await.map_err(database_error)?;
                let excess = base_db::sqlx::query_scalar::<_, String>("SELECT audit_id FROM guard_integration_audit ORDER BY created_at_ms DESC,audit_id LIMIT ? OFFSET ?")
                    .bind(i64::MAX).bind(10_000_i64).fetch_all(self.pool()).await.map_err(database_error)?;
                for audit_id in excess {
                    base_db::sqlx::query("DELETE FROM guard_integration_audit WHERE audit_id=?")
                        .bind(audit_id).execute(self.pool()).await.map_err(database_error)?;
                }
                Ok(())
            }

            pub async fn list_integration_audits(&self, limit: usize) -> GuardResult<Vec<IntegrationAudit>> {
                let rows = base_db::sqlx::query_as::<_, AuditRow>("SELECT audit_id,integration_id,actor,action,target_id,outcome,detail_summary,created_at_ms FROM guard_integration_audit ORDER BY created_at_ms DESC,audit_id LIMIT ?")
                    .bind(i64::try_from(limit).unwrap_or(i64::MAX)).fetch_all(self.pool()).await.map_err(database_error)?;
                Ok(rows.into_iter().map(audit_from_row).collect())
            }
        }
    };
}

#[cfg(feature = "db-mysql")]
impl_integration_store!(MysqlStore);
#[cfg(feature = "db-sqlite")]
impl_integration_store!(SqliteStore);

fn integration_from_row(row: IntegrationRow) -> GuardResult<Integration> {
    Ok(Integration {
        integration_id: row.0,
        name: row.1,
        transport: IntegrationTransport::parse(&row.2)?,
        inbound_enabled: row.3 != 0,
        outbound_enabled: row.4 != 0,
        enabled: row.5 != 0,
        scopes: base::serde_json::from_str(&row.6)
            .map_err(|error| GuardError::Conflict(format!("invalid stored scopes: {error}")))?,
        expires_at_ms: row.7,
        config_version: row.8,
        created_by: row.9,
        created_at_ms: row.10,
        updated_at_ms: row.11,
    })
}

fn credential_from_row(row: CredentialRow) -> GuardResult<IntegrationCredential> {
    Ok(IntegrationCredential {
        credential_id: row.0,
        access_key: row.1,
        integration_id: row.2,
        purpose: CredentialPurpose::parse(&row.3)?,
        secret_ciphertext: row.4,
        key_version: row.5,
        status: CredentialStatus::parse(&row.6)?,
        not_before_ms: row.7,
        expires_at_ms: row.8,
        revoked_at_ms: row.9,
        created_by: row.10,
        created_at_ms: row.11,
        updated_at_ms: row.12,
    })
}

fn http_config_from_row(
    row: (
        String,
        Option<String>,
        i64,
        String,
        String,
        i64,
        i64,
        i64,
        i64,
    ),
) -> GuardResult<IntegrationHttpConfig> {
    Ok(IntegrationHttpConfig {
        integration_id: row.0,
        callback_url: row.1,
        callback_timeout_ms: row.2,
        private_network_policy: row.3,
        private_network_allowlist: base::serde_json::from_str(&row.4).map_err(|error| {
            GuardError::Conflict(format!("invalid stored HTTP private allowlist: {error}"))
        })?,
        max_attempts: row.5,
        event_ttl_ms: row.6,
        max_response_bytes: row.7,
        updated_at_ms: row.8,
    })
}

fn mqtt_config_from_row(
    row: (String, String, String, String, String, String, i64),
) -> GuardResult<IntegrationMqttConfig> {
    Ok(IntegrationMqttConfig {
        integration_id: row.0,
        protocol_version: row.1,
        allowed_actions: base::serde_json::from_str(&row.2).map_err(|error| {
            GuardError::Conflict(format!("invalid stored MQTT actions: {error}"))
        })?,
        command_topic: row.3,
        result_topic: row.4,
        event_topic_prefix: row.5,
        updated_at_ms: row.6,
    })
}

fn mapping_from_row(row: MappingRow) -> IntegrationMapping {
    IntegrationMapping {
        mapping_id: row.0,
        integration_id: row.1,
        direction: row.2,
        source_type: row.3,
        schema_version: row.4,
        destination_kind: row.5,
        destination: row.6,
        payload_profile: row.7,
        enabled: row.8 != 0,
        created_at_ms: row.9,
        updated_at_ms: row.10,
    }
}

fn audit_from_row(row: AuditRow) -> IntegrationAudit {
    IntegrationAudit {
        audit_id: row.0,
        integration_id: row.1,
        actor: row.2,
        action: row.3,
        target_id: row.4,
        outcome: row.5,
        detail_summary: row.6,
        created_at_ms: row.7,
    }
}

fn database_error(error: impl std::fmt::Display) -> GuardError {
    GuardError::Conflict(format!("integration database operation failed: {error}"))
}
