use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

use parking_lot::lock_api::RwLockReadGuard;
use parking_lot::RwLock;

use common::anyhow::anyhow;
use common::dashmap::DashMap;
use common::dashmap::mapref::entry::Entry;
use common::dashmap::mapref::one::Ref;
use common::err::{GlobalResult, TransError};
use common::err::GlobalError::SysErr;
use common::log::{debug, error, info};
use common::once_cell::sync::Lazy;
use common::tokio::runtime;
use common::tokio::runtime::Runtime;
use common::tokio::sync::Notify;
use common::tokio::sync::oneshot::Receiver;
use common::tokio::time::timeout;

use crate::data::session;

//缓冲空间大小
pub const BUFFER_SIZE: usize = 64;
const ROW: RwLock<(u16, u32, Vec<u8>)> = row_init_fn();

const fn row_init_fn() -> RwLock<(u16, u32, Vec<u8>)> {
    RwLock::new((0, 0, Vec::new()))
}

static BUFFER: Lazy<DashMap<u32, Buf>> = Lazy::new(|| DashMap::new());

#[derive(Debug, Clone)]
pub struct Cache;

impl Cache {
    ///@Description 新增初始化一块SSRC缓冲数据
    ///@Param
    pub(super) fn add_ssrc(ssrc: u32) -> GlobalResult<()> {
        match BUFFER.entry(ssrc) {
            Entry::Occupied(_) => { Err(SysErr(anyhow!("ssrc = {:?},媒体流标识重复",ssrc))) }
            Entry::Vacant(en) => {
                en.insert(Buf::default());
                Ok(())
            }
        }
    }
    ///@Description 移除ssrc
    ///@Param ssrc
    pub fn rm_ssrc(ssrc: &u32){
        BUFFER.get(ssrc).map(|v| {
            v.async_block.1.store(false, Ordering::SeqCst);
            v.async_block.0.notify_one();
        });
        BUFFER.remove(ssrc);
    }
    ///@Description 获取当前流信息
    ///@Param
    ///@return 存在ssrc-(当前缓冲区有效{含乱序}数据大小，可变缓冲区大小，输出流即时时间戳)
    pub fn get_state(ssrc: &u32) -> Option<(u8, u8, u32)> {
        BUFFER.get(ssrc).map(
            |c| {
                let guard = c.state.read();
                let temp = guard.clone();
                drop(guard);
                (c.counter.load(Ordering::SeqCst), temp.sliding_window, temp.ts)
            })
    }

    ///@Description 生产数据
    ///@Param
    ///@return
    pub fn produce(ssrc: u32, sn: u16, ts: u32, raw: Vec<u8>) {
        match BUFFER.get(&ssrc) {
            None => {
                info!("未注册的ssrc,抛弃");
                //todo 未知ssrc 是否 每隔N秒回调信令无该SSRC
                // 插入缓存，设置状态为0，开始计时，
                // 计时结束后设置状态为1，进入下一轮计时，计时结束如果状态为1则移除，表示已无该SSRC;
                // 当还有该SSRC插入则-回调，并改状态为0，重新计时；
            }
            Some(buf) => {
                // info!("produce data => ssrc = {}, sn = {}, ts = {},data-size = {}", ssrc, sn, ts,raw.len());
                if buf.add_counter_by_ts_sn(sn, ts) {
                    buf.update_inner_raw(sn, ts, raw);
                    let _ = session::refresh(ssrc).hand_err(|msg| error!("{msg}"));
                }
            }
        }
    }

    pub async fn readable(ssrc: &u32) -> GlobalResult<()> {
        let able = match BUFFER.get(ssrc) {
            None => { Err(SysErr(anyhow!("ssrc = {:?},媒体流或过期未注册",ssrc)))? }
            Some(buf) => {
                buf.readable().await
            }
        };
        if !able {
            BUFFER.remove(ssrc);
        }
        Ok(())
    }

