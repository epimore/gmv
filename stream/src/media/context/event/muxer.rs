pub enum MuxerEvent {
    Open(MuxerOpenEvent),
    Close(MuxerCloseEvent),
}

pub struct MuxerOpenEvent {}
pub struct MuxerCloseEvent {}