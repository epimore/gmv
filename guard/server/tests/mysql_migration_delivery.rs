#![cfg(feature = "db-mysql")]

use std::str::FromStr;

use base_db::{
    dbx::{DatabasePoolConfig, mysqlx::build_mysql_pool},
    sqlx::mysql::{MySqlConnectOptions, MySqlSslMode},
};
use gmv_guard_server::store::mysql::MysqlStore;

#[test]
#[ignore = "requires GMV_TEST_MYSQL_URL"]
fn mysql_schema_migrations_recover_after_partial_ddl() {
    let url = std::env::var("GMV_TEST_MYSQL_URL").expect("GMV_TEST_MYSQL_URL is required");
    base::tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(async {
            let options = MySqlConnectOptions::from_str(&url)
                .unwrap()
                .ssl_mode(MySqlSslMode::Disabled);
            let schema = format!("gmv_test_guard_{}", uuid::Uuid::new_v4().simple());
            let admin_pool = build_mysql_pool(
                options.clone().database("mysql"),
                DatabasePoolConfig::default(),
            )
            .unwrap();
            base_db::sqlx::query(base_db::sqlx::AssertSqlSafe(format!(
                "CREATE DATABASE {schema} CHARACTER SET utf8mb4 COLLATE utf8mb4_bin"
            )))
            .execute(&admin_pool)
            .await
            .unwrap();
            let pool = build_mysql_pool(
                options.database(&schema),
                DatabasePoolConfig::default(),
            )
            .unwrap();
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

            base_db::sqlx::query("DELETE FROM _base_db_migrations WHERE version=8")
                .execute(&pool)
                .await
                .unwrap();
            store.migrate().await.unwrap();
            let protocol_column_exists = base_db::sqlx::query_scalar::<_, i64>(
                "SELECT EXISTS(SELECT 1 FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME='guard_integration_mqtt' AND COLUMN_NAME='protocol_version')",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(protocol_column_exists, 0);
            let cleanup_migration = base_db::sqlx::query_as::<_, (i64, String)>(
                "SELECT version,name FROM _base_db_migrations WHERE version=8",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(
                cleanup_migration,
                (8, "guard_mqtt_runtime_schema_cleanup".to_string())
            );

            base_db::sqlx::raw_sql(
                "DELETE FROM _base_db_migrations WHERE version=9;
                 ALTER TABLE guard_mqtt_runtime_revision DROP FOREIGN KEY fk_guard_mqtt_revision_integration_slot;
                 ALTER TABLE guard_mqtt_runtime_state DROP FOREIGN KEY fk_guard_mqtt_state_integration_slot;
                 DROP INDEX idx_guard_integration_slot ON guard_integration;
                 ALTER TABLE guard_integration DROP COLUMN slot;
                 CREATE TABLE guard_integration_slot (
                   slot VARCHAR(32) NOT NULL PRIMARY KEY,
                   integration_id VARCHAR(128) NULL UNIQUE,
                   updated_by VARCHAR(128) NOT NULL,
                   updated_at_ms BIGINT NOT NULL,
                   FOREIGN KEY (integration_id) REFERENCES guard_integration(integration_id)
                 );
                 INSERT INTO guard_integration_slot(slot,integration_id,updated_by,updated_at_ms)
                   VALUES ('business',NULL,'migration',0);
                 CREATE TABLE guard_integration_mqtt (
                   integration_id VARCHAR(128) NOT NULL PRIMARY KEY,
                   command_topic TEXT NOT NULL,
                   result_topic TEXT NOT NULL,
                   event_topic_prefix TEXT NOT NULL,
                   updated_at_ms BIGINT NOT NULL,
                   FOREIGN KEY (integration_id) REFERENCES guard_integration(integration_id)
                 );
                 ALTER TABLE guard_mqtt_runtime_revision
                   ADD CONSTRAINT guard_mqtt_runtime_revision_ibfk_1
                   FOREIGN KEY (slot) REFERENCES guard_integration_slot(slot);
                 ALTER TABLE guard_mqtt_runtime_state
                   ADD CONSTRAINT guard_mqtt_runtime_state_ibfk_1
                   FOREIGN KEY (slot) REFERENCES guard_integration_slot(slot);
                 INSERT INTO guard_integration(integration_id,name,transport,inbound_enabled,outbound_enabled,enabled,scopes,expires_at_ms,config_version,created_by,created_at_ms,updated_at_ms)
                   VALUES ('app-1','app','MQTT',1,1,1,'[]',NULL,1,'admin',1,1),
                          ('app-2','legacy app','HTTP',0,0,0,'[]',NULL,1,'admin',2,2);
                 UPDATE guard_integration_slot SET integration_id='app-1',updated_by='admin',updated_at_ms=1 WHERE slot='business';
                 INSERT INTO guard_integration_mqtt(integration_id,command_topic,result_topic,event_topic_prefix,updated_at_ms)
                   VALUES ('app-1','gmv/commands/app-1','gmv/command-results/app-1','gmv/events/app-1',1);
                 INSERT INTO guard_integration_http(integration_id,callback_url,callback_timeout_ms,private_network_policy,private_network_allowlist,max_attempts,event_ttl_ms,max_response_bytes,updated_at_ms)
                   VALUES ('app-2',NULL,5000,'deny','[]',5,259200000,65536,2);
                 INSERT INTO guard_mqtt_runtime_revision(slot,revision,protocol_version,broker,port,client_id,username,password_ciphertext,tls,publish_event_ttl_sec,created_by,created_at_ms)
                   VALUES ('business',1,'v5','broker.example.test',1883,'guard',NULL,NULL,0,86400,'admin',1);
                 INSERT INTO guard_mqtt_runtime_state(slot,desired_revision,active_revision,config_version,apply_state,last_error_code,last_error_summary,last_transition_at_ms,updated_by,updated_at_ms)
                   VALUES ('business',1,1,1,'CONNECTED',NULL,NULL,1,'admin',1);",
            )
                .execute(&pool)
                .await
                .unwrap();
            store.migrate().await.unwrap();
            let integrations = base_db::sqlx::query_as::<_, (String, Option<String>)>(
                "SELECT integration_id,slot FROM guard_integration ORDER BY integration_id",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(
                integrations,
                [
                    ("app-1".to_string(), Some("business".to_string())),
                    ("app-2".to_string(), None)
                ]
            );
            let preserved_http = base_db::sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM guard_integration_http WHERE integration_id='app-2'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(preserved_http, 1);
            let consolidated_tables = base_db::sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM information_schema.TABLES WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME IN ('guard_integration_mqtt','guard_integration_slot')",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(consolidated_tables, 0);
            let slot_index_exists = base_db::sqlx::query_scalar::<_, i64>(
                "SELECT EXISTS(SELECT 1 FROM information_schema.STATISTICS WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME='guard_integration' AND INDEX_NAME='idx_guard_integration_slot')",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(slot_index_exists, 1);
            let runtime_foreign_keys = base_db::sqlx::query_as::<_, (String, String)>(
                "SELECT TABLE_NAME,REFERENCED_TABLE_NAME FROM information_schema.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME IN ('guard_mqtt_runtime_revision','guard_mqtt_runtime_state') AND COLUMN_NAME='slot' AND REFERENCED_TABLE_NAME IS NOT NULL ORDER BY TABLE_NAME",
            )
            .fetch_all(&pool)
            .await
            .unwrap();
            assert_eq!(
                runtime_foreign_keys,
                [
                    (
                        "guard_mqtt_runtime_revision".to_string(),
                        "guard_integration".to_string()
                    ),
                    (
                        "guard_mqtt_runtime_state".to_string(),
                        "guard_integration".to_string()
                    )
                ]
            );
            let consolidation_migration = base_db::sqlx::query_as::<_, (i64, String)>(
                "SELECT version,name FROM _base_db_migrations WHERE version=9",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(
                consolidation_migration,
                (9, "guard_integration_schema_consolidation".to_string())
            );
            pool.close().await;
            base_db::sqlx::query(base_db::sqlx::AssertSqlSafe(format!(
                "DROP DATABASE {schema}"
            )))
            .execute(&admin_pool)
            .await
            .unwrap();
            let schema_exists = base_db::sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME=?",
            )
            .bind(&schema)
            .fetch_one(&admin_pool)
            .await
            .unwrap();
            assert_eq!(schema_exists, 0);
            admin_pool.close().await;
        });
}
