pub mod output_layer {
    use base::exception::code::conf_err::CONFIG_ERROR_CODE;
    use base::exception::{GlobalError, GlobalResult};
    use base::log::error;
    use shared::paste::paste;
    use shared::{impl_check_empty, impl_open_close};
    use shared::info::output::OutputKind;

    pub struct OutputLayer {
        pub http_flv: Option<HttpFlvLayer>,
        pub rtmp: Option<RtmpLayer>,
        pub dash_fmp4: Option<DashFmp4Layer>,
        pub hls_fmp4: Option<HlsFmp4Layer>,
        pub hls_ts: Option<HlsTsLayer>,
        pub rtsp: Option<RtspLayer>,
        pub gb28181_frame: Option<Gb28181FrameLayer>,
        pub gb28181_ps: Option<Gb28181PsLayer>,
        pub web_rtc: Option<WebRtcLayer>,
        pub local_mp4: Option<LocalMp4Layer>,
        pub local_ts: Option<LocalTsLayer>,
    }
    impl OutputLayer {
        pub fn put_if_absent(&mut self, output: OutputKind) {
            if self.local.is_none() {
                self.local = output.local.map(LocalMp4Layer::layer);
            }
            if self.rtmp.is_none() {
                self.rtmp = output.rtmp.map(RtmpLayer::layer);
            }
            if self.http_flv.is_none() {
                self.http_flv = output.http_flv.map(HttpFlvLayer::layer);
            }
            if self.dash.is_none() {
                self.dash = output.dash.map(DashFmp4Layer::layer);
            }
            if self.hls.is_none() {
                self.hls = output.hls.map(HlsTsLayer::layer);
            }
            if self.rtsp.is_none() {
                self.rtsp = output.rtsp.map(RtspLayer::layer);
            }
            if self.gb28181.is_none() {
                self.gb28181 = output.gb28181.map(Gb28181PsLayer::layer);
            }
            if self.web_rtc.is_none() {
                self.web_rtc = output.web_rtc.map(WebRtcLayer::layer);
            }
        }

        pub fn layer(output: Output) -> GlobalResult<Self> {
            if output.check_empty() {
                return Err(GlobalError::new_biz_error(
                    CONFIG_ERROR_CODE,
                    "Output cannot be empty",
                    |msg| error!("{msg}"),
                ));
            }
            let layer = OutputLayer {
                local: output.local.map(LocalMp4Layer::layer),
                rtmp: output.rtmp.map(RtmpLayer::layer),
                http_flv: output.http_flv.map(HttpFlvLayer::layer),
                dash: output.dash.map(DashFmp4Layer::layer),
                hls: output.hls.map(HlsTsLayer::layer),
                rtsp: output.rtsp.map(RtspLayer::layer),
                gb28181: output.gb28181.map(Gb28181PsLayer::layer),
                web_rtc: output.web_rtc.map(WebRtcLayer::layer),
            };
            Ok(layer)
        }
    }

    pub struct LocalTsLayer {
        pub local: Local,
    }
    pub struct LocalMp4Layer {
        pub local: Local,
    }
    impl LocalMp4Layer {
        pub fn layer(local: Local) -> Self {
            Self { local }
        }
    }
    pub struct HlsFmp4Layer {}
    impl HlsFmp4Layer {
        pub fn layer(hls: Hls) -> Self {
            unimplemented!()
        }
    }

    pub struct HlsTsLayer {}
    impl HlsTsLayer {
        pub fn layer(hls: Hls) -> Self {
            unimplemented!()
        }
    }
    pub struct HttpFlvLayer {}
    impl HttpFlvLayer {
        pub fn layer(http_flv: HttpFlv) -> Self {
            Self {}
        }
    }
    pub struct RtmpLayer {}
    impl RtmpLayer {
        pub fn layer(rtmp: Rtmp) -> Self {
            unimplemented!()
        }
    }
    pub struct RtspLayer {}
    impl RtspLayer {
        pub fn layer(rtsp: Rtsp) -> Self {
            unimplemented!()
        }
    }
    pub struct DashFmp4Layer {}
    impl DashFmp4Layer {
        pub fn layer(dash: Dash) -> Self {
            unimplemented!()
        }
    }
    pub struct Gb28181FrameLayer {}
    pub struct Gb28181PsLayer {}
    impl Gb28181PsLayer {
        pub fn layer(gb28181: Gb28181) -> Self {
            unimplemented!()
        }
    }
    pub struct WebRtcLayer {}
    impl WebRtcLayer {
        pub fn layer(web_rtc: WebRtc) -> Self {
            unimplemented!()
        }
    }

