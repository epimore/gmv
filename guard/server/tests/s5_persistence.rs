use std::time::Duration;

use base_db::dbx::{
    DatabasePoolConfig,
    sqlitex::{SqliteConnectionConfig, build_sqlite_pool},
};
use gmv_guard_server::app_config::GuardAppConfig;
use gmv_guard_server::store::migration::{SQLITE_0001, SQLITE_0003};
use gmv_guard_server::store::persistent::PersistentStore;
use gmv_guard_server::store::sqlite::SqliteStore;

#[test]
fn yaml_annotation_auto_migrates_and_bootstraps_only_once() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let root = std::env::temp_dir().join(format!("guard-s5-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let db_path = root.join("guard.db");
            let config_path = root.join("config.yml");
            std::fs::write(
                &config_path,
                format!(
                    r#"log:
  level: info
  prefix: guard-test
  store_path: {}

guard:
  http:
    bind_addr: 127.0.0.1:18080
    origins:
      - http://127.0.0.1:18080
    tls:
      enabled: false
  grpc:
    bind_addr: 127.0.0.1:18081
    tls:
      enabled: false
  database:
    backend: sqlite
    auto_migrate: true
    pool:
      max_connections: 1
      min_connections: 0
    sqlite:
      path: {}
  bootstrap:
    admin:
      username: admin
      pass_crypto_enable: false
      pass: first-password
      local_login_only: true
"#,
                    root.join("logs").display(),
                    db_path.display()
                ),
            )
            .unwrap();

            let config = GuardAppConfig::load(config_path.to_string_lossy().into_owned());
            let store = PersistentStore::connect(&config).await.unwrap();
            store.initialize(&config).await.unwrap();
            let users = store.load_users().await.unwrap();
            assert_eq!(users.len(), 1);
            assert!(users[0].verify_password("first-password").unwrap());

            let mut second_config = config.clone();
            second_config.bootstrap.admin.pass = "second-password".to_string();
            store.initialize(&second_config).await.unwrap();
            let users = store.load_users().await.unwrap();
            assert!(users[0].verify_password("first-password").unwrap());
            assert!(!users[0].verify_password("second-password").unwrap());

            let mut restart_config = config.clone();
            restart_config.bootstrap.admin.pass.clear();
            store.initialize(&restart_config).await.unwrap();
            let users = store.load_users().await.unwrap();
            assert!(users[0].verify_password("first-password").unwrap());

            let pool = build_sqlite_pool(
                SqliteConnectionConfig::new(&db_path),
                DatabasePoolConfig {
                    max_size: 1,
                    min_idle: Some(0),
                    connection_timeout: Duration::from_secs(2),
                    ..DatabasePoolConfig::default()
                },
            )
            .unwrap();
            let tables = base_db::sqlx::query_scalar::<_, String>(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(
                tables,
                [
                    "_base_db_migrations",
                    "guard_command",
                    "guard_integration",
                    "guard_integration_audit",
                    "guard_integration_credential",
                    "guard_integration_delivery",
                    "guard_integration_http",
                    "guard_integration_mapping",
                    "guard_integration_master_key",
                    "guard_integration_mqtt",
                    "guard_integration_slot",
                    "guard_mqtt_runtime_revision",
                    "guard_mqtt_runtime_state",
                    "guard_outbox",
                    "guard_user"
                ]
            );
            let migrations = base_db::sqlx::query_as::<_, (i64, String)>(
                "SELECT version,name FROM _base_db_migrations ORDER BY version",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(
                migrations,
                [
                    (1, "guard_preview_baseline".to_string()),
                    (3, "guard_integrations".to_string()),
                    (4, "guard_command_idempotency".to_string()),
                    (5, "guard_singleton_mqtt_runtime".to_string()),
                    (6, "guard_mqtt_action_policy_removal".to_string()),
                    (7, "guard_integration_master_key".to_string())
                ]
            );
            let master_key = base_db::sqlx::query_as::<_, (i64, i64)>(
                "SELECT key_version,LENGTH(key_material) FROM guard_integration_master_key WHERE slot='business'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(master_key, (1, 43));
            let user_columns = base_db::sqlx::query_scalar::<_, String>(
                "SELECT name FROM pragma_table_info('guard_user') ORDER BY cid",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            assert!(user_columns.iter().any(|column| column == "expires_at_ms"));
            let mqtt_columns = base_db::sqlx::query_scalar::<_, String>(
                "SELECT name FROM pragma_table_info('guard_integration_mqtt') ORDER BY cid",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            assert!(!mqtt_columns.iter().any(|column| column == "allowed_actions"));
            pool.close().await;
            drop(store);
            let _ = std::fs::remove_dir_all(root);
        });
}

#[test]
fn sqlite_preserves_reserved_user_expiration_v2_and_applies_integrations_v3() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let path = std::env::temp_dir().join(format!(
                "guard-user-expiration-upgrade-{}.db",
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
            base_db::sqlx::raw_sql(SQLITE_0001)
                .execute(&pool)
                .await
                .unwrap();
            base_db::sqlx::raw_sql(
                "CREATE TABLE _base_db_migrations(version INTEGER PRIMARY KEY,name TEXT NOT NULL,applied_at_ms INTEGER NOT NULL);\
                 INSERT INTO _base_db_migrations(version,name,applied_at_ms) VALUES\
                   (1,'guard_preview_baseline',0),(2,'guard_user_expiration',0);",
            )
            .execute(&pool)
            .await
            .unwrap();

            SqliteStore::new(pool.clone()).migrate().await.unwrap();

            let migrations = base_db::sqlx::query_as::<_, (i64, String)>(
                "SELECT version,name FROM _base_db_migrations ORDER BY version",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(
                migrations,
                [
                    (1, "guard_preview_baseline".to_string()),
                    (2, "guard_user_expiration".to_string()),
                    (3, "guard_integrations".to_string()),
                    (4, "guard_command_idempotency".to_string()),
                    (5, "guard_singleton_mqtt_runtime".to_string()),
                    (6, "guard_mqtt_action_policy_removal".to_string()),
                    (7, "guard_integration_master_key".to_string())
                ]
            );
            let integration_table = base_db::sqlx::query_scalar::<_, String>(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='guard_integration'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(integration_table, "guard_integration");

            pool.close().await;
            let _ = std::fs::remove_file(path);
        });
}

#[test]
fn sqlite_aliases_integrations_v2_without_reapplying_schema() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let path = std::env::temp_dir().join(format!(
                "guard-integrations-v2-upgrade-{}.db",
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
            base_db::sqlx::raw_sql(SQLITE_0001)
                .execute(&pool)
                .await
                .unwrap();
            base_db::sqlx::raw_sql(SQLITE_0003)
                .execute(&pool)
                .await
                .unwrap();
            base_db::sqlx::raw_sql(
                "CREATE TABLE _base_db_migrations(version INTEGER PRIMARY KEY,name TEXT NOT NULL,applied_at_ms INTEGER NOT NULL);\
                 INSERT INTO _base_db_migrations(version,name,applied_at_ms) VALUES\
                   (1,'guard_preview_baseline',0),(2,'guard_integrations',123);",
            )
            .execute(&pool)
            .await
            .unwrap();

            SqliteStore::new(pool.clone()).migrate().await.unwrap();

            let migrations = base_db::sqlx::query_as::<_, (i64, String, i64)>(
                "SELECT version,name,applied_at_ms FROM _base_db_migrations ORDER BY version",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(
                &migrations[..3],
                &[
                    (1, "guard_preview_baseline".to_string(), 0),
                    (2, "guard_integrations".to_string(), 123),
                    (3, "guard_integrations".to_string(), 123)
                ]
            );
            assert_eq!(
                (migrations[3].0, migrations[3].1.as_str()),
                (4, "guard_command_idempotency")
            );

            pool.close().await;
            let _ = std::fs::remove_file(path);
        });
}
