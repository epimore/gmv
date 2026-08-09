#![cfg(feature = "db-sqlite")]

use std::time::Duration;

use base_db::dbx::{
    DatabasePoolConfig,
    sqlitex::{SqliteConnectionConfig, build_sqlite_pool},
};
use gmv_guard_server::integration::model::{
    CredentialPurpose, CredentialStatus, Integration, IntegrationCredential, IntegrationTransport,
    MqttRuntimeRevision,
};
use gmv_guard_server::integration::secret::IntegrationSecretCipher;
use gmv_guard_server::store::migration::MIGRATIONS;
use gmv_guard_server::store::persistent::IntegrationRepository;
use gmv_guard_server::store::sqlite::SqliteStore;

#[test]
fn master_key_initialization_is_idempotent_and_rotation_reencrypts_all_secrets() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let path = std::env::temp_dir().join(format!(
                "guard-integration-master-key-{}.db",
                uuid::Uuid::new_v4()
            ));
            let pool = build_sqlite_pool(
                SqliteConnectionConfig::new(&path),
                DatabasePoolConfig {
                    max_size: 1,
                    min_idle: Some(0),
                    connection_timeout: Duration::from_secs(2),
                    ..DatabasePoolConfig::default()
                },
            )
            .unwrap();
            let store = SqliteStore::new(pool);
            store.migrate().await.unwrap();
            let repository = IntegrationRepository::Sqlite(store.clone());
            let old_material = IntegrationSecretCipher::random_key_material();
            let initialized = repository
                .ensure_master_key(&old_material, "system:init", 10)
                .await
                .unwrap();
            let ignored_material = IntegrationSecretCipher::random_key_material();
            let existing = repository
                .ensure_master_key(&ignored_material, "system:init", 11)
                .await
                .unwrap();
            assert_eq!(existing.key_material, initialized.key_material);
            assert_eq!(existing.key_version, 1);

            let old_cipher =
                IntegrationSecretCipher::from_base64_key_no_pad(&initialized.key_material).unwrap();
            repository
                .upsert(&Integration {
                    integration_id: "app-1".to_string(),
                    name: "App 1".to_string(),
                    transport: IntegrationTransport::Http,
                    inbound_enabled: true,
                    outbound_enabled: true,
                    enabled: false,
                    scopes: vec!["*".to_string()],
                    expires_at_ms: None,
                    config_version: 1,
                    created_by: "admin".to_string(),
                    created_at_ms: 10,
                    updated_at_ms: 10,
                })
                .await
                .unwrap();
            repository
                .insert_credential(&IntegrationCredential {
                    credential_id: "cred-1".to_string(),
                    access_key: "ak-1".to_string(),
                    integration_id: "app-1".to_string(),
                    purpose: CredentialPurpose::HttpInboundVerify,
                    secret_ciphertext: old_cipher.encrypt("http-secret").unwrap(),
                    key_version: 1,
                    status: CredentialStatus::Active,
                    not_before_ms: 10,
                    expires_at_ms: None,
                    revoked_at_ms: None,
                    created_by: "admin".to_string(),
                    created_at_ms: 10,
                    updated_at_ms: 10,
                })
                .await
                .unwrap();
            repository
                .save_mqtt_runtime_config(
                    &MqttRuntimeRevision {
                        revision: 0,
                        protocol_version: "v5".to_string(),
                        broker: "broker.example.test".to_string(),
                        port: 1883,
                        client_id: "guard-test".to_string(),
                        username: Some("guard".to_string()),
                        password_ciphertext: Some(old_cipher.encrypt("mqtt-secret").unwrap()),
                        tls: false,
                        publish_event_ttl_sec: 86_400,
                        created_by: "admin".to_string(),
                        created_at_ms: 10,
                    },
                    0,
                )
                .await
                .unwrap();

            let new_material = IntegrationSecretCipher::random_key_material();
            let new_cipher =
                IntegrationSecretCipher::from_base64_key_no_pad(&new_material).unwrap();
            let rotated = repository
                .rotate_master_key(
                    &old_cipher,
                    &new_cipher,
                    &new_material,
                    1,
                    "admin",
                    "audit-1",
                    20,
                )
                .await
                .unwrap();
            assert_eq!(rotated.key_version, 2);
            assert_eq!(rotated.updated_by, "admin");

            let credential = repository
                .list_credentials("app-1")
                .await
                .unwrap()
                .remove(0);
            assert_eq!(credential.key_version, 2);
            assert_eq!(
                new_cipher.decrypt(&credential.secret_ciphertext).unwrap(),
                "http-secret"
            );
            assert!(old_cipher.decrypt(&credential.secret_ciphertext).is_err());
            let mqtt = repository.mqtt_runtime_revision(1).await.unwrap().unwrap();
            assert_eq!(
                new_cipher
                    .decrypt(mqtt.password_ciphertext.as_deref().unwrap())
                    .unwrap(),
                "mqtt-secret"
            );
            assert_eq!(
                repository.list_audits(10).await.unwrap()[0].action,
                "master_key.rotate"
            );

            drop(repository);
            drop(store);
            let _ = std::fs::remove_file(path);
        });
}

