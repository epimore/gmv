use common::constructor::Get;
use common::exception::code::conf_err::CONFIG_ERROR_CODE;
use common::exception::{GlobalError, GlobalResult};
use common::log::error;
use common::serde::{Deserialize, Serialize};
use paste::paste;

#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct MediaStreamConfig {
    pub ssrc: u32,
    pub stream_id: String,
    /// 输入流,超时自动释放不受此配置影响
    /// 输出流, None:默认配置,负数:立即关闭,0:不关闭
    /// 如仅输出http-flv时, -1 表示立即释放该SSRC媒体流，不监听该SSRC,并发起回调事件通知信令，媒体流已关闭
    pub expires: Option<i32>,
    /// 转换
    pub converter: Converter,
    /// 输出:至少一个
    pub export: Output,
}


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
#[macro_export]
macro_rules! impl_check_empty {
    ($struct:ident, [$($field:ident),*]) => {
        impl $struct {
            pub fn check_empty(&self) -> bool {
                true $(&& self.$field.is_none())*
            }
        }
    }
}

#[macro_export]
macro_rules! impl_open_close {
    ($struct:ident, { $( $field:ident : $type:ty ),* $(,)? }) => {
        impl $struct {
            $(
                paste! {
                    pub fn [<close_$field>](&mut self) -> bool {
                        self.$field = None;
                        self.check_empty()
                    }

                    pub fn [<open_$field>](&mut self, val: $type) -> bool {
                        if self.$field.is_some() {
                            return false;
                        }
                        self.$field = Some(val);
                        true
                    }
                }
            )*
        }
    };
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
pub struct Local {}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct Hls {}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct HttpFlv {}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct Rtmp {}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct Rtsp {}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct Dash {}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct Gb28181 {}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct WebRtc {}

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


#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(crate = "common::serde")]
pub struct Converter {
    pub codec: Option<Codec>,
    pub muxer: Option<Muxer>,
    pub filter: Option<Filter>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct Capture {}
#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct Filter {
    //抽图
    pub capture: Option<Capture>,
    //缩放
    // pub scale: Option<Scale>,
    //裁剪
    // pub crop: Option<Crop>,
    //旋转
    // pub rotate: Option<Rotate>,
    //镜像
    // pub mirror: Option<Mirror>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(crate = "common::serde")]
pub struct Muxer {
    pub flv: Option<Flv>,
    pub mp4: Option<Mp4>,
    pub ts: Option<Ts>,
    pub rtp: Option<RtpFrame>,
    pub rtp_ps: Option<RtpPs>,
    pub rtp_enc: Option<RtpEnc>,
    pub frame: Option<Frame>,
}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct Frame {}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct Mp4 {}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct Flv {}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct RtpFrame {}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct RtpPs {}
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct RtpEnc {}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub struct Ts {}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "common::serde")]
pub enum Codec {
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