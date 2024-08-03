use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, AtomicU8, Ordering};

use common::log::debug;
use parking_lot::RwLock;
use rtp::packet::Packet;
use common::bytes::{Buf, BufMut, Bytes, BytesMut};

use common::tokio::sync::Notify;

const BUFFER_SIZE: usize = 128;
//检查sn是否回绕；sn变小，且差值的绝对值大于u16的一半。65535/2=32767
const ROUND_SIZE: u16 = 32767;

pub struct RtpBuffer {
    sequence_number: AtomicU16,
    buf: Arc<[RwLock<Option<Packet>>; BUFFER_SIZE]>,
    block: Notify,
    index: AtomicU8,
    sliding_window: AtomicU8,
    sliding_counter: AtomicU8,
}

impl RtpBuffer {
    pub fn init() -> Self {
        Self {
            sequence_number: AtomicU16::new(0),
            buf: Arc::new(std::array::from_fn(|_| RwLock::new(None))),
            block: Notify::new(),
            index: AtomicU8::new(0),
            sliding_window: AtomicU8::new(1),
            sliding_counter: AtomicU8::new(0),
        }
    }

    pub fn insert(&self, pkt: Packet) {
        let sn = self.sequence_number.load(Ordering::SeqCst);
        let seq_num = pkt.header.sequence_number;
        //仅插入有效数据包
        if seq_num > sn || sn.wrapping_sub(seq_num) > ROUND_SIZE || sn == 0 {
            let index = seq_num as usize % BUFFER_SIZE;
            let mut item = unsafe { self.buf.get_unchecked(index).write() };
            *item = Some(pkt);
            drop(item);
            if self.sliding_counter.load(Ordering::SeqCst) < BUFFER_SIZE as u8 {
                self.sliding_counter.fetch_add(1, Ordering::SeqCst);
            }
            if self.sliding_counter.load(Ordering::SeqCst) >= self.sliding_window.load(Ordering::SeqCst) {
                self.block.notify_one();
            }
        } else {
            debug!("无效数据包:丢弃; {:?}",pkt.header);
        }
    }

