use crate::state::layer::codec_layer::CodecLayer;

pub struct CodecContext{}
impl CodecContext {
    pub fn init(codec: Option<CodecLayer>) -> Option<CodecContext> {
        None
    }
}