pub mod codec;
pub mod filter;
pub mod muxer;
pub mod output;

pub enum ConverterEvent {
    Codec(codec::CodecEvent),
    Muxer(muxer::MuxerEvent),
    Filter(filter::FilterEvent),
}