    impl_check_empty!(
        OutputLayer,
        [local, rtmp, http_flv, dash, hls, rtsp, gb28181, web_rtc]
    );

    impl_open_close!(OutputLayer, {
    local: LocalLayer,
    rtmp: RtmpLayer,
    http_flv: HttpFlvLayer,
    dash: DashFmp4Layer,
    hls: HlsLayer,
    rtsp: RtspLayer,
    gb28181: Gb28181PsLayer,
    web_rtc: WebRtcLayer,
    });
}

pub mod converter_layer {
    use crate::state::layer::codec_layer::CodecLayer;
    use crate::state::layer::filter_layer::FilterLayer;
    use crate::state::layer::muxer_layer::MuxerLayer;

    #[derive(Clone)]
    pub struct ConverterLayer {
        pub codec: Option<CodecLayer>,
        pub muxer: MuxerLayer,
        pub filter: FilterLayer,
    }

    impl ConverterLayer {
        pub fn put_if_absent(&mut self, converter: Converter, output: &Output) {
            if self.codec.is_none() {
                self.codec = converter.codec.map(CodecLayer::layer);
            }
            self.muxer.put_if_absent(converter.muxer, output);
            self.filter.put_if_absent(converter.filter);
        }
        pub fn layer(converter: Converter, output: &Output) -> Self {
            ConverterLayer {
                codec: converter.codec.map(CodecLayer::layer),
                muxer: MuxerLayer::layer(converter.muxer, output),
                filter: FilterLayer::layer(converter.filter),
            }
        }
    }
}
pub mod filter_layer {
    use shared::info::filter::{Capture, Filter};

    #[derive(Clone)]
    pub struct CaptureLayer {}
    impl CaptureLayer {
        pub fn layer(capture: Capture) -> Self {
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
        pub fn put_if_absent(&mut self, filter: Filter) {
            if self.capture.is_none() {
                {
                    self.capture = filter.capture.map(CaptureLayer::layer);
                }
            }
        }
        pub fn layer(filter: Filter) -> Self {
            FilterLayer {
                capture: filter.capture.map(CaptureLayer::layer),
            }
        }
    }
}
pub mod muxer_layer {
    use crate::media::context::format::MuxPacket;
    use crate::media::context::format::mp4::Mp4Packet;
    use crate::state::FORMAT_BROADCAST_BUFFER;
    use base::tokio::sync::broadcast;
    use shared::info::format::{
        CMaf, Flv, Frame, GB28181MuxerType, HlsTs, Mp4, Muxer, MuxerType, RtpEnc, RtpFrame, RtpPs,
        Ts, WebRtcMuxerType,
    };
    use shared::info::muxer::MuxerEnum;
    use shared::info::output1::{Gb28181, Local, Output, WebRtc};
    use shared::paste::paste;
    use shared::{impl_check_empty, impl_close};
    use std::sync::Arc;

    #[derive(Clone, Default)]
    pub struct MuxerLayer {
        pub flv: Option<FlvLayer>,
        pub mp4: Option<Mp4Layer>,
        pub ts: Option<TsLayer>,
        pub fmp4: Option<CMafLayer>,
        pub hls_ts: Option<HlsTsLayer>,
        pub rtp_frame: Option<RtpFrameLayer>,
        pub rtp_ps: Option<RtpPsLayer>,
        pub rtp_enc: Option<RtpEncLayer>,
    }
    impl_check_empty!(
        MuxerLayer,
        [flv, mp4, ts, fmp4, hls_ts, rtp_frame, rtp_ps, rtp_enc]
    );

