use std::sync::Arc;
use std::sync::atomic::{AtomicU16, AtomicU32, AtomicU8, Ordering};

use log::{debug, info};
use rtp::packet::Packet;

use common::tokio::sync::{Mutex, Notify};

const BUFFER_SIZE: usize = 32;

pub struct RtpBuffer {
    sequence_number: AtomicU16,
    timestamp: AtomicU32,
    buf: Arc<[Mutex<Option<Packet>>; BUFFER_SIZE]>,
    block: Notify,
    index: AtomicU8,
    sliding_window: AtomicU8,
    sliding_counter: AtomicU8,
}

impl RtpBuffer {
    pub fn init() -> Self {
        Self {
            sequence_number: AtomicU16::new(0),
            timestamp: AtomicU32::new(0),
            buf: Arc::new(std::array::from_fn(|_| Mutex::new(None))),
            block: Notify::new(),
            index: AtomicU8::new(0),
            sliding_window: AtomicU8::new(1),
            sliding_counter: AtomicU8::new(0),
        }
    }

    pub async fn insert(&self, pkt: Packet) {
        let sn = self.sequence_number.load(Ordering::SeqCst);
        let ts = self.timestamp.load(Ordering::SeqCst);
        let seq_num = pkt.header.sequence_number;
        //仅插入有效数据包
        if sn > seq_num || ts > pkt.header.timestamp || Self::check_sn_wrap(sn, seq_num) || ts == 0 || ts == 0 {
            let index = seq_num as usize % BUFFER_SIZE;
            let mut item = unsafe { self.buf.get_unchecked(index).lock().await };
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
        let mut pkt = None;
        let sn = self.sequence_number.load(Ordering::SeqCst);
        let ts = self.timestamp.load(Ordering::SeqCst);
        for i in 0..BUFFER_SIZE {
            //首次/回绕获取包时，不减少计数
            if self.sliding_counter.load(Ordering::SeqCst) > 1 && (sn != 0 || ts != 0) {
                self.sliding_counter.fetch_sub(1, Ordering::SeqCst);
            }
            let mut guard = unsafe { self.buf.get_unchecked(index).lock().await };
            index += 1;
            if index == BUFFER_SIZE {
                index = 0;
            }
            if let Some(item) = &*guard {
                self.sequence_number.store(item.header.sequence_number, Ordering::SeqCst);
                self.timestamp.store(item.header.timestamp, Ordering::SeqCst);
                self.index.store(index as u8, Ordering::Relaxed);
                let window = self.sliding_window.load(Ordering::SeqCst);
                if i == 1 {
                    if window == 2 || window == 4 || window == 8 {
                        self.sliding_window.store(window / 2, Ordering::SeqCst);
                    }
                } else if i > 3 {
                    if window == 1 || window == 2 || window == 4 {
                        self.sliding_window.store(window * 2, Ordering::SeqCst);
                    }
                }
                std::mem::swap(&mut *guard, &mut pkt);
                return pkt;
            }
        }
        pkt
    }

    //最后一次获取所有数据?
    pub async fn flush_pkt(&self) -> Vec<Packet> {
        let mut vec = Vec::new();
        let mut index = self.index.load(Ordering::Relaxed) as usize;
        for i in 0..BUFFER_SIZE {
            let mut guard = unsafe { self.buf.get_unchecked(index).lock().await };
            index += 1;
            if index == BUFFER_SIZE {
                index = 0;
            }
            if let Some(item) = &*guard {
                let mut pkt = None;
                std::mem::swap(&mut *guard, &mut pkt);
                vec.push(pkt.unwrap());
            }
        }
        vec
    }

    fn u16_sub_abs(a: u16, b: u16) -> u16 {
        if a > b {
            return a - b;
        }
        b - a
    }
    //检查sn是否回绕；sn变小，且差值的绝对值大于u16。65535/2=32767
    fn check_sn_wrap(a: u16, b: u16) -> bool {
        Self::u16_sub_abs(a, b) > 32767
    }
}

#[cfg(test)]
mod test {
    use std::collections::VecDeque;

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
}