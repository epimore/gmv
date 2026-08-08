#![cfg(feature = "db-mysql")]

use std::str::FromStr;

use base_db::{
    dbx::{DatabasePoolConfig, mysqlx::build_mysql_pool},
    sqlx::mysql::{MySqlConnectOptions, MySqlSslMode},
};
use gmv_guard_server::store::mysql::MysqlStore;

#[test]
#[ignore = "requires GMV_TEST_MYSQL_URL"]
fn mysql_command_idempotency_migration_recovers_after_partial_ddl() {
    let url = std::env::var("GMV_TEST_MYSQL_URL").expect("GMV_TEST_MYSQL_URL is required");
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let options = MySqlConnectOptions::from_str(&url)
                .unwrap()
                .ssl_mode(MySqlSslMode::Disabled);
            let pool = build_mysql_pool(options, DatabasePoolConfig::default()).unwrap();
            let store = MysqlStore::new(pool.clone());
            store.migrate().await.unwrap();

            base_db::sqlx::raw_sql(
                "DELETE FROM _base_db_migrations WHERE version=4;
                 DROP INDEX idx_guard_command_integration_created ON guard_command;
                 ALTER TABLE guard_command DROP COLUMN response_body;",
            )
            .execute(&pool)
            .await
            .unwrap();

            store.migrate().await.unwrap();
            let columns = base_db::sqlx::query_scalar::<_, String>(
                "SELECT COLUMN_NAME FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME='guard_command' AND COLUMN_NAME IN ('request_hash','http_status','response_body') ORDER BY COLUMN_NAME",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(columns, ["http_status", "request_hash", "response_body"]);
            let migration = base_db::sqlx::query_as::<_, (i64, String)>(
                "SELECT version,name FROM _base_db_migrations WHERE version=4",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(migration, (4, "guard_command_idempotency".to_string()));
            let index_exists = base_db::sqlx::query_scalar::<_, i64>(
                "SELECT EXISTS(SELECT 1 FROM information_schema.STATISTICS WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME='guard_command' AND INDEX_NAME='idx_guard_command_integration_created')",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(index_exists, 1);
            pool.close().await;
        });
}