    pub fn consume(ssrc: &u32) -> GlobalResult<Option<Vec<u8>>> {
        match BUFFER.get(ssrc) {
            None => { Err(SysErr(anyhow!("ssrc = {:?},媒体流或过期未注册",ssrc))) }
            Some(buf) => {
                let mut state_guard = buf.state.write();
                buf.sub_counter();
                //丢包处理，获取下一个值
                let mut inx: usize = state_guard.index;
                for i in 0..BUFFER_SIZE {
                    let mut inner_guard = unsafe { buf.inner.get_unchecked(inx).write() };
                    inx += 1;
                    if inx >= BUFFER_SIZE {
                        inx = 0;
                    }
                    if !inner_guard.2.is_empty() {
                        let vec = std::mem::take(&mut inner_guard.2);
                        state_guard.ts = inner_guard.1;
                        state_guard.sn = inner_guard.0;
                        drop(inner_guard);
                        if i == 0 {//首次就命中则网络良好，减小缓冲区窗口
                            state_guard.down_sliding_window();
                        }
                        if i > 2 {
                            state_guard.up_sliding_window();
                        }
                        state_guard.index = inx;
                        // info!("consume data => ssrc = {}, sn = {}, ts = {}, index = {},vec-len = {}",ssrc,state_guard.sn,state_guard.ts,inx,vec.len());
                        return Ok(Some(vec));
                    }
                    //非(首次读取与回绕)查找有效数据不累减计数器与扩大缓存滑动窗口
                    if state_guard.ts != 0 && !State::check_sn_abs_more_32767(inner_guard.0, state_guard.sn) {
                        buf.sub_counter();
                    }
                }
                Ok(None)
            }
        }
    }
}

#[derive(Debug)]
struct Buf {
    //(sn,ts,row data)
    inner: Arc<[RwLock<(u16, u32, Vec<u8>)>; BUFFER_SIZE]>,
    counter: Arc<AtomicU8>,
    state: Arc<RwLock<State>>,
    //异步阻塞等待数据
    async_block: (Notify, AtomicBool),
}

impl Default for Buf {
    fn default() -> Self {
        Self {
            inner: Arc::new([ROW; BUFFER_SIZE]),
            counter: Arc::new(AtomicU8::new(0)),
            state: Arc::new(RwLock::new(State::default())),
            async_block: (Notify::new(), AtomicBool::new(true)),
        }
    }
}

impl Buf {
    ///@Description 插入更新缓冲区数据，sn%BUFFER_SIZE<BUFFER_SIZE故不会下标越界
    /// (此处使用的指针偏移的方式定位到数据，也可使用uncheck_get来定位数据，避免下标校验损坏性能)
    ///@Param
    ///@return
    fn update_inner_raw(&self, sn: u16, ts: u32, raw: Vec<u8>) {
        let index = sn as usize % BUFFER_SIZE;
        // let ptr = self.inner.as_ptr();
        // let mut lock = unsafe { (*ptr.add(index)).write() };
        let mut lock = unsafe { self.inner.get_unchecked(index).write() };
        *lock = (sn, ts, raw);
    }

    async fn readable(&self) -> bool {
        if self.counter.load(Ordering::SeqCst) < self.state.read().sliding_window {
            info!("等待唤醒");
            self.async_block.0.notified().await;
            return self.async_block.1.load(Ordering::SeqCst);
        }
        true
    }

    //判断是否为有效数据
    fn add_counter_by_ts_sn(&self, sn: u16, ts: u32) -> bool {
        let read_guard = self.state.read();
        //序号增加、时间戳增加、序号回绕 皆为有效数据
        if sn > read_guard.sn || ts > read_guard.ts || State::check_sn_abs_more_32767(sn, read_guard.sn) {
            self.add_counter();
            //计数器大于等于滑动缓存窗口提示可读
            if self.counter.load(Ordering::SeqCst) >= read_guard.sliding_window {
                self.async_block.0.notify_one();
            }
            true
        } else {
            false
        }
    }

    ///@Description 计数器加一，当计数器<255
    ///@Param
    ///@return
    fn add_counter(&self) {
        if self.counter.load(Ordering::SeqCst) < 255 {
            self.counter.fetch_add(1, Ordering::SeqCst);
        }
    }
    ///@Description 计数器减一，当计数器>1
    ///@Param
    ///@return
    fn sub_counter(&self) {
        if self.counter.load(Ordering::SeqCst) > 1 {
            self.counter.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

#[derive(Debug, Copy, Clone)]
struct State {
    sliding_window: u8,
    index: usize,
    ts: u32,
    sn: u16,
}

impl Default for State {
    fn default() -> Self {
        Self {
            sliding_window: 2,
            index: 0,
            ts: 0,
            sn: 0,
        }
    }
}

impl State {
    fn up_sliding_window(&mut self) {
        let size = self.sliding_window;
        if size == 1 || size == 2 || size == 4 {
            self.sliding_window *= 2;
        }
    }
    fn down_sliding_window(&mut self) {
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
    fn check_sn_abs_more_32767(a: u16, b: u16) -> bool {
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
