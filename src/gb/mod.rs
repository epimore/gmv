mod shard;
pub mod handler;

use rsip::{Error, SipMessage};
use rsip::message::HeadersExt;
use common::err::{GlobalResult, TransError};
use common::log::{debug, error, warn};
use common::net::shard::{Bill, Event, Package, Zip};
use common::tokio::sync::mpsc::{Receiver, Sender};
use crate::gb::handler::parser;
use crate::gb::shard::event::{EventSession, Ident};
use crate::gb::shard::rw::RWSession;

pub async fn gb_run(output: Sender<Zip>, input: Receiver<Zip>) -> GlobalResult<()> {
    Ok(())
}

async fn input_msg(output: Sender<Zip>, mut input: Receiver<Zip>) -> GlobalResult<()> {
    while let Some(zip) = input.recv().await {
        match zip {
            Zip::Data(package) => {
                match SipMessage::try_from(package.get_owned_data()) {
                    Ok(msg) => {
                        match msg {
                            SipMessage::Request(req) => {}
                            SipMessage::Response(res) => {
                                let call_id: String = res.call_id_header().hand_err(|msg| error!("{msg}"))?.clone().into();
                                let cs_eq: String = res.cseq_header().hand_err(|msg| error!("{msg}"))?.clone().into();
                                //todo 是否固定为device_id;play时需测试
                                let device_id = parser::header::get_device_id_by_response(&res)?;
                                let ident = Ident::new(device_id, call_id, cs_eq);
                                EventSession::handle_event(&ident, res).await?
                            }
                        }
                    }
                    Err(err) => {
                        debug!("invalid data {err:?}");
                    }
                }
            }
            Zip::Event(event) => {
                if event.get_type_code() == &0u8 {
                    RWSession::clean_rw_session_by_bill(event.get_bill())?;
                }
            }
        }
    }
    Ok(())
}