#[test]
fn master_key_migration_removes_unsupported_legacy_ciphertext() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let path = std::env::temp_dir().join(format!(
                "guard-integration-master-key-migration-{}.db",
                uuid::Uuid::new_v4()
            ));
            let pool = build_sqlite_pool(
                SqliteConnectionConfig::new(&path),
                DatabasePoolConfig {
                    max_size: 1,
                    min_idle: Some(0),
                    connection_timeout: Duration::from_secs(2),
                    ..DatabasePoolConfig::default()
                },
            )
            .unwrap();
            base_db::migration::run_sqlite_migrations(&pool, &MIGRATIONS[..5])
                .await
                .unwrap();
            base_db::sqlx::query("INSERT INTO guard_integration(integration_id,name,transport,inbound_enabled,outbound_enabled,enabled,scopes,expires_at_ms,config_version,created_by,created_at_ms,updated_at_ms) VALUES ('app-1','App 1','HTTP',1,1,0,'[\"*\"]',NULL,1,'admin',1,1)")
                .execute(&pool).await.unwrap();
            base_db::sqlx::query("INSERT INTO guard_integration_credential(credential_id,access_key,integration_id,purpose,secret_ciphertext,key_version,status,not_before_ms,expires_at_ms,revoked_at_ms,created_by,created_at_ms,updated_at_ms) VALUES ('cred-1','ak-1','app-1','HTTP_INBOUND_VERIFY','legacy-ciphertext',1,'ACTIVE',1,NULL,NULL,'admin',1,1)")
                .execute(&pool).await.unwrap();
            base_db::sqlx::query("INSERT INTO guard_mqtt_runtime_revision(slot,revision,protocol_version,broker,port,client_id,username,password_ciphertext,tls,publish_event_ttl_sec,created_by,created_at_ms) VALUES ('business',1,'v5','broker.example.test',1883,'guard','admin','legacy-ciphertext',0,86400,'admin',1)")
                .execute(&pool).await.unwrap();
            base_db::sqlx::query("INSERT INTO guard_mqtt_runtime_state(slot,desired_revision,active_revision,config_version,apply_state,last_error_code,last_error_summary,last_transition_at_ms,updated_by,updated_at_ms) VALUES ('business',1,NULL,1,'PENDING',NULL,NULL,1,'admin',1)")
                .execute(&pool).await.unwrap();

            base_db::migration::run_sqlite_migrations(&pool, MIGRATIONS)
                .await
                .unwrap();

            for (table, query) in [
                (
                    "guard_integration_credential",
                    "SELECT COUNT(*) FROM guard_integration_credential",
                ),
                (
                    "guard_mqtt_runtime_revision",
                    "SELECT COUNT(*) FROM guard_mqtt_runtime_revision",
                ),
                (
                    "guard_mqtt_runtime_state",
                    "SELECT COUNT(*) FROM guard_mqtt_runtime_state",
                ),
            ] {
                let count = base_db::sqlx::query_scalar::<_, i64>(query)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                assert_eq!(count, 0, "{table} should be reset by migration 0007");
            }

            pool.close().await;
            let _ = std::fs::remove_file(path);
        });
}
