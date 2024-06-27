use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, AtomicU8, Ordering};

use rtp::packet::Packet;

use common::tokio::sync::{Mutex, Notify};

const BUFFER_SIZE: usize = 32;
const MAX_SLIDING_WINDOW: u8 = 8;
const ARRAY_REPEAT_VALUE: Option<Packet> = None;

pub struct RtpBuffer {
    pub sequence_number: AtomicU16,
    pub buf: Arc<Mutex<VecDeque<Option<Packet>>>>,
    pub block: Notify,
    pub sliding_window: AtomicU8,
    pub sliding_counter: AtomicU8,
}

impl RtpBuffer {
    pub fn init() -> Self {
        Self {
            sequence_number: AtomicU16::new(0),
            buf: Arc::new(Mutex::new(VecDeque::from([ARRAY_REPEAT_VALUE; BUFFER_SIZE]))),
            block: Notify::new(),
            sliding_window: AtomicU8::new(1),
            sliding_counter: AtomicU8::new(0),
        }
    }

    pub async fn insert(&self, pkt: Packet) {
        let index = pkt.header.sequence_number as usize % BUFFER_SIZE;
        let mut deque = self.buf.lock().await;
        deque.insert(index, Some(pkt));
        drop(deque);
        if self.sliding_counter.load(Ordering::SeqCst) < 255 {
            self.sliding_counter.fetch_add(1, Ordering::SeqCst);
        }
        if self.sliding_counter.load(Ordering::SeqCst) >= self.sliding_window.load(Ordering::SeqCst) {
            self.block.notify_one();
        }
    }

    pub async fn next_pkt(&self) -> Option<Packet> {
        if self.sliding_counter.load(Ordering::SeqCst) < self.sliding_window.load(Ordering::SeqCst) {
            self.block.notified().await;
        }
        let mut index = 1;
        let mut mutex_guard = self.buf.lock().await;
        while let Some(item) = mutex_guard.pop_front() {
            mutex_guard.push_back(None);
            if self.sliding_counter.load(Ordering::SeqCst) > 1 {
                self.sliding_counter.fetch_sub(1, Ordering::SeqCst);
            }
            if let Some(pkt) = &item {
                let window = self.sliding_window.load(Ordering::SeqCst);
                if pkt.header.sequence_number == self.sequence_number.load(Ordering::SeqCst).wrapping_add(1) {
                    if window == 2 || window == 4 || window == 8 {
                        self.sliding_window.store(window / 2, Ordering::SeqCst);
                    }
                } else {
                    if window == 1 || window == 2 || window == 4 {
                        self.sliding_window.store(window * 2, Ordering::SeqCst);
                    }
                }
                self.sequence_number.store(pkt.header.sequence_number, Ordering::SeqCst);
                return item;
            }
            if index == BUFFER_SIZE {
                return None;
            }
            index += 1;
        }
        None
    }
}

#[cfg(test)]
mod test {
    use std::collections::VecDeque;

    #[test]
    fn test_deque_vec() {
        let mut d = VecDeque::new();
        assert_eq!(d.front(), None);

        d.push_back(1);
        d.push_back(2);
        assert_eq!(d.front(), Some(&1));
    }
}