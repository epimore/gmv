use rsip::{Method, Request};
use rsip::message::HeadersExt;
use common::anyhow::anyhow;
use common::bytes::Bytes;
use common::chrono::Local;
use common::err::GlobalError::SysErr;
use common::err::{GlobalResult, TransError};
use common::log::{error, warn};
use common::net::shard::{Bill, Package, Protocol, Zip};
use common::net::udp_turn_bill;
use common::tokio::sync::mpsc::Sender;
use crate::gb::handler::{builder, cmd, parser};
use crate::gb::handler::builder::ResponseBuilder;
use crate::gb::shard::event::EventSession;
use crate::gb::shard::rw::RWSession;
use crate::storage::entity::{GmvDevice, GmvOauth};

pub async fn hand_request(req: Request, tx: Sender<Zip>) {
    match req.method {
        Method::Ack => {}
        Method::Bye => {}
        Method::Cancel => {}
        Method::Info => {}
        Method::Invite => {}
        Method::Message => {}
        Method::Notify => {}
        Method::Options => {}
        Method::PRack => {}
        Method::Publish => {}
        Method::Refer => {}
        Method::Register => {}
        Method::Subscribe => {}
        Method::Update => {}
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
                    RWSession::insert(&device_id, tx.clone(), *oauth.get_heartbeat_sec(), bill)?;
                    let gmv_device = GmvDevice::build_gmv_device(&req, bill)?;
                    gmv_device.insert_single_gmv_device_by_register();
                    let ok_response = ResponseBuilder::build_register_ok_response(&req, bill.get_from())?;
                    let res_bill = if bill.get_protocol().eq(&Protocol::UDP) {
                        udp_turn_bill(bill)
                    } else { bill.clone() };
                    let zip = Zip::build_data(Package::new(res_bill, Bytes::from(ok_response)));
                    let _ = tx.clone().send(zip).await.hand_err(|msg| warn!("{msg}"));
                    // query subscribe device msg
                    cmd::CmdQuery::lazy_query_device_info(&device_id).await?;
                    cmd::CmdQuery::lazy_query_device_catalog(&device_id).await?;
                    cmd::CmdQuery::lazy_subscribe_device_catalog(&device_id).await?;
                } else {
                    //设备下线
                    let ok_response = ResponseBuilder::build_logout_ok_response(&req, bill.get_from())?;
                    let res_bill = if bill.get_protocol().eq(&Protocol::UDP) {
                        udp_turn_bill(bill)
                    } else { bill.clone() };
                    let zip = Zip::build_data(Package::new(res_bill, Bytes::from(ok_response)));
                    let _ = tx.clone().send(zip).await.hand_err(|msg| warn!("{msg}"));
                    GmvDevice::update_gmv_device_status_by_device_id(&device_id, 0);
                    RWSession::clean_rw_session_and_net(&device_id).await?;
                }
            }
            _ => {}
        }

        unimplemented!()
    }
    async fn ok(req: &Request, tx: Sender<Zip>) -> GlobalResult<()> {
        let transport = parser::header::get_transport(&req)?;
        let local_addr = parser::header::get_local_addr(&req)?;
        let from = parser::header::get_from(&req)?;
        let to = parser::header::get_to(&req)?;
        let time = Local::now().timestamp();
        let device_id = parser::header::get_device_id_by_request(&req)?;


        unimplemented!()
    }
}