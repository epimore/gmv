use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;

use ffmpeg_next as ffmpeg;
use ffmpeg_next::format;
use ffmpeg_next::format::context;
use ffmpeg_next::log::Flags;
use ffmpeg_next::sys::{av_find_input_format, av_freep, av_malloc, AVFMT_FLAG_CUSTOM_IO, AVFMT_NOFILE, AVFMT_SHOW_IDS, avformat_alloc_context, avformat_close_input, avformat_find_stream_info, avformat_open_input, AVFormatContext, avio_alloc_context, avio_context_free, AVIO_FLAG_READ};
use ffmpeg_next::util::error as er;

use common::err::{GlobalResult, TransError};
use common::log::error;

use crate::data;
use crate::general::mode::AV_IO_CTX_BUFFER_SIZE;

pub async fn parse(ssrc: u32) -> GlobalResult<()> {
    ffmpeg_next::init().hand_err(|msg| error!("{msg}")).unwrap();
    unsafe { ffmpeg::ffi::av_log_set_level(ffmpeg::ffi::AV_LOG_TRACE); }
    let call = data::call();
    let in_tx_bf = unsafe { av_malloc(AV_IO_CTX_BUFFER_SIZE as usize) }.cast();
    let opa = Box::into_raw(Box::new(ssrc)) as *mut c_void;
    let io_ctx = unsafe { avio_alloc_context(in_tx_bf, AV_IO_CTX_BUFFER_SIZE as c_int, 0, opa, Some(call), None, None) };
    let mut fmt_ctx = unsafe { avformat_alloc_context() };
    data::buffer::Cache::readable(&ssrc).await?;
    unsafe {
        (*fmt_ctx).pb = io_ctx;
        (*fmt_ctx).flags |= AVFMT_FLAG_CUSTOM_IO;
        // (*fmt_ctx).max_analyze_duration = 1000000;
        (*fmt_ctx).fps_probe_size = 5;
    }
    if fmt_ctx.is_null() || io_ctx.is_null() {
        println!("is null");
    }
    data::buffer::Cache::readable(&ssrc).await?;
    unsafe {
        match avformat_open_input(&mut fmt_ctx, ptr::null(), ptr::null_mut(), ptr::null_mut()) {
            0 =>
                match avformat_find_stream_info(fmt_ctx, ptr::null_mut()) {
                r if r >= 0 => {
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
                        data::buffer::Cache::readable(&1).await.expect("readable exception");

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
            },

            e => error!("Could not open input. err = {:?}", er::Error::from(e))
        }
        if !io_ctx.is_null() {
            av_freep((*io_ctx).buffer.cast());
        }
        avio_context_free(io_ctx.cast());
    }
    Ok(())
}