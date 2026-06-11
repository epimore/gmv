use crate::gb::sip::auth;
use crate::storage::entity::GmvOauth;
use crate::storage::mapper;
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

    //使用纳秒的后两位生成填充字符串,并取7个字符
    let now = SystemTime::now();
    let since_the_epoch = now.duration_since(UNIX_EPOCH).map_err(|_| {
        GlobalError::new_sys_error("System time went backwards", |msg| error!("{msg}"))
    })?;
    let secs = since_the_epoch.as_millis();
    let ori_key = format!("{device_id}{channel_id}{ssrc}{secs}");
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

//为十进制整数字符串,表示SSRC值。格式如下:dddddddddd。其中,第1位为历史或实时
// 媒体流的标识位,0为实时,1为历史;第2位至第6位取20位SIP监控域ID之中的4到8位作为域标
// 识,例如“13010000002000000001”中取数字“10000”;第7位至第10位作为域内媒体流标识,是一个与
// 当前域内产生的媒体流SSRC值后4位不重复的四位十进制整数
// 返回(ssrc,stream_id)
pub async fn build_ssrc_stream_id(
    device_id: &String,
    channel_id: &String,
    num_ssrc: u16,
    live: bool,
) -> GlobalResult<(String, String)> {
    let gmv_oauth = if let Some(cache) = auth::global() {
        cache.get(device_id)
    } else {
        GmvOauth::read_gmv_oauth_by_device_id(device_id).await?
    }
    .ok_or_else(|| {
        GlobalError::new_biz_error(BaseErrorCode::NotFound.code(), "设备不存在", |msg| {
            error!("{msg}")
        })
    })?;
    //直播：需校验摄像头是否在线；回放：录像机在线即可
    let mut front_live_or_back = 1;
    if live {
        let channel_status = mapper::get_device_channel_status(device_id, channel_id)
            .await?
            .ok_or_else(|| {
                GlobalError::new_biz_error(BaseErrorCode::NotFound.code(), "未知设备", |msg| {
                    error!("{msg}")
                })
            })?;
        match &channel_status.to_ascii_uppercase()[..] {
            "OK" | "ON" | "ONLINE" | "ONLY" | "" => {}
            _ => {
                return Err(GlobalError::new_biz_error(
                    BaseErrorCode::Network.code(),
                    "设备已离线",
                    |msg| error!("{msg}"),
                ));
            }
        }
        front_live_or_back = 0;
    }
    let domain_id = gmv_oauth.domain_id;
    validate_decimal_field(&domain_id, 20, "domain_id")?;
    let middle_domain_mark = &domain_id[4..9];
    let ssrc = format!("{front_live_or_back}{middle_domain_mark}{num_ssrc:04}");
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
fn test1() {
    let device_id = "34020000001110000001";
    let channel_id = "34020000001320000101";
    let ssrc = "1100000001";
    let stream_id = en_stream_id(device_id, channel_id, ssrc).unwrap();
    println!("stream_id = {}", &stream_id);
    let (d_d_id, d_c_id, d_ssrc) = de_stream_id(&stream_id).unwrap();
    assert_eq!(device_id, &d_d_id[..]);
    assert_eq!(channel_id, &d_c_id[..]);
    assert_eq!(ssrc, &d_ssrc[..]);
}

#[test]
fn test_ssrc_to_ssrc_num() {
    let ssrc1: u32 = 1100009001;
    let ssrc_num1 = (ssrc1 % 10000) as u16;
    assert_eq!(ssrc_num1, 9001);
    let ssrc2: u32 = 1100000001;
    let ssrc_num2 = (ssrc2 % 10000) as u16;
    assert_eq!(ssrc_num2, 1);
    let ssrc3: u32 = 1100000801;
    let ssrc_num3 = (ssrc3 % 10000) as u16;
    assert_eq!(ssrc_num3, 801);
    let ssrc4: u32 = 1100019999;
    let ssrc_num4 = (ssrc4 % 10000) as u16;
    assert_eq!(ssrc_num4, 9999)
}

#[test]
fn invalid_stream_components_are_rejected() {
    assert!(en_stream_id("short", "34020000001320000101", "1100000001").is_err());
    assert!(en_stream_id("34020000001110000001", "3402000000132000010a", "1100000001").is_err());
}