    impl_close!(
        MuxerLayer,
        [flv, mp4, ts, fmp4, hls_ts, rtp_frame, rtp_ps, rtp_enc]
    );
    impl MuxerLayer {
        pub fn put_if_absent(&mut self, muxer: Muxer, output: &Output) {
            if self.flv.is_none()
                && (output.http_flv.is_some()
                    || output.rtmp.is_some()
                    || matches!(
                        output.local,
                        Some(Local {
                            muxer: MuxerType::Flv,
                            ..
                        })
                    ))
            {
                let flv = muxer.flv.unwrap_or_else(|| Flv::default());
                self.flv = Some(FlvLayer::layer(flv));
            }
            if self.mp4.is_none()
                && (output.dash.is_some()
                    || matches!(
                        output.local,
                        Some(Local {
                            muxer: MuxerType::Mp4,
                            ..
                        })
                    ))
            {
                let mp4 = muxer.mp4.unwrap_or_else(|| Mp4::default());
                self.mp4 = Some(Mp4Layer::layer(mp4));
            }
            if self.ts.is_none()
                && (output.hls.is_some()
                    || matches!(
                        output.local,
                        Some(Local {
                            muxer: MuxerType::Ts,
                            ..
                        })
                    ))
            {
                let ts = muxer.ts.unwrap_or_else(|| Ts::default());
                self.ts = Some(TsLayer::layer(ts));
            }
            if self.rtp_frame.is_none()
                && (output.rtsp.is_some()
                    || matches!(
                        output.local,
                        Some(Local {
                            muxer: MuxerType::RtpFrame,
                            ..
                        })
                    )
                    || matches!(
                        output.gb28181,
                        Some(Gb28181 {
                            muxer: GB28181MuxerType::RtpFrame
                        })
                    )
                    || matches!(
                        output.web_rtc,
                        Some(WebRtc {
                            muxer: WebRtcMuxerType::RtpFrame
                        })
                    ))
            {
                let rtp_frame = muxer.rtp_frame.unwrap_or_else(|| RtpFrame::default());
                self.rtp_frame = Some(RtpFrameLayer::layer(rtp_frame));
            }
            if self.rtp_ps.is_none()
                && (matches!(
                    output.local,
                    Some(Local {
                        muxer: MuxerType::RtpPs,
                        ..
                    })
                ) || matches!(
                    output.gb28181,
                    Some(Gb28181 {
                        muxer: GB28181MuxerType::RtpPs
                    })
                ))
            {
                let rtp_ps = muxer.rtp_ps.unwrap_or_else(|| RtpPs::default());
                self.rtp_ps = Some(RtpPsLayer::layer(rtp_ps));
            }

            if self.rtp_enc.is_none()
                && (matches!(
                    output.local,
                    Some(Local {
                        muxer: MuxerType::RtpEnc,
                        ..
                    })
                ) || matches!(
                    output.web_rtc,
                    Some(WebRtc {
                        muxer: WebRtcMuxerType::RtpEnc
                    })
                ))
            {
                let rtp_enc = muxer.rtp_enc.unwrap_or_else(|| RtpEnc::default());
                self.rtp_enc = Some(RtpEncLayer::layer(rtp_enc));
            }
        }
        pub fn layer(muxer: Muxer, output: &Output) -> Self {
            let mut ml = MuxerLayer::default();
            if output.http_flv.is_some()
                || output.rtmp.is_some()
                || matches!(
                    output.local,
                    Some(Local {
                        muxer: MuxerType::Flv,
                        ..
                    })
                )
            {
                let flv = muxer.flv.unwrap_or_else(|| Flv::default());
                ml.flv = Some(FlvLayer::layer(flv));
            }
            if output.dash.is_some()
                || matches!(
                    output.local,
                    Some(Local {
                        muxer: MuxerType::Mp4,
                        ..
                    })
                )
            {
                let mp4 = muxer.mp4.unwrap_or_else(|| Mp4::default());
                ml.mp4 = Some(Mp4Layer::layer(mp4));
            }
            if output.hls.is_some()
                || matches!(
                    output.local,
                    Some(Local {
                        muxer: MuxerType::Ts,
                        ..
                    })
                )
            {
                let ts = muxer.ts.unwrap_or_else(|| Ts::default());
                ml.ts = Some(TsLayer::layer(ts));
            }
            if output.rtsp.is_some()
                || matches!(
                    output.local,
                    Some(Local {
                        muxer: MuxerType::RtpFrame,
                        ..
                    })
                )
                || matches!(
                    output.gb28181,
                    Some(Gb28181 {
                        muxer: GB28181MuxerType::RtpFrame
                    })
                )
                || matches!(
                    output.web_rtc,
                    Some(WebRtc {
                        muxer: WebRtcMuxerType::RtpFrame
                    })
                )
            {
                let rtp_frame = muxer.rtp_frame.unwrap_or_else(|| RtpFrame::default());
                ml.rtp_frame = Some(RtpFrameLayer::layer(rtp_frame));
            }
            if matches!(
                output.local,
                Some(Local {
                    muxer: MuxerType::RtpPs,
                    ..
                })
            ) || matches!(
                output.gb28181,
                Some(Gb28181 {
                    muxer: GB28181MuxerType::RtpPs
                })
            ) {
                let rtp_ps = muxer.rtp_ps.unwrap_or_else(|| RtpPs::default());
                ml.rtp_ps = Some(RtpPsLayer::layer(rtp_ps));
            }

            if matches!(
                output.local,
                Some(Local {
                    muxer: MuxerType::RtpEnc,
                    ..
                })
            ) || matches!(
                output.web_rtc,
                Some(WebRtc {
                    muxer: WebRtcMuxerType::RtpEnc
                })
            ) {
                let rtp_enc = muxer.rtp_enc.unwrap_or_else(|| RtpEnc::default());
                ml.rtp_enc = Some(RtpEncLayer::layer(rtp_enc));
            }
            ml
        }

