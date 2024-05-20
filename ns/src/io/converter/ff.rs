use std::ffi::CString;
use std::os::raw::{c_int, c_void};
use std::ptr;
use ffmpeg_next::{codec, Dictionary, encoder, format, media, Rational, Rescale, Rounding};
use ffmpeg_next::ffi::{av_dict_free, av_find_input_format, av_freep, av_malloc, avcodec_copy_context, AVFMT_FLAG_CUSTOM_IO, AVFMT_FLAG_FLUSH_PACKETS, AVFMT_FLAG_NOBUFFER, AVFMT_NOFILE, avformat_alloc_context, avformat_alloc_output_context2, avformat_close_input, avformat_find_stream_info, avformat_open_input, AVFormatContext, avio_alloc_context, avio_context_free};
use ffmpeg_next::ffi::AVRounding::{AV_ROUND_NEAR_INF, AV_ROUND_PASS_MINMAX};
use ffmpeg_next::format::context;
use ffmpeg_next::format::context::input;
use ffmpeg_next::sys::AV_NOPTS_VALUE;
use common::err::{GlobalResult, TransError};
use common::log::error;
use crate::general::mode::AV_IO_CTX_BUFFER_SIZE;
use ffmpeg_next::util::error as er;
use serde_json::Value::String;
use common::anyhow::anyhow;
use common::err::GlobalError::SysErr;
use crate::io::converter::handler;

pub(crate) fn parse(ssrc: u32) -> GlobalResult<()> {
    unsafe {
        let sdp_str = r#"
v=0
o=- 0 0 IN IP4 127.0.0.1
s=No Name
c=IN IP4 172.18.38.186
t=0 0
a=tool:libavformat 58.76.100
m=video 18568 RTP/AVP 96
b=AS:980
a=rtpmap:96 H264/90000
a=fmtp:96 packetization-mode=1; sprop-parameter-sets=Z00AKpWoHgCJ+VA=,aO48gA==; profile-level-id=4D002A"#;

        let mut dict = Dictionary::new();
        dict.set("sdp_flags", "custom_io");
        let mut sdp_options = dict.disown();
        let sdp_protocol = CString::new("sdp".to_string()).unwrap().into_raw();
        let input_format = av_find_input_format(sdp_protocol);
        let mut fctx = avformat_alloc_context();
        (*fctx).flags |= AVFMT_FLAG_NOBUFFER | AVFMT_FLAG_FLUSH_PACKETS | AVFMT_FLAG_CUSTOM_IO;
        let sdp_buf_size = 4096 as c_int;
        let sdp_buf = av_malloc(AV_IO_CTX_BUFFER_SIZE as usize).cast();
        let sdp_ioctx =
            avio_alloc_context(
                sdp_buf,                                            // buffer
                sdp_buf_size,                                       // buffer_size
                0,                                                  // write_flag
                &sdp_str.to_string() as *const _ as *mut c_void,                    // opaque
                Some(handler::call_sdp_str_read()),                               // read_packet
                None,                                            // write_packet
                None);                                           // seek
        (*fctx).pb = sdp_ioctx;
        let ret = avformat_open_input(&mut fctx, ptr::null(), input_format, &mut sdp_options);
        if ret < 0 {
            println!("sdp open err");
            return Err(SysErr(anyhow!("sdp open err")));
        }
        let opa = Box::into_raw(Box::new(ssrc)) as *mut c_void;
        let rtp_ioctx =
            avio_alloc_context(
                av_malloc(AV_IO_CTX_BUFFER_SIZE as usize).cast(), // buffer
                AV_IO_CTX_BUFFER_SIZE as c_int,                                   // buffer size
                1,                                                  // write_flag
                opa,        // opaque
                Some(handler::call_rtp_read()),                                 // read_packet
                Some(handler::call_rtcp_write()),                                 // write_packet
                None);

        (*fctx).pb = rtp_ioctx;
        // (*fctx).flags |= AVFMT_NOFILE;
        // (*fctx).max_analyze_duration = 10000;
        // (*fctx).probesize = 1024;

        // let mut dict = Dictionary::new();
        // dict.set("sdp_flags", "custom_io");
        // dict.set("reorder_queue_size", "8");
        // dict.set("protocol_whitelist", "file,udp,rtp");
        // dict.set("buffer_size", "80000");
        // let mut opts = dict.disown();
        // println!("to open rtp io");
        // // let ret = avformat_open_input(&mut fctx, ptr::null(), input_format, &mut sdp_options);
        // let ret = avformat_open_input(&mut fctx, ptr::null(), input_format, &mut opts);
        // let ret = avformat_open_input(&mut fctx, ptr::null(), ptr::null_mut(), ptr::null_mut());
        // if ret < 0 {
        //     println!("rtp open err");
        //     return Err(SysErr(anyhow!("rtp open err")));
        // }
        av_dict_free(&mut sdp_options);
        let output_file = "123.flv".to_string();
        let mut output = format::output(&output_file).unwrap();
        println!("rtp open io");
        match avformat_find_stream_info(fctx, ptr::null_mut()) {
            r if r >= 0 => {
                let mut ictx = context::Input::wrap(fctx);
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
                    avcodec_copy_context((*ost.as_mut_ptr()).codec, (*ist.as_ptr()).codec);
                    // let mut ost = output.add_stream(encoder::find(codec::Id::None)).unwrap();
                    ost.set_parameters(ist.parameters());
                    (*ost.parameters().as_mut_ptr()).codec_tag = 0;
                }
                println!("ost index size = {ost_index}");
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

                    packet.set_pts(packet.pts().map(|v| v.rescale_with(stream.time_base(), ost.time_base(), Rounding::PassMinMax)));
                    packet.set_dts(packet.dts().map(|v| v.rescale_with(stream.time_base(), ost.time_base(), Rounding::PassMinMax)));
                    packet.set_duration(packet.duration().rescale(stream.time_base(), ost.time_base()));

                    packet.set_position(-1);
                    packet.set_stream(ost_index as _);
                    let _ = packet.write_interleaved(&mut output).hand_err(|msg| println!("write err: {msg}"));
                }
                output.write_trailer().unwrap();

                // if !io_ctx.is_null() {
                //     av_freep((*io_ctx).buffer.cast());
                // }
                // avio_context_free(io_ctx.cast());
            }
            e => {
                // avformat_close_input(&mut fmt_ctx);
                error!("Could not find stream information. err = {:?}", er::Error::from(e));
            }
        }
        Ok(())
    }
}


