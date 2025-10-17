use crate::info::codec::Codec;
use crate::info::filter::Filter;
use crate::info::output::OutputKind;
use base::serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct MediaConfig {
    pub ssrc: u32,
    pub stream_id: String,
    /// 输入流,超时自动释放不受此配置影响
    /// 输出流, None:默认配置,负数:立即关闭,0:不关闭
    /// 如仅输出http-flv时, -1 表示立即释放该SSRC媒体流，不监听该SSRC,并发起回调事件通知信令，媒体流已关闭
    pub expires: Option<i32>,
    pub codec: Option<Codec>,
    pub filter: Filter,
    pub output: OutputKind,
}