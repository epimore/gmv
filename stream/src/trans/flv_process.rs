use log::warn;
use xflv::define::tag_type;
use xflv::muxer::{FlvMuxer, HEADER_LENGTH};

use common::anyhow::anyhow;
use common::bytes::{Bytes, BytesMut};
use common::err::{GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use common::tokio::sync::broadcast::Sender;

use crate::container::flv;
use crate::container::flv::{FlvTag, TagType};
use crate::state::cache;
use crate::trans::FrameData;

pub async fn run(ssrc: u32, mut rx: crossbeam_channel::Receiver<FrameData>) -> GlobalResult<()> {
    if let Some(tx) = cache::get_flv_tx(&ssrc) {
        while let Ok(frameData) = rx.recv() {
            let sender = tx.clone();
            let handle_muxer_data_fn = Box::new(
                move |data: Bytes| -> GlobalResult<()> {
                    println!("flv sender channel len = {}",sender.len());
                    if let Err(err) = sender.send(data) {
                        log::error!("send flv tag error: {}", err);
                    }
                    Ok(())
                },
            );
            match frameData {
                FrameData::Video { timestamp, data } => {
                    FlvTag::process(handle_muxer_data_fn, TagType::Video, timestamp, data)?
                }
                FrameData::Audio { timestamp, data } => { println!("ts = {timestamp}, data len = {}", data.len()); }
                FrameData::MetaData { timestamp, data } => { println!("ts = {timestamp}, data len = {}", data.len()); }
            }
        }
    }
    Ok(())
}