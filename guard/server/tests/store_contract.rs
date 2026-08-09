use gmv_guard_server::auth::{Role, Secret};
use gmv_guard_server::core::GuardConfig;
use gmv_guard_server::store::migration::{
    MYSQL_0003, MYSQL_0003_COLUMNS, MYSQL_0003_INDEXES, MYSQL_0004_COLUMNS, MYSQL_0004_INDEXES,
    MYSQL_0007, SQLITE_0003, SQLITE_0004, SQLITE_0007, migration_pairs,
};

#[test]
fn guard_config_and_secret_baselines_hold() {
    GuardConfig::default().validate().unwrap();
    assert!(Role::Admin.allows(Role::Operator));
    let secret = Secret::new("super-secret");
    assert!(!format!("{secret:?}").contains("super-secret"));
}

#[test]
fn master_key_migration_drops_unsupported_legacy_ciphertext() {
    for migration in [MYSQL_0007, SQLITE_0007] {
        assert!(migration.contains("DELETE FROM guard_mqtt_runtime_state"));
        assert!(migration.contains("DELETE FROM guard_mqtt_runtime_revision"));
        assert!(migration.contains("DELETE FROM guard_integration_credential"));
    }
}

#[test]
fn mysql_and_sqlite_migrations_stay_compatible() {
    let forbidden = [
        " AUTO_INCREMENT",
        " AUTOINCREMENT",
        "`",
        "JSON",
        "ENGINE=",
        "TEXT NOT NULL PRIMARY KEY",
    ];
    let mut mysql_all = String::new();
    let mut sqlite_all = String::new();
    for (mysql, sqlite) in migration_pairs() {
        let mysql_upper = mysql.to_ascii_uppercase();
        let sqlite_upper = sqlite.to_ascii_uppercase();
        for item in forbidden {
            assert!(
                !mysql_upper.contains(item),
                "mysql migration contains {item}"
            );
            assert!(
                !sqlite_upper.contains(item),
                "sqlite migration contains {item}"
            );
        }
        mysql_all.push_str(mysql);
        sqlite_all.push_str(sqlite);
    }
    for table in [
        "GMV_FILE_INFO",
        "GMV_RECORD",
        "GMV_DEVICE_PTZ_PRESET",
        "GMV_OAUTH",
        "gmv_gb28181_channel",
        "gmv_gb28181_channel_image",
    ] {
        assert!(!mysql_all.contains(table), "mysql should not own {table}");
        assert!(!sqlite_all.contains(table), "sqlite should not own {table}");
    }
    for table in [
        "guard_outbox",
        "guard_command",
        "guard_user",
        "guard_integration",
        "guard_integration_credential",
        "guard_integration_http",
        "guard_integration_mqtt",
        "guard_integration_mapping",
        "guard_integration_audit",
        "guard_integration_delivery",
        "guard_integration_slot",
        "guard_integration_master_key",
        "guard_mqtt_runtime_revision",
        "guard_mqtt_runtime_state",
    ] {
        assert!(mysql_all.contains(table), "mysql missing {table}");
        assert!(sqlite_all.contains(table), "sqlite missing {table}");
    }
    for table in [
        "guard_node",
        "guard_lease",
        "guard_route",
        "guard_event",
        "guard_service_credential",
        "guard_ui_session",
        "guard_system_setting",
    ] {
        assert!(!mysql_all.contains(table), "mysql should not own {table}");
        assert!(!sqlite_all.contains(table), "sqlite should not own {table}");
    }
}

#[test]
fn mysql_integration_mapping_index_fits_innodb_and_repair_steps_are_registered() {
    assert!(
        MYSQL_0003
            .contains("source_type VARCHAR(255) CHARACTER SET ascii COLLATE ascii_bin NOT NULL")
    );
    assert!(
        MYSQL_0003.contains(
            "destination VARCHAR(512) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL"
        )
    );
    let maximum_key_bytes = 128 * 4 + 16 + 255 + 16 + 512 * 4;
    assert!(maximum_key_bytes <= 3_072);
    assert_eq!(MYSQL_0003_COLUMNS.len(), 8);
    assert_eq!(MYSQL_0003_INDEXES.len(), 1);
    for (_, column, _) in MYSQL_0003_COLUMNS {
        assert!(
            SQLITE_0003.contains(&format!("ADD COLUMN {column} ")),
            "SQLite migration missing MySQL repair column {column}"
        );
    }
    for (_, index, _) in MYSQL_0003_INDEXES {
        assert!(
            SQLITE_0003.contains(index),
            "SQLite migration missing MySQL repair index {index}"
        );
    }
    for (_, column, _) in MYSQL_0004_COLUMNS {
        assert!(
            SQLITE_0004.contains(&format!("ADD COLUMN {column} ")),
            "SQLite migration missing MySQL repair column {column}"
        );
    }
    for (_, index, _) in MYSQL_0004_INDEXES {
        assert!(
            SQLITE_0004.contains(index),
            "SQLite migration missing MySQL repair index {index}"
        );
    }
}
