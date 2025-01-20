use std::collections::HashMap;
use crossbeam_channel::{Receiver};
use common::log::{error, warn};
use rtp::packet::Packet;
use common::chrono::Local;
use common::constructor::New;
use common::exception::{GlobalError, GlobalResult, TransError};
use crate::coder::{MediaInfo};
use crate::general::mode::Media;

/*
视频帧率是25(FPS)，采样率是90KHZ(每秒钟抽取图像样本的次数)。
 两视频帧的间隔为：1 秒/ 25帧 = 0.04(秒/帧) = 40(毫秒/帧)
 时间戳增量单位：1/90000(秒/个) ，特别注意RTP时间戳是有单位的 每帧对应的采样： 90000 / 25 = 3600 (个/帧)
*/
//rtp包读取超时(8帧)：毫秒
const BASE_TIMEOUT: i64 = 320;
//缓冲区大小
const BUFFER_SIZE: usize = 64;
const BLOCK_BUFFER_SIZE: usize = 32;
//检查sn是否回绕；sn变小，且差值的绝对值大于u16的一半。65535/2=32767
const ROUND_SIZE: u16 = 32767;

#[derive(New)]
pub struct MediaHandler {
    media_map: HashMap<u8, Media>,
    media_info: MediaInfo,
}

pub struct DemuxContext {
    last_read_rtp_sn: u16, // 上一个读取的 RTP 包的序列号
    last_read_time: i64, // 上一次读取时间
    last_read_queue_index: usize,  // 上一次读取缓冲区索引

    queue: [Option<Packet>; BUFFER_SIZE],
    queue_count: usize,  // 缓冲区有效的包数量
    queue_window: usize,  //缓冲区窗口大小:2/4/8/16
    packet_rx: Receiver<Packet>, //数据接收句柄
    media_handler: MediaHandler,
}

impl DemuxContext {
    pub fn init(packet_rx: Receiver<Packet>, media_handler: MediaHandler) -> Self {
        Self {
            last_read_rtp_sn: 0,
            last_read_time: 0,
            queue: std::array::from_fn(|_| None),
            last_read_queue_index: 0,
            queue_count: 0,
            queue_window: 2,
            packet_rx,
            media_handler,
        }
    }

    //1 判断缓冲区数据数量：[queue_count <  queue_window]? 1.1 : 1.2
    //1.1阻塞线程等待数据+超时
    //1.2直接取数据
    pub fn demux_packet(&mut self) -> GlobalResult<()> {
        if self.queue_count < self.queue_window {
            self.reduce_packet()?;
        }
        let mut index = self.last_read_queue_index;
        let count = self.queue_count;
        for i in 0..BUFFER_SIZE {
            index += 1;
            if index == BUFFER_SIZE {
                index = 0;
            }
            let item = unsafe { self.queue.get_unchecked_mut(index) };
            if item.is_some() {
                let pkt = std::mem::take(item).unwrap();
                self.queue_count -= 1;
                self.last_read_rtp_sn = pkt.header.sequence_number;

                //处理数据
                Self::demux_data(&mut self.media_handler.media_info, pkt, &self.media_handler.media_map)?;

                if self.queue_count == 0 {
                    self.last_read_time = Local::now().timestamp_millis();
                    self.last_read_queue_index = index;
                    //遍历次数大于有效数据数量,则中间有不连续，需增加缓存窗口
                    if i > count {
                        if matches!(self.queue_window,2|4|8) {
                            self.queue_window *= 2;
                        }
                    } else {
                        if matches!(self.queue_window,4|8|16) {
                            self.queue_window /= 2;
                        }
                    }
                    break;
                }
            }
        }
        Ok(())
    }

    fn reduce_packet(&mut self) -> GlobalResult<()> {
        loop {
            let pkt = self.packet_rx.recv().hand_log(|msg| error!("{msg}"))?;
            let seq_num = pkt.header.sequence_number;
            //检查是否为有效的数据包
            if seq_num > self.last_read_rtp_sn || self.last_read_rtp_sn.wrapping_sub(seq_num) > ROUND_SIZE || self.last_read_rtp_sn == 0 {
                let index = seq_num as usize % BUFFER_SIZE;
                let item = unsafe { self.queue.get_unchecked_mut(index) };
                *item = Some(pkt);
                self.queue_count += 1;
                //检查是否已充满一个缓冲块
                if self.queue_count == BLOCK_BUFFER_SIZE {
                    break;
                }
            }
            //等待超时
            if Local::now().timestamp_millis() >= self.last_read_time + BASE_TIMEOUT {
                break;
            }
        }
        Ok(())
    }

    fn demux_data(coder: &mut MediaInfo, pkt: Packet, media_map: &HashMap<u8, Media>) -> GlobalResult<()> {
        let media_type = pkt.header.payload_type;
        if let Some(media) = media_map.get(&media_type) {
            match *media {
                Media::PS => {
                    coder.ps.handle_demuxer(pkt.header.marker, pkt.header.timestamp, pkt.payload)
                }
                Media::H264 => {
                    coder.h264.handle_demuxer(pkt.payload, pkt.header.timestamp)
                }
            }
        } else {
            match media_type {
                98 => {
                    coder.h264.handle_demuxer(pkt.payload, pkt.header.timestamp)
                }
                96 => {
                    coder.ps.handle_demuxer(pkt.header.marker, pkt.header.timestamp, pkt.payload)
                }
                other => {
                    Err(GlobalError::new_biz_error(1199, &format!("系统暂不支持RTP负载类型:{other}"), |msg| warn!("{msg}")))
                }
            }
        }
    }
}

#[test]
fn test() {
    let mut i = 4;
    i /= 2;
    println!("{i}");
}