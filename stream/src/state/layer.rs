pub mod output_layer {
    use shared::impl_close;
    use shared::info::output::{
        DashFmp4Output, Gb28181FrameOutput, Gb28181PsOutput, HlsFmp4Output, HlsTsOutput,
        HttpFlvOutput, LocalMp4Output, LocalTsOutput, OutputKind, RtmpOutput, RtspOutput,
        WebRtcOutput,
    };
    use shared::paste::paste;

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
    impl_close!(
        OutputLayer,
        [
            http_flv,
            rtmp,
            dash_fmp4,
            hls_fmp4,
            hls_ts,
            rtsp,
            gb28181_frame,
            gb28181_ps,
            web_rtc,
            local_mp4,
            local_ts
        ]
    );

    impl OutputLayer {
        pub fn new(output: OutputKind) -> Self {
            let mut layer = Self {
                http_flv: None,
                rtmp: None,
                dash_fmp4: None,
                hls_fmp4: None,
                hls_ts: None,
                rtsp: None,
                gb28181_frame: None,
                gb28181_ps: None,
                web_rtc: None,
                local_mp4: None,
                local_ts: None,
            };

            match output {
                OutputKind::HttpFlv(inner) => {
                    layer.http_flv = Some(HttpFlvLayer::layer(inner));
                }
                OutputKind::Rtmp(inner) => {
                    layer.rtmp = Some(RtmpLayer::layer(inner));
                }
                OutputKind::DashFmp4(inner) => {
                    layer.dash_fmp4 = Some(DashFmp4Layer::layer(inner));
                }
                OutputKind::HlsFmp4(inner) => {
                    layer.hls_fmp4 = Some(HlsFmp4Layer::layer(inner));
                }
                OutputKind::HlsTs(inner) => {
                    layer.hls_ts = Some(HlsTsLayer::layer(inner));
                }
                OutputKind::Rtsp(inner) => {
                    layer.rtsp = Some(RtspLayer::layer(inner));
                }
                OutputKind::Gb28181Frame(inner) => {
                    layer.gb28181_frame = Some(Gb28181FrameLayer::layer(inner));
                }
                OutputKind::Gb28181Ps(inner) => {
                    layer.gb28181_ps = Some(Gb28181PsLayer::layer(inner));
                }
                OutputKind::WebRtc(inner) => {
                    layer.web_rtc = Some(WebRtcLayer::layer(inner));
                }
                OutputKind::LocalMp4(inner) => {
                    layer.local_mp4 = Some(LocalMp4Layer::layer(inner));
                }
                OutputKind::LocalTs(inner) => {
                    layer.local_ts = Some(LocalTsLayer::layer(inner));
                }
            }
            layer
        }
        //存在则返回false;不存在则put，返回true
        pub fn put_if_absent(&mut self, output: OutputKind) -> bool {
            match output {
                OutputKind::HttpFlv(inner) => {
                    if self.http_flv.is_none() {
                        self.http_flv = Some(HttpFlvLayer::layer(inner));
                        return true;
                    }
                }
                OutputKind::Rtmp(inner) => {
                    if self.rtmp.is_none() {
                        self.rtmp = Some(RtmpLayer::layer(inner));
                        return true;
                    }
                }
                OutputKind::DashFmp4(inner) => {
                    if self.dash_fmp4.is_none() {
                        self.dash_fmp4 = Some(DashFmp4Layer::layer(inner));
                        return true;
                    }
                }
                OutputKind::HlsFmp4(inner) => {
                    if self.hls_fmp4.is_none() {
                        self.hls_fmp4 = Some(HlsFmp4Layer::layer(inner));
                        return true;
                    }
                }
                OutputKind::HlsTs(inner) => {
                    if self.hls_ts.is_none() {
                        self.hls_ts = Some(HlsTsLayer::layer(inner));
                        return true;
                    }
                }
                OutputKind::Rtsp(inner) => {
                    if self.rtsp.is_none() {
                        self.rtsp = Some(RtspLayer::layer(inner));
                        return true;
                    }
                }
                OutputKind::Gb28181Frame(inner) => {
                    if self.gb28181_frame.is_none() {
                        self.gb28181_frame = Some(Gb28181FrameLayer::layer(inner));
                        return true;
                    }
                }
                OutputKind::Gb28181Ps(inner) => {
                    if self.gb28181_ps.is_none() {
                        self.gb28181_ps = Some(Gb28181PsLayer::layer(inner));
                        return true;
                    }
                }
                OutputKind::WebRtc(inner) => {
                    if self.web_rtc.is_none() {
                        self.web_rtc = Some(WebRtcLayer::layer(inner));
                        return true;
                    }
                }
                OutputKind::LocalMp4(inner) => {
                    if self.local_mp4.is_none() {
                        self.local_mp4 = Some(LocalMp4Layer::layer(inner));
                        return true;
                    }
                }
                OutputKind::LocalTs(inner) => {
                    if self.local_ts.is_none() {
                        self.local_ts = Some(LocalTsLayer::layer(inner));
                        return true;
                    }
                }
            }
            false
        }
    }

    pub struct LocalTsLayer {
        pub local_ts: LocalTsOutput,
    }
    impl LocalTsLayer {
        pub fn layer(local_ts: LocalTsOutput) -> Self {
            Self { local_ts }
        }
    }
    pub struct LocalMp4Layer {
        pub local_mp4: LocalMp4Output,
    }
    impl LocalMp4Layer {
        pub fn layer(local_mp4: LocalMp4Output) -> Self {
            Self { local_mp4 }
        }
    }
    pub struct HlsFmp4Layer {
        pub hls_fmp4: HlsFmp4Output,
    }
    impl HlsFmp4Layer {
        pub fn layer(hls_fmp4: HlsFmp4Output) -> Self {
            Self { hls_fmp4 }
        }
    }

    pub struct HlsTsLayer {
        pub hls_ts: HlsTsOutput,
    }
    impl HlsTsLayer {
        pub fn layer(hls_ts: HlsTsOutput) -> Self {
            Self { hls_ts }
        }
    }
    pub struct HttpFlvLayer {
        pub http_flv: HttpFlvOutput,
    }
    impl HttpFlvLayer {
        pub fn layer(http_flv: HttpFlvOutput) -> Self {
            Self { http_flv }
        }
    }
    pub struct RtmpLayer {
        pub rtmp: RtmpOutput,
    }
    impl RtmpLayer {
        pub fn layer(rtmp: RtmpOutput) -> Self {
            Self { rtmp }
        }
    }
    pub struct RtspLayer {
        pub rtsp: RtspOutput,
    }
    impl RtspLayer {
        pub fn layer(rtsp: RtspOutput) -> Self {
            Self { rtsp }
        }
    }
    pub struct DashFmp4Layer {
        pub dash_fmp4: DashFmp4Output,
    }
    impl DashFmp4Layer {
        pub fn layer(dash_fmp4: DashFmp4Output) -> Self {
            Self { dash_fmp4 }
        }
    }
    pub struct Gb28181FrameLayer {
        pub gb28181_frame: Gb28181FrameOutput,
    }
    impl Gb28181FrameLayer {
        pub fn layer(gb28181_frame: Gb28181FrameOutput) -> Self {
            Self { gb28181_frame }
        }
    }
    pub struct Gb28181PsLayer {
        pub gb28181_ps: Gb28181PsOutput,
    }
    impl Gb28181PsLayer {
        pub fn layer(gb28181_ps: Gb28181PsOutput) -> Self {
            Self { gb28181_ps }
        }
    }
    pub struct WebRtcLayer {
        pub web_rtc: WebRtcOutput,
    }
    impl WebRtcLayer {
        pub fn layer(web_rtc: WebRtcOutput) -> Self {
            Self { web_rtc }
        }
    }
}

