use base::log::{debug, info, LevelFilter};
use encoding_rs::GB18030;
use quick_xml::encoding;
use rsip::headers::ToTypedHeader;
use rsip::message::HeadersExt;
use rsip::services::DigestGenerator;
use rsip::{Method, Request};

use anyhow::anyhow;
use base::bytes::Bytes;
use base::chrono::{Duration, Local};
use base::exception::GlobalError::SysErr;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::{error, warn};
use base::net::state::{Association, Package, Zip};
use base::{serde_json, tokio};
use base::tokio::sync::mpsc::Sender;

use crate::gb::core::rw::RWContext;
use crate::gb::depot::SipPackage;
use crate::gb::handler::builder::ResponseBuilder;
use crate::gb::handler::parser::xml::{KV2Model, MESSAGE_UPLOAD_SNAPSHOT_SESSION_ID};
use crate::gb::handler::{cmd, parser};
use crate::http::client::{HttpBiz, HttpClient};
use crate::service::{KEY_SNAPSHOT_IMAGE, KEY_STREAM_IN};
use crate::state;
use crate::state::model::AlarmInfo;
use crate::state::AlarmConf;
use crate::storage::entity::{GmvDevice, GmvDeviceChannel, GmvDeviceExt, GmvOauth};
use crate::storage::mapper;

pub async fn hand_request(
    req: Request,
    tx: Sender<SipPackage>,
    bill: Association,
) -> GlobalResult<()> {
    let device_id = parser::header::get_device_id_by_request(&req)?;
    //校验设备是否注册
    if req.method == Method::Register {
        let _ = Register::process(&device_id, req, tx, &bill)
            .await
            .hand_log(|msg| error!("设备 = [{}],注册失败;err={}", &device_id, msg));
        Ok(())
    } else {
        match State::check_session(&bill, &device_id).await? {
            State::Enable | State::ReCache => match req.method {
                Method::Ack => Ok(()),
                Method::Bye => Ok(()),
                Method::Cancel => Ok(()),
                Method::Info => Ok(()),
                Method::Invite => Ok(()),
                Method::Message => Message::process(&device_id, req, tx.clone(), &bill).await,
                Method::Notify => Notify::process(&device_id, req, tx.clone(), &bill).await,
                Method::Options => Ok(()),
                Method::PRack => Ok(()),
                Method::Publish => Ok(()),
                Method::Refer => Ok(()),
                Method::Subscribe => Ok(()),
                Method::Update => Ok(()),
                _ => {
                    info!("invalid method");
                    Ok(())
                }
            },
            State::Expired => {
                let unregister_response =
                    ResponseBuilder::build_401_response(&req, bill.get_remote_addr())?;
                let sip_package = SipPackage::build_response(unregister_response, bill);
                let _ = tx
                    .clone()
                    .send(sip_package)
                    .await
                    .hand_log(|msg| warn!("{msg}"));
                Ok(())
            }
            State::Invalid => Ok(()),
        }
    }
}

#[derive(Eq, PartialEq)]
enum State {
    //可用的
    Enable,
    //需重新插入缓存的
    ReCache,
    //过期需重新注册的
    Expired,
    //未知设备/未启用设备，仅日志-不处理
    Invalid,
}

impl State {
    ///校验设备是否已注册
    ///1.持有读写句柄，
    ///2.未持有读写句柄->查询DB：
    /// 1)设备在注册有效期内,插入读写句柄
    /// 2)设备未在注册有效期内，重新注册
    /// 3）其他日志记录
    /// 目的->服务端重启后，不需要设备重新注册
    async fn check_session(
        bill: &Association,
        device_id: &String,
    ) -> GlobalResult<State> {
        if RWContext::get_device_id_by_association(bill).is_some()
            || RWContext::has_session_by_device_id(device_id)
        {
            RWContext::keep_alive(device_id, bill.clone());
            Ok(State::Enable)
        } else {
            match mapper::get_device_status_info(device_id).await? {
                None => {
                    warn!("未知设备：{device_id}");
                    Ok(State::Invalid)
                }
                Some((heart, enable, expire, reg_ts, on)) => {
                    if enable == 0 {
                        warn!("未启用设备: {device_id}");
                        Ok(State::Invalid)
                    } else {
                        //判断是否在注册有效期内
                        if reg_ts + Duration::seconds(expire as i64) > Local::now().naive_local() {
                            //刷新缓存
                            RWContext::insert(device_id, heart, bill);
                            //如果设备是离线状态，则更新为在线
                            if on == 0 {
                                GmvDevice::update_gmv_device_status_by_device_id(device_id, 1)
                                    .await?;
                            }
                            Ok(State::ReCache)
                        } else {
                            //401告知对端重新注册
                            Ok(State::Expired)
                        }
                    }
                }
            }
        }
    }
}

