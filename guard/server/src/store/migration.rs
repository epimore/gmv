pub const MYSQL_0001: &str = include_str!("../../migrations/mysql/0001_guard_preview_baseline.sql");
pub const SQLITE_0001: &str =
    include_str!("../../migrations/sqlite/0001_guard_preview_baseline.sql");
pub const MYSQL_0003: &str = include_str!("../../migrations/mysql/0003_guard_integrations.sql");
pub const SQLITE_0003: &str = include_str!("../../migrations/sqlite/0003_guard_integrations.sql");

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

pub fn migration_pairs() -> [(&'static str, &'static str); 2] {
    [(MYSQL_0001, SQLITE_0001), (MYSQL_0003, SQLITE_0003)]
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
];
