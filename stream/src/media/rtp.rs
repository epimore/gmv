use base::bytes::{Bytes, BytesMut};
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::info;
use crossbeam_channel::Receiver;

pub struct RtpPacket {
    pub ssrc: u32,
    pub timestamp: u32,
    pub seq: u16,
    pub payload: Bytes,
}

/*
视频帧率是25(FPS)，采样率是90KHZ(每秒钟抽取图像样本的次数)。
 两视频帧的间隔为：1 秒/ 25帧 = 0.04(秒/帧) = 40(毫秒/帧)
 时间戳增量单位：1/90000(秒/个) ，特别注意RTP时间戳是有单位的 每帧对应的采样： 90000 / 25 = 3600 (个/帧)
*/
//缓冲区大小
const BUFFER_SIZE: usize = 64;
//检查sn是否回绕；sn变小，且差值的绝对值大于u16的一半。65535/2=32767
const ROUND_SIZE: u16 = 32767;

pub struct RtpPacketBuffer {
    ssrc: u32,
    last_read_rtp_sn: u16, // 上一个读取的 RTP 包的序列号
    queue: [Option<RtpPacket>; BUFFER_SIZE],
    queue_count: usize,  // 缓冲区有效的包数量
    queue_window: usize,  //缓冲区窗口大小:4/8/16
    packet_rx: Receiver<RtpPacket>, //数据接收句柄
    remaining: Bytes,
}

impl RtpPacketBuffer {
    pub fn init(ssrc: u32, packet_rx: Receiver<RtpPacket>) -> Self {
        Self {
            ssrc,
            last_read_rtp_sn: 0,
            queue: std::array::from_fn(|_| None),
            queue_count: 0,
            queue_window: 16,
            packet_rx,
            remaining: Default::default(),
        }
    }

    pub fn cache_remaining_data(&mut self, remaining: &[u8]) {
        self.remaining = BytesMut::from(remaining).freeze();
    }


    //1 判断缓冲区数据数量：[queue_count <  queue_window]? 1.1 : 1.2
    //1.1阻塞线程等待数据+超时
    //1.2直接取数据
    pub fn demux_packet(&mut self) -> GlobalResult<Option<Bytes>>
    {
        // 优先返回缓存的剩余数据
        if !self.remaining.is_empty() {
            let data = std::mem::take(&mut self.remaining);
            return Ok(Some(data));
        }
        self.reduce_packet()?;
        let mut index = self.last_read_rtp_sn as usize % BUFFER_SIZE;
        for i in 0..BUFFER_SIZE {
            index += 1;
            if index == BUFFER_SIZE {
                index = 0;
            }
            let item = unsafe { self.queue.get_unchecked_mut(index) };
            if item.is_some() {
                let pkt = std::mem::take(item).unwrap();
                self.queue_count -= 1;
                self.last_read_rtp_sn = pkt.seq;

                if self.queue_count <= self.queue_window {
                    //遍历次数大于有效数据数量,则中间有不连续，需增加缓存窗口
                    if i > self.queue_window + 2 {
                        if self.queue_window < 16 {
                            self.queue_window *= 2;
                        }
                    } else if i == self.queue_window {
                        if self.queue_window > 16 {
                            self.queue_window /= 2;
                        }
                    }
                }
                // println!("seq:{},timestamp:{}", pkt.seq, pkt.timestamp);
                return Ok(Some(pkt.payload));
            }
        }
        Ok(None)
    }

    fn reduce_packet(&mut self) -> GlobalResult<()> {
        loop {
            let pkt = self.packet_rx.recv().hand_log(|_| info!("ssrc:{}, 关闭RTP传输通道",self.ssrc))?;
            let seq_num = pkt.seq;
            //检查是否为有效的数据包
            if seq_num > self.last_read_rtp_sn || self.last_read_rtp_sn.wrapping_sub(seq_num) > ROUND_SIZE || self.last_read_rtp_sn == 0 {
                let index = seq_num as usize % BUFFER_SIZE;
                let item = unsafe { self.queue.get_unchecked_mut(index) };
                *item = Some(pkt);
                self.queue_count += 1;
                //检查是否已充满2个缓冲窗口-1
                //初始化缓冲窗口为1，以便消费时定位到正确的有效数据包
                if self.queue_count >= self.queue_window * 2 - 1 {
                    break;
                }
            }
        }
        Ok(())
    }
}