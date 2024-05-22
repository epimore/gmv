use bytesio::bytes_errors::{BytesReadError, BytesReadErrorValue};
use bytesio::bytes_reader::BytesReader;
use log::{debug, error, info, warn};
use streamhub::define::{FrameData, FrameDataSender};
use xmpegts::define::epsi_stream_type;
use xmpegts::errors::MpegErrorValue;
use xmpegts::ps::errors::{MpegPsError, MpegPsErrorValue};
use xmpegts::ps::ps_demuxer::PsDemuxer;
use xrtsp::rtp::errors::{UnPackerError, UnPackerErrorValue};
use xrtsp::rtp::rtp_aac::RtpAacUnPacker;
use xrtsp::rtp::rtp_h264::RtpH264UnPacker;
use xrtsp::rtp::rtp_h265::RtpH265UnPacker;
use xrtsp::rtp::rtp_queue::RtpQueue;
use xrtsp::rtp::RtpPacket;
use xrtsp::rtp::utils::{TUnPacker, Unmarshal};

use common::anyhow::anyhow;
use common::bytes::{Bytes, BytesMut};
use common::err::{GlobalError, GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use common::tokio::sync::mpsc::UnboundedSender;

use crate::state::cache;

pub async fn run(ssrc: u32, tx: FrameDataSender) -> GlobalResult<()> {
    let mut ps_demuxer = new_ps_demuxer(tx.clone());
    let mut h264un_packer = RtpH264UnPacker::new();
    let mut h265un_packer = RtpH265UnPacker::new();
    let mut aacun_packer = RtpAacUnPacker::new();
    let mut nalu_bytes_reader = BytesReader::new(BytesMut::default());
    let mut sort_bytes_reader = BytesReader::new(BytesMut::default());
    let mut rtp_queue = RtpQueue::new(200);
    if let Some(rx) = cache::get_rtp_rx(&ssrc) {
        loop {
            match rx.recv() {
                Ok(data) => {
                    sort_bytes_reader.extend_from_slice(&data[..]);
                    match RtpPacket::unmarshal(&mut sort_bytes_reader) {
                        Ok(rtp_packet) => {
                            rtp_queue.write_queue(rtp_packet);
                            while let Some(rtp_packet) = rtp_queue.read_queue() {
                                match rtp_packet.header.payload_type {
                                    98 => {
                                        parse_ps(&mut ps_demuxer, rtp_packet)?;
                                    }
                                    96 => { parse_nalu_h264(&mut h264un_packer, &mut nalu_bytes_reader, rtp_packet, tx.clone())? }
                                    100 => { parse_nalu_h265(&mut h265un_packer, &mut nalu_bytes_reader, rtp_packet, tx.clone())? }
                                    102 => { parse_nalu_aac(&mut aacun_packer, &mut nalu_bytes_reader, rtp_packet, tx.clone())? }
                                    _ => {
                                        return Err(GlobalError::new_biz_error(4005, "系统暂不支持", |msg| debug!("{msg}")));
                                    }
                                }
                            }
                        }
                        Err(err) => {
                            warn!("BytesReadError: {}",err.to_string());
                        }
                    }
                }
                Err(_) => {
                    info!("ssrc = {ssrc},流已释放");
                    break;
                }
            }
        }
    }
    Ok(())
}

fn parse_nalu_h264(h264un_packer: &mut RtpH264UnPacker, nalu_bytes_reader: &mut BytesReader, rtp_packet: RtpPacket, tx: FrameDataSender) -> GlobalResult<()> {
    nalu_bytes_reader.extend_from_slice(&*rtp_packet.payload);
    match h264un_packer.unpack(nalu_bytes_reader) {
        Ok(_) => {}
        Err(UnPackerError { value: UnPackerErrorValue::BytesReadError(BytesReadError { value: BytesReadErrorValue::NotEnoughBytes }) }) => {}
        Err(err) => {
            return Err(SysErr(anyhow!("UnPackerError:{}",err.to_string())));
        }
    }
    h264un_packer.on_frame_handler(Box::new(
        move |data: FrameData| -> Result<(), UnPackerError> {
            if let Err(err) = tx.send(data) {
                log::error!("send frame error: {}", err);
            }
            Ok(())
        },
    ));
    Ok(())
}

fn parse_nalu_h265(h265un_packer: &mut RtpH265UnPacker, nalu_bytes_reader: &mut BytesReader, rtp_packet: RtpPacket, tx: FrameDataSender) -> GlobalResult<()> {
    nalu_bytes_reader.extend_from_slice(&*rtp_packet.payload);
    match h265un_packer.unpack(nalu_bytes_reader) {
        Ok(_) => {}
        Err(UnPackerError { value: UnPackerErrorValue::BytesReadError(BytesReadError { value: BytesReadErrorValue::NotEnoughBytes }) }) => {}
        Err(err) => {
            return Err(SysErr(anyhow!("UnPackerError:{}",err.to_string())));
        }
    }
    h265un_packer.on_frame_handler(Box::new(
        move |data: FrameData| -> Result<(), UnPackerError> {
            if let Err(err) = tx.send(data) {
                log::error!("send frame error: {}", err);
            }
            Ok(())
        },
    ));
    Ok(())
}

fn parse_nalu_aac(aacun_packer: &mut RtpAacUnPacker, nalu_bytes_reader: &mut BytesReader, rtp_packet: RtpPacket, tx: FrameDataSender) -> GlobalResult<()> {
    nalu_bytes_reader.extend_from_slice(&*rtp_packet.payload);
    match aacun_packer.unpack(nalu_bytes_reader) {
        Ok(_) => {}
        Err(UnPackerError { value: UnPackerErrorValue::BytesReadError(BytesReadError { value: BytesReadErrorValue::NotEnoughBytes }) }) => {}
        Err(err) => {
            return Err(SysErr(anyhow!("UnPackerError:{}",err.to_string())));
        }
    }
    aacun_packer.on_frame_handler(Box::new(
        move |data: FrameData| -> Result<(), UnPackerError> {
            if let Err(err) = tx.send(data) {
                log::error!("send frame error: {}", err);
            }
            Ok(())
        },
    ));
    Ok(())
}

fn parse_ps(ps_demuxer: &mut PsDemuxer, rtp_packet: RtpPacket) -> GlobalResult<()> {
    if let Err(err) = ps_demuxer.demux(rtp_packet.payload) {
        return match err.value {
            MpegErrorValue::MpegPsError(ps_err) => match ps_err.value {
                MpegPsErrorValue::NotEnoughBytes => {
                    Ok(())
                }
                _ => {
                    Err(SysErr(anyhow!("MpegPsError:{}",ps_err.to_string())))
                }
            },
            _ => {
                Err(SysErr(anyhow!("MpegError: {}",err)))
            }
        };
    }
    Ok(())
}
/*
async fn parse(data: Bytes, ps_demuxer: &mut PsDemuxer, sort_bytes_reader: &mut BytesReader, rtp_queue: &mut RtpQueue) -> GlobalResult<()> {
    sort_bytes_reader.extend_from_slice(&data[..]);
    let rtp_packet = RtpPacket::unmarshal(sort_bytes_reader).map_err(|err| SysErr(anyhow!("BytesReadError: {}",err.to_string())))?;
    rtp_queue.write_queue(rtp_packet);

    while let Some(rtp_packet) = rtp_queue.read_queue() {
        match rtp_packet.header.payload_type {
            //ps
            96 => {
                if let Err(err) = ps_demuxer.demux(rtp_packet.payload) {
                    match err.value {
                        MpegErrorValue::MpegPsError(ps_err) => match ps_err.value {
                            MpegPsErrorValue::NotEnoughBytes => {
                                continue;
                            }
                            _ => {
                                return Err(SysErr(anyhow!("MpegPsError:{}",ps_err.to_string())));
                            }
                        },
                        _ => {
                            return Err(SysErr(anyhow!("MpegError: {}",err)));
                        }
                    }
                }
            }
            //todo 将rtp负载的媒体流解封装为原始流发送出去
            //h264
            98 => {
                let mut h264un_packer = RtpH264UnPacker::new();
                let mut reader = BytesReader::new(rtp_packet.payload);
                h264un_packer.unpack(&mut reader).map_err(|err| SysErr(anyhow!("UnPackerError:{}",err.to_string())))?;
                h264un_packer.on_frame_handler(|data|)
            }
            //h265
            100 => {}
            //aac
            102 => {}
            //裸流
            _ => {
                return Err(GlobalError::new_biz_error(4005, &*format!("rtp type = {:?},系统暂不支持。", v), |msg| debug!("{msg}")));
            }
        }
    }
    Ok(())
}*/

fn new_ps_demuxer(sender: UnboundedSender<FrameData>) -> PsDemuxer {
    let handler = Box::new(
        move |pts: u64,
              _dts: u64,
              stream_type: u8,
              payload: BytesMut|
              -> Result<(), MpegPsError> {
            match stream_type {
                epsi_stream_type::PSI_STREAM_H264 | epsi_stream_type::PSI_STREAM_H265 => {
                    let video_frame_data = FrameData::Video {
                        timestamp: pts as u32,
                        data: payload,
                    };
                    log::trace!("receive video data");
                    if let Err(err) = sender.send(video_frame_data) {
                        log::error!("send video frame err: {}", err);
                    }
                }
                epsi_stream_type::PSI_STREAM_AAC => {
                    let audio_frame_data = FrameData::Audio {
                        timestamp: pts as u32,
                        data: payload,
                    };
                    log::trace!("receive audio data");
                    if let Err(err) = sender.send(audio_frame_data) {
                        log::error!("send audio frame err: {}", err);
                    }
                }
                _ => {}
            }
            Ok(())
        },
    );

    PsDemuxer::new(handler)
}
