use std::ffi::CString;
use std::os::raw::{c_int, c_void};
use std::ptr;
use ffmpeg_next::{codec, Dictionary, encoder, format, media, Rational, Rescale, Rounding};
use ffmpeg_next::ffi::{av_find_input_format, av_freep, av_malloc, avcodec_copy_context, AVFMT_NOFILE, avformat_alloc_context, avformat_alloc_output_context2, avformat_close_input, avformat_find_stream_info, avformat_open_input, avio_alloc_context, avio_context_free};
use ffmpeg_next::ffi::AVRounding::{AV_ROUND_NEAR_INF, AV_ROUND_PASS_MINMAX};
use ffmpeg_next::format::context;
use ffmpeg_next::format::context::input;
use ffmpeg_next::sys::AV_NOPTS_VALUE;
use common::err::{GlobalResult, TransError};
use common::log::error;
use crate::general::mode::AV_IO_CTX_BUFFER_SIZE;
use ffmpeg_next::util::error as er;
use common::anyhow::anyhow;
use common::err::GlobalError::SysErr;
use crate::io::converter::handler;

pub(crate) fn parse(ssrc: u32) -> GlobalResult<()> {
    unsafe {
        let in_f = CString::new("sdp".to_string()).unwrap().into_raw();
        let input_format = av_find_input_format(in_f);
        let mut fmt_ctx =  avformat_alloc_context();
        let mut dict = Dictionary::new();
        dict.set("sdp_flags", "custom_io");
        dict.set("reorder_queue_size", "8");
        dict.set("protocol_whitelist", "file,udp,rtp");
        let mut opts = dict.disown();
        let in_tx_bf = av_malloc(AV_IO_CTX_BUFFER_SIZE as usize).cast();
        let opa = Box::into_raw(Box::new(ssrc)) as *mut c_void;

        // let mut out_fmt_ctx = avformat_alloc_context();
        // avformat_alloc_output_context2(&mut out_fmt_ctx, ptr::null_mut(), CString::new("flv").unwrap().as_ptr(), ptr::null());
        // (*out_fmt_ctx).pb = avio_alloc_context(in_tx_bf, AV_IO_CTX_BUFFER_SIZE as c_int, 0, opa, None, Some(handler::call_flv_write()), None);
        // let mut output = context::Output::wrap(out_fmt_ctx);
        // if out_fmt_ctx.is_null() {
        //     error!("Could not deduce output format from flv");
        //     return Err(SysErr(anyhow!("Could not deduce output format from flv")));
        // }

        let output_file = "123.flv".to_string();
        let mut output = format::output(&output_file).unwrap();
        let sdp = CString::new("/home/ubuntu20/code/rs/mv/github/epimore/gmv/ns/123.sdp".to_string()).expect("CString::new failed").into_raw();
        match avformat_open_input(&mut fmt_ctx, sdp, input_format, &mut opts) {
            0 =>
                {
                    //Some(handler::call_rtcp_write())
                    let io_ctx = avio_alloc_context(in_tx_bf, AV_IO_CTX_BUFFER_SIZE as c_int, 1, opa, Some(handler::call_rtp_read()), Some(handler::call_rtcp_write()), None);
                    (*fmt_ctx).pb = io_ctx;
                    (*fmt_ctx).flags |= AVFMT_NOFILE;
                    (*fmt_ctx).max_analyze_duration = 8000;
                    (*fmt_ctx).probesize = 512;
                    if fmt_ctx.is_null() || io_ctx.is_null() {
                        println!("is null");
                        return Err(SysErr(anyhow!("Context is null")));
                    }
                    match avformat_find_stream_info(fmt_ctx, ptr::null_mut()) {
                        r if r >= 0 => {
                            let mut ictx = context::Input::wrap(fmt_ctx);
                            input::dump(&ictx, 0, None);
                            let mut stream_mapping = vec![0; ictx.nb_streams() as _];
                            let mut ist_time_bases = vec![Rational(0, 1); ictx.nb_streams() as _];
                            let mut ost_index = 0;
                            for (ist_index, ist) in ictx.streams().enumerate() {
                                let ist_medium = ist.parameters().medium();
                                if ist_medium != media::Type::Audio
                                    && ist_medium != media::Type::Video
                                    && ist_medium != media::Type::Subtitle
                                {
                                    stream_mapping[ist_index] = -1;
                                    continue;
                                }
                                stream_mapping[ist_index] = ost_index;
                                ist_time_bases[ist_index] = ist.time_base();
                                ost_index += 1;
                                let mut ost = output.add_stream(encoder::find(codec::Id::None)).unwrap();
                                // avcodec_copy_context((*ost.as_mut_ptr()).codec, (*ist.as_ptr()).codec);
                                // let mut ost = output.add_stream(encoder::find(codec::Id::None)).unwrap();
                                ost.set_parameters(ist.parameters());
                                (*ost.parameters().as_mut_ptr()).codec_tag = 0;
                            }
                            output.set_metadata(ictx.metadata().to_owned());
                            if output.write_header().is_err() {
                                return Err(SysErr(anyhow!("output write_header err")));
                            }
                            (*output.as_mut_ptr()).flags |= AVFMT_NOFILE;
                            for (stream, mut packet) in ictx.packets() {
                                let ist_index = stream.index();
                                let ost_index = stream_mapping[ist_index];
                                if ost_index < 0 {
                                    continue;
                                }
                                let ost = output.stream(ost_index as _).unwrap();
                                packet.rescale_ts(ist_time_bases[ist_index], ost.time_base());

                                // packet.set_pts(packet.pts().map(|v| v.rescale_with(stream.time_base(), ost.time_base(), Rounding::PassMinMax)));
                                // packet.set_dts(packet.dts().map(|v| v.rescale_with(stream.time_base(), ost.time_base(), Rounding::PassMinMax)));
                                // packet.set_duration(packet.duration().rescale(stream.time_base(), ost.time_base()));

                                packet.set_position(-1);
                                packet.set_stream(ost_index as _);
                                let _ = packet.write_interleaved(&mut output).hand_err(|msg| println!("write err: {msg}"));
                            }
                            output.write_trailer().unwrap();

                            if !io_ctx.is_null() {
                                av_freep((*io_ctx).buffer.cast());
                            }
                            avio_context_free(io_ctx.cast());
                        }
                        e => {
                            avformat_close_input(&mut fmt_ctx);
                            error!("Could not find stream information. err = {:?}", er::Error::from(e));
                        }
                    }
                }

            e => error!("Could not open input. err = {:?}", er::Error::from(e))
        }
        Ok(())
    }
}