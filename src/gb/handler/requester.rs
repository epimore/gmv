use encoding_rs::GB18030;
use log::{debug, info, LevelFilter};
use quick_xml::encoding;
use rsip::{Method, Request};
use rsip::headers::ToTypedHeader;
use rsip::message::HeadersExt;
use rsip::services::DigestGenerator;

use common::anyhow::anyhow;
use common::bytes::Bytes;
use common::chrono::{Local, Timelike};
use common::err::{GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use common::log::{error, warn};
use common::net::shard::{Bill, Package, Zip};
use common::tokio::sync::mpsc::Sender;

use crate::gb::handler::{cmd, parser};
use crate::gb::handler::builder::ResponseBuilder;
use crate::gb::handler::parser::xml::MESSAGE_TYPE;
use crate::gb::shard::rw::RWSession;
use crate::storage::entity::{GmvDevice, GmvOauth};
use crate::storage::mapper;

pub async fn hand_request(req: Request, tx: Sender<Zip>, bill: &Bill) -> GlobalResult<()> {
    let device_id = parser::header::get_device_id_by_request(&req)?;
    //校验设备是否注册
    if req.method == Method::Register {
        Register::process(&device_id, req, tx, bill).await.hand_err(|msg| error!("设备 = [{}],注册失败",&device_id))?;
        Ok(())
    } else {
        match State::check_session(&device_id, tx.clone(), bill)? {
            State::Usable | State::ReCache => {
                match req.method {
                    Method::Ack => { Ok(()) }
                    Method::Bye => { Ok(()) }
                    Method::Cancel => { Ok(()) }
                    Method::Info => { Ok(()) }
                    Method::Invite => { Ok(()) }
                    Method::Message => { Ok(()) }
                    Method::Notify => { Ok(()) }
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
                let _ = tx.clone().send(zip).await.hand_err(|msg| error!("{msg}"));
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
    fn check_session(device_id: &String, tx: Sender<Zip>, bill: &Bill) -> GlobalResult<State> {
        let rw_session = RWSession::check_session_by_device_id(device_id)?;
        if rw_session {
            Ok(State::Usable)
        } else {
            match mapper::get_device_status_info(device_id)? {
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
                        if reg_ts + expire > Local::now().timestamp() as u32 {
                            //刷新缓存
                            RWSession::insert(device_id, tx, heart, bill)?;
                            //如果设备是离线状态，则更新为在线
                            if on == 0 {
                                GmvDevice::update_gmv_device_status_by_device_id(device_id, 1);
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
    async fn process(device_id: &String, req: Request, tx: Sender<Zip>, bill: &Bill) -> GlobalResult<()> {
        let oauth = GmvOauth::read_gmv_oauth_by_device_id(device_id)?
            .ok_or(SysErr(anyhow!("device id = [{}] 未知设备，拒绝接入",device_id)))
            .hand_err(|msg| warn!("{msg}"))?;
        if oauth.get_status() == &0u8 {
            warn!("device id = [{}] 未启用设备，拒绝接入",device_id);
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
    async fn login_ok(device_id: &String, req: &Request, tx: Sender<Zip>, bill: &Bill, oauth: GmvOauth) -> GlobalResult<()> {
        RWSession::insert(device_id, tx.clone(), *oauth.get_heartbeat_sec(), bill)?;
        let gmv_device = GmvDevice::build_gmv_device(&req)?;
        gmv_device.insert_single_gmv_device_by_register();
        let ok_response = ResponseBuilder::build_register_ok_response(&req, bill.get_remote_addr())?;
        let zip = Zip::build_data(Package::new(bill.clone(), Bytes::from(ok_response)));
        let _ = tx.clone().send(zip).await.hand_err(|msg| warn!("{msg}"));
        // query subscribe device msg
        cmd::CmdQuery::lazy_query_device_info(device_id).await?;
        cmd::CmdQuery::lazy_query_device_catalog(device_id).await?;
        cmd::CmdQuery::lazy_subscribe_device_catalog(device_id).await
    }

    async fn logout_ok(device_id: &String, req: &Request, tx: Sender<Zip>, bill: &Bill) -> GlobalResult<()> {
        let ok_response = ResponseBuilder::build_logout_ok_response(&req, bill.get_remote_addr())?;
        let zip = Zip::build_data(Package::new(bill.clone(), Bytes::from(ok_response)));
        let _ = tx.clone().send(zip).await.hand_err(|msg| warn!("{msg}"));
        GmvDevice::update_gmv_device_status_by_device_id(device_id, 0);
        RWSession::clean_rw_session_and_net(device_id).await
    }

    async fn unauthorized(req: &Request, tx: Sender<Zip>, bill: &Bill) -> GlobalResult<()> {
        let unauthorized_register_response = ResponseBuilder::unauthorized_register_response(&req, bill.get_remote_addr())?;
        let zip = Zip::build_data(Package::new(bill.clone(), Bytes::from(unauthorized_register_response)));
        let _ = tx.clone().send(zip).await.hand_err(|msg| error!("{msg}"));
        Ok(())
    }
}

struct Message;

impl Message {
    async fn process(device_id: &String, req: Request, tx: Sender<Zip>, bill: &Bill) -> GlobalResult<()> {
        use parser::xml::*;
        match parse_xlm_to_vec(&req.body) {
            Ok(vs) => {
                let response = ResponseBuilder::build_register_ok_response(&req, bill.get_remote_addr())?;
                for (k, v) in &vs {
                    if MESSAGE_TYPE.contains(&&k[..]) {
                        match &v[..] {
                            MESSAGE_KEEP_ALIVE => {
                                Self::keep_alive(device_id, vs, bill)?;
                            }
                            MESSAGE_CONFIG_DOWNLOAD => {}
                            MESSAGE_NOTIFY_CATALOG => {}
                            MESSAGE_DEVICE_INFO => {}
                            MESSAGE_ALARM => {}
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
                        let _ = tx.clone().send(zip).await.hand_err(|msg| error!("{msg}"));
                        break;
                    }
                }
                unimplemented!()
            }
            Err(err) => {
                let val = encoding::decode(&req.body, GB18030).hand_err(|msg| error!("{msg}"))?;
                Err(SysErr(anyhow!("xml解析失败: {err:?}; xml = [{}]",val)))?
            }
        }
    }
    fn keep_alive(device_id: &String, vs: Vec<(String, String)>, bill: &Bill) -> GlobalResult<()> {
        use parser::xml::{NOTIFY_DEVICE_ID, NOTIFY_STATUS};
        if log::max_level() <= LevelFilter::Info {
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
            info!("keep_alive: device_id = {},status = {}",&device_id,&status);
        }
        RWSession::heart(device_id, bill.clone())
    }
}

#[cfg(test)]
mod tests {
    use log::LevelFilter;
    use common::chrono::Local;

    #[test]
    fn test_time_stamp() {
        println!("= {}", Local::now().timestamp());
    }

    #[test]
    fn test_log_level() {
        // 设置日志级别为 Info
        log::set_max_level(LevelFilter::Info);

        // 获取当前日志级别
        let max_level = log::max_level();
        println!("Current log level: {:?}", max_level);

        // 判断当前级别是否达到指定级别
        if log::max_level() >= LevelFilter::Debug {
            println!("Debug messages are enabled");
        } else {
            println!("Debug messages are disabled");
        }
    }
}