pub mod trans;

use std::ffi::{CStr, CString};
use std::mem::MaybeUninit;
use std::os::raw::{c_char, c_int, c_void};
use std::{mem, ptr};

use ffmpeg_next as ffmpeg;
use ffmpeg_next::ffi::{AV_NOPTS_VALUE, av_parser_close, av_parser_init, av_parser_parse2, AV_TIME_BASE, avcodec_alloc_context3, avcodec_find_decoder, avcodec_free_context, avcodec_open2, AVCodecID, AVCodecParserContext};
use ffmpeg_next::{Dictionary, format};
use ffmpeg_next::format::{context, input};
use ffmpeg_next::log::Flags;
use ffmpeg_next::sys::{av_find_input_format, av_freep, av_init_packet, av_malloc, AVFMT_FLAG_CUSTOM_IO, AVFMT_NOFILE, AVFMT_SHOW_IDS, avformat_alloc_context, avformat_close_input, avformat_find_stream_info, avformat_open_input, AVFormatContext, avio_alloc_context, avio_context_free, AVIO_FLAG_READ, AVPacket};
use ffmpeg_next::util::error as er;
use common::anyhow::anyhow;

use common::err::{GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use common::log::error;

use crate::data;
use crate::data::buffer;
use crate::general::mode::AV_IO_CTX_BUFFER_SIZE;


pub async fn parse2(ssrc: u32) -> GlobalResult<()> {
    unsafe {
        ffmpeg::init().unwrap();
        let codec_id = AVCodecID::AV_CODEC_ID_H264 as i32;
        let codec_parser = av_parser_init(codec_id);
        if codec_parser.is_null() {
            println!("Failed to initialize parser");
            return Err(SysErr(anyhow!("Failed to initialize parser")));
        }
        let mut avctx = avcodec_alloc_context3(ptr::null_mut());
        let codec = avcodec_find_decoder(AVCodecID::AV_CODEC_ID_H264);
        if unsafe { avcodec_open2(avctx, codec, ptr::null_mut()) } < 0 {
            println!("Failed to open codec");
        }
        // let mut packet: AVPacket = mem::zeroed();
        // av_init_packet(&mut packet);
        buffer::Cache::readable(&ssrc).await?;
        while let Ok(Some(data)) = buffer::Cache::consume(&ssrc) {
            let mut buf = data.as_ptr();
            let mut buf_size = data.len() as i32;
            println!("buf_size = {buf_size}");
            while buf_size > 0 {
                let ret = unsafe { av_parser_parse2(codec_parser, avctx, ptr::null_mut(), ptr::null_mut(), buf, buf_size, AV_NOPTS_VALUE, AV_NOPTS_VALUE, 0) };
                if ret >= 0 {
                    println!("Parsed {} bytes", ret);
                    println!("bit_rate = {}", (*avctx).bit_rate);
                } else {
                    println!("Failed to parse packet");
                }
                buf = buf.offset(ret as isize);
                buf_size -= ret;
            }
            buffer::Cache::readable(&ssrc).await?;
        }
        av_parser_close(codec_parser);
        avcodec_free_context(&mut avctx);
    }
    println!("exit----");
    Ok(())
}

pub async fn parse(ssrc: u32) -> GlobalResult<()> {
    unsafe {
        ffmpeg_next::init().hand_err(|msg| error!("{msg}")).unwrap();
        unsafe { ffmpeg::ffi::av_log_set_level(ffmpeg::ffi::AV_LOG_TRACE); }
        let in_f = CString::new("sdp".to_string()).unwrap().into_raw();
        let input_format = av_find_input_format(in_f);
        let mut fmt_ctx = unsafe { avformat_alloc_context() };
        let mut dict = Dictionary::new();
        dict.set("sdp_flags", "custom_io");
        let mut opts = dict.disown();
        let sdp = CString::new("/home/ubuntu20/code/rs/mv/github/epimore/gmv/stream/123.sdp".to_string()).expect("CString::new failed").into_raw();
        match avformat_open_input(&mut fmt_ctx, sdp, input_format, &mut opts) {
            0 =>
                {
                    let in_tx_bf = unsafe { av_malloc(AV_IO_CTX_BUFFER_SIZE as usize) }.cast();
                    let opa = Box::into_raw(Box::new(ssrc)) as *mut c_void;
                    let call = data::call();
                    let io_ctx = unsafe { avio_alloc_context(in_tx_bf, AV_IO_CTX_BUFFER_SIZE as c_int, 0, opa, Some(call), None, None) };
                    buffer::Cache::readable(&ssrc).await?;
                    (*fmt_ctx).pb = io_ctx;
                    (*fmt_ctx).flags |= AVFMT_NOFILE;
                    (*fmt_ctx).max_analyze_duration = 0;
                    (*fmt_ctx).probesize = 8;
                    if fmt_ctx.is_null() || io_ctx.is_null() {
                        println!("is null");
                    }
                    match avformat_find_stream_info(fmt_ctx, ptr::null_mut()) {
                        r if r >= 0 => {
                            println!("inin2 ............");
                            let context = context::Input::wrap(fmt_ctx);
                            for (k, v) in context.metadata().iter() {
                                println!("{}: {}", k, v);
                            }

                            if let Some(stream) = context.streams().best(ffmpeg::media::Type::Video) {
                                println!("Best video stream index: {}", stream.index());
                            }

                            if let Some(stream) = context.streams().best(ffmpeg::media::Type::Audio) {
                                println!("Best audio stream index: {}", stream.index());
                            }

                            if let Some(stream) = context.streams().best(ffmpeg::media::Type::Subtitle) {
                                println!("Best subtitle stream index: {}", stream.index());
                            }

                            println!(
                                "duration (seconds): {:.2}",
                                context.duration() as f64 / f64::from(ffmpeg::ffi::AV_TIME_BASE)
                            );

                            for stream in context.streams() {
                                buffer::Cache::readable(&1).await.expect("readable exception");

                                println!("stream index {}:", stream.index());
                                println!("\ttime_base: {}", stream.time_base());
                                println!("\tstart_time: {}", stream.start_time());
                                println!("\tduration (stream timebase): {}", stream.duration());
                                println!(
                                    "\tduration (seconds): {:.2}",
                                    stream.duration() as f64 * f64::from(stream.time_base())
                                );
                                println!("\tframes: {}", stream.frames());
                                println!("\tdisposition: {:?}", stream.disposition());
                                println!("\tdiscard: {:?}", stream.discard());
                                println!("\trate: {}", stream.rate());

                                let codec = ffmpeg::codec::context::Context::from_parameters(stream.parameters()).hand_err(|msg| error!("{msg}")).unwrap();
                                println!("\tmedium: {:?}", codec.medium());
                                println!("\tid: {:?}", codec.id());

                                if codec.medium() == ffmpeg::media::Type::Video {
                                    if let Ok(video) = codec.decoder().video() {
                                        println!("\tbit_rate: {}", video.bit_rate());
                                        println!("\tmax_rate: {}", video.max_bit_rate());
                                        println!("\tdelay: {}", video.delay());
                                        println!("\tvideo.width: {}", video.width());
                                        println!("\tvideo.height: {}", video.height());
                                        println!("\tvideo.format: {:?}", video.format());
                                        println!("\tvideo.has_b_frames: {}", video.has_b_frames());
                                        println!("\tvideo.aspect_ratio: {}", video.aspect_ratio());
                                        println!("\tvideo.color_space: {:?}", video.color_space());
                                        println!("\tvideo.color_range: {:?}", video.color_range());
                                        println!("\tvideo.color_primaries: {:?}", video.color_primaries());
                                        println!(
                                            "\tvideo.color_transfer_characteristic: {:?}",
                                            video.color_transfer_characteristic()
                                        );
                                        println!("\tvideo.chroma_location: {:?}", video.chroma_location());
                                        println!("\tvideo.references: {}", video.references());
                                        println!("\tvideo.intra_dc_precision: {}", video.intra_dc_precision());
                                    }
                                } else if codec.medium() == ffmpeg::media::Type::Audio {
                                    if let Ok(audio) = codec.decoder().audio() {
                                        println!("\tbit_rate: {}", audio.bit_rate());
                                        println!("\tmax_rate: {}", audio.max_bit_rate());
                                        println!("\tdelay: {}", audio.delay());
                                        println!("\taudio.rate: {}", audio.rate());
                                        println!("\taudio.channels: {}", audio.channels());
                                        println!("\taudio.format: {:?}", audio.format());
                                        println!("\taudio.frames: {}", audio.frames());
                                        println!("\taudio.align: {}", audio.align());
                                        println!("\taudio.channel_layout: {:?}", audio.channel_layout());
                                    }
                                }
                            }
                        }
                        e => {
                            avformat_close_input(&mut fmt_ctx);
                            error!("Could not find stream information. err = {:?}", er::Error::from(e));
                        }
                    }
                }

            e => error!("Could not open input. err = {:?}", er::Error::from(e))
        }
        // if !io_ctx.is_null() {
        //     av_freep((*io_ctx).buffer.cast());
        // }
        // avio_context_free(io_ctx.cast());
        Ok(())
    }
}

#[cfg(test)]
mod tests{
    fn test_c_str(){

    }
}