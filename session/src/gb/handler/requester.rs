use common::log::{debug, info, LevelFilter};
use quick_xml::encoding;
use encoding_rs::GB18030;
use rsip::{Method, Request};
use rsip::headers::ToTypedHeader;
use rsip::message::HeadersExt;
use rsip::services::DigestGenerator;

use anyhow::anyhow;
use common::bytes::Bytes;
use common::chrono::{Duration, Local};
use common::exception::{GlobalResult, GlobalResultExt};
use common::exception::GlobalError::SysErr;
use common::log::{error, warn};
use common::net::state::{Association, Package, Zip};
use common::tokio::sync::mpsc::Sender;

use crate::gb::handler::{cmd, parser};
use crate::gb::handler::builder::ResponseBuilder;
use crate::gb::handler::parser::xml::KV2Model;
use crate::gb::shared::rw::RWSession;
use crate::general::model::AlarmInfo;
use crate::service::callback;
use crate::storage::entity::{GmvDevice, GmvDeviceChannel, GmvDeviceExt, GmvOauth};
use crate::storage::mapper;

pub async fn hand_request(req: Request, tx: Sender<Zip>, bill: &Association) -> GlobalResult<()> {
    let device_id = parser::header::get_device_id_by_request(&req)?;
    //校验设备是否注册
    if req.method == Method::Register {
        let _ = Register::process(&device_id, req, tx, bill).await.hand_log(|msg| error!("设备 = [{}],注册失败;err={}",&device_id,msg));
        Ok(())
    } else {
        match State::check_session(tx.clone(), bill, &device_id).await? {
            State::Usable | State::ReCache => {
                match req.method {
                    Method::Ack => { Ok(()) }
                    Method::Bye => { Ok(()) }
                    Method::Cancel => { Ok(()) }
                    Method::Info => { Ok(()) }
                    Method::Invite => { Ok(()) }
                    Method::Message => { Message::process(&device_id, req, tx.clone(), bill).await }
                    Method::Notify => { Notify::process(&device_id, req, tx.clone(), bill).await }
                    Method::Options => { Ok(()) }
                    Method::PRack => { Ok(()) }
                    Method::Publish => { Ok(()) }
                    Method::Refer => { Ok(()) }
                    Method::Subscribe => { Ok(()) }
                    Method::Update => { Ok(()) }
                    _ => {
                        info!("invalid method");
                        Ok(())
                    }
                }
            }
            State::Expired => {
                let unregister_response = ResponseBuilder::build_401_response(&req, bill.get_remote_addr())?;
                let zip = Zip::build_data(Package::new(bill.clone(), Bytes::from(unregister_response)));
                let _ = tx.clone().send(zip).await.hand_log(|msg| error!("{msg}"));
                Ok(())
            }
            State::Invalid => { Ok(()) }
        }
    }
}