struct Register;

impl Register {
    async fn process(
        device_id: &String,
        req: Request,
        tx: Sender<SipPackage>,
        bill: &Association,
    ) -> GlobalResult<()> {
        let oauth = GmvOauth::read_gmv_oauth_by_device_id(device_id)
            .await?
            .ok_or(SysErr(anyhow!(
                "device id = [{}] 未知设备，拒绝接入",
                device_id
            )))
            .hand_log(|msg| warn!("{msg}"))?;
        if oauth.status == 0u8 {
            warn!("device id = [{}] 未启用设备，拒绝接入", device_id);
            return Ok(());
        }
        match oauth.pwd_check {
            //不进行鉴权校验
            0u8 => {
                let expires = parser::header::get_expires(&req)?;
                if expires > 0 {
                    Self::login_ok(device_id, &req, tx, bill, oauth).await
                } else {
                    //设备下线
                    Self::logout_ok(device_id, &req, tx, bill).await
                }
            }
            _ => {
                match req.authorization_header() {
                    None => Self::unauthorized(&req, tx, bill).await,
                    Some(auth) => {
                        match auth.typed() {
                            Ok(au) => {
                                let pwd_opt = oauth.pwd.clone().unwrap_or_default();
                                let dsg = DigestGenerator::from(&au, &pwd_opt, &Method::Register);
                                if dsg.verify(&au.response) {
                                    let expires = parser::header::get_expires(&req)?;
                                    //注册与注销判断
                                    if expires > 0 {
                                        Self::login_ok(device_id, &req, tx, bill, oauth).await
                                    } else {
                                        //注销  设备下线
                                        Self::logout_ok(device_id, &req, tx, bill).await
                                    }
                                } else {
                                    Self::unauthorized(&req, tx, bill).await
                                }
                            }
                            Err(err) => {
                                warn!("device_id = {},register request err ={}", device_id, err);
                                Self::unauthorized(&req, tx, bill).await
                            }
                        }
                    }
                }
            }
        }
    }
    async fn login_ok(
        device_id: &String,
        req: &Request,
        tx: Sender<SipPackage>,
        bill: &Association,
        oauth: GmvOauth,
    ) -> GlobalResult<()> {
        RWContext::insert(device_id, oauth.heartbeat_sec, bill);
        let gmv_device = GmvDevice::build_gmv_device(&req)?;
        gmv_device.insert_single_gmv_device_by_register().await?;
        let ok_response = ResponseBuilder::build_ok_response(&req, bill.get_remote_addr())?;
        let sip_package = SipPackage::build_response(ok_response, bill.clone());
        let _ = tx
            .clone()
            .send(sip_package)
            .await
            .hand_log(|msg| warn!("{msg}"));
        tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;
        cmd::CmdQuery::query_device_info(device_id).await?;
        cmd::CmdQuery::query_device_catalog(device_id).await?;
        cmd::CmdQuery::subscribe_device_catalog(device_id, gmv_device.register_expires).await
    }

    async fn logout_ok(
        device_id: &String,
        req: &Request,
        tx: Sender<SipPackage>,
        bill: &Association,
    ) -> GlobalResult<()> {
        let ok_response = ResponseBuilder::build_logout_ok_response(&req, bill.get_remote_addr())?;
        let sip_package = SipPackage::build_response(ok_response, bill.clone());
        let _ = tx
            .clone()
            .send(sip_package)
            .await
            .hand_log(|msg| warn!("{msg}"));
        GmvDevice::update_gmv_device_status_by_device_id(device_id, 0).await?;
        RWContext::clean_rw_session_and_net(device_id);
        Ok(())
    }

