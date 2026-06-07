use base::log::{LevelFilter, debug, info};
use rsip::headers::ToTypedHeader;
use rsip::message::HeadersExt;
use rsip::prelude::UntypedHeader;
use rsip::services::DigestGenerator;
use rsip::{Method, Request};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use base::chrono::Local;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::{error, warn};
use base::net::state::Association;
use base::tokio;
use base::tokio::sync::mpsc::Sender;
use parking_lot::RwLock;
use uuid::Uuid;

use crate::gb::core::rw::RWContext;
use crate::gb::depot::SipPackage;
use crate::gb::depot::extract::HeaderItemExt;
use crate::gb::handler::builder::ResponseBuilder;
use crate::gb::handler::parser::xml::{KV2Model, MESSAGE_UPLOAD_SNAPSHOT_SESSION_ID};
use crate::gb::handler::{cmd, parser};
use crate::http::client::{HttpBiz, HttpClient};
use crate::service::KEY_SNAPSHOT_IMAGE;
use crate::service::{api_serv, stream_close};
use crate::state::AlarmConf;
use crate::state::model::AlarmInfo;
use crate::storage::db_task::{self, DbTask};
use crate::storage::entity::{DeviceStatus, GmvDevice, GmvOauth};
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
    if req.method != Method::Message {
        return false;
    }
    parser::xml::parse_xlm_to_vec(&req.body).is_ok_and(|items| {
        items.iter().any(|(key, value)| {
            key.rsplit(',').next() == Some("CmdType")
                && value.trim().eq_ignore_ascii_case("Keepalive")
        })
    })
}

fn normalize_register_expires(expires: u32) -> u32 {
    expires.min(60 * 60 * 24)
}