#[derive(Eq, PartialEq)]
enum State {
    //可用的
    Usable,
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
    async fn check_session(tx: Sender<Zip>, bill: &Association, device_id: &String) -> GlobalResult<State> {
        if RWSession::get_device_id_by_association(bill).is_some() || RWSession::has_session_by_device_id(device_id) {
            Ok(State::Usable)
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
                            RWSession::insert(device_id, tx, heart, bill);
                            //如果设备是离线状态，则更新为在线
                            if on == 0 {
                                GmvDevice::update_gmv_device_status_by_device_id(device_id, 1).await?;
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
    async fn process(device_id: &String, req: Request, tx: Sender<Zip>, bill: &Association) -> GlobalResult<()> {
        let oauth = GmvOauth::read_gmv_oauth_by_device_id(device_id).await?
            .ok_or(SysErr(anyhow!("device id = [{}] 未知设备，拒绝接入",device_id)))
            .hand_log(|msg| warn!("{msg}"))?;
        if oauth.get_status() == &0u8 {
            warn!("device id = [{}] 未启用设备，拒绝接入",device_id);
            return Ok(());
        }
        match oauth.get_pwd_check() {
            //不进行鉴权校验
            &0u8 => {
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
                    None => {
                        Self::unauthorized(&req, tx, bill).await
                    }
                    Some(auth) => {
                        match auth.typed() {
                            Ok(au) => {
                                let pwd_opt = oauth.get_pwd().clone().unwrap_or_default();
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
                                warn!("device_id = {},register request err ={}",device_id,err);
                                Self::unauthorized(&req, tx, bill).await
                            }
                        }
                    }
                }
            }
        }
    }
    async fn login_ok(device_id: &String, req: &Request, tx: Sender<Zip>, bill: &Association, oauth: GmvOauth) -> GlobalResult<()> {
        RWSession::insert(device_id, tx.clone(), *oauth.get_heartbeat_sec(), bill);
        let gmv_device = GmvDevice::build_gmv_device(&req)?;
        gmv_device.insert_single_gmv_device_by_register().await?;
        let ok_response = ResponseBuilder::build_register_ok_response(&req, bill.get_remote_addr())?;
        let zip = Zip::build_data(Package::new(bill.clone(), Bytes::from(ok_response)));
        let _ = tx.clone().send(zip).await.hand_log(|msg| warn!("{msg}"));

        // query subscribe device msg
        cmd::CmdQuery::lazy_query_device_info(device_id).await?;
        // cmd::CmdQuery::lazy_subscribe_device_catalog(device_id).await?;
        cmd::CmdQuery::lazy_query_device_catalog(device_id).await
    }

    async fn logout_ok(device_id: &String, req: &Request, tx: Sender<Zip>, bill: &Association) -> GlobalResult<()> {
        let ok_response = ResponseBuilder::build_logout_ok_response(&req, bill.get_remote_addr())?;
        let zip = Zip::build_data(Package::new(bill.clone(), Bytes::from(ok_response)));
        let _ = tx.clone().send(zip).await.hand_log(|msg| warn!("{msg}"));
        GmvDevice::update_gmv_device_status_by_device_id(device_id, 0).await?;
        RWSession::clean_rw_session_and_net(device_id).await;
        Ok(())
    }

    async fn unauthorized(req: &Request, tx: Sender<Zip>, bill: &Association) -> GlobalResult<()> {
        let unauthorized_register_response = ResponseBuilder::unauthorized_register_response(&req, bill.get_remote_addr())?;
        let zip = Zip::build_data(Package::new(bill.clone(), Bytes::from(unauthorized_register_response)));
        let _ = tx.clone().send(zip).await.hand_log(|msg| error!("{msg}"));
        Ok(())
    }
}

struct Message;

impl Message {
    async fn process(device_id: &String, req: Request, tx: Sender<Zip>, bill: &Association) -> GlobalResult<()> {
        use parser::xml::*;
        match parse_xlm_to_vec(&req.body) {
            Ok(vs) => {
                let response = ResponseBuilder::build_register_ok_response(&req, bill.get_remote_addr())?;
                for (k, v) in &vs {
                    if MESSAGE_TYPE.contains(&&k[..]) {
                        match &v[..] {
                            MESSAGE_KEEP_ALIVE => { Self::keep_alive(device_id, vs, bill).await; }
                            MESSAGE_CONFIG_DOWNLOAD => {}
                            MESSAGE_NOTIFY_CATALOG => { Self::device_catalog(device_id, vs).await; }
                            MESSAGE_DEVICE_INFO => { Self::device_info(vs).await; }
                            MESSAGE_ALARM => { let _ = Self::message_notify_alarm(device_id, vs).await; }
                            MESSAGE_RECORD_INFO => {}
                            MESSAGE_MEDIA_STATUS => {}
                            MESSAGE_BROADCAST => {}
                            MESSAGE_DEVICE_STATUS => {}
                            MESSAGE_DEVICE_CONTROL => {}
                            MESSAGE_DEVICE_CONFIG => {}
                            MESSAGE_PRESET_QUERY => {}
                            _ => {
                                warn!("device_id = {};message -- > {} 不支持。", device_id,v)
                            }
                        }
                        let zip = Zip::build_data(Package::new(bill.clone(), Bytes::from(response)));
                        let _ = tx.clone().send(zip).await.hand_log(|msg| error!("{msg}"));
                        break;
                    }
                }
                Ok(())
            }
            Err(err) => {
                let val = encoding::decode(&req.body, GB18030).hand_log(|msg| error!("{msg}"))?;
                Err(SysErr(anyhow!("xml解析失败: {err:?}; xml = [{}]",val)))?
            }
        }
    }
    async fn keep_alive(device_id: &String, vs: Vec<(String, String)>, bill: &Association) {
        use parser::xml::{NOTIFY_DEVICE_ID, NOTIFY_STATUS};
        if common::log::max_level() <= LevelFilter::Info {
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
            debug!("keep_alive: device_id = {},status = {}",&device_id,&status);
        }
        RWSession::heart(device_id, bill.clone());
    }

    async fn device_info(vs: Vec<(String, String)>) {
        let _ = GmvDeviceExt::update_gmv_device_ext_info(vs).await.hand_log(|msg| error!("{msg}"));
    }

    async fn device_catalog(device_id: &String, vs: Vec<(String, String)>) {
        if let Ok(_arr) = GmvDeviceChannel::insert_gmv_device_channel(device_id, vs).await.hand_log(|msg| error!("{msg}")) {
            //通过预置位探测是否有云台可用
            // for dc in arr {
            //     let _ = CmdQuery::query_preset(&dc.device_id, Some(&dc.channel_id)).await.hand_log(|msg| error!("{msg}"));
            // }
        }
    }

    async fn message_notify_alarm(device_id: &String, vs: Vec<(String, String)>) -> GlobalResult<()> {
        let mut info = AlarmInfo::kv_to_model(vs)?;
        info.deviceId = device_id.clone();
        callback::call_alarm_info(&info).await?;
        Ok(())
    }
}

struct Notify;

impl Notify {
    async fn process(device_id: &String, req: Request, tx: Sender<Zip>, bill: &Association) -> GlobalResult<()> {
        use parser::xml::*;
        match parse_xlm_to_vec(&req.body) {
            Ok(vs) => {
                let response = ResponseBuilder::build_register_ok_response(&req, bill.get_remote_addr())?;
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
                        let zip = Zip::build_data(Package::new(bill.clone(), Bytes::from(response)));
                        let _ = tx.clone().send(zip).await.hand_log(|msg| error!("{msg}"));
                        break;
                    }
                }
                Ok(())
            }
            Err(err) => {
                let val = encoding::decode(&req.body, GB18030).hand_log(|msg| error!("{msg}"))?;
                Err(SysErr(anyhow!("xml解析失败: {err:?}; xml = [{}]",val)))?
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rsip::headers::Authorization;
    use rsip::headers::ToTypedHeader;
    use rsip::{headers, Method};
    use rsip::services::DigestGenerator;
    use common::log::LevelFilter;
    use common::chrono::Local;

    #[test]
    fn test_time_stamp() {
        println!("= {}", Local::now().timestamp());
    }

    #[test]
    fn test_log_level() {
        // 设置日志级别为 Info
        common::log::set_max_level(LevelFilter::Info);

        // 获取当前日志级别
        let max_level = common::log::max_level();
        println!("Current log level: {:?}", max_level);

        // 判断当前级别是否达到指定级别
        if common::log::max_level() >= LevelFilter::Debug {
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