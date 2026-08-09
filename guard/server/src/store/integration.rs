use crate::core::{GuardError, GuardResult};
use crate::integration::model::{
    CredentialPurpose, CredentialStatus, Integration, IntegrationAudit, IntegrationCredential,
    IntegrationHttpConfig, IntegrationMapping, IntegrationMasterKey, IntegrationMqttConfig,
    IntegrationTransport, MqttRuntimeApplyState, MqttRuntimeConfig, MqttRuntimeRevision,
};
use crate::integration::secret::IntegrationSecretCipher;
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
type MqttRuntimeRevisionRow = (
    i64,
    String,
    String,
    i64,
    String,
    Option<String>,
    Option<String>,
    i64,
    i64,
    String,
    i64,
);
type MqttRuntimeStateRow = (
    i64,
    Option<i64>,
    i64,
    String,
    Option<String>,
    Option<String>,
    i64,
    String,
    i64,
);
type IntegrationMasterKeyRow = (String, i64, i64, String, i64);

macro_rules! impl_integration_store {
    ($store:ty) => {
        impl $store {
            pub async fn get_integration_master_key(
                &self,
            ) -> GuardResult<Option<IntegrationMasterKey>> {
                base_db::sqlx::query_as::<_, IntegrationMasterKeyRow>(
                    "SELECT key_material,key_version,created_at_ms,updated_by,updated_at_ms FROM guard_integration_master_key WHERE slot='business'",
                )
                .fetch_optional(self.pool())
                .await
                .map_err(database_error)
                .map(|value| value.map(integration_master_key_from_row))
            }

            pub async fn ensure_integration_master_key(
                &self,
                key_material: &str,
                actor: &str,
                now_ms: i64,
            ) -> GuardResult<IntegrationMasterKey> {
                base_db::sqlx::query("INSERT INTO guard_integration_master_key(slot,key_material,key_version,created_at_ms,updated_by,updated_at_ms) SELECT 'business',?,1,?,?,? WHERE NOT EXISTS (SELECT 1 FROM guard_integration_master_key WHERE slot='business')")
                    .bind(key_material).bind(now_ms).bind(actor).bind(now_ms)
                    .execute(self.pool()).await.map_err(database_error)?;
                self.get_integration_master_key().await?.ok_or_else(|| {
                    GuardError::Conflict("integration master key initialization failed".to_string())
                })
            }

            #[allow(clippy::too_many_arguments)]
            pub async fn rotate_integration_master_key(
                &self,
                current_cipher: &IntegrationSecretCipher,
                new_cipher: &IntegrationSecretCipher,
                new_key_material: &str,
                expected_key_version: i64,
                actor: &str,
                audit_id: &str,
                now_ms: i64,
            ) -> GuardResult<IntegrationMasterKey> {
                let mut transaction = self.pool().begin().await.map_err(database_error)?;
                let current = base_db::sqlx::query_as::<_, IntegrationMasterKeyRow>(
                    "SELECT key_material,key_version,created_at_ms,updated_by,updated_at_ms FROM guard_integration_master_key WHERE slot='business'",
                )
                .fetch_optional(&mut *transaction)
                .await
                .map_err(database_error)?
                .map(integration_master_key_from_row)
                .ok_or_else(|| GuardError::Conflict("integration master key is missing".to_string()))?;
                if current.key_version != expected_key_version {
                    return Err(GuardError::Conflict(format!(
                        "integration master key version changed: expected {expected_key_version}, actual {}",
                        current.key_version
                    )));
                }
                let new_key_version = current.key_version.saturating_add(1);
                let credentials = base_db::sqlx::query_as::<_, (String, String)>(
                    "SELECT credential_id,secret_ciphertext FROM guard_integration_credential",
                )
                .fetch_all(&mut *transaction)
                .await
                .map_err(database_error)?;
                for (credential_id, ciphertext) in credentials {
                    let plaintext = current_cipher.decrypt(&ciphertext)?;
                    let replacement = new_cipher.encrypt(&plaintext)?;
                    let result = base_db::sqlx::query("UPDATE guard_integration_credential SET secret_ciphertext=?,key_version=?,updated_at_ms=? WHERE credential_id=? AND secret_ciphertext=?")
                        .bind(replacement).bind(new_key_version).bind(now_ms).bind(&credential_id).bind(&ciphertext)
                        .execute(&mut *transaction).await.map_err(database_error)?;
                    if result.rows_affected() != 1 {
                        return Err(GuardError::Conflict(format!(
                            "integration credential {credential_id} changed during master key rotation"
                        )));
                    }
                }
                let mqtt_passwords = base_db::sqlx::query_as::<_, (i64, String)>(
                    "SELECT revision,password_ciphertext FROM guard_mqtt_runtime_revision WHERE slot='business' AND password_ciphertext IS NOT NULL",
                )
                .fetch_all(&mut *transaction)
                .await
                .map_err(database_error)?;
                for (revision, ciphertext) in mqtt_passwords {
                    let plaintext = current_cipher.decrypt(&ciphertext)?;
                    let replacement = new_cipher.encrypt(&plaintext)?;
                    let result = base_db::sqlx::query("UPDATE guard_mqtt_runtime_revision SET password_ciphertext=? WHERE slot='business' AND revision=? AND password_ciphertext=?")
                        .bind(replacement).bind(revision).bind(&ciphertext)
                        .execute(&mut *transaction).await.map_err(database_error)?;
                    if result.rows_affected() != 1 {
                        return Err(GuardError::Conflict(format!(
                            "MQTT runtime revision {revision} changed during master key rotation"
                        )));
                    }
                }
                let result = base_db::sqlx::query("UPDATE guard_integration_master_key SET key_material=?,key_version=?,updated_by=?,updated_at_ms=? WHERE slot='business' AND key_version=?")
                    .bind(new_key_material).bind(new_key_version).bind(actor).bind(now_ms).bind(expected_key_version)
                    .execute(&mut *transaction).await.map_err(database_error)?;
                if result.rows_affected() != 1 {
                    return Err(GuardError::Conflict(
                        "integration master key changed concurrently".to_string(),
                    ));
                }
                base_db::sqlx::query("INSERT INTO guard_integration_audit(audit_id,integration_id,actor,action,target_id,outcome,detail_summary,created_at_ms) VALUES (?,NULL,?,'master_key.rotate','business','rotated','integration secrets re-encrypted',?)")
                    .bind(audit_id).bind(actor).bind(now_ms)
                    .execute(&mut *transaction).await.map_err(database_error)?;
                transaction.commit().await.map_err(database_error)?;
                self.get_integration_master_key().await?.ok_or_else(|| {
                    GuardError::Conflict("rotated integration master key is missing".to_string())
                })
            }

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
                    "SELECT c.credential_id,c.access_key,c.integration_id,c.purpose,c.secret_ciphertext,c.key_version,c.status,c.not_before_ms,c.expires_at_ms,c.revoked_at_ms,c.created_by,c.created_at_ms,c.updated_at_ms FROM guard_integration_credential c JOIN guard_integration_slot s ON s.slot='business' AND s.integration_id=c.integration_id WHERE c.access_key=?",
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
                let row = base_db::sqlx::query_as::<_, (String, String, String, String, i64)>(
                    "SELECT integration_id,command_topic,result_topic,event_topic_prefix,updated_at_ms FROM guard_integration_mqtt WHERE integration_id=?",
                ).bind(integration_id).fetch_optional(self.pool()).await.map_err(database_error)?;
                row.map(mqtt_config_from_row).transpose()
            }

            pub async fn upsert_integration_mqtt_config(
                &self,
                value: &IntegrationMqttConfig,
            ) -> GuardResult<()> {
                let result = base_db::sqlx::query("UPDATE guard_integration_mqtt SET command_topic=?,result_topic=?,event_topic_prefix=?,updated_at_ms=? WHERE integration_id=?")
                    .bind(&value.command_topic).bind(&value.result_topic).bind(&value.event_topic_prefix)
                    .bind(value.updated_at_ms).bind(&value.integration_id).execute(self.pool()).await.map_err(database_error)?;
                if result.rows_affected() == 0 {
                    base_db::sqlx::query("INSERT INTO guard_integration_mqtt(integration_id,protocol_version,command_topic,result_topic,event_topic_prefix,updated_at_ms) VALUES (?,?,?,?,?,?)")
                        .bind(&value.integration_id).bind("managed").bind(&value.command_topic).bind(&value.result_topic)
                        .bind(&value.event_topic_prefix).bind(value.updated_at_ms).execute(self.pool()).await.map_err(database_error)?;
                }
                Ok(())
            }

            pub async fn business_integration_id(&self) -> GuardResult<Option<String>> {
                base_db::sqlx::query_scalar::<_, Option<String>>(
                    "SELECT integration_id FROM guard_integration_slot WHERE slot='business'",
                )
                .fetch_optional(self.pool())
                .await
                .map_err(database_error)
                .map(Option::flatten)
            }

            pub async fn bind_business_integration(
                &self,
                integration_id: &str,
                actor: &str,
                now_ms: i64,
            ) -> GuardResult<()> {
                if self.get_integration(integration_id).await?.is_none() {
                    return Err(GuardError::NotFound(format!("integration {integration_id}")));
                }
                let result = base_db::sqlx::query(
                    "UPDATE guard_integration_slot SET integration_id=?,updated_by=?,updated_at_ms=? WHERE slot='business'",
                )
                .bind(integration_id)
                .bind(actor)
                .bind(now_ms)
                .execute(self.pool())
                .await
                .map_err(database_error)?;
                if result.rows_affected() == 0 {
                    base_db::sqlx::query("INSERT INTO guard_integration_slot(slot,integration_id,updated_by,updated_at_ms) VALUES ('business',?,?,?)")
                        .bind(integration_id).bind(actor).bind(now_ms).execute(self.pool()).await.map_err(database_error)?;
                }
                Ok(())
            }

            pub async fn integration_transport_switch_blockers(
                &self,
                integration_id: &str,
            ) -> GuardResult<(i64, i64)> {
                let commands = base_db::sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM guard_command WHERE integration_id=? AND state NOT IN ('SUCCEEDED','FAILED','CANCELLED')",
                )
                .bind(integration_id)
                .fetch_one(self.pool())
                .await
                .map_err(database_error)?;
                let outbox = base_db::sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM guard_outbox WHERE integration_id=? AND state IN ('PENDING','SENDING','RETRY_WAIT')",
                )
                .bind(integration_id)
                .fetch_one(self.pool())
                .await
                .map_err(database_error)?;
                Ok((commands, outbox))
            }

            pub async fn deactivate_integration_transport(
                &self,
                integration_id: &str,
                transport: IntegrationTransport,
                now_ms: i64,
            ) -> GuardResult<()> {
                if transport == IntegrationTransport::Http {
                    base_db::sqlx::query("UPDATE guard_integration_credential SET status='REVOKED',revoked_at_ms=?,updated_at_ms=? WHERE integration_id=? AND status='ACTIVE'")
                        .bind(now_ms).bind(now_ms).bind(integration_id).execute(self.pool()).await.map_err(database_error)?;
                }
                base_db::sqlx::query("UPDATE guard_integration_mapping SET enabled=0,updated_at_ms=? WHERE integration_id=? AND enabled<>0")
                    .bind(now_ms).bind(integration_id).execute(self.pool()).await.map_err(database_error)?;
                Ok(())
            }

            pub async fn get_mqtt_runtime_revision(
                &self,
                revision: i64,
            ) -> GuardResult<Option<MqttRuntimeRevision>> {
                base_db::sqlx::query_as::<_, MqttRuntimeRevisionRow>(
                    "SELECT revision,protocol_version,broker,port,client_id,username,password_ciphertext,tls,publish_event_ttl_sec,created_by,created_at_ms FROM guard_mqtt_runtime_revision WHERE slot='business' AND revision=?",
                )
                .bind(revision)
                .fetch_optional(self.pool())
                .await
                .map_err(database_error)?
                .map(mqtt_runtime_revision_from_row)
                .transpose()
            }

            pub async fn get_mqtt_runtime_config(&self) -> GuardResult<Option<MqttRuntimeConfig>> {
                let state = base_db::sqlx::query_as::<_, MqttRuntimeStateRow>(
                    "SELECT desired_revision,active_revision,config_version,apply_state,last_error_code,last_error_summary,last_transition_at_ms,updated_by,updated_at_ms FROM guard_mqtt_runtime_state WHERE slot='business'",
                )
                .fetch_optional(self.pool())
                .await
                .map_err(database_error)?;
                let Some(state) = state else {
                    return Ok(None);
                };
                let revision = self
                    .get_mqtt_runtime_revision(state.0)
                    .await?
                    .ok_or_else(|| GuardError::Conflict("MQTT desired revision is missing".to_string()))?;
                Ok(Some(mqtt_runtime_config_from_parts(revision, state)?))
            }

            pub async fn save_mqtt_runtime_config(
                &self,
                value: &MqttRuntimeRevision,
                expected_config_version: i64,
            ) -> GuardResult<MqttRuntimeConfig> {
                value.validate()?;
                let mut transaction = self.pool().begin().await.map_err(database_error)?;
                let current = base_db::sqlx::query_as::<_, (i64, i64)>(
                    "SELECT desired_revision,config_version FROM guard_mqtt_runtime_state WHERE slot='business'",
                )
                .fetch_optional(&mut *transaction)
                .await
                .map_err(database_error)?;
                let revision = match current {
                    Some((desired_revision, config_version)) => {
                        if config_version != expected_config_version {
                            return Err(GuardError::Conflict(format!(
                                "MQTT config version changed: expected {expected_config_version}, actual {config_version}"
                            )));
                        }
                        desired_revision.saturating_add(1)
                    }
                    None if expected_config_version == 0 => 1,
                    None => {
                        return Err(GuardError::Conflict(format!(
                            "MQTT config version changed: expected {expected_config_version}, actual 0"
                        )));
                    }
                };
                base_db::sqlx::query("INSERT INTO guard_mqtt_runtime_revision(slot,revision,protocol_version,broker,port,client_id,username,password_ciphertext,tls,publish_event_ttl_sec,created_by,created_at_ms) VALUES ('business',?,?,?,?,?,?,?,?,?,?,?)")
                    .bind(revision).bind(&value.protocol_version).bind(&value.broker).bind(i64::from(value.port)).bind(&value.client_id)
                    .bind(&value.username).bind(&value.password_ciphertext).bind(i64::from(value.tls)).bind(value.publish_event_ttl_sec)
                    .bind(&value.created_by).bind(value.created_at_ms).execute(&mut *transaction).await.map_err(database_error)?;
                if current.is_some() {
                    let result = base_db::sqlx::query("UPDATE guard_mqtt_runtime_state SET desired_revision=?,config_version=config_version+1,apply_state='PENDING',last_error_code=NULL,last_error_summary=NULL,last_transition_at_ms=?,updated_by=?,updated_at_ms=? WHERE slot='business' AND config_version=?")
                        .bind(revision).bind(value.created_at_ms).bind(&value.created_by).bind(value.created_at_ms).bind(expected_config_version)
                        .execute(&mut *transaction).await.map_err(database_error)?;
                    if result.rows_affected() == 0 {
                        return Err(GuardError::Conflict("MQTT config changed concurrently".to_string()));
                    }
                } else {
                    base_db::sqlx::query("INSERT INTO guard_mqtt_runtime_state(slot,desired_revision,active_revision,config_version,apply_state,last_error_code,last_error_summary,last_transition_at_ms,updated_by,updated_at_ms) VALUES ('business',?,NULL,1,'PENDING',NULL,NULL,?,?,?)")
                        .bind(revision).bind(value.created_at_ms).bind(&value.created_by).bind(value.created_at_ms)
                        .execute(&mut *transaction).await.map_err(database_error)?;
                }
                transaction.commit().await.map_err(database_error)?;
                self.get_mqtt_runtime_config().await?.ok_or_else(|| {
                    GuardError::Conflict("MQTT runtime configuration was not persisted".to_string())
                })
            }

            pub async fn update_mqtt_runtime_state(
                &self,
                desired_revision: i64,
                active_revision: Option<i64>,
                apply_state: MqttRuntimeApplyState,
                last_error_code: Option<&str>,
                last_error_summary: Option<&str>,
                now_ms: i64,
            ) -> GuardResult<()> {
                let result = base_db::sqlx::query("UPDATE guard_mqtt_runtime_state SET active_revision=?,apply_state=?,last_error_code=?,last_error_summary=?,last_transition_at_ms=? WHERE slot='business' AND desired_revision=?")
                    .bind(active_revision).bind(apply_state.as_str()).bind(last_error_code).bind(last_error_summary).bind(now_ms).bind(desired_revision)
                    .execute(self.pool()).await.map_err(database_error)?;
                if result.rows_affected() == 0 {
                    return Err(GuardError::Conflict("MQTT desired revision changed while applying".to_string()));
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

fn integration_master_key_from_row(row: IntegrationMasterKeyRow) -> IntegrationMasterKey {
    IntegrationMasterKey {
        key_material: row.0,
        key_version: row.1,
        created_at_ms: row.2,
        updated_by: row.3,
        updated_at_ms: row.4,
    }
}

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
    row: (String, String, String, String, i64),
) -> GuardResult<IntegrationMqttConfig> {
    Ok(IntegrationMqttConfig {
        integration_id: row.0,
        command_topic: row.1,
        result_topic: row.2,
        event_topic_prefix: row.3,
        updated_at_ms: row.4,
    })
}

fn mqtt_runtime_revision_from_row(row: MqttRuntimeRevisionRow) -> GuardResult<MqttRuntimeRevision> {
    Ok(MqttRuntimeRevision {
        revision: row.0,
        protocol_version: row.1,
        broker: row.2,
        port: u16::try_from(row.3)
            .map_err(|_| GuardError::Conflict("invalid stored MQTT port".to_string()))?,
        client_id: row.4,
        username: row.5,
        password_ciphertext: row.6,
        tls: row.7 != 0,
        publish_event_ttl_sec: row.8,
        created_by: row.9,
        created_at_ms: row.10,
    })
}

fn mqtt_runtime_config_from_parts(
    revision: MqttRuntimeRevision,
    state: MqttRuntimeStateRow,
) -> GuardResult<MqttRuntimeConfig> {
    Ok(MqttRuntimeConfig {
        protocol_version: revision.protocol_version,
        broker: revision.broker,
        port: revision.port,
        client_id: revision.client_id,
        username: revision.username,
        password_configured: revision.password_ciphertext.is_some(),
        tls: revision.tls,
        publish_event_ttl_sec: revision.publish_event_ttl_sec,
        desired_revision: state.0,
        active_revision: state.1,
        config_version: state.2,
        apply_state: MqttRuntimeApplyState::parse(&state.3)?,
        last_error_code: state.4,
        last_error_summary: state.5,
        last_transition_at_ms: state.6,
        updated_by: state.7,
        updated_at_ms: state.8,
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
