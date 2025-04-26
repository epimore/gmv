pub mod mp4_h264 {
    use crate::biz::call::StreamRecordInfo;
    use crate::coder::h264::H264Context;
    use crate::container::PacketWriter;
    use crate::io::hook_handler::OutEvent;
    use crate::state::cache;
    use common::bytes::Bytes;
    use common::exception::{GlobalResult, TransError};
    use common::log::{error};
    use common::tokio::sync::broadcast;
    use mp4::{MediaConfig, Mp4Config, Mp4Sample, Mp4Writer, TrackConfig, TrackType};
    use std::fs;
    use std::fs::File;
    use std::io::{Seek, Write};
    use std::time::Instant;

    const H264_NAL_SPS_TYPE: u8 = 0x07;
    const H264_NAL_PPS_TYPE: u8 = 0x08;
    const H264_NAL_IDR_TYPE: u8 = 0x05;


    pub struct MediaMp4Context<W: Write + Seek> {
        pub first_frame_next: bool,
        //单轨道
        pub track_init: bool,
        pub seq_param_set: Option<Vec<u8>>,
        pub pic_param_set: Option<Vec<u8>>,
        pub timestamp: u32,
        pub last_ts: Instant,
        pub last_frame_timestamp: u32,
        pub file_name: String,
        pub writer: Mp4Writer<W>,
        pub down_tx: broadcast::Sender<StreamRecordInfo>,
    }

    impl MediaMp4Context<File> {
        pub fn register(down_tx: broadcast::Sender<StreamRecordInfo>, file_name: String) -> GlobalResult<Self> {
            let file = File::create(&file_name).hand_log(|msg| error!("{msg}"))?;
            let config = Mp4Config {
                major_brand: str::parse("isom").unwrap(),
                minor_version: 512,
                compatible_brands: vec![
                    str::parse("isom").unwrap(),
                    str::parse("iso2").unwrap(),
                    str::parse("avc1").unwrap(),
                    str::parse("mp41").unwrap(),
                ],
                timescale: 90000,
            };
            let writer = Mp4Writer::write_start(file, &config).hand_log(|msg| error!("{msg}"))?;
            Ok(Self { first_frame_next: false, track_init: false, seq_param_set: None, pic_param_set: None, timestamp: 0, last_ts: Instant::now(), last_frame_timestamp: 0, file_name, writer, down_tx })
        }
    }

    impl<W: Write + Seek> PacketWriter for MediaMp4Context<W> {
        fn packet(&mut self, vec_frame: &mut Vec<Bytes>, timestamp: u32) {
            let mut bytes_len = 0;
            while let Some(frame) = vec_frame.pop() {
                let mut is_sync = false;
                let nal_type = frame[4] & 0x1F;
                match nal_type {
                    H264_NAL_SPS_TYPE => {
                        if self.first_frame_next { 
                            continue;
                        }
                        if self.seq_param_set.is_none() {
                            self.seq_param_set = Some(frame.to_vec());
                        }
                        if !self.track_init && self.pic_param_set.is_some() {
                            if let Ok((width, height, _fps)) = H264Context::get_width_height_frame_rate(&frame.slice(4..)) {
                                let track_config = TrackConfig {
                                    track_type: mp4::TrackType::Video,
                                    timescale: 90000,
                                    language: "und".to_string(), // 未指定语言
                                    media_conf: MediaConfig::AvcConfig(mp4::AvcConfig {
                                        width: width as u16,
                                        height: height as u16,
                                        seq_param_set: frame[4..].to_vec(),
                                        pic_param_set: self.pic_param_set.clone().unwrap()[4..].to_vec(),
                                    }),
                                };
                                if let Ok(_) = self.writer.add_track(&track_config).hand_log(|msg| error!("{msg}")) {
                                    self.track_init = true;
                                };
                            }
                        }
                    }
                    H264_NAL_PPS_TYPE => {
                        if self.first_frame_next {
                            continue;
                        }
                        if self.pic_param_set.is_none() {
                            self.pic_param_set = Some(frame.to_vec());
                        }
                        if !self.track_init && self.seq_param_set.is_some() {
                            if let Ok((width, height, _fps)) = H264Context::get_width_height_frame_rate(&frame.slice(4..)) {
                                let track_config = TrackConfig {
                                    track_type: TrackType::Video,
                                    timescale: 90000,
                                    language: "und".to_string(), // 未指定语言
                                    media_conf: MediaConfig::AvcConfig(mp4::AvcConfig {
                                        width: width as u16,
                                        height: height as u16,
                                        seq_param_set: self.seq_param_set.clone().unwrap()[4..].to_vec(),
                                        pic_param_set: frame[4..].to_vec(),
                                    }),
                                };
                                if let Ok(_) = self.writer.add_track(&track_config).hand_log(|msg| error!("{msg}")) {
                                    self.track_init = true;
                                };
                            }
                        }
                    }
                    H264_NAL_IDR_TYPE => {
                        is_sync = true;
                    }
                    _ => {}
                }

                if !self.track_init { continue; }
                let size = frame.len();
                if self.first_frame_next {
                    let sample = Mp4Sample {
                        start_time: (timestamp - self.timestamp) as u64,
                        duration: timestamp - self.last_frame_timestamp,
                        rendering_offset: 0,
                        is_sync,
                        bytes: frame,
                    };
                    if let Ok(_) = self.writer.write_sample(1, &sample).hand_log(|msg| error!("{msg}")) {
                        bytes_len += size;
                    };
                } else {
                    if is_sync {
                        self.first_frame_next = true;
                        let sample = Mp4Sample {
                            start_time: 0,
                            duration: timestamp - self.last_frame_timestamp,
                            rendering_offset: 0,
                            is_sync,
                            bytes: frame,
                        };
                        if let Ok(_) = self.writer.write_sample(1, &sample).hand_log(|msg| error!("{msg}")) {
                            bytes_len += size;
                        };
                        self.timestamp = timestamp;
                    }
                }
            }
            let now = Instant::now();
            let unit_sec = now.duration_since(self.last_ts).as_millis() as usize;
            if self.first_frame_next && unit_sec >= 1000 {
                let bytes_sec = bytes_len * 1000 / unit_sec;
                let info = StreamRecordInfo { file_name: None, file_size: None, timestamp: timestamp / 90000, bytes_sec };
                //不监听是否发送成功，接收端是http随机访问
                let _ = self.down_tx.send(info);
                self.last_ts = now;
            }
            self.last_frame_timestamp = timestamp;
        }
        fn packet_end(&mut self) {
            if let Ok(_) = self.writer.write_end().hand_log(|msg| error!("{msg}")) {
                if let Ok(m) = fs::metadata(&self.file_name) {
                    let info = StreamRecordInfo { file_name: Some(self.file_name.clone()), file_size: Some(m.len()), timestamp: self.timestamp / 90000, bytes_sec: 0 };
                    let sender = cache::get_event_tx();
                    let _ = sender.try_send((OutEvent::EndRecord(info), None)).hand_log(|msg| error!("{}; MP4录制完成事件推送失败：{}",self.file_name,msg));
                }
            }
        }
    }
}


