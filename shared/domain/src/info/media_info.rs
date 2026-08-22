use crate::info::codec::Codec;
use crate::info::filter::Filter;
use crate::info::output::OutputKind;
use base::serde::{Deserialize, Serialize};

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(crate = "base::serde", rename_all = "snake_case")]
pub enum OutputAudioCodec {
    Aac,
}

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(crate = "base::serde")]
pub struct TranscodeConfig {
    #[serde(default)]
    pub audio_codec: Option<OutputAudioCodec>,
}

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct MediaConfig {
    pub ssrc: u32,
    pub stream_id: String,
    /// None:默认配置
    /// 如超时立即发起回调事件通知信令，是否立即释放该SSRC媒体流资源，不监听该SSRC,根据返回信息进行下一步操作，释放或等待流保活
    /// 执行优先级：回调>监听配置>默认配置
    ///   in_wait_timeout: 4 # 单位秒；连续无 RTP 包判死时间，范围 1-30
    ///   out_idle_timeout: 20 # 单位秒；output/viewer session 延迟释放时间，范围 12-120
    pub in_wait_timeout: Option<u8>,
    pub out_idle_timeout: Option<u8>,
    /// Legacy ambiguous codec target. New callers must use `transcode.audio_codec` for audio.
    pub codec: Option<Codec>,
    #[serde(default)]
    pub transcode: Option<TranscodeConfig>,
    pub filter: Filter,
    pub output: OutputKind,
    #[serde(default)]
    pub session_hook_endpoint: Option<String>,
}
