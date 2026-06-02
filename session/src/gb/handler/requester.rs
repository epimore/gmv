use base::log::{LevelFilter, debug, info};
use rsip::headers::ToTypedHeader;
use rsip::message::HeadersExt;
use rsip::services::DigestGenerator;
use rsip::{Method, Request};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use base::chrono::{Duration as TimeDelta, Local};
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::{error, warn};
use base::net::state::Association;
use base::tokio;
use base::tokio::sync::mpsc::Sender;
use parking_lot::RwLock;
use uuid::Uuid;

use crate::gb::core::rw::RWContext;
use crate::gb::depot::SipPackage;
use crate::gb::handler::builder::ResponseBuilder;
use crate::gb::handler::parser::xml::{KV2Model, MESSAGE_UPLOAD_SNAPSHOT_SESSION_ID};
use crate::gb::handler::{cmd, parser};
use crate::http::client::{HttpBiz, HttpClient};
use crate::service::KEY_SNAPSHOT_IMAGE;
use crate::state::AlarmConf;
use crate::state::model::AlarmInfo;
use crate::storage::entity::{DeviceStatus, GmvDevice, GmvDeviceChannel, GmvDeviceExt, GmvOauth};
use crate::{register, state};

const REGISTER_NONCE_TTL: Duration = Duration::from_secs(300);
const MAX_REGISTER_NONCES: usize = 100_000;

struct RegisterNonce {
    device_id: Arc<str>,
    expires_at: Instant,
    max_nc: u32,
}

struct RegisterNonceCache {
    inner: RwLock<HashMap<String, RegisterNonce>>,
}

impl RegisterNonceCache {
    fn global() -> &'static Self {
        static CACHE: OnceLock<RegisterNonceCache> = OnceLock::new();
        CACHE.get_or_init(|| RegisterNonceCache {
            inner: RwLock::new(HashMap::new()),
        })
    }

    fn issue(device_id: &Arc<str>) -> String {
        let nonce = Uuid::new_v4().as_simple().to_string();
        let mut inner = Self::global().inner.write();
        Self::clean_locked(&mut inner);
        if inner.len() >= MAX_REGISTER_NONCES {
            Self::remove_one_locked(&mut inner);
        }
        inner.insert(
            nonce.clone(),
            RegisterNonce {
                device_id: device_id.clone(),
                expires_at: Instant::now() + REGISTER_NONCE_TTL,
                max_nc: 0,
            },
        );
        nonce
    }

    fn validate_and_update(device_id: &Arc<str>, nonce: &str, nc: Option<&str>) -> bool {
        let mut inner = Self::global().inner.write();
        Self::clean_locked(&mut inner);
        let Some(state) = inner.get_mut(nonce) else {
            return false;
        };
        if state.device_id != *device_id || state.expires_at <= Instant::now() {
            return false;
        }
        let Some(nc) = nc.and_then(|nc| u32::from_str_radix(nc.trim(), 16).ok()) else {
            if state.max_nc == 0 {
                state.max_nc = 1;
                return true;
            }
            return false;
        };
        if nc <= state.max_nc {
            return false;
        }
        state.max_nc = nc;
        true
    }

    fn clean_locked(inner: &mut HashMap<String, RegisterNonce>) {
        let now = Instant::now();
        inner.retain(|_, item| item.expires_at > now);
    }

    fn remove_one_locked(inner: &mut HashMap<String, RegisterNonce>) {
        if let Some(key) = inner
            .iter()
            .min_by_key(|(_, item)| item.expires_at)
            .map(|(key, _)| key.clone())
        {
            inner.remove(&key);
        }
    }
}

async fn send_status_response(
    req: &Request,
    tx: &Sender<SipPackage>,
    bill: &Association,
    status_code: u16,
) -> GlobalResult<()> {
    let response = ResponseBuilder::build_status_response(req, &bill.remote_addr, status_code)?;
    let sip_package = SipPackage::build_response(response, bill.clone());
    let _ = tx
        .clone()
        .send(sip_package)
        .await
        .hand_log(|msg| warn!("{msg}"));
    Ok(())
}

async fn send_ok_response(
    req: &Request,
    tx: &Sender<SipPackage>,
    bill: &Association,
) -> GlobalResult<()> {
    let response = ResponseBuilder::build_ok_response(req, &bill.remote_addr)?;
    let sip_package = SipPackage::build_response(response, bill.clone());
    let _ = tx
        .clone()
        .send(sip_package)
        .await
        .hand_log(|msg| warn!("{msg}"));
    Ok(())
}

