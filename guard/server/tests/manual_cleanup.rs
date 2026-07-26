#![cfg(feature = "db-sqlite")]

use base_db::dbx::{
    DatabasePoolConfig,
    sqlitex::{SqliteConnectionConfig, build_sqlite_pool},
};

const SQLITE_CLEANUP: &str =
    include_str!("../migrations/manual/sqlite/cleanup_legacy_preview_schema.sql");
const MYSQL_CLEANUP: &str =
    include_str!("../migrations/manual/mysql/cleanup_legacy_preview_schema.sql");

const REMOVED_TABLES: &[&str] = &[
    "guard_node",
    "guard_lease",
    "guard_route",
    "guard_event",
    "guard_service_credential",
    "guard_ui_session",
    "guard_integration",
    "guard_system_setting",
];

#[test]
fn sqlite_manual_cleanup_is_idempotent_and_preserves_allowed_objects() {
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let path = std::env::temp_dir().join(format!(
                "guard-manual-cleanup-{}.db",
                uuid::Uuid::new_v4()
            ));
            let pool = build_sqlite_pool(
                SqliteConnectionConfig::new(&path),
                DatabasePoolConfig {
                    max_size: 1,
                    min_idle: Some(0),
                    ..DatabasePoolConfig::default()
                },
            )
            .unwrap();

            base_db::sqlx::raw_sql(
                "CREATE TABLE _base_db_migrations(version INTEGER PRIMARY KEY,name TEXT NOT NULL,applied_at_ms INTEGER NOT NULL);\
                 CREATE TABLE guard_user(id INTEGER PRIMARY KEY);\
                 CREATE TABLE guard_outbox(id INTEGER PRIMARY KEY);\
                 CREATE TABLE guard_command(id INTEGER PRIMARY KEY);\
                 CREATE TABLE guard_node(id INTEGER PRIMARY KEY);\
                 CREATE TABLE guard_lease(id INTEGER PRIMARY KEY);\
                 CREATE TABLE guard_route(id INTEGER PRIMARY KEY);\
                 CREATE TABLE guard_event(id INTEGER PRIMARY KEY);\
                 CREATE TABLE guard_service_credential(id INTEGER PRIMARY KEY);\
                 CREATE TABLE guard_ui_session(id INTEGER PRIMARY KEY);\
                 CREATE TABLE guard_integration(id INTEGER PRIMARY KEY);\
                 CREATE TABLE guard_system_setting(id INTEGER PRIMARY KEY);\
                 INSERT INTO _base_db_migrations(version,name,applied_at_ms) VALUES\
                   (1,'guard_core',0),(2,'guard_outbox',0),(3,'guard_security',0),\
                   (4,'guard_integrations',0),(5,'guard_settings',0),\
                   (6,'guard_user_profile',0),(7,'other_schema',0);",
            )
            .execute(&pool)
            .await
            .unwrap();

            base_db::sqlx::raw_sql(SQLITE_CLEANUP)
                .execute(&pool)
                .await
                .unwrap();
            base_db::sqlx::raw_sql(SQLITE_CLEANUP)
                .execute(&pool)
                .await
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
            let migrations = base_db::sqlx::query_as::<_, (i64, String)>(
                "SELECT version,name FROM _base_db_migrations ORDER BY version",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(migrations, [(7, "other_schema".to_string())]);

            pool.close().await;
            let _ = std::fs::remove_file(path);
        });
}

#[test]
fn manual_cleanup_scripts_have_the_same_table_allowlist() {
    for table in REMOVED_TABLES {
        let statement = format!("DROP TABLE IF EXISTS {table};");
        assert!(MYSQL_CLEANUP.contains(&statement), "mysql missing {table}");
        assert!(
            SQLITE_CLEANUP.contains(&statement),
            "sqlite missing {table}"
        );
    }
    for table in ["guard_user", "guard_outbox", "guard_command"] {
        let statement = format!("DROP TABLE IF EXISTS {table};");
        assert!(!MYSQL_CLEANUP.contains(&statement), "mysql drops {table}");
        assert!(!SQLITE_CLEANUP.contains(&statement), "sqlite drops {table}");
    }
}
