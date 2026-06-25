pub const MYSQL_0001: &str = include_str!("../../migrations/mysql/0001_guard_core.sql");
pub const MYSQL_0003: &str = include_str!("../../migrations/mysql/0003_guard_security.sql");
pub const SQLITE_0003: &str = include_str!("../../migrations/sqlite/0003_guard_security.sql");
pub const MYSQL_0004: &str = include_str!("../../migrations/mysql/0004_guard_integrations.sql");
pub const SQLITE_0004: &str = include_str!("../../migrations/sqlite/0004_guard_integrations.sql");
pub const MYSQL_0005: &str = include_str!("../../migrations/mysql/0005_guard_settings.sql");
pub const SQLITE_0005: &str = include_str!("../../migrations/sqlite/0005_guard_settings.sql");

pub const SQLITE_0001: &str = include_str!("../../migrations/sqlite/0001_guard_core.sql");
pub const MYSQL_0002: &str = include_str!("../../migrations/mysql/0002_guard_outbox.sql");
pub const SQLITE_0002: &str = include_str!("../../migrations/sqlite/0002_guard_outbox.sql");

pub fn migration_pairs() -> [(&'static str, &'static str); 5] {
    [
        (MYSQL_0001, SQLITE_0001),
        (MYSQL_0002, SQLITE_0002),
        (MYSQL_0003, SQLITE_0003),
        (MYSQL_0004, SQLITE_0004),
        (MYSQL_0005, SQLITE_0005),
    ]
}

pub const MIGRATIONS: &[base_db::migration::Migration] = &[
    base_db::migration::Migration {
        version: 1,
        name: "guard_core",
        sql: SQLITE_0001,
    },
    base_db::migration::Migration {
        version: 2,
        name: "guard_outbox",
        sql: SQLITE_0002,
    },
    base_db::migration::Migration {
        version: 3,
        name: "guard_security",
        sql: SQLITE_0003,
    },
    base_db::migration::Migration {
        version: 4,
        name: "guard_integrations",
        sql: SQLITE_0004,
    },
    base_db::migration::Migration {
        version: 5,
        name: "guard_settings",
        sql: SQLITE_0005,
    },
];

pub const MYSQL_MIGRATIONS: &[base_db::migration::Migration] = &[
    base_db::migration::Migration {
        version: 1,
        name: "guard_core",
        sql: MYSQL_0001,
    },
    base_db::migration::Migration {
        version: 2,
        name: "guard_outbox",
        sql: MYSQL_0002,
    },
    base_db::migration::Migration {
        version: 3,
        name: "guard_security",
        sql: MYSQL_0003,
    },
    base_db::migration::Migration {
        version: 4,
        name: "guard_integrations",
        sql: MYSQL_0004,
    },
    base_db::migration::Migration {
        version: 5,
        name: "guard_settings",
        sql: MYSQL_0005,
    },
];
