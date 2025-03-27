use std::time::{SystemTime, UNIX_EPOCH};
use common::exception::{GlobalError, GlobalResult};
use common::log::error;
use common::utils::{dig62, crypto};

const KEY: &str = "GMV:SESSION v1.0";

pub fn build_token_session_id(device_id: &str, channel_id: &str) -> GlobalResult<(String, String)> {
    let now = SystemTime::now();
    let since_the_epoch = now.duration_since(UNIX_EPOCH)
        .expect("Time went backwards");
    let secs = since_the_epoch.as_millis();
    let text = format!("{}{}{}", device_id, channel_id, secs);
    let session_id = dig62::en(&text)?;
    let input = format!("{}@{}", KEY, session_id);
    let token = crypto::generate_token(&input);
    Ok((token, session_id))
}

//返回(device_id, channel_id)
pub fn split_dc(session_id: &str) -> GlobalResult<(String, String)> {
    let dcs = dig62::de(session_id)?;
    Ok((dcs[0..20].to_string(), dcs[20..40].to_string()))
}

pub fn check_token(session_id: &str, token: &str) -> GlobalResult<()> {
    let input = format!("{}@{}", KEY, session_id);
    let r_token = crypto::generate_token(&input);
    if r_token.eq(token) {
        return Ok(());
    }
    Err(GlobalError::new_sys_error("Invalid token", |msg| error!("{msg}")))
}