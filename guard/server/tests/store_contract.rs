use guard::auth::{Role, Secret};
use guard::core::GuardConfig;
use guard::store::migration::migration_pairs;
use guard::store::{GuardStore, InMemoryGuardStore};

#[test]
fn guard_config_and_secret_baselines_hold() {
    GuardConfig::default().validate().unwrap();
    assert!(Role::Admin.allows(Role::Operator));
    let secret = Secret::new("super-secret");
    assert!(!format!("{secret:?}").contains("super-secret"));
}

#[test]
fn guard_store_is_explicit_backend_enum() {
    let store = GuardStore::Memory(InMemoryGuardStore::default());
    assert!(matches!(store, GuardStore::Memory(_)));
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
        "guard_node",
        "guard_lease",
        "guard_route",
        "guard_event",
        "guard_outbox",
        "guard_command",
        "guard_user",
        "guard_service_credential",
        "guard_ui_session",
        "guard_integration",
        "guard_system_setting",
    ] {
        assert!(mysql_all.contains(table), "mysql missing {table}");
        assert!(sqlite_all.contains(table), "sqlite missing {table}");
    }
}
