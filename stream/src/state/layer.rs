pub mod output_layer {
    use common::exception::code::conf_err::CONFIG_ERROR_CODE;
    use common::exception::{GlobalError, GlobalResult};
    use common::log::error;
    use shared::info::io::{Dash, Gb28181, Hls, HttpFlv, Local, Output, Rtmp, Rtsp, WebRtc};
    use shared::paste::paste;
    use shared::{impl_check_empty, impl_open_close};

    pub struct OutputLayer {
        local: Option<LocalLayer>,
        rtmp: Option<RtmpLayer>,
        http_flv: Option<HttpFlvLayer>,
        dash: Option<DashLayer>,
        hls: Option<HlsLayer>,
        rtsp: Option<RtspLayer>,
        gb28181: Option<Gb28181Layer>,
        web_rtc: Option<WebRtcLayer>,
    }
    impl OutputLayer {
        pub fn bean_to_layer(output: Output) -> GlobalResult<Self> {
            if output.check_empty() {
                return Err(GlobalError::new_biz_error(CONFIG_ERROR_CODE, "Output cannot be empty", |msg| error!("{msg}")));
            }
            let layer = OutputLayer {
                local: output.local.map(LocalLayer::bean_to_layer),
                rtmp: output.rtmp.map(RtmpLayer::bean_to_layer),
                http_flv: output.http_flv.map(HttpFlvLayer::bean_to_layer),
                dash: output.dash.map(DashLayer::bean_to_layer),
                hls: output.hls.map(HlsLayer::bean_to_layer),
                rtsp: output.rtsp.map(RtspLayer::bean_to_layer),
                gb28181: output.gb28181.map(Gb28181Layer::bean_to_layer),
                web_rtc: output.web_rtc.map(WebRtcLayer::bean_to_layer),
            };
            Ok(layer)
        }
    }

    pub struct LocalLayer {}
    impl LocalLayer {
        pub fn bean_to_layer(local: Local) -> Self {
            unimplemented!()
        }
    }
    pub struct HlsLayer {}
    impl HlsLayer {
        pub fn bean_to_layer(hls: Hls) -> Self {
            unimplemented!()
        }
    }
    pub struct HttpFlvLayer {}
    impl HttpFlvLayer {
        pub fn bean_to_layer(http_flv: HttpFlv) -> Self {
            unimplemented!()
        }
    }
    pub struct RtmpLayer {}
    impl RtmpLayer {
        pub fn bean_to_layer(rtmp: Rtmp) -> Self {
            unimplemented!()
        }
    }
    pub struct RtspLayer {}
    impl RtspLayer {
        pub fn bean_to_layer(rtsp: Rtsp) -> Self {
            unimplemented!()
        }
    }
    pub struct DashLayer {}
    impl DashLayer {
        pub fn bean_to_layer(dash: Dash) -> Self {
            unimplemented!()
        }
    }
    pub struct Gb28181Layer {}
    impl Gb28181Layer {
        pub fn bean_to_layer(gb28181: Gb28181) -> Self {
            unimplemented!()
        }
    }
    pub struct WebRtcLayer {}
    impl WebRtcLayer {
        pub fn bean_to_layer(web_rtc: WebRtc) -> Self {
            unimplemented!()
        }
    }

    impl_check_empty!(OutputLayer, [local, rtmp, http_flv, dash, hls, rtsp, gb28181, web_rtc]);

    impl_open_close!(OutputLayer, {
    local: LocalLayer,
    rtmp: RtmpLayer,
    http_flv: HttpFlvLayer,
    dash: DashLayer,
    hls: HlsLayer,
    rtsp: RtspLayer,
    gb28181: Gb28181Layer,
    web_rtc: WebRtcLayer,
    });
}

pub mod converter_layer {
    use crate::state::layer::codec_layer::CodecLayer;
    use crate::state::layer::filter_layer::FilterLayer;
    use crate::state::layer::muxer_layer::MuxerLayer;
    use shared::info::media_info::Converter;

    #[derive(Clone)]
    pub struct ConverterLayer {
        pub codec: Option<CodecLayer>,
        pub muxer: MuxerLayer,
        pub filter: FilterLayer,
    }

    impl ConverterLayer {
        pub fn bean_to_layer(converter: Converter) -> Self {
            ConverterLayer {
                codec: converter.codec.map(CodecLayer::bean_to_layer),
                muxer: MuxerLayer::bean_to_layer(converter.muxer),
                filter: FilterLayer::bean_to_layer(converter.filter),
            }
        }
        
    }
}
pub mod filter_layer {
    use shared::info::filter::{Capture, Filter};

    #[derive(Clone)]
    pub struct CaptureLayer {}
    impl CaptureLayer {
        pub fn bean_to_layer(capture: Capture) -> Self {
            unimplemented!()
        }
    }
    #[derive(Clone)]
    pub struct FilterLayer {
        //抽图
        pub capture: Option<CaptureLayer>,
        //缩放
        // pub scale: Option<Scale>,
        //裁剪
        // pub crop: Option<Crop>,
        //旋转
        // pub rotate: Option<Rotate>,
        //镜像
        // pub mirror: Option<Mirror>,
    }

