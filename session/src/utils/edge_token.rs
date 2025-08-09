use std::time::{SystemTime, UNIX_EPOCH};
use base::exception::{GlobalError, GlobalResult};
use base::log::error;
use base::utils::{dig62, crypto};

const KEY: &str = "GMV:SESSION v1.0";

pub fn build_file_name(device_id: &str, channel_id: &str) -> GlobalResult<String> {
    let now = SystemTime::now();
    let since_the_epoch = now.duration_since(UNIX_EPOCH)
        .expect("Time went backwards");
    let mils = since_the_epoch.as_millis();
    let text = format!("{}{}{}", device_id, channel_id, mils);
    dig62::en(&text)
}

pub fn build_token_session_id(device_id: &str, channel_id: &str) -> GlobalResult<(String, String)> {
    let now = SystemTime::now();
    let since_the_epoch = now.duration_since(UNIX_EPOCH)
        .expect("Time went backwards");
    let mils = since_the_epoch.as_millis();
    let text = format!("{}{}{}", device_id, channel_id, mils);
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

pub fn check(session_id: &str, token: &str) -> GlobalResult<()> {
    let input = format!("{}@{}", KEY, session_id);
    let r_token = crypto::generate_token(&input);
    if r_token.eq(token) {
        return Ok(());
    }
    Err(GlobalError::new_sys_error("Invalid token", |msg| error!("{msg}")))
}

#[cfg(test)]
mod test {
    #[test]
    fn t1() {
        let device_id = "34020000001110000009";
        let channel_id = "34020000001320000101";
        let (token, session_id) = super::build_token_session_id(device_id, channel_id).unwrap();
        println!("token: {}", token);
        println!("session_id: {}", session_id);
        let (dc_device_id, dc_channel_id) = super::split_dc(&session_id).unwrap();
        println!("dc_device_id: {}", dc_device_id);
        println!("dc_channel_id: {}", dc_channel_id);
        super::check(&session_id, &token).unwrap();
    }
}