pub(crate) fn parse3(ssrc: u32) -> GlobalResult<()> {
    unsafe {
        let in_f = CString::new("rtp".to_string()).unwrap().into_raw();
        let input_format = av_find_input_format(in_f);
        let mut fmt_ctx = avformat_alloc_context();
        let mut dict = Dictionary::new();
        // dict.set("sdp_flags", "custom_io");
        // dict.set("reorder_queue_size", "8");
        // dict.set("protocol_whitelist", "file,udp,rtp");
        let sdp_str = "
        v=0
o=- 0 0 IN IP4 127.0.0.1
s=No Name
c=IN IP4 172.18.38.186
t=0 0
a=tool:libavformat 58.76.100
m=video 18568 RTP/AVP 96
b=AS:980
a=rtpmap:96 H264/90000
a=fmtp:96 packetization-mode=1; sprop-parameter-sets=Z00AKpWoHgCJ+VA=,aO48gA==; profile-level-id=4D002A";
        dict.set("sdp", sdp_str);
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


        // let mut sdp_ctx = ptr::null_mut();
        // let i = avformat_open_input(&mut sdp_ctx, sdp, ptr::null_mut(), &mut opts);
        // if i < 0 {
        //     println!("Could not open SDP file");
        //     return Err(SysErr(anyhow!("Could not open SDP file")));
        // }
        // avformat_close_input(&mut sdp_ctx);

        //Some(handler::call_rtcp_write())
        let io_ctx = avio_alloc_context(in_tx_bf, AV_IO_CTX_BUFFER_SIZE as c_int, 0, opa, Some(handler::call_rtp_read()), None, None);
        (*fmt_ctx).pb = io_ctx;
        (*fmt_ctx).flags |= AVFMT_NOFILE;
        // (*fmt_ctx).max_analyze_duration = 80000;
        // (*fmt_ctx).probesize = 10240;
        // if fmt_ctx.is_null() || io_ctx.is_null() {
        //     println!("is null");
        //     return Err(SysErr(anyhow!("Context is null")));
        // }
        match avformat_open_input(&mut fmt_ctx, ptr::null(), input_format, &mut opts) {
            // match avformat_open_input(&mut fmt_ctx, ptr::null(), ptr::null_mut(), &mut opts) {
            0 =>
                {
                    // let io_ctx = avio_alloc_context(in_tx_bf, AV_IO_CTX_BUFFER_SIZE as c_int, 0, opa, Some(handler::call_rtp_read()), None, None);
                    // (*fmt_ctx).pb = io_ctx;
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
                                avcodec_copy_context((*ost.as_mut_ptr()).codec, (*ist.as_ptr()).codec);
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

                                packet.set_pts(packet.pts().map(|v| v.rescale_with(stream.time_base(), ost.time_base(), Rounding::PassMinMax)));
                                packet.set_dts(packet.dts().map(|v| v.rescale_with(stream.time_base(), ost.time_base(), Rounding::PassMinMax)));
                                packet.set_duration(packet.duration().rescale(stream.time_base(), ost.time_base()));

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

pub(crate) fn parse(ssrc: u32) -> GlobalResult<()> {
    unsafe {
        let in_f = CString::new("sdp".to_string()).unwrap().into_raw();
        let input_format = av_find_input_format(in_f);
        let mut fmt_ctx = avformat_alloc_context();
        let mut dict = Dictionary::new();
        dict.set("sdp_flags", "custom_io");
        dict.set("reorder_queue_size", "8");
        dict.set("protocol_whitelist", "file,udp,rtp");
        dict.set("buffer_size", "8000000");
        let mut opts = dict.disown();
        let in_tx_bf = av_malloc(AV_IO_CTX_BUFFER_SIZE as usize).cast();
        let opa = Box::into_raw(Box::new(ssrc)) as *mut c_void;

        let mut out_fmt_ctx = avformat_alloc_context();
        avformat_alloc_output_context2(&mut out_fmt_ctx, ptr::null_mut(), CString::new("flv").unwrap().as_ptr(), ptr::null());
        (*out_fmt_ctx).pb = avio_alloc_context(in_tx_bf, AV_IO_CTX_BUFFER_SIZE as c_int, 1, opa, None, Some(handler::call_flv_write()), None);
        let mut output = context::Output::wrap(out_fmt_ctx);
        if out_fmt_ctx.is_null() {
            error!("Could not deduce output format from flv");
            return Err(SysErr(anyhow!("Could not deduce output format from flv")));
        }

        let output_file = "123.flv".to_string();
        let mut output = format::output(&output_file).unwrap();
        let sdp = CString::new("/home/ubuntu20/code/rs/mv/github/epimore/gmv/ns/123.sdp".to_string()).expect("CString::new failed").into_raw();
        match avformat_open_input(&mut fmt_ctx, sdp, input_format, &mut opts) {
            0 => {
                //Some(handler::call_rtcp_write())
                let io_ctx = avio_alloc_context(in_tx_bf, AV_IO_CTX_BUFFER_SIZE as c_int, 1, opa, Some(handler::call_rtp_read()), Some(handler::call_rtcp_write()), None);
                (*fmt_ctx).pb = io_ctx;
                (*fmt_ctx).flags |= AVFMT_FLAG_NOBUFFER | AVFMT_FLAG_FLUSH_PACKETS;
                (*fmt_ctx).max_analyze_duration = 10000;
                (*fmt_ctx).probesize = 1024;
                (*fmt_ctx).fps_probe_size = 0;
                (*fmt_ctx).avio_flags = 0;
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
                            avcodec_copy_context((*ost.as_mut_ptr()).codec, (*ist.as_ptr()).codec);
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

                            packet.set_pts(packet.pts().map(|v| v.rescale_with(stream.time_base(), ost.time_base(), Rounding::PassMinMax)));
                            packet.set_dts(packet.dts().map(|v| v.rescale_with(stream.time_base(), ost.time_base(), Rounding::PassMinMax)));
                            packet.set_duration(packet.duration().rescale(stream.time_base(), ost.time_base()));

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