pub mod converter_layer {
    use crate::state::layer::codec_layer::CodecLayer;
    use crate::state::layer::filter_layer::FilterLayer;
    use crate::state::layer::muxer_layer::MuxerLayer;
    use shared::info::codec::Codec;
    use shared::info::filter::Filter;
    use shared::info::output::OutputKind;

    #[derive(Clone)]
    pub struct ConverterLayer {
        pub codec: Option<CodecLayer>,
        pub muxer: MuxerLayer,
        pub filter: FilterLayer,
    }

    impl ConverterLayer {
        pub fn new(codec: Option<Codec>, filter: Filter, output: &OutputKind) -> Self {
            let muxer = MuxerLayer::new(output);
            let filter = FilterLayer::new(filter);
            let codec = codec.map(CodecLayer::new);
            Self {
                codec,
                muxer,
                filter,
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
        pub fn new(filter: Filter) -> Self {
            FilterLayer {
                capture: filter.capture.map(CaptureLayer::layer),
            }
        }
    }
}
pub mod muxer_layer {
    use crate::media::context::format::mp4::Mp4Packet;
    use crate::media::context::format::MuxPacket;
    use crate::state::FORMAT_BROADCAST_BUFFER;
    use base::tokio::sync::broadcast;
    use shared::info::format::{CMaf, HlsTs, Mp4, RtpEnc, RtpFrame, RtpPs, Ts};
    use shared::info::muxer::MuxerEnum;
    use shared::info::output::OutputKind;
    use std::sync::Arc;

