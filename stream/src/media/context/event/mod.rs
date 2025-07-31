pub mod codec;
pub mod filter;
pub mod muxer;
pub mod output;
pub mod inner;

pub enum ContextEvent {
    Codec(codec::CodecEvent),
    Muxer(muxer::MuxerEvent),
    Filter(filter::FilterEvent),
    Inner(inner::InnerEvent),
}