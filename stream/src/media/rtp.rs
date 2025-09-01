use base::bytes::{Buf, Bytes, BytesMut};
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{error, info};
use crossbeam_channel::Receiver;
use std::ptr;
use crate::general::util;
use crate::media::DEFAULT_IO_BUF_SIZE;

pub struct RtpPacket {
    pub ssrc: u32,
    pub timestamp: u32,
    pub seq: u16,
    pub payload: Bytes,
}
// 暂不考虑数据重传
/*
视频帧率是25(FPS)，采样率是90KHZ(每秒钟抽取图像样本的次数)。
 两视频帧的间隔为：1 秒/ 25帧 = 0.04(秒/帧) = 40(毫秒/帧)
 时间戳增量单位：1/90000(秒/个) ，特别注意RTP时间戳是有单位的 每帧对应的采样： 90000 / 25 = 3600 (个/帧)
*/
//缓冲区大小
const BUFFER_SIZE: usize = 64;
//检查sn是否回绕；sn变小，且差值的绝对值大于u16的一半。65535/2=32767
const ROUND_SIZE: u16 = 32767;
const DEFAULT_QUEUE_WINDOW: usize = 8;
const MAX_QUEUE_WINDOW: usize = BUFFER_SIZE / 2;
const LAST_MAX_QUEUE_WINDOW: usize = MAX_QUEUE_WINDOW - 2;
const MIN_QUEUE_WINDOW: usize = 4;

pub struct RtpPacketBuffer {
    ssrc: u32,
    first_read_rtp_sn: u16, // 第一个读取的 RTP 包的序列号
    queue: [Option<RtpPacket>; BUFFER_SIZE],
    queue_count: usize,  // 缓冲区有效的包数量
    queue_window: usize,  //缓冲区窗口大小:4/8/16
    packet_rx: Receiver<RtpPacket>, //数据接收句柄
    remaining: Bytes,
}

impl RtpPacketBuffer {
    pub fn init(ssrc: u32, packet_rx: Receiver<RtpPacket>) -> GlobalResult<Self> {
        let mut buffer = Self {
            ssrc,
            first_read_rtp_sn: u16::MAX,
            queue: std::array::from_fn(|_| None),
            queue_count: 0,
            queue_window: DEFAULT_QUEUE_WINDOW,
            packet_rx,
            remaining: Default::default(),
        };
        buffer.calculate_index()?;
        Ok(buffer)
    }

    // 计算起始序列号
    fn calculate_index(&mut self) -> GlobalResult<()> {
        loop {
            let pkt = self.packet_rx.recv().hand_log(|_| info!("ssrc:{}, 关闭RTP传输通道",self.ssrc))?;
            let seq_num = pkt.seq;
            let index = seq_num as usize % BUFFER_SIZE;
            let item = unsafe { self.queue.get_unchecked_mut(index) };
            if item.is_some() && item.as_ref().unwrap().seq == seq_num {
                continue;
            }
            if seq_num < self.first_read_rtp_sn {
                self.first_read_rtp_sn = seq_num;
            }
            *item = Some(pkt);
            self.queue_count += 1;

            if self.queue_count == DEFAULT_QUEUE_WINDOW {
                break;
            }
        }
        Ok(())
    }


    //1 判断缓冲区数据数量：[queue_count <  queue_window]? 1.1 : 1.2
    //1.1阻塞线程等待数据+超时
    //1.2直接取数据
    pub fn consume_packet(&mut self, max_consume_len: usize, buf: *mut u8) -> GlobalResult<usize>
    {
        // 优先返回缓存的剩余数据
        if !self.remaining.is_empty() {
            let data = std::mem::take(&mut self.remaining);
            let copy_len = std::cmp::min(data.len(), max_consume_len);
            unsafe {
                ptr::copy_nonoverlapping(data.as_ptr(), buf, copy_len);
            }

            util::dump("ps", &data, false)?;

            self.remaining = data.slice(copy_len..);
            return Ok(copy_len);
        }
        self.reduce_packet()?;
        let mut index = self.first_read_rtp_sn as usize % BUFFER_SIZE;
        let mut size = 0;
        for i in 0..BUFFER_SIZE {
            if index == BUFFER_SIZE {
                index = 0;
            }
            let item = unsafe { self.queue.get_unchecked_mut(index) };
            index += 1;
            if item.is_some() {
                let mut pkt = std::mem::take(item).unwrap();
                self.queue_count -= 1;
                self.first_read_rtp_sn = pkt.seq + 1;

                // 动态计算剩余空间，确保不溢出
                let remaining_space = max_consume_len - size;
                if pkt.payload.len() >= remaining_space {
                    
                    util::dump("ps", &pkt.payload[..remaining_space], false)?;

                    // 复制部分数据并保存剩余到 remaining
                    unsafe {
                        ptr::copy_nonoverlapping(
                            pkt.payload.as_ptr(),
                            buf.add(size),
                            remaining_space,
                        );
                    }
                    self.remaining = pkt.payload.split_off(remaining_space);
                    size += remaining_space;
                    // return Ok(size); // 空间用尽，返回当前长度
                } else {
                    util::dump("ps", &pkt.payload, false)?;
                    // 完全复制 payload
                    unsafe {
                        ptr::copy_nonoverlapping(
                            pkt.payload.as_ptr(),
                            buf.add(size),
                            pkt.payload.len(),
                        );
                    }
                    size += pkt.payload.len();
                }
                if size == max_consume_len || self.queue_count == 0 {
                    //一个读写周期内，丢包大于缓冲区一半 && 缓冲区未满
                    if i > self.queue_window + self.queue_window / 4 {
                        if self.queue_window < MAX_QUEUE_WINDOW {
                            self.queue_window += 1;
                        }
                    } else if i == self.queue_window { //一个读写周期内，未丢包 && 大于最小缓冲区
                        if self.queue_window > MIN_QUEUE_WINDOW {
                            self.queue_window -= 1;
                        }
                    }
                    return Ok(size);
                }
            }
        }
        Ok(size)
    }

    fn reduce_packet(&mut self) -> GlobalResult<()> {
        loop {
            //检查是否已充满缓冲窗口
            if self.queue_count == self.queue_window {
                break;
            }
            let pkt = self.packet_rx.recv().hand_log(|_| info!("ssrc:{}, 关闭RTP传输通道",self.ssrc))?;
            let seq_num = pkt.seq;

            //检查是否为有效的数据包
            if seq_num >= self.first_read_rtp_sn || self.first_read_rtp_sn.wrapping_sub(seq_num) > ROUND_SIZE {
                let index = seq_num as usize % BUFFER_SIZE;
                let item = unsafe { self.queue.get_unchecked_mut(index) };
                //检查是否有重复数据,避免相同数据导致queue_count虚假增加
                if item.is_some() && item.as_ref().unwrap().seq == seq_num { continue; }
                *item = Some(pkt);
                self.queue_count += 1;
            }
        }
        Ok(())
    }
}