fn is_keepalive_request(req: &Request) -> bool {
    req.method == Method::Message
        && req
            .body
            .windows(b"<CmdType>Keepalive</CmdType>".len())
            .any(|window| window == b"<CmdType>Keepalive</CmdType>")
}

fn digest_param(header: &str, name: &str) -> Option<String> {
    for part in header.split(',') {
        let part = part.trim();
        let Some((key, val)) = part.split_once('=') else {
            continue;
        };
        let key = key.split_whitespace().last().unwrap_or(key).trim();
        if key.eq_ignore_ascii_case(name) {
            return Some(val.trim().trim_matches('"').to_string());
        }
    }
    None
}

pub async fn hand_request(
    req: Request,
    tx: Sender<SipPackage>,
    bill: Association,
) -> GlobalResult<()> {
    let device_id = parser::header::get_device_id_by_request(&req)?;
    let device_id: Arc<str> = Arc::from(device_id);
    //校验设备是否注册
    if req.method == Method::Register {
        let _ = Register::process(device_id.clone(), req, tx, &bill)
            .await
            .hand_log(|msg| error!("设备 = [{}],注册失败;err={}", &device_id, msg));
        Ok(())
    } else {
        match State::check_session(&bill, device_id.clone(), &req).await? {
            State::Enable | State::ReCache => match req.method {
                Method::Ack => Ok(()),
                Method::Bye => send_ok_response(&req, &tx, &bill).await,
                Method::Cancel => send_ok_response(&req, &tx, &bill).await,
                Method::Info => Ok(()),
                Method::Invite => Ok(()),
                Method::Message => Message::process(&device_id, req, tx.clone(), &bill).await,
                Method::Notify => Notify::process(&device_id, req, tx.clone(), &bill).await,
                Method::Options => send_ok_response(&req, &tx, &bill).await,
                Method::PRack => Ok(()),
                Method::Publish => Ok(()),
                Method::Refer => Ok(()),
                Method::Subscribe => Ok(()),
                Method::Update => Ok(()),
                _ => {
                    info!("invalid method");
                    send_status_response(&req, &tx, &bill, 405).await
                }
            },
            State::Expired => {
                let unregister_response =
                    ResponseBuilder::build_401_response(&req, &bill.remote_addr)?;
                let sip_package = SipPackage::build_response(unregister_response, bill);
                let _ = tx
                    .clone()
                    .send(sip_package)
                    .await
                    .hand_log(|msg| warn!("{msg}"));
                Ok(())
            }
            State::Forbidden => send_status_response(&req, &tx, &bill, 403).await,
            State::NotFound => send_status_response(&req, &tx, &bill, 404).await,
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
    Forbidden,
    NotFound,
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
        device_id: Arc<str>,
        req: &Request,
    ) -> GlobalResult<State> {
        let device_key = device_id.to_string();
        if let Some(mapped_device_id) = RWContext::get_device_id_by_association(bill) {
            if mapped_device_id != device_key {
                warn!(
                    "association device mismatch: association={}, mapped={}, request={}",
                    bill, mapped_device_id, device_key
                );
                return Ok(State::Forbidden);
            }
            RWContext::keep_alive(&device_key, bill.clone());
            return Ok(State::Enable);
        }
        if let Some((_uri, association, _lr)) = RWContext::get_ds_by_device_id(&device_key) {
            if association == *bill || is_keepalive_request(req) {
                RWContext::keep_alive(&device_key, bill.clone());
                return Ok(State::Enable);
            }
            warn!(
                "request association mismatch: device_id={}, registered={}, request={}",
                device_key, association, bill
            );
            return Ok(State::Forbidden);
        }
        if RWContext::has_session_by_device_id(&device_key) {
            RWContext::keep_alive(&device_key, bill.clone());
            Ok(State::Enable)
        } else {
            match DeviceStatus::get_device_status(&device_key).await? {
                None => {
                    warn!("未知设备：{device_id}");
                    Ok(State::NotFound)
                }
                Some(ds) => {
                    let DeviceStatus {
                        heartbeat,
                        enable,
                        expires,
                        register_time,
                        online,
                        contact_uri,
                        lr,
                    } = ds;
                    if enable == 0 {
                        warn!("未启用设备: {device_id}");
                        Ok(State::Forbidden)
                    } else {
                        //判断是否在注册有效期内
                        if register_time + TimeDelta::seconds(expires as i64)
                            > Local::now().naive_local()
                        {
                            let mut device_session = register::core::DeviceSession::build(
                                contact_uri,
                                bill.clone(),
                                heartbeat,
                                std::time::Duration::from_secs(expires as u64),
                            );
                            if lr == 1 {
                                device_session.enable_lr();
                            }
                            //刷新缓存
                            let _ = register::core::Register::register_device(
                                device_id.clone(),
                                device_session,
                            );
                            // RWContext::insert(device_id, device_session);
                            //如果设备是离线状态，则更新为在线
                            if online == 0 {
                                GmvDevice::update_gmv_device_status_by_device_id(
                                    device_id.as_ref(),
                                    1,
                                )
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
        device_id: Arc<str>,
        req: Request,
        tx: Sender<SipPackage>,
        bill: &Association,
    ) -> GlobalResult<()> {
        let Some(oauth) = GmvOauth::read_gmv_oauth_by_device_id(&device_id.to_string()).await?
        else {
            warn!("device id = [{}] unknown, reject access", device_id);
            send_status_response(&req, &tx, bill, 404).await?;
            return Ok(());
        };
        if oauth.status == 0u8 {
            warn!("device id = [{}] disabled, reject access", device_id);
            send_status_response(&req, &tx, bill, 403).await?;
            return Ok(());
        }
        match oauth.pwd_check {
            0u8 => Self::process_authorized(device_id, &req, &tx, bill, oauth).await,
            _ => match req.authorization_header() {
                None => Self::unauthorized(&device_id, &req, tx, bill).await,
                Some(auth) => match auth.typed() {
                    Ok(au) => {
                        let nc = digest_param(&auth.to_string(), "nc");
                        let pwd_opt = oauth.pwd.clone().unwrap_or_default();
                        let dsg = DigestGenerator::from(&au, &pwd_opt, &Method::Register);
                        if dsg.verify(&au.response)
                            && RegisterNonceCache::validate_and_update(
                                &device_id,
                                au.nonce.as_str(),
                                nc.as_deref(),
                            )
                        {
                            Self::process_authorized(device_id, &req, &tx, bill, oauth).await
                        } else {
                            Self::unauthorized(&device_id, &req, tx, bill).await
                        }
                    }
                    Err(err) => {
                        warn!("device_id = {}, register request err = {}", device_id, err);
                        Self::unauthorized(&device_id, &req, tx, bill).await
                    }
                },
            },
        }
    }

    async fn process_authorized(
        device_id: Arc<str>,
        req: &Request,
        tx: &Sender<SipPackage>,
        bill: &Association,
        oauth: GmvOauth,
    ) -> GlobalResult<()> {
        let expires = match parser::header::get_expires(req) {
            Ok(expires) => expires,
            Err(err) => {
                warn!("device_id = {}; invalid register expires: {err}", device_id);
                send_status_response(req, tx, bill, 400).await?;
                return Ok(());
            }
        };
        if expires > 60 * 60 * 24 {
            warn!("device_id = {}; register expires exceeds 86400", device_id);
            send_status_response(req, tx, bill, 400).await?;
            return Ok(());
        }
        if expires > 0 {
            match Self::login_ok(
                device_id.clone(),
                req,
                tx.clone(),
                bill,
                oauth,
                std::time::Duration::from_secs(expires as u64),
            )
            .await
            {
                Ok(()) => Ok(()),
                Err(err) => {
                    error!("device_id = {}; register login failed: {err}", device_id);
                    send_status_response(req, tx, bill, 500).await
                }
            }
        } else {
            Self::logout_ok(&device_id, req, tx.clone(), bill).await
        }
    }
    async fn login_ok(
        device_id: Arc<str>,
        req: &Request,
        tx: Sender<SipPackage>,
        bill: &Association,
        oauth: GmvOauth,
        registration_duration: std::time::Duration,
    ) -> GlobalResult<()> {
        let contact_uri = parser::header::get_contact_uri(req)?;
        let mut device_session = register::core::DeviceSession::build(
            contact_uri,
            bill.clone(),
            oauth.heartbeat_sec,
            registration_duration,
        );
        if parser::header::enable_lr(req)? == 1 {
            device_session.enable_lr();
        }
        register::core::Register::register_device(device_id.clone(), device_session)?;
        // RWContext::insert(device_id, device_session);
        let gmv_device = GmvDevice::build_gmv_device(&req)?;
        gmv_device.insert_single_gmv_device_by_register().await?;
        let ok_response = ResponseBuilder::build_register_ok_response(
            &req,
            &bill.remote_addr,
            registration_duration.as_secs() as u32,
        )?;
        let sip_package = SipPackage::build_response(ok_response, bill.clone());
        let _ = tx
            .clone()
            .send(sip_package)
            .await
            .hand_log(|msg| warn!("{msg}"));
        tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;
        let device_key = device_id.to_string();
        let _ = cmd::CmdQuery::query_device_info(&device_key)
            .await
            .hand_log(|msg| warn!("query device info after register failed: {msg}"));
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        let _ = cmd::CmdQuery::query_device_catalog(&device_key)
            .await
            .hand_log(|msg| warn!("query device catalog after register failed: {msg}"));
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        let _ = cmd::CmdQuery::subscribe_device_catalog(&device_key, gmv_device.register_expires)
            .await
            .hand_log(|msg| warn!("subscribe device catalog after register failed: {msg}"));
        Ok(())
    }

    async fn logout_ok(
        device_id: &Arc<str>,
        req: &Request,
        tx: Sender<SipPackage>,
        bill: &Association,
    ) -> GlobalResult<()> {
        let ok_response = ResponseBuilder::build_logout_ok_response(&req, &bill.remote_addr)?;
        let sip_package = SipPackage::build_response(ok_response, bill.clone());
        let _ = tx
            .clone()
            .send(sip_package)
            .await
            .hand_log(|msg| warn!("{msg}"));
        register::core::Register::remove_device(device_id);
        GmvDevice::update_gmv_device_status_by_device_id(device_id, 0).await?;
        // RWContext::clean_rw_session_and_net(device_id);
        Ok(())
    }

    async fn unauthorized(
        device_id: &Arc<str>,
        req: &Request,
        tx: Sender<SipPackage>,
        bill: &Association,
    ) -> GlobalResult<()> {
        let nonce = RegisterNonceCache::issue(device_id);
        let unauthorized_register_response =
            ResponseBuilder::unauthorized_register_response(&req, &bill.remote_addr, nonce)?;
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
        device_id: &Arc<str>,
        req: Request,
        tx: Sender<SipPackage>,
        bill: &Association,
    ) -> GlobalResult<()> {
        use parser::xml::*;
        match parse_xlm_to_vec(&req.body) {
            Ok(vs) => {
                let response = ResponseBuilder::build_ok_response(&req, &bill.remote_addr)?;
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
                            MESSAGE_UPLOAD_SNAPSHOT_FINISHED => Self::handle_snapshot_image(vs),
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
                warn!("device_id = {}; SIP XML parse failed: {err:?}", device_id);
                send_status_response(&req, &tx, bill, 400).await
            }
        }
    }

    fn handle_snapshot_image(vs: Vec<(String, String)>) {
        if let Some((_, v)) = vs
            .iter()
            .find(|(k, _)| k == MESSAGE_UPLOAD_SNAPSHOT_SESSION_ID)
        {
            let key = format!("{}{}", KEY_SNAPSHOT_IMAGE, v);
            let _ = state::session::Cache::notify_snapshot_wait(&key);
        }
    }
    async fn keep_alive(device_id: &Arc<str>, vs: Vec<(String, String)>, bill: &Association) {
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
        let _ = register::core::Register::device_heart(device_id, bill.clone());
        // RWContext::keep_alive(device_id, bill.clone());
    }

    async fn device_info(vs: Vec<(String, String)>) {
        let _ = GmvDeviceExt::update_gmv_device_ext_info(vs)
            .await
            .hand_log(|msg| error!("{msg}"));
    }

    async fn device_catalog(device_id: &Arc<str>, vs: Vec<(String, String)>) {
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
        device_id: &Arc<str>,
        vs: Vec<(String, String)>,
    ) -> GlobalResult<()> {
        let mut info = AlarmInfo::kv_to_model(vs)?;
        info.deviceId = device_id.to_string();
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
        device_id: &Arc<str>,
        req: Request,
        tx: Sender<SipPackage>,
        bill: &Association,
    ) -> GlobalResult<()> {
        use parser::xml::*;
        match parse_xlm_to_vec(&req.body) {
            Ok(vs) => {
                let response = ResponseBuilder::build_ok_response(&req, &bill.remote_addr)?;
                for (k, v) in &vs {
                    if MESSAGE_TYPE.contains(&&**k) {
                        match &v[..] {
                            MESSAGE_NOTIFY_CATALOG => {
                                let _ = GmvDeviceChannel::insert_gmv_device_channel(device_id, vs)
                                    .await
                                    .hand_log(|msg| error!("{msg}"));
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
                warn!("device_id = {}; SIP XML parse failed: {err:?}", device_id);
                send_status_response(&req, &tx, bill, 400).await
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
    use rsip::{Method, headers};

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
