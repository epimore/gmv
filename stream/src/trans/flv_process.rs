use log::warn;
use rtmp::cache::metadata::MetaData;
use streamhub::define::{FrameData, FrameDataReceiver};
use xflv::define::tag_type;
use xflv::muxer::{FlvMuxer, HEADER_LENGTH};

use common::anyhow::anyhow;
use common::bytes::{Bytes, BytesMut};
use common::err::{GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use common::tokio::sync::broadcast::Sender;

pub async fn run(ssrc: u32, mut rx: FrameDataReceiver) -> GlobalResult<()> {
    if let Some(tx) = cache::get_flv_tx(&ssrc) {
        let mut flv_muxer = FlvMuxer::new();
        flv_muxer.write_flv_header().map_err(|err| SysErr(anyhow!("{}",err.to_string())))?;
        flv_muxer.write_previous_tag_size(0).map_err(|err| SysErr(anyhow!("{}",err.to_string())))?;
        flush_data(&mut flv_muxer, tx.clone());
        //write flv body
        loop {
            if let Some(data) = rx.recv().await {
                let _ = write_flv_tag(&mut flv_muxer, data, tx.clone()).hand_log(|msg| warn!("{msg}"));
            }
        }
    }
    Ok(())
}

use crate::state::cache;

fn flush_data(flv_muxer: &mut FlvMuxer, tx: Sender<Bytes>) {
    let data = flv_muxer.writer.extract_current_bytes();
    println!("flv data size = {}", &data.len());
    let _ = tx.send(data.freeze()).hand_log(|msg| warn!("{msg}"));
}

fn write_flv_tag(flv_muxer: &mut FlvMuxer, data: FrameData, tx: Sender<Bytes>) -> GlobalResult<()> {
    let (common_data, common_timestamp, tag_type) = match data {
        FrameData::Audio { timestamp, data } => (data, timestamp, tag_type::AUDIO),
        FrameData::Video { timestamp, data } => (data, timestamp, tag_type::VIDEO),
        FrameData::MetaData { timestamp, data } => {
            let mut metadata = MetaData::new();
            metadata.save(&data);
            let data = metadata.remove_set_data_frame().map_err(|err| SysErr(anyhow!("{}",err.to_string())))?;
            (data, timestamp, tag_type::SCRIPT_DATA_AMF)
        }
        _ => {
            log::error!("should not be here!!!");
            (BytesMut::new(), 0, 0)
        }
    };

    let common_data_len = common_data.len() as u32;
    println!("common_data_len size = {}", common_data_len);
    flv_muxer.write_flv_tag_header(tag_type, common_data_len, common_timestamp).map_err(|err| SysErr(anyhow!("{}",err.to_string())))?;
    flv_muxer.write_flv_tag_body(common_data).map_err(|err| SysErr(anyhow!("{}",err.to_string())))?;
    flv_muxer.write_previous_tag_size(common_data_len + HEADER_LENGTH).map_err(|err| SysErr(anyhow!("{}",err.to_string())))?;
    flush_data(flv_muxer, tx.clone());
    Ok(())
}