        pub fn close_by_muxer_type(&mut self, mt: MuxerEnum) {
            match mt {
                MuxerEnum::Flv => self.flv = None,
                MuxerEnum::Mp4 => self.mp4 = None,
                MuxerEnum::Ts => self.ts = None,
                MuxerEnum::RtpFrame => self.rtp_frame = None,
                MuxerEnum::RtpPs => self.rtp_ps = None,
                MuxerEnum::RtpEnc => self.rtp_enc = None,
                MuxerEnum::CMaf => self.fmp4 = None,
                MuxerEnum::HlsTs => self.hls_ts = None,
            }
        }
    }
    #[derive(Clone)]
    pub struct FlvLayer {
        pub tx: broadcast::Sender<Arc<MuxPacket>>,
        pub flv: Flv,
    }
    impl FlvLayer {
        pub fn layer(flv: Flv) -> Self {
            let (tx, _) = broadcast::channel(FORMAT_BROADCAST_BUFFER);
            Self { tx, flv }
        }
    }
    #[derive(Clone)]
    pub struct Mp4Layer {
        pub tx: broadcast::Sender<Arc<Mp4Packet>>,
        pub mp4: Mp4,
    }
    impl Mp4Layer {
        pub fn layer(mp4: Mp4) -> Self {
            let (tx, _) = broadcast::channel(FORMAT_BROADCAST_BUFFER);
            Self { tx, mp4 }
        }
    }

    #[derive(Clone)]
    pub struct TsLayer {}
    impl TsLayer {
        pub fn layer(ts: Ts) -> Self {
            unimplemented!()
        }
    }

    #[derive(Clone)]
    pub struct CMafLayer {}
    impl CMafLayer {
        pub fn layer(cmaf: CMaf) -> Self {
            unimplemented!()
        }
    }
    #[derive(Clone)]
    pub struct RtpFrameLayer {}
    impl RtpFrameLayer {
        pub fn layer(rtp: RtpFrame) -> Self {
            unimplemented!()
        }
    }
    #[derive(Clone)]
    pub struct RtpPsLayer {}
    impl RtpPsLayer {
        pub fn layer(rtp: RtpPs) -> Self {
            unimplemented!()
        }
    }
    #[derive(Clone)]
    pub struct RtpEncLayer {}
    impl RtpEncLayer {
        pub fn layer(rtp: RtpEnc) -> Self {
            unimplemented!()
        }
    }

    #[derive(Clone)]
    pub struct HlsTsLayer {}
    impl HlsTsLayer {
        pub fn layer(hls_ts: HlsTs) -> Self {
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
        pub fn layer(codec: Codec) -> Self {
            match codec {
                Codec::Mpeg4 => CodecLayer::Mpeg4,
                Codec::H264 => CodecLayer::H264,
                Codec::SvacVideo => CodecLayer::SvacVideo,
                Codec::H265 => CodecLayer::H265,
                Codec::G711a => CodecLayer::G711a,
                Codec::G711u => CodecLayer::G711u,
                Codec::G7221 => CodecLayer::G7221,
                Codec::G7231 => CodecLayer::G7231,
                Codec::G729 => CodecLayer::G729,
                Codec::SvacAudio => CodecLayer::SvacAudio,
                Codec::Aac => CodecLayer::Aac,
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