    impl FilterLayer {
        pub fn bean_to_layer(filter: Filter) -> Self {
            FilterLayer {
                capture: filter.capture.map(CaptureLayer::bean_to_layer),
                // scale: filter.scale,
                // crop: filter.crop,
                // rotate: filter.rotate,
                // mirror: filter.mirror,
            }
        }
    }
}
pub mod muxer_layer {
    use crate::media::context::format::flv::FlvPacket;
    use crate::state::FORMAT_BROADCAST_BUFFER;
    use common::tokio::sync::broadcast;
    use shared::info::format::{Flv, Frame, Mp4, Muxer, RtpEnc, RtpFrame, RtpPs, Ts};
    use std::sync::Arc;

    #[derive(Clone)]
    pub struct MuxerLayer {
        pub flv: Option<FlvLayer>,
        pub mp4: Option<Mp4Layer>,
        pub ts: Option<TsLayer>,
        pub rtp_frame: Option<RtpFrameLayer>,
        pub rtp_ps: Option<RtpPsLayer>,
        pub rtp_enc: Option<RtpEncLayer>,
        pub frame: Option<FrameLayer>,
    }
    impl MuxerLayer {
        pub fn bean_to_layer(muxer: Muxer) -> Self {
            MuxerLayer {
                flv: muxer.flv.map(FlvLayer::bean_to_layer),
                mp4: muxer.mp4.map(Mp4Layer::bean_to_layer),
                ts: muxer.ts.map(TsLayer::bean_to_layer),
                rtp_frame: muxer.rtp_frame.map(RtpFrameLayer::bean_to_layer),
                rtp_ps: muxer.rtp_ps.map(RtpPsLayer::bean_to_layer),
                rtp_enc: muxer.rtp_enc.map(RtpEncLayer::bean_to_layer),
                frame: muxer.frame.map(FrameLayer::bean_to_layer),
            }
        }
    }
    #[derive(Clone)]
    pub struct FlvLayer {
        pub tx: broadcast::Sender<Arc<FlvPacket>>,
        pub flv: Flv,
    }
    impl FlvLayer {
        pub fn bean_to_layer(flv: Flv) -> Self {
            let (tx, _) = broadcast::channel(FORMAT_BROADCAST_BUFFER);
            Self {
                tx,
                flv,
            }
        }
    }
    #[derive(Clone)]
    pub struct FrameLayer {}
    impl FrameLayer {
        pub fn bean_to_layer(frame: Frame) -> Self {
            unimplemented!()
        }
    }
    #[derive(Clone)]
    pub struct Mp4Layer {}
    impl Mp4Layer {
        pub fn bean_to_layer(mp4: Mp4) -> Self {
            unimplemented!()
        }
    }
    #[derive(Clone)]
    pub struct RtpFrameLayer {}
    impl RtpFrameLayer {
        pub fn bean_to_layer(rtp: RtpFrame) -> Self {
            unimplemented!()
        }
    }
    #[derive(Clone)]
    pub struct RtpPsLayer {}
    impl RtpPsLayer {
        pub fn bean_to_layer(rtp: RtpPs) -> Self {
            unimplemented!()
        }
    }
    #[derive(Clone)]
    pub struct RtpEncLayer {}
    impl RtpEncLayer {
        pub fn bean_to_layer(rtp: RtpEnc) -> Self {
            unimplemented!()
        }
    }
    #[derive(Clone)]
    pub struct TsLayer {}
    impl TsLayer {
        pub fn bean_to_layer(ts: Ts) -> Self {
            unimplemented!()
        }
    }
}
pub mod codec_layer {
    use shared::info::codec::Codec;

    #[derive(Clone)]
    pub enum CodecLayer {
        //video
        Mpeg4,
        H264,
        SvacVideo,
        H265,
        //audio
        G711a,
        G711u,
        G7221,
        G7231,
        G729,
        SvacAudio,
        Aac,
    }
    impl CodecLayer {
        pub fn bean_to_layer(codec: Codec) -> Self {
            match codec {
                Codec::Mpeg4 => { CodecLayer::Mpeg4 }
                Codec::H264 => { CodecLayer::H264 }
                Codec::SvacVideo => { CodecLayer::SvacVideo }
                Codec::H265 => { CodecLayer::H265 }
                Codec::G711a => { CodecLayer::G711a }
                Codec::G711u => { CodecLayer::G711u }
                Codec::G7221 => { CodecLayer::G7221 }
                Codec::G7231 => { CodecLayer::G7231 }
                Codec::G729 => { CodecLayer::G729 }
                Codec::SvacAudio => { CodecLayer::SvacAudio }
                Codec::Aac => { CodecLayer::Aac }
            }
        }
        pub fn to_string(&self) -> String {
            match self {
                CodecLayer::Mpeg4 => "mpeg4".to_string(),
                CodecLayer::H264 => "h264".to_string(),
                CodecLayer::SvacVideo => "svac_video".to_string(),
                CodecLayer::H265 => "h265".to_string(),
                CodecLayer::G711a => "g711a".to_string(),
                CodecLayer::G711u => "g711u".to_string(),
                CodecLayer::G7221 => "g7221".to_string(),
                CodecLayer::G7231 => "g7231".to_string(),
                CodecLayer::G729 => "g729".to_string(),
                CodecLayer::SvacAudio => "svac_audio".to_string(),
                CodecLayer::Aac => "aac".to_string(),
            }
        }
    }
}