    #[derive(Clone, Default)]
    pub struct MuxerLayer {
        pub flv: Option<FlvLayer>,
        pub fmp4: Option<CMafLayer>,
        pub hls_ts: Option<HlsTsLayer>,
        pub rtp_frame: Option<RtpFrameLayer>,
        pub rtp_ps: Option<RtpPsLayer>,
        pub rtp_enc: Option<RtpEncLayer>,
        pub mp4: Option<Mp4Layer>,
        pub ts: Option<TsLayer>,
    }
    impl MuxerLayer {
        pub fn new(output: &OutputKind) -> Self {
            let mut layer = MuxerLayer::default();
            layer.put_if_absent(output);
            layer
        }
        pub fn put_if_absent(&mut self, output: &OutputKind) {
            match output {
                OutputKind::HttpFlv(_) | OutputKind::Rtmp(_) => {
                    if self.flv.is_none() {
                        self.flv = Some(FlvLayer::layer());
                    }
                }
                OutputKind::DashFmp4(inner) | OutputKind::HlsFmp4(inner) => {
                    if self.fmp4.is_none() {
                        unimplemented!()
                    }
                }
                OutputKind::HlsTs(inner) => {
                    if self.hls_ts.is_none() {
                        unimplemented!()
                    }
                }
                OutputKind::Rtsp(inner) | OutputKind::Gb28181Frame(inner) => {
                    if self.rtp_frame.is_none() {
                        unimplemented!()
                    }
                }
                OutputKind::Gb28181Ps(inner) => {
                    if self.rtp_ps.is_none() {
                        unimplemented!()
                    }
                }
                OutputKind::WebRtc(inner) => {
                    if self.rtp_enc.is_none() {
                        unimplemented!()
                    }
                }
                OutputKind::LocalMp4(inner) => {
                    if self.mp4.is_none() {
                        unimplemented!()
                    }
                }
                OutputKind::LocalTs(inner) => {
                    if self.ts.is_none() {
                        unimplemented!()
                    }
                }
            }
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
    }
    impl FlvLayer {
        pub fn layer() -> Self {
            let (tx, _) = broadcast::channel(FORMAT_BROADCAST_BUFFER);
            Self { tx }
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
        pub fn new(codec: Codec) -> Self {
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
