use discortp::{demux, Packet};
use discortp::demux::Demuxed;
use discortp::rtp::RtpType;
use ffmpeg_next::format::{input, Pixel};
use ffmpeg_next::codec::{Context, decoder, Id, packet};
use ffmpeg_next::decoder::{Decoder, Opened};
use ffmpeg_next::{Frame, Packet as fPacket};
use ffmpeg_next::media::Type;
use ffmpeg_next::util::frame::video::Video;
use xmpegts::ps::errors::MpegPsError;
use xmpegts::{define::epsi_stream_type, errors::MpegErrorValue};
use xmpegts::ps::ps_demuxer::PsDemuxer;
use common::anyhow::anyhow;
use common::bytes::BytesMut;
use common::err::{GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use common::log::{error, trace};
use crate::data::buffer;

const RTP_INPUT_ADDR: &str = "127.0.0.1:1234";
const HTTP_FLV_OUTPUT_ADDR: &str = "127.0.0.1:8080";

pub async fn process(ssrc: u32) -> GlobalResult<()> {
    // Initialize FFmpeg
    ffmpeg_next::init().expect("Failed to initialize FFmpeg");
    /*
        // Open input format
        let mut input = input("rtp").expect("Failed to open input format");

        // Add UDP input option
        input.set_option("protocol_whitelist", "udp").expect("Failed to set option");


        // Open input context
        let mut input_ctx = input.open::<packet::Packet>().expect("Failed to open input context");

        // Open video decoder
        let mut video_decoder = decoder::find_best(&input_ctx.streams().video().next().unwrap())
            .expect("Failed to find video decoder")
            .video()
            .expect("Failed to get video decoder");
*/
    // Open HTTP-FLV output
    /*    let mut output = ffmpeg_next::format::output(HTTP_FLV_OUTPUT_ADDR)
            .expect("Failed to open output format");

        // Open output context
        let mut output_ctx = output
            .add_stream(Type::Video)
            .expect("Failed to add video stream")
            .expect("Failed to open output context");

        // Write header to output
        output.write_header().expect("Failed to write header");*/

    // let option = decoder::find_by_name("rtp").ok_or(SysErr(anyhow!("Codec with rtp Not found."))).hand_err(|msg| error!("{msg}"))?;

    let h264_codec = decoder::find(Id::H264).ok_or(SysErr(anyhow!("Codec with rtp Not found."))).hand_err(|msg| error!("{msg}"))?;
    let context = Context::new();
    // context.set_parameters()
    let decoder = context.decoder();
    let mut opened = decoder.open_as(h264_codec).hand_err(|msg| error!("{msg}"))?;

    buffer::Cache::readable(&ssrc).await?;
    while let Ok(Some(buf)) = buffer::Cache::consume(&ssrc) {
        if let Demuxed::Rtp(rtp_packet) = demux::demux(&buf) {
            if let RtpType::Dynamic(tp) = rtp_packet.get_payload_type() {
                // if tp == 98 {
                //     let mut demuxer = handle_ps();
                //     let _ = demuxer.demux(BytesMut::from(rtp_packet.payload())).map_err(|en| error!("{}",en.value.to_string()));
                // }
                if tp == 96 {
                    println!("+++++++++   {:?}", &rtp_packet);
                    let len = rtp_packet.payload().len();
                    let packet = fPacket::copy(rtp_packet.payload());
                    let _ = opened.send_packet(&packet).hand_err(|msg| error!("{msg}"));
                    let mut frame = unsafe { Frame::empty() };
                    let _ = opened.receive_frame(&mut frame).hand_err(|msg| error!("{msg}"));
                    let packet1 = frame.packet();
                    println!("------------------   {packet1:?}");
                }
            }
        }
        buffer::Cache::readable(&ssrc).await?;
    }
    Ok(())
}

fn handle_ps() -> PsDemuxer {
    let handler = Box::new(|pts: u64,
                            _dts: u64,
                            stream_type: u8,
                            payload: BytesMut|
                            -> Result<(), MpegPsError> {
        match stream_type {
            epsi_stream_type::PSI_STREAM_H264 | epsi_stream_type::PSI_STREAM_H265 => {
                ps_handle_video(pts, _dts, stream_type, payload);
            }
            epsi_stream_type::PSI_STREAM_AAC => {
                ps_handle_audio(pts, _dts, stream_type, payload);
            }
            _ => {}
        }
        Ok(())
    });
    PsDemuxer::new(handler)
}


fn ps_handle_video(pts: u64, _dts: u64, stream_type: u8, payload: BytesMut) {}

fn ps_handle_audio(pts: u64, _dts: u64, stream_type: u8, payload: BytesMut) {}

