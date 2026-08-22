use crate::gb::SessionConf;
use crate::storage::mapper;
use crate::storage::ssrc_sequence::{SsrcKind, SsrcSequence};
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult};
use base::log::error;
use base::utils::dig62;
use std::time::{SystemTime, UNIX_EPOCH};

//生成stream_id,参数由调用方校验,简单对称加密算法
// device_id 20位十进制纯数字
// channel_id 20位十进制纯数字
// ssrc 10位十进制纯数字
pub fn en_stream_id(device_id: &str, channel_id: &str, ssrc: &str) -> GlobalResult<String> {
    validate_decimal_field(device_id, 20, "device_id")?;
    validate_decimal_field(channel_id, 20, "channel_id")?;
    validate_decimal_field(ssrc, 10, "ssrc")?;

    let now = SystemTime::now();
    let since_the_epoch = now.duration_since(UNIX_EPOCH).map_err(|_| {
        GlobalError::new_sys_error("System time went backwards", |msg| error!("{msg}"))
    })?;
    let millis = since_the_epoch.as_millis();
    let ori_key = format!("{device_id}{channel_id}{ssrc}{millis}");
    dig62::en(&ori_key)
}

//返回(device_id,channel_id,ssrc)
pub fn de_stream_id(stream_id: &str) -> GlobalResult<(String, String, String)> {
    let ori_str = dig62::de(stream_id)?;
    if ori_str.len() < 50 || !ori_str.as_bytes()[..50].iter().all(u8::is_ascii_digit) {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "Invalid stream id",
            |msg| error!("{msg}: decoded stream_id prefix is invalid"),
        ));
    }
    Ok((
        ori_str[0..20].to_string(),
        ori_str[20..40].to_string(),
        ori_str[40..50].to_string(),
    ))
}

// y字段为10位十进制SSRC：实时/历史标识1位 + session SIP域第4～8位5位 + 域内序号4位。
pub async fn build_ssrc_stream_id(
    device_id: &String,
    channel_id: &String,
    live: bool,
) -> GlobalResult<(String, String)> {
    if live {
        let channel_status = mapper::get_device_channel_status(device_id, channel_id)
            .await?
            .ok_or_else(|| {
                GlobalError::new_biz_error(BaseErrorCode::NotFound.code(), "未知设备", |msg| {
                    error!("{msg}")
                })
            })?;
        match channel_status.to_ascii_uppercase().as_str() {
            "OK" | "ON" | "ONLINE" | "ONLY" | "" => {}
            _ => {
                return Err(GlobalError::new_biz_error(
                    BaseErrorCode::Network.code(),
                    "设备已离线",
                    |msg| error!("{msg}"),
                ));
            }
        }
    }

    let domain_id = SessionConf::get_session_by_conf().domain_id;
    let kind = if live {
        SsrcKind::Realtime
    } else {
        SsrcKind::History
    };
    let ssrc = SsrcSequence::allocate(&domain_id, kind).await?;
    let stream_id = en_stream_id(device_id, channel_id, &ssrc)?;
    Ok((ssrc, stream_id))
}

fn validate_decimal_field(value: &str, expected_len: usize, field: &str) -> GlobalResult<()> {
    if value.len() == expected_len && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(());
    }
    Err(GlobalError::new_biz_error(
        BaseErrorCode::InvalidRequest.code(),
        &format!("Invalid {field}"),
        |msg| error!("{msg}: expected {expected_len} decimal digits, value={value}"),
    ))
}

#[test]
fn stream_id_round_trip_preserves_ssrc() {
    let device_id = "34020000001110000001";
    let channel_id = "34020000001320000101";
    let ssrc = "0200000001";
    let stream_id = en_stream_id(device_id, channel_id, ssrc).unwrap();
    let (actual_device, actual_channel, actual_ssrc) = de_stream_id(&stream_id).unwrap();
    assert_eq!(device_id, actual_device);
    assert_eq!(channel_id, actual_channel);
    assert_eq!(ssrc, actual_ssrc);
}

#[test]
fn invalid_stream_components_are_rejected() {
    assert!(en_stream_id("short", "34020000001320000101", "0200000001").is_err());
    assert!(en_stream_id("34020000001110000001", "3402000000132000010a", "0200000001").is_err());
}
