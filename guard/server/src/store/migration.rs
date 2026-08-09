pub const MYSQL_0001: &str = include_str!("../../migrations/mysql/0001_guard_preview_baseline.sql");
pub const SQLITE_0001: &str =
    include_str!("../../migrations/sqlite/0001_guard_preview_baseline.sql");
pub const MYSQL_0003: &str = include_str!("../../migrations/mysql/0003_guard_integrations.sql");
pub const SQLITE_0003: &str = include_str!("../../migrations/sqlite/0003_guard_integrations.sql");
pub const MYSQL_0004: &str =
    include_str!("../../migrations/mysql/0004_guard_command_idempotency.sql");
pub const SQLITE_0004: &str =
    include_str!("../../migrations/sqlite/0004_guard_command_idempotency.sql");
pub const MYSQL_0005: &str =
    include_str!("../../migrations/mysql/0005_guard_singleton_mqtt_runtime.sql");
pub const SQLITE_0005: &str =
    include_str!("../../migrations/sqlite/0005_guard_singleton_mqtt_runtime.sql");
pub const MYSQL_0006: &str =
    include_str!("../../migrations/mysql/0006_guard_mqtt_action_policy_removal.sql");
pub const SQLITE_0006: &str =
    include_str!("../../migrations/sqlite/0006_guard_mqtt_action_policy_removal.sql");
pub const MYSQL_0007: &str =
    include_str!("../../migrations/mysql/0007_guard_integration_master_key.sql");
pub const SQLITE_0007: &str =
    include_str!("../../migrations/sqlite/0007_guard_integration_master_key.sql");

pub const INTEGRATIONS_V2_COMPATIBILITY_SQL: &str = "INSERT INTO _base_db_migrations(version,name,applied_at_ms) \
     SELECT 3,'guard_integrations',applied_at_ms FROM _base_db_migrations \
     WHERE version=2 AND name='guard_integrations' \
     AND NOT EXISTS (SELECT 1 FROM _base_db_migrations WHERE version=3)";

pub const MYSQL_0003_COLUMNS: &[(&str, &str, &str)] = &[
    (
        "guard_command",
        "integration_id",
        "integration_id VARCHAR(128) NOT NULL DEFAULT ''",
    ),
    (
        "guard_command",
        "operation_id",
        "operation_id VARCHAR(128) NOT NULL DEFAULT ''",
    ),
    (
        "guard_command",
        "action",
        "action VARCHAR(128) NOT NULL DEFAULT ''",
    ),
    (
        "guard_command",
        "state",
        "state VARCHAR(32) NOT NULL DEFAULT 'CLAIMED'",
    ),
    (
        "guard_command",
        "updated_at_ms",
        "updated_at_ms BIGINT NOT NULL DEFAULT 0",
    ),
    (
        "guard_outbox",
        "integration_id",
        "integration_id VARCHAR(128) NOT NULL DEFAULT ''",
    ),
    (
        "guard_outbox",
        "mapping_id",
        "mapping_id VARCHAR(128) NOT NULL DEFAULT ''",
    ),
    ("guard_outbox", "expires_at_ms", "expires_at_ms BIGINT NULL"),
];

pub const MYSQL_0003_INDEXES: &[(&str, &str, &str)] = &[(
    "guard_outbox",
    "idx_guard_outbox_integration_state",
    "CREATE INDEX idx_guard_outbox_integration_state ON guard_outbox(integration_id, state, next_attempt_at_ms)",
)];

pub const MYSQL_0004_COLUMNS: &[(&str, &str, &str)] = &[
    (
        "guard_command",
        "request_hash",
        "request_hash VARCHAR(64) NOT NULL DEFAULT ''",
    ),
    ("guard_command", "http_status", "http_status BIGINT NULL"),
    (
        "guard_command",
        "response_body",
        "response_body MEDIUMBLOB NULL",
    ),
];

pub const MYSQL_0004_INDEXES: &[(&str, &str, &str)] = &[(
    "guard_command",
    "idx_guard_command_integration_created",
    "CREATE INDEX idx_guard_command_integration_created ON guard_command(integration_id, created_at_ms)",
)];

pub fn migration_pairs() -> [(&'static str, &'static str); 6] {
    [
        (MYSQL_0001, SQLITE_0001),
        (MYSQL_0003, SQLITE_0003),
        (MYSQL_0004, SQLITE_0004),
        (MYSQL_0005, SQLITE_0005),
        (MYSQL_0006, SQLITE_0006),
        (MYSQL_0007, SQLITE_0007),
    ]
}

pub const MIGRATIONS: &[base_db::migration::Migration] = &[
    base_db::migration::Migration {
        version: 1,
        name: "guard_preview_baseline",
        sql: SQLITE_0001,
    },
    base_db::migration::Migration {
        version: 3,
        name: "guard_integrations",
        sql: SQLITE_0003,
    },
    base_db::migration::Migration {
        version: 4,
        name: "guard_command_idempotency",
        sql: SQLITE_0004,
    },
    base_db::migration::Migration {
        version: 5,
        name: "guard_singleton_mqtt_runtime",
        sql: SQLITE_0005,
    },
    base_db::migration::Migration {
        version: 6,
        name: "guard_mqtt_action_policy_removal",
        sql: SQLITE_0006,
    },
    base_db::migration::Migration {
        version: 7,
        name: "guard_integration_master_key",
        sql: SQLITE_0007,
    },
];

pub const MYSQL_MIGRATIONS: &[base_db::migration::Migration] = &[
    base_db::migration::Migration {
        version: 1,
        name: "guard_preview_baseline",
        sql: MYSQL_0001,
    },
    base_db::migration::Migration {
        version: 3,
        name: "guard_integrations",
        sql: MYSQL_0003,
    },
    base_db::migration::Migration {
        version: 4,
        name: "guard_command_idempotency",
        sql: MYSQL_0004,
    },
    base_db::migration::Migration {
        version: 5,
        name: "guard_singleton_mqtt_runtime",
        sql: MYSQL_0005,
    },
    base_db::migration::Migration {
        version: 6,
        name: "guard_mqtt_action_policy_removal",
        sql: MYSQL_0006,
    },
    base_db::migration::Migration {
        version: 7,
        name: "guard_integration_master_key",
        sql: MYSQL_0007,
    },
];
