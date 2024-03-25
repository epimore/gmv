use rsip::{Method, Request};
use rsip::headers::{ToTypedHeader};
use rsip::message::HeadersExt;
use rsip::services::DigestGenerator;
use common::anyhow::anyhow;
use common::bytes::Bytes;
use common::err::GlobalError::SysErr;
use common::err::{GlobalResult, TransError};
use common::log::{error, warn};
use common::net::shard::{Bill, Package, Zip};
use common::tokio::sync::mpsc::Sender;
use crate::gb::handler::{cmd, parser};
use crate::gb::handler::builder::ResponseBuilder;
use crate::gb::shard::rw::RWSession;
use crate::storage::entity::{GmvDevice, GmvOauth};

pub async fn hand_request(req: Request, tx: Sender<Zip>, bill: &Bill) -> GlobalResult<()> {
    match req.method {
        Method::Ack => {Ok(())}
        Method::Bye => {Ok(())}
        Method::Cancel => {Ok(())}
        Method::Info => {Ok(())}
        Method::Invite => {Ok(())}
        Method::Message => {Ok(())}
        Method::Notify => {Ok(())}
        Method::Options => {Ok(())}
        Method::PRack => {Ok(())}
        Method::Publish => {Ok(())}
        Method::Refer => {Ok(())}
        Method::Register => {
            Register::process(req, tx, bill).await
        }
        Method::Subscribe => {Ok(())}
        Method::Update => {Ok(())}
    }
}

struct Register;

impl Register {
    async fn process(req: Request, tx: Sender<Zip>, bill: &Bill) -> GlobalResult<()> {
        let device_id = parser::header::get_device_id_by_request(&req)?;
        let oauth = GmvOauth::read_gmv_oauth_by_device_id(&device_id)?
            .ok_or(SysErr(anyhow!("device id = [{}] 未知设备，拒绝接入",&device_id)))
            .hand_err(|msg| warn!("{msg}"))?;
        if oauth.get_status() == &0u8 {
            warn!("device id = [{}] 未启用设备，拒绝接入",&device_id);
        }
        match oauth.get_pwd_check() {
            //不进行鉴权校验
            &0u8 => {
                let expires = parser::header::get_expires(&req)?;
                if expires > 0 {
                    Self::login_ok(&device_id, &req, tx, bill, oauth).await
                } else {
                    //设备下线
                    Self::logout_ok(&device_id, &req, tx, bill).await
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
                                        Self::login_ok(&device_id, &req, tx, bill, oauth).await
                                    } else {
                                        //注销  设备下线
                                        Self::logout_ok(&device_id, &req, tx, bill).await
                                    }
                                } else {
                                    Self::unauthorized(&req, tx, bill).await
                                }
                            }
                            Err(err) => {
                                warn!("device_id = {},register request err ={}",&device_id,err);
                                Self::unauthorized(&req, tx, bill).await
                            }
                        }
                    }
                }
            }
        }
    }
    async fn login_ok(device_id: &String, req: &Request, tx: Sender<Zip>, bill: &Bill, oauth: GmvOauth) -> GlobalResult<()> {
        RWSession::insert(&device_id, tx.clone(), *oauth.get_heartbeat_sec(), bill)?;
        let gmv_device = GmvDevice::build_gmv_device(&req)?;
        gmv_device.insert_single_gmv_device_by_register();
        let ok_response = ResponseBuilder::build_register_ok_response(&req, bill.get_remote_addr())?;
        let zip = Zip::build_data(Package::new(bill.clone(), Bytes::from(ok_response)));
        let _ = tx.clone().send(zip).await.hand_err(|msg| warn!("{msg}"));
        // query subscribe device msg
        cmd::CmdQuery::lazy_query_device_info(&device_id).await?;
        cmd::CmdQuery::lazy_query_device_catalog(&device_id).await?;
        cmd::CmdQuery::lazy_subscribe_device_catalog(&device_id).await
    }

    async fn logout_ok(device_id: &String, req: &Request, tx: Sender<Zip>, bill: &Bill) -> GlobalResult<()> {
        let ok_response = ResponseBuilder::build_logout_ok_response(&req, bill.get_remote_addr())?;
        let zip = Zip::build_data(Package::new(bill.clone(), Bytes::from(ok_response)));
        let _ = tx.clone().send(zip).await.hand_err(|msg| warn!("{msg}"));
        GmvDevice::update_gmv_device_status_by_device_id(&device_id, 0);
        RWSession::clean_rw_session_and_net(&device_id).await
    }

    async fn unauthorized(req: &Request, tx: Sender<Zip>, bill: &Bill) -> GlobalResult<()> {
        let unauthorized_register_response = ResponseBuilder::unauthorized_register_response(&req, bill.get_remote_addr())?;
        let zip = Zip::build_data(Package::new(bill.clone(), Bytes::from(unauthorized_register_response)));
        let _ = tx.clone().send(zip).await.hand_err(|msg| error!("{msg}"));
        Ok(())
    }
}