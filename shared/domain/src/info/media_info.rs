use crate::info::codec::Codec;
use crate::info::filter::Filter;
use crate::info::output::OutputKind;
use base::serde::{Deserialize, Serialize};

#[cfg_attr(debug_assertions, derive(utoipa::ToSchema))]
#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct MediaConfig {
    pub ssrc: u32,
    pub stream_id: String,
    /// None:默认配置
    /// 如超时立即发起回调事件通知信令，是否立即释放该SSRC媒体流资源，不监听该SSRC,根据返回信息进行下一步操作，释放或等待流保活
    /// 执行优先级：回调>监听配置>默认配置
    ///   in_wait_timeout: 4 #u8 单位秒；输入流等待超时,需大于等于1,建议：2-8;
    ///   out_idle_timeout: 6 #u8 单位秒；输出流闲置超时,0：立即关闭,建议：2-8；
    pub in_wait_timeout: Option<u8>,
    pub out_idle_timeout: Option<u8>,
    pub codec: Option<Codec>,
    pub filter: Filter,
    pub output: OutputKind,
    #[serde(default)]
    pub session_hook_endpoint: Option<String>,
}
