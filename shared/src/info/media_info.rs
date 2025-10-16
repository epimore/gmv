use crate::info::codec::Codec;
use crate::info::filter::Filter;
use crate::info::output1::Output;
use base::serde::{Deserialize, Serialize};
use crate::info::format::Muxer;
use crate::info::output::OutputKind;

// #[derive(Serialize, Deserialize, Debug)]
// #[serde(crate = "base::serde")]
// pub struct MediaStreamConfig {
//     pub ssrc: u32,
//     pub stream_id: String,
//     /// 输入流,超时自动释放不受此配置影响
//     /// 输出流, None:默认配置,负数:立即关闭,0:不关闭
//     /// 如仅输出http-flv时, -1 表示立即释放该SSRC媒体流，不监听该SSRC,并发起回调事件通知信令，媒体流已关闭
//     pub expires: Option<i32>,
//     /// 转换
//     pub converter: Converter,
//     /// 输出:至少一个
//     pub output: Output,
// }
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


// #[derive(Serialize, Deserialize, Debug, Default, Clone)]
// #[serde(crate = "base::serde")]
// pub struct Converter {
//     pub codec: Option<Codec>,
//     pub muxer: Muxer,
//     pub filter: Filter,
// }

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(crate = "base::serde")]
pub struct MediaAction {
    pub enable_codec: bool,
    pub enable_filter: bool,
    pub enable_output: bool,
    pub codec: Codec,
    pub filter: Filter,
    pub output: Output,
}

