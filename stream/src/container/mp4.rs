use common::bytes::Bytes;
use crate::container::PacketWriter;


pub mod mp4_h264 {
    use common::bytes::Bytes;
    use crate::container::PacketWriter;

    pub struct MediaMp4Context {
        pub file_name: String,
    }
    impl MediaMp4Context {
        pub fn register() -> Self {
            todo!()
        }
    }

    impl PacketWriter for MediaMp4Context {
        fn packet(&mut self, vec_frame: &mut Vec<Bytes>, timestamp: u32) {
            todo!()
        }
    }
}
