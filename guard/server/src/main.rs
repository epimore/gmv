use std::time::{SystemTime, UNIX_EPOCH};

use base::utils::crypto::{default_decrypt, default_encrypt};
use gmv_guard_server::app_config::GuardAppConfig;
use gmv_guard_server::auth::{Role, hash_password};
use gmv_guard_server::core::{GuardError, GuardResult};
use gmv_guard_server::store::persistent::PersistentStore;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("reset-admin-password") => reset_admin_password(&args[1..]),
        Some("encrypt") => crypto_command(&args[1..], CryptoAction::Encrypt),
        Some("decrypt") => crypto_command(&args[1..], CryptoAction::Decrypt),
        _ => {
            gmv_guard_server::run();
            Ok(())
        }
    }
}

#[derive(Clone, Copy)]
enum CryptoAction {
    Encrypt,
    Decrypt,
}

fn crypto_command(args: &[String], action: CryptoAction) -> Result<(), Box<dyn std::error::Error>> {
    let label = match action {
        CryptoAction::Encrypt => "plaintext",
        CryptoAction::Decrypt => "ciphertext",
    };
    let input = match args {
        [value] if !value.is_empty() => value,
        [_] => {
            return Err(GuardError::InvalidConfig(format!("{label} is required")).into());
        }
        _ => {
            return Err(GuardError::InvalidConfig(
                "usage: guard encrypt|decrypt <value>".to_string(),
            )
            .into());
        }
    };
    let output = match action {
        CryptoAction::Encrypt => default_encrypt(input),
        CryptoAction::Decrypt => default_decrypt(input),
    }
    .map_err(|error| GuardError::InvalidConfig(error.to_string()))?;
    println!("{output}");
    Ok(())
}

fn reset_admin_password(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let (config_path, username, password) = reset_admin_password_args(args)?;
    let config = GuardAppConfig::load(config_path);
    let username = username.unwrap_or_else(|| config.bootstrap.admin.username.clone());
    let password_hash = hash_password(&password)?;
    base::tokio::runtime::Runtime::new()?.block_on(async {
        let persistent = PersistentStore::connect(&config).await?;
        if config.database.auto_migrate {
            persistent.migrate().await?;
        }
        let users = persistent.user_repository();
        let existing = users
            .load_user(&username)
            .await?
            .ok_or_else(|| GuardError::NotFound(format!("user {username}")))?;
        users
            .upsert_user(
                &username,
                Role::Admin,
                Some(&password_hash),
                Some(&existing.nickname),
                true,
                now_ms()?,
            )
            .await?;
        users.revoke_ui_sessions(&username).await?;
        Ok::<_, GuardError>(())
    })?;
    println!("reset admin password for user {username}");
    Ok(())
}

fn reset_admin_password_args(args: &[String]) -> GuardResult<(String, Option<String>, String)> {
    let mut config_path = "./config.yml".to_string();
    let mut username = None;
    let mut password = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-c" | "--config" => {
                index += 1;
                config_path = args.get(index).cloned().ok_or_else(reset_usage)?;
            }
            "-u" | "--username" => {
                index += 1;
                let value = args.get(index).cloned().ok_or_else(reset_usage)?;
                if value.trim().is_empty() {
                    return Err(reset_usage());
                }
                username = Some(value);
            }
            value if !value.is_empty() && password.is_none() => {
                password = Some(value.to_string());
            }
            _ => return Err(reset_usage()),
        }
        index += 1;
    }
    let password = password.ok_or_else(reset_usage)?;
    Ok((config_path, username, password))
}

fn reset_usage() -> GuardError {
    GuardError::InvalidConfig(
        "usage: guard reset-admin-password [-c|--config <path>] [-u|--username <name>] <password>"
            .to_string(),
    )
}

fn now_ms() -> GuardResult<i64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| GuardError::InvalidConfig(format!("system clock before epoch: {error}")))?
        .as_millis()
        .min(i64::MAX as u128) as i64)
}
