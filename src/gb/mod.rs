mod shard;
pub mod handler;

use rsip::{Error, SipMessage};
use common::err::GlobalResult;
use common::log::debug;
use common::net::shard::{Package, Zip};
use common::tokio::sync::mpsc::{Receiver, Sender};

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
                            SipMessage::Response(res) => {}
                        }
                    }
                    Err(err) => {
                        debug!("invalid data {err:?}");
                    }
                }
            }
            Zip::Event(event) => {
                
            }
        }
    }
    Ok(())
}