    async fn unauthorized(
        req: &Request,
        tx: Sender<SipPackage>,
        bill: &Association,
    ) -> GlobalResult<()> {
        let unauthorized_register_response =
            ResponseBuilder::unauthorized_register_response(&req, bill.get_remote_addr())?;
        let sip_package = SipPackage::build_response(unauthorized_register_response, bill.clone());
        let _ = tx
            .clone()
            .send(sip_package)
            .await
            .hand_log(|msg| warn!("{msg}"));
        Ok(())
    }
}

struct Message;

impl Message {
    async fn process(
        device_id: &String,
        req: Request,
        tx: Sender<SipPackage>,
        bill: &Association,
    ) -> GlobalResult<()> {
        use parser::xml::*;
        match parse_xlm_to_vec(&req.body) {
            Ok(vs) => {
                let response = ResponseBuilder::build_ok_response(&req, bill.get_remote_addr())?;
                for (k, v) in &vs {
                    if MESSAGE_TYPE.contains(&&k[..]) {
                        match &v[..] {
                            MESSAGE_KEEP_ALIVE => {
                                Self::keep_alive(device_id, vs, bill).await;
                            }
                            MESSAGE_CONFIG_DOWNLOAD => {}
                            MESSAGE_NOTIFY_CATALOG => {
                                Self::device_catalog(device_id, vs).await;
                            }
                            MESSAGE_DEVICE_INFO => {
                                Self::device_info(vs).await;
                            }
                            MESSAGE_ALARM => {
                                let _ = Self::message_notify_alarm(device_id, vs).await;
                            }
                            MESSAGE_RECORD_INFO => {}
                            MESSAGE_MEDIA_STATUS => {}
                            MESSAGE_BROADCAST => {}
                            MESSAGE_DEVICE_STATUS => {}
                            MESSAGE_DEVICE_CONTROL => {}
                            MESSAGE_DEVICE_CONFIG => {}
                            MESSAGE_PRESET_QUERY => {}
                            MESSAGE_UPLOAD_SNAPSHOT_FINISHED => {
                                Self::handle_snapshot_image(vs)
                            }
                            _ => {
                                warn!("device_id = {};message -- > {} 不支持。", device_id, v)
                            }
                        }
                        break;
                    }
                }
                let sip_package = SipPackage::build_response(response, bill.clone());
                let _ = tx
                    .clone()
                    .send(sip_package)
                    .await
                    .hand_log(|msg| warn!("{msg}"));
                Ok(())
            }
            Err(err) => {
                let val = encoding::decode(&req.body, GB18030).hand_log(|msg| error!("{msg}"))?;
                Err(SysErr(anyhow!("xml解析失败: {err:?}; xml = [{}]", val)))?
            }
        }
    }

    fn handle_snapshot_image(vs:Vec<(String,String)>){
        if let Some((_, v)) = vs.iter().find(|(k, _)| k == MESSAGE_UPLOAD_SNAPSHOT_SESSION_ID) {
            let key = format!("{}{}", KEY_SNAPSHOT_IMAGE, v);
            if let Some((_, Some(tx))) = state::session::Cache::state_get(&key) {
                let _ = tx.try_send(Some(Bytes::new())).hand_log(|msg| error!("{msg}"));
            }
        }
    }
    async fn keep_alive(device_id: &String, vs: Vec<(String, String)>, bill: &Association) {
        use parser::xml::{NOTIFY_DEVICE_ID, NOTIFY_STATUS};
        if base::log::max_level() <= LevelFilter::Info {
            let (mut device_id, mut status) = (String::new(), String::new());
            for (k, v) in &vs {
                match &k[..] {
                    NOTIFY_DEVICE_ID => {
                        device_id = v.to_string();
                    }
                    NOTIFY_STATUS => {
                        status = v.to_string();
                    }
                    _ => {}
                }
            }
            debug!(
                "keep_alive: device_id = {},status = {}",
                &device_id, &status
            );
        }
        RWContext::keep_alive(device_id, bill.clone());
    }

    async fn device_info(vs: Vec<(String, String)>) {
        let _ = GmvDeviceExt::update_gmv_device_ext_info(vs)
            .await
            .hand_log(|msg| error!("{msg}"));
    }

    async fn device_catalog(device_id: &String, vs: Vec<(String, String)>) {
        if let Ok(_arr) = GmvDeviceChannel::insert_gmv_device_channel(device_id, vs)
            .await
            .hand_log(|msg| error!("{msg}"))
        {
            //通过预置位探测是否有云台可用
            // for dc in arr {
            //     let _ = CmdQuery::query_preset(&dc.device_id, Some(&dc.channel_id)).await.hand_log(|msg| error!("{msg}"));
            // }
        }
    }

    async fn message_notify_alarm(
        device_id: &String,
        vs: Vec<(String, String)>,
    ) -> GlobalResult<()> {
        let mut info = AlarmInfo::kv_to_model(vs)?;
        info.deviceId = device_id.clone();
        let conf = AlarmConf::get_alarm_conf();
        let pretend = HttpClient::template(conf.push_url.as_ref().unwrap())?;
        let _ = pretend
            .call_alarm_info(&info)
            .await
            .hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }
}

struct Notify;

impl Notify {
    async fn process(
        device_id: &String,
        req: Request,
        tx: Sender<SipPackage>,
        bill: &Association,
    ) -> GlobalResult<()> {
        use parser::xml::*;
        match parse_xlm_to_vec(&req.body) {
            Ok(vs) => {
                let response = ResponseBuilder::build_ok_response(&req, bill.get_remote_addr())?;
                for (k, v) in &vs {
                    if MESSAGE_TYPE.contains(&&**k) {
                        match &v[..] {
                            MESSAGE_NOTIFY_CATALOG => {
                                GmvDeviceChannel::insert_gmv_device_channel(device_id, vs).await?;
                            }
                            _ => {
                                debug!("cmdType暂不支持;{k} : {v}");
                            }
                        }
                        break;
                    }
                }
                let sip_package = SipPackage::build_response(response, bill.clone());
                let _ = tx
                    .clone()
                    .send(sip_package)
                    .await
                    .hand_log(|msg| warn!("{msg}"));
                Ok(())
            }
            Err(err) => {
                let val = encoding::decode(&req.body, GB18030).hand_log(|msg| error!("{msg}"))?;
                Err(SysErr(anyhow!("xml解析失败: {err:?}; xml = [{}]", val)))?
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use base::chrono::Local;
    use base::log::LevelFilter;
    use rsip::headers::Authorization;
    use rsip::headers::ToTypedHeader;
    use rsip::services::DigestGenerator;
    use rsip::{headers, Method};

    #[test]
    fn test_time_stamp() {
        println!("= {}", Local::now().timestamp());
    }

    #[test]
    fn test_log_level() {
        // 设置日志级别为 Info
        base::log::set_max_level(LevelFilter::Info);

        // 获取当前日志级别
        let max_level = base::log::max_level();
        println!("Current log level: {:?}", max_level);

        // 判断当前级别是否达到指定级别
        if base::log::max_level() >= LevelFilter::Debug {
            println!("Debug messages are enabled");
        } else {
            println!("Debug messages are disabled");
        }
    }

    #[test]
    fn test_authorization() {
        let auth = r#"Digest username="34020000001110000009", realm="3402000000", nonce="3ca91737e8d546edbc86ff1c0042dde8", uri="sip:34020000002000000001@3402000000", response="5ffa4f2a5445d462b5a862a5b6366b9a", algorithm=MD5, cnonce="0a4f113b", qop=auth, nc=00000001"#;
        let authorization = Authorization::try_from(auth).unwrap();
        let au: headers::typed::Authorization = authorization.typed().unwrap();
        let dsg = DigestGenerator::from(&au, "Ab123456", &Method::Register);
        let x = dsg.verify(&au.response);
        println!("{:?}", x);
    }
}
