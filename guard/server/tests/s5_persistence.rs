use std::time::Duration;

use base_db::dbx::{
    DatabasePoolConfig,
    sqlitex::{SqliteConnectionConfig, build_sqlite_pool},
};
use guard::app_config::GuardAppConfig;
use guard::store::persistent::PersistentStore;

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
                    r#"db:
  mysql:
    host_or_ip: 127.0.0.1
    port: 3306
    db_name: gmv
    user: gmv
    pass_crypto_enable: false
    pass: ""
    attrs:
      log_global_sql_level: debug
      log_slow_sql_timeout: 30
      timezone: "+8:00"
      charset: utf8mb4
      ssl_level: 0
    pool:
      max_connections: 10
      min_connections: 0
      connection_timeout: 8
      max_lifetime: 1800
      idle_timeout: 60
      check_health: false

log:
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
            for table in [
                "guard_user",
                "guard_service_credential",
                "guard_ui_session",
                "guard_integration",
                "guard_system_setting",
            ] {
                let found = base_db::sqlx::query_scalar::<_, String>(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name=?",
                )
                .bind(table)
                .fetch_optional(&pool)
                .await
                .unwrap();
                assert_eq!(found.as_deref(), Some(table));
            }
            pool.close().await;
            drop(store);
            let _ = std::fs::remove_dir_all(root);
        });
}
