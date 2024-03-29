use std::sync::Arc;

use parking_lot::RwLock;

use common::err::TransError;
use common::tokio::sync::Notify;

//缓冲空间大小
pub const BUFFER_SIZE: usize = 64;
//流超时时间：秒
pub const EXPIRE_SEC: u64 = 16;
const ROW: RwLock<(u16, u32, Vec<u8>)> = row_init_fn();

const fn row_init_fn() -> RwLock<(u16, u32, Vec<u8>)> {
    RwLock::new((0, 0, Vec::new()))
}

#[derive(Debug)]
pub(super) struct Buf {
    //(sn,ts,row data)
    pub inner: Arc<[RwLock<(u16, u32, Vec<u8>)>; BUFFER_SIZE]>,
    pub counter: Arc<RwLock<u8>>,
    pub state: Arc<RwLock<State>>,
    //异步阻塞等待数据
    pub async_block: Notify,
}

impl Default for Buf {
    fn default() -> Self {
        Self {
            inner: Arc::new([ROW; BUFFER_SIZE]),
            counter: Arc::new(RwLock::new(0)),
            state: Arc::new(RwLock::new(State::default())),
            async_block: Notify::new(),
        }
    }
}

impl Buf {
    ///@Description 插入更新缓冲区数据，sn%BUFFER_SIZE<BUFFER_SIZE故不会下标越界
    /// (此处使用的指针偏移的方式定位到数据，也可使用uncheck_get来定位数据，避免下标校验损坏性能)
    ///@Param
    ///@return
    pub(crate) fn update_inner_raw(&self, sn: u16, ts: u32, raw: Vec<u8>) {
        let index = sn as usize % BUFFER_SIZE;
        let ptr = self.inner.as_ptr();
        let mut lock = unsafe { (*ptr.add(index)).write() };
        *lock = (sn, ts, raw);
    }
    ///@Description 计数器加一，当计数器<255
    ///@Param
    ///@return
    pub(crate) fn add_counter(&self) {
        if *self.counter.read() < 255 {
            *self.counter.write() += 1;
        }
    }
    ///@Description 计数器减一，当计数器>1
    ///@Param
    ///@return
    pub(crate) fn sub_counter(&self) {
        if *self.counter.read() > 1 {
            *self.counter.write() -= 1;
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub(super) struct State {
    pub sliding_window: u8,
    pub index: usize,
    pub ts: u32,
    pub sn: u16,
    pub round_back: bool,
}

impl Default for State {
    fn default() -> Self {
        Self {
            sliding_window: 1,
            index: 0,
            ts: 0,
            sn: 0,
            round_back: false,
        }
    }
}

impl State {
    pub(crate) fn up_sliding_window(&mut self) {
        let size = self.sliding_window;
        if size == 1 || size == 2 || size == 4 {
            self.sliding_window *= 2;
        }
    }
    pub(crate) fn down_sliding_window(&mut self) {
        let size = self.sliding_window;
        if size == 2 || size == 4 || size == 8 {
            self.sliding_window /= 2;
        }
    }
    fn u16_sub_abs(a: u16, b: u16) -> u16 {
        if a > b {
            return a - b;
        }
        b - a
    }
    //检查sn是否回绕；sn变小，且差值的绝对值大于u16。65535/2=32767
    pub(crate) fn check_sn_abs_more_32767(a: u16, b: u16) -> bool {
        Self::u16_sub_abs(a, b) > 32767
    }
}

#[cfg(test)]
mod test {
    use crate::data::buffer::Buf;

    #[test]
    fn test_init_and_modify_buf() {
        let buf = Buf::default();
        println!("init buf = {:?}", &buf);
        println!("\n-----------------------------------\n");
        let mut v1 = unsafe { buf.inner.get_unchecked(1) }.write();
        *v1 = (1, 2, vec![12, 23]);
        drop(v1);
        let mut v2 = unsafe { buf.inner.get_unchecked(2) }.write();
        *v2 = (5, 7, vec![45, 39]);
        drop(v2);
        println!("modify buf = {:?}", &buf);
        println!("\n-----------------------------------\n");
        println!("modify 0 buf = {:?}", unsafe { buf.inner.get_unchecked(0) });
        let guard = unsafe { buf.inner.get_unchecked(1).read() };
        let x = guard.clone();
        println!("modify 1 buf = {:?}", x);
        let guard = unsafe { buf.inner.get_unchecked(2).read() };
        let x = guard.clone();
        println!("modify 2 buf = {:?}", x);
        println!("modify 3 buf = {:?}", unsafe { buf.inner.get_unchecked(3) });
        println!("modify 4 buf = {:?}", unsafe { buf.inner.get_unchecked(9) });
    }
}