    pub async fn next_pkt(&self) -> Option<Packet> {
        if self.sliding_counter.load(Ordering::SeqCst) < self.sliding_window.load(Ordering::SeqCst) {
            self.block.notified().await;
        }
        let mut index = self.index.load(Ordering::Relaxed) as usize;
        let sn = self.sequence_number.load(Ordering::SeqCst);
        for i in 0..BUFFER_SIZE {
            let guard = unsafe { self.buf.get_unchecked(index).read() };
            index += 1;
            if index == BUFFER_SIZE {
                index = 0;
            }
            if let Some(item) = &*guard {
                let seq_num = item.header.sequence_number;
                let sub = seq_num as i32 - sn as i32;
                //非首次/非回绕/计数器大于1；获取包时，不减少计数
                if self.sliding_counter.load(Ordering::SeqCst) > 1 {
                    if sub.abs() < ROUND_SIZE as i32 {
                        self.sliding_counter.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                // 验证有效包:回绕、首次
                if Self::check_valid(sub) || (sn == 0 && seq_num == 0) {
                    let pkt = item.clone();
                    drop(guard);
                    self.sequence_number.store(seq_num, Ordering::SeqCst);
                    self.index.store(index as u8, Ordering::Relaxed);
                    let window = self.sliding_window.load(Ordering::SeqCst);
                    if i == 1 {
                        if matches!(window,2|4|8) {
                            self.sliding_window.store(window / 2, Ordering::SeqCst);
                        }
                    } else if i > 3 {
                        if matches!(window,1|2|4) {
                            self.sliding_window.store(window * 2, Ordering::SeqCst);
                        }
                    }
                    return Some(pkt);
                }
            }
        }
        None
    }
    fn check_valid(sub: i32) -> bool {
        (sub > 0 && sub <= ROUND_SIZE as i32) || (sub >= -(u16::MAX as i32) && sub <= -(ROUND_SIZE as i32))
    }

    //最后一次获取所有数据
    pub fn flush_pkt(&self) -> Vec<Packet> {
        let mut vec = Vec::new();
        let mut index = self.index.load(Ordering::Relaxed) as usize;
        let sn = self.sequence_number.load(Ordering::SeqCst);
        for _i in 0..BUFFER_SIZE {
            let guard = unsafe { self.buf.get_unchecked(index).read() };
            index += 1;
            if index == BUFFER_SIZE {
                index = 0;
            }
            if let Some(pkt) = &*guard {
                let sub = pkt.header.sequence_number as i32 - sn as i32;
                if Self::check_valid(sub) {
                    vec.push(pkt.clone());
                }
            }
        }
        vec
    }
}

pub struct TcpRtpBuffer {
    inner: HashMap<(SocketAddr, SocketAddr), BytesMut>,
}

impl TcpRtpBuffer {
    pub fn register_buffer() -> Self {
        Self { inner: Default::default() }
    }

    pub fn fresh_data(&mut self, local_addr: SocketAddr, remote_addr: SocketAddr, data: Bytes) -> Option<Bytes> {
        match self.inner.entry((local_addr, remote_addr)) {
            Entry::Occupied(mut occ) => {
                let buffer_mut = occ.get_mut();
                buffer_mut.put(data);
                let buffer_len = buffer_mut.len();
                //2 len + 12 rtp header len
                if buffer_len > 14 {
                    let data_len = buffer_mut.get_u16() as usize;
                    if data_len + 2 <= buffer_len {
                        let rtp_data = buffer_mut.split_to(data_len);
                        Some(rtp_data.freeze())
                    } else { None }
                } else {
                    None
                }
            }
            Entry::Vacant(vac) => {
                let mut buffer_mut = BytesMut::with_capacity(4096);
                buffer_mut.put(data);
                let buffer_len = buffer_mut.len();
                if buffer_len > 14 {
                    let data_len = buffer_mut.get_u16() as usize;
                    if data_len + 2 <= buffer_len {
                        let rtp_data = buffer_mut.split_to(data_len);
                        vac.insert(buffer_mut);
                        Some(rtp_data.freeze())
                    } else {
                        vac.insert(buffer_mut);
                        None
                    }
                } else {
                    vac.insert(buffer_mut);
                    None
                }
            }
        }
    }
    pub fn remove_map(&mut self, local_addr: SocketAddr, remote_addr: SocketAddr) {
        self.inner.remove(&(local_addr, remote_addr));
    }
}

#[cfg(test)]
mod test {
    use std::collections::VecDeque;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use common::bytes::{Buf, BytesMut};

    #[test]
    fn test_deque_vec() {
        let mut d = VecDeque::new();
        d.push_back(1);
        d.push_back(2);
        d.push_back(3);
        d.push_back(4);
        d.push_back(5);
        d.push_back(6);
        d.push_back(7);
        d.push_back(8);
        assert_eq!(d.pop_front(), Some(1));
        d.push_back(1);
        assert_eq!(d.pop_front(), Some(2));
        d.push_back(1);
        assert_eq!(d.pop_front(), Some(3));
        d.push_back(1);
        d.insert(0, 444);
        assert_eq!(d.pop_front(), Some(444));
        d.push_back(1);
    }

    #[test]
    fn test_socket_addr_to_string() {
        let socket = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080);
        println!("{}", socket.to_string());
    }

    #[test]
    fn test_tcp_rtp_buffer() {
        let mut buffer = BytesMut::with_capacity(2048);
        buffer.extend_from_slice(&[0x00, 0x10]); // 长度字段：16
        buffer.extend_from_slice(&[0x90, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x00]); // RTP 数据示例
        let data_len = buffer.get_u16() as usize;
        let rtp_data = buffer.split_to(data_len).freeze();
        println!("{:02x?}", rtp_data.to_vec());
        println!("buffer len: {}, data: {:02x?}", buffer.len(), buffer.to_vec());
    }
}