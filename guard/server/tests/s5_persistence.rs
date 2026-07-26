use std::time::Duration;

use base_db::dbx::{
    DatabasePoolConfig,
    sqlitex::{SqliteConnectionConfig, build_sqlite_pool},
};
use gmv_guard_server::app_config::GuardAppConfig;
use gmv_guard_server::store::persistent::PersistentStore;

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
                    "guard_outbox",
                    "guard_user"
                ]
            );
            let migration = base_db::sqlx::query_as::<_, (i64, String)>(
                "SELECT version,name FROM _base_db_migrations",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(migration, (1, "guard_preview_baseline".to_string()));
            pool.close().await;
            drop(store);
            let _ = std::fs::remove_dir_all(root);
        });
}
