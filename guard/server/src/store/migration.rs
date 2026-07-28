pub const MYSQL_0001: &str = include_str!("../../migrations/mysql/0001_guard_preview_baseline.sql");
pub const SQLITE_0001: &str =
    include_str!("../../migrations/sqlite/0001_guard_preview_baseline.sql");
pub const MYSQL_0002: &str = include_str!("../../migrations/mysql/0002_guard_integrations.sql");
pub const SQLITE_0002: &str = include_str!("../../migrations/sqlite/0002_guard_integrations.sql");

pub fn migration_pairs() -> [(&'static str, &'static str); 2] {
    [(MYSQL_0001, SQLITE_0001), (MYSQL_0002, SQLITE_0002)]
}

pub const MIGRATIONS: &[base_db::migration::Migration] = &[
    base_db::migration::Migration {
        version: 1,
        name: "guard_preview_baseline",
        sql: SQLITE_0001,
    },
    base_db::migration::Migration {
        version: 2,
        name: "guard_integrations",
        sql: SQLITE_0002,
    },
];

pub const MYSQL_MIGRATIONS: &[base_db::migration::Migration] = &[
    base_db::migration::Migration {
        version: 1,
        name: "guard_preview_baseline",
        sql: MYSQL_0001,
    },
    base_db::migration::Migration {
        version: 2,
        name: "guard_integrations",
        sql: MYSQL_0002,
    },
];
