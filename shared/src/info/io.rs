use common::exception::{GlobalError, GlobalResult};
use common::exception::code::conf_err::CONFIG_ERROR_CODE;
use common::log::error;
use common::serde::{Deserialize, Serialize};
use paste::paste;
use crate::{impl_check_empty, impl_open_close};
use crate::info::format::MuxerType;

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(crate = "common::serde")]
pub struct Output {
    pub local: Option<Local>,
    pub rtmp: Option<Rtmp>,
    pub http_flv: Option<HttpFlv>,
    pub dash: Option<Dash>,
    pub hls: Option<Hls>,
    pub rtsp: Option<Rtsp>,
    pub gb28181: Option<Gb28181>,
    pub web_rtc: Option<WebRtc>,
}
impl_check_empty!(Output, [local, rtmp, http_flv, dash, hls, rtsp, gb28181, web_rtc]);

impl_open_close!(Output, {
    local: Local,
    rtmp: Rtmp,
    http_flv: HttpFlv,
    dash: Dash,
    hls: Hls,
    rtsp: Rtsp,
    gb28181: Gb28181,
    web_rtc: WebRtc,
});

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct Local {
    muxer: MuxerType,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct Hls {
    muxer: MuxerType,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct HttpFlv {
    muxer: MuxerType,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct Rtmp {
    muxer: MuxerType,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct Rtsp {
    muxer: MuxerType,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct Dash {
    muxer: MuxerType,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct Gb28181 {
    muxer: MuxerType,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct WebRtc {
    muxer: MuxerType,
}

impl Output {
    pub fn new(
        local: Option<Local>,
        rtmp: Option<Rtmp>,
        http_flv: Option<HttpFlv>,
        dash: Option<Dash>,
        hls: Option<Hls>,
        rtsp: Option<Rtsp>,
        gb28181: Option<Gb28181>,
        web_rtc: Option<WebRtc>,
    ) -> GlobalResult<Self> {
        if local.is_none()
            && rtmp.is_none()
            && http_flv.is_none()
            && dash.is_none()
            && hls.is_none()
            && rtsp.is_none()
            && gb28181.is_none()
            && web_rtc.is_none() {
            Err(GlobalError::new_biz_error(CONFIG_ERROR_CODE, "Output cannot be empty", |msg| error!("{msg}")))
        } else {
            Ok(Output { local, rtmp, http_flv, dash, hls, rtsp, gb28181, web_rtc })
        }
    }
}

pub enum PlayType {
    Rtmp(MuxerType),
    Rtsp(MuxerType),
    WebRtc(MuxerType),
    Http(HttpStreamType),
}
impl PlayType {
    pub fn get_type(&self) -> MuxerType {
        match self {
            PlayType::Rtmp(muxer) => muxer.clone(),
            PlayType::Rtsp(muxer) => muxer.clone(),
            PlayType::WebRtc(muxer) => muxer.clone(),
            PlayType::Http(muxer) => muxer.get_type(),
        }
    }
}

//Mp4下载及rtp推流；由发起方控制，不回调鉴权
//    Rtmp,
//     Rtsp,
//     WebRtc,
#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub enum HttpStreamType {
    HttpFlv(MuxerType),
    Hls(MuxerType),
    Dash(MuxerType),
}
impl HttpStreamType {
    pub fn get_type(&self) -> MuxerType {
        match self {
            HttpStreamType::HttpFlv(muxer) => muxer.clone(),
            HttpStreamType::Hls(muxer) => muxer.clone(),
            HttpStreamType::Dash(muxer) => muxer.clone(),
        }
    }
}