pub mod hls_h264 {
    use std::collections::VecDeque;
    use std::time::Duration;
    use common::bytes::Bytes;
    use hls_m3u8::MediaPlaylist;

    pub struct HlsContext {
        sequence_number: u32,
        ts_path_vec: VecDeque<String>,
    }

    impl HlsContext {
        pub fn packet(&mut self, vec_frame: &Vec<Bytes>, timestamp: u32) {
            let mut builder = MediaPlaylist::builder();
            builder.target_duration(Duration::from_secs(3));

        }
    }
}

#[cfg(test)]
mod tests {
    use hls_m3u8::MediaPlaylist;

    #[test]
    fn test_builder() {
        // let playlist = MediaPlaylist::builder();
        // println!("{:?}", playlist);
    }
}