fn authorization_matches_register(
    authorization: &rsip::headers::typed::Authorization,
    device_id: &str,
    req: &Request,
) -> bool {
    authorization.username == device_id
        && authorization.realm == ResponseBuilder::digest_realm(req)
        && authorization.uri == req.uri
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
                Method::Bye => {
                    let call_id = req
                        .call_id_header()
                        .hand_log(|msg| warn!("{msg}"))?
                        .value()
                        .to_string();
                    if !state::session::Cache::has_dialog_call_id(&call_id) {
                        return send_status_response(&req, &tx, &bill, 481).await;
                    }
                    send_ok_response(&req, &tx, &bill).await?;
                    tokio::spawn(api_serv::peer_dialog_terminated(call_id));
                    Ok(())
                }
                Method::Cancel => send_status_response(&req, &tx, &bill, 481).await,
                Method::Info => send_status_response(&req, &tx, &bill, 405).await,
                Method::Invite => send_status_response(&req, &tx, &bill, 405).await,
                Method::Message => Message::process(&device_id, req, tx.clone(), &bill).await,
                Method::Notify => Notify::process(&device_id, req, tx.clone(), &bill).await,
                Method::Options => {
                    let response =
                        ResponseBuilder::build_options_response(&req, &bill.remote_addr)?;
                    tx.send(SipPackage::build_response(response, bill))
                        .await
                        .hand_log(|msg| warn!("{msg}"))
                }
                Method::PRack
                | Method::Publish
                | Method::Refer
                | Method::Subscribe
                | Method::Update => send_status_response(&req, &tx, &bill, 405).await,
                _ => {
                    info!("invalid method");
                    send_status_response(&req, &tx, &bill, 405).await
                }
            },
            State::Expired => send_status_response(&req, &tx, &bill, 403).await,
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
            if is_keepalive_request(req) {
                RWContext::keep_alive(&device_key, bill.clone());
                Ok(State::Enable)
            } else {
                warn!(
                    "device signaling connection is unavailable: device_id={}, request={}",
                    device_key, bill
                );
                Ok(State::Forbidden)
            }
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
                        online_expire_time,
                        contact_uri,
                        lr,
                    } = ds;
                    if enable == 0 {
                        warn!("未启用设备: {device_id}");
                        Ok(State::Forbidden)
                    } else {
                        //判断是否在在线有效期内
                        let now = Local::now().naive_local();
                        let online_alive =
                            online_expire_time.map_or(false, |expire_time| expire_time > now);
                        if online_alive && is_keepalive_request(req) {
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
                            Ok(State::ReCache)
                        } else if online_alive {
                            Ok(State::Forbidden)
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
                        if authorization_matches_register(&au, device_id.as_ref(), &req)
                            && dsg.verify(&au.response)
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
        let normalized_expires = normalize_register_expires(expires);
        if normalized_expires != expires {
            debug!("device_id = {}; register expires capped at 86400", device_id);
        }
        let expires = normalized_expires;
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
        device_session.set_gb_version(parser::header::get_gb_version(req));
        let registration_call_id = req
            .call_id_header()
            .hand_log(|msg| warn!("{msg}"))?
            .value()
            .to_string();
        let registration_cseq = req
            .cseq_header()
            .hand_log(|msg| warn!("{msg}"))?
            .seq()
            .hand_log(|msg| warn!("{msg}"))?;
        device_session.set_registration_identity(registration_call_id, registration_cseq);
        if parser::header::enable_lr(req)? == 1 {
            device_session.enable_lr();
        }
        register::core::Register::register_device(device_id.clone(), device_session)?;
        // RWContext::insert(device_id, device_session);
        let gmv_device = GmvDevice::build_gmv_device(&req, oauth.heartbeat_sec)?;
        let register_expires = gmv_device.register_expires;
        db_task::submit(DbTask::UpsertDevice(gmv_device));
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
        let device_key = device_id.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;
            let _ = cmd::CmdQuery::query_device_info(&device_key)
                .await
                .hand_log(|msg| warn!("query device info after register failed: {msg}"));
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            let _ = cmd::CmdQuery::query_device_catalog(&device_key)
                .await
                .hand_log(|msg| warn!("query device catalog after register failed: {msg}"));
            tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            let _ = cmd::CmdQuery::subscribe_device_catalog(&device_key, register_expires)
                .await
                .hand_log(|msg| warn!("subscribe device catalog after register failed: {msg}"));
        });
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
        db_task::submit(DbTask::ExpireDeviceOnline {
            device_id: device_id.to_string(),
        });
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
                tx.send(SipPackage::build_response(response, bill.clone()))
                    .await
                    .hand_log(|msg| warn!("{msg}"))?;
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
                            MESSAGE_MEDIA_STATUS => Self::media_status(device_id, &vs),
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

    fn media_status(device_id: &str, items: &[(String, String)]) {
        use parser::xml::{NOTIFY_DEVICE_ID, NOTIFY_TYPE};
        let channel_id = items
            .iter()
            .find_map(|(key, value)| (key == NOTIFY_DEVICE_ID).then_some(value.as_str()));
        let notify_type = items
            .iter()
            .find_map(|(key, value)| (key == NOTIFY_TYPE).then_some(value.as_str()));
        if notify_type.is_some_and(|value| value != "121") {
            return;
        }
        let Some(channel_id) = channel_id else {
            warn!("MediaStatus missing DeviceID: device_id={device_id}");
            return;
        };
        for stream_id in
            state::session::Cache::stream_ids_for_media_status(device_id, channel_id)
        {
            stream_close::begin(stream_id);
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
        if register::core::Register::device_heart(device_id, bill.clone()).is_ok() {
            db_task::submit(DbTask::TouchDeviceHeartbeat {
                device_id: device_id.to_string(),
            });
        }
        // RWContext::keep_alive(device_id, bill.clone());
    }

    async fn device_info(vs: Vec<(String, String)>) {
        db_task::submit(DbTask::UpdateDeviceExtInfo(vs));
    }

    async fn device_catalog(device_id: &Arc<str>, vs: Vec<(String, String)>) {
        db_task::submit(DbTask::InsertDeviceCatalog {
            device_id: device_id.to_string(),
            items: vs,
        });
        //通过预置位探测是否有云台可用
        // for dc in arr {
        //     let _ = CmdQuery::query_preset(&dc.device_id, Some(&dc.channel_id)).await.hand_log(|msg| error!("{msg}"));
        // }
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

#[derive(Debug, Eq, PartialEq)]
enum NotifySubscriptionState {
    Active(Option<u32>),
    Pending(Option<u32>),
    Terminated,
}

impl Notify {
    fn subscription_state(req: &Request) -> GlobalResult<NotifySubscriptionState> {
        let value = req
            .headers
            .iter()
            .find_map(|header| match header {
                rsip::Header::SubscriptionState(value) => {
                    Some(value.value().to_string())
                }
                _ => None,
            })
            .ok_or_else(|| {
                base::exception::GlobalError::new_sys_error(
                    "NOTIFY missing Subscription-State",
                    |msg| warn!("{msg}"),
                )
            })?;
        let mut parts = value.split(';').map(str::trim);
        let state = parts.next().unwrap_or_default();
        let expires = parts.find_map(|part| {
            let (key, value) = part.split_once('=')?;
            key.eq_ignore_ascii_case("expires")
                .then(|| value.parse::<u32>().ok())
                .flatten()
        });
        if state.eq_ignore_ascii_case("active") {
            Ok(NotifySubscriptionState::Active(expires))
        } else if state.eq_ignore_ascii_case("pending") {
            Ok(NotifySubscriptionState::Pending(expires))
        } else if state.eq_ignore_ascii_case("terminated") {
            Ok(NotifySubscriptionState::Terminated)
        } else {
            Err(base::exception::GlobalError::new_sys_error(
                "invalid NOTIFY Subscription-State",
                |msg| warn!("value={value}; {msg}"),
            ))
        }
    }

    fn catalog_dialog(req: &Request, device_id: &str) -> GlobalResult<u64> {
        let call_id = req.call_id()?.value();
        let event = req
            .headers
            .iter()
            .find_map(|header| match header {
                rsip::Header::Event(value) => Some(value.value()),
                _ => None,
            })
            .ok_or_else(|| {
                base::exception::GlobalError::new_sys_error(
                    "NOTIFY missing Event",
                    |msg| warn!("{msg}"),
                )
            })?;
        let remote_tag = req.header_from_tag()?.to_string();
        let local_tag = req.header_to_tag()?.map(|tag| tag.to_string());
        state::session::Cache::catalog_subscription_validate_notify(
            device_id,
            call_id,
            event,
            Some(&remote_tag),
            local_tag.as_deref(),
        )
        .ok_or_else(|| {
            base::exception::GlobalError::new_sys_error(
                "NOTIFY does not match catalog subscription dialog",
                |msg| warn!("device_id={device_id}; call_id={call_id}; {msg}"),
            )
        })
    }

    async fn process(
        device_id: &Arc<str>,
        req: Request,
        tx: Sender<SipPackage>,
        bill: &Association,
    ) -> GlobalResult<()> {
        use parser::xml::*;
        let generation = match Self::catalog_dialog(&req, device_id) {
            Ok(generation) => generation,
            Err(err) => {
                warn!("reject catalog NOTIFY: {err}");
                return send_status_response(&req, &tx, bill, 481).await;
            }
        };
        let subscription_state = match Self::subscription_state(&req) {
            Ok(state) => state,
            Err(err) => {
                warn!("reject catalog NOTIFY: {err}");
                return send_status_response(&req, &tx, bill, 400).await;
            }
        };
        match parse_xlm_to_vec(&req.body) {
            Ok(vs) => {
                let response = ResponseBuilder::build_ok_response(&req, &bill.remote_addr)?;
                tx.send(SipPackage::build_response(response, bill.clone()))
                    .await
                    .hand_log(|msg| warn!("{msg}"))?;
                match subscription_state {
                    NotifySubscriptionState::Active(Some(expires))
                    | NotifySubscriptionState::Pending(Some(expires)) => {
                        cmd::CmdQuery::update_catalog_subscription_from_notify(
                            device_id,
                            generation,
                            expires,
                        );
                    }
                    NotifySubscriptionState::Terminated => {
                        cmd::CmdQuery::terminate_catalog_subscription(
                            device_id,
                            generation,
                        );
                    }
                    NotifySubscriptionState::Active(None)
                    | NotifySubscriptionState::Pending(None) => {}
                }
                for (k, v) in &vs {
                    if MESSAGE_TYPE.contains(&&**k) {
                        match &v[..] {
                            MESSAGE_NOTIFY_CATALOG => {
                                db_task::submit(DbTask::InsertDeviceCatalog {
                                    device_id: device_id.to_string(),
                                    items: vs,
                                });
                            }
                            _ => {
                                debug!("cmdType暂不支持;{k} : {v}");
                            }
                        }
                        break;
                    }
                }
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
    use super::{
        Notify, NotifySubscriptionState, authorization_matches_register,
        is_keepalive_request, normalize_register_expires,
    };
    use base::chrono::Local;
    use base::log::LevelFilter;
    use rsip::headers::Authorization;
    use rsip::headers::ToTypedHeader;
    use rsip::headers::UntypedHeader;
    use rsip::services::DigestGenerator;
    use rsip::{Method, headers};
    use rsip::{Request, Uri, Version};

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

    #[test]
    fn register_authorization_must_match_username_realm_and_uri() {
        let request = Request {
            method: Method::Register,
            uri: Uri::try_from(
                "sip:34020000002000000001@3402000000",
            )
            .unwrap(),
            headers: Default::default(),
            version: Version::V2,
            body: Default::default(),
        };
        let valid = Authorization::try_from(
            r#"Digest username="34020000001320000001", realm="3402000000", nonce="nonce", uri="sip:34020000002000000001@3402000000", response="response", algorithm=MD5"#,
        )
        .unwrap()
        .typed()
        .unwrap();
        let invalid_realm = Authorization::try_from(
            r#"Digest username="34020000001320000001", realm="other", nonce="nonce", uri="sip:34020000002000000001@3402000000", response="response", algorithm=MD5"#,
        )
        .unwrap()
        .typed()
        .unwrap();

        assert!(authorization_matches_register(
            &valid,
            "34020000001320000001",
            &request,
        ));
        assert!(!authorization_matches_register(
            &invalid_realm,
            "34020000001320000001",
            &request,
        ));
    }

    #[test]
    fn notify_subscription_state_parses_active_and_terminated() {
        let request = |value: &str| Request {
            method: Method::Notify,
            uri: Uri::try_from("sip:platform@example.com").unwrap(),
            headers: vec![
                rsip::headers::SubscriptionState::new(value).into(),
            ]
            .into(),
            version: Version::V2,
            body: Default::default(),
        };

        assert_eq!(
            Notify::subscription_state(&request("active;expires=3600")).unwrap(),
            NotifySubscriptionState::Active(Some(3600))
        );
        assert_eq!(
            Notify::subscription_state(&request("terminated;reason=timeout"))
                .unwrap(),
            NotifySubscriptionState::Terminated
        );
    }

    #[test]
    fn keepalive_detection_accepts_xml_whitespace() {
        let request = Request {
            method: Method::Message,
            uri: Uri::try_from("sip:platform@example.com").unwrap(),
            headers: Default::default(),
            version: Version::V2,
            body: br#"<?xml version="1.0" encoding="UTF-8"?>
                <Notify>
                    <CmdType>
                        Keepalive
                    </CmdType>
                </Notify>"#
                .to_vec(),
        };

        assert!(is_keepalive_request(&request));
    }

    #[test]
    fn register_expiry_is_capped_instead_of_rejected() {
        assert_eq!(normalize_register_expires(172_800), 86_400);
        assert_eq!(normalize_register_expires(3_600), 3_600);
        assert_eq!(normalize_register_expires(0), 0);
    }
}
