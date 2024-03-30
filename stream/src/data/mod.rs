mod buffer;
mod session;

pub mod live_session {
    use std::sync::Arc;
    use std::time::Duration;
    use common::anyhow::anyhow;
    use common::dashmap::DashMap;
    use common::dashmap::mapref::entry::Entry;
    use common::err::GlobalError::SysErr;
    use common::err::GlobalResult;
    use common::log::info;
    use common::once_cell::sync::Lazy;
    use common::tokio::time::timeout;
    use crate::data::buffer::{Buf, BUFFER_SIZE, EXPIRE_SEC, State};

    static LIVE_SESSION: Lazy<Arc<DashMap<u32, LiveSession>>> = Lazy::new(|| Arc::new(DashMap::new()));

    #[derive(Default)]
    pub struct LiveSession {
        stream_id: String,
        // trace: Sender<State>,
        buf: Buf,
    }

    impl LiveSession {
        /// 插入ssrc与stream_id,其他为默认值
        ///
        /// # Arguments
        ///
        /// * `ssrc`:
        /// * `stream_id`:
        pub fn insert_by_session(ssrc: u32, stream_id: String) -> GlobalResult<()> {
            match LIVE_SESSION.entry(ssrc) {
                Entry::Occupied(_) => { Err(SysErr(anyhow!("ssrc = {:?},媒体流标识重复",ssrc))) }
                Entry::Vacant(en) => {
                    let mut session = LiveSession::default();
                    session.stream_id = stream_id;
                    en.insert(session);
                    Ok(())
                }
            }
        }
        pub fn remove(ssrc: &u32) -> Option<(u32, LiveSession)> {
            LIVE_SESSION.remove(ssrc)
        }

        ///@Description 获取当前流信息
        ///@Param
        ///@return 存在ssrc-(当前缓冲区有效{含乱序}数据大小，可变缓冲区大小，输出流即时时间戳)
        pub fn get_state(ssrc: &u32) -> Option<(u8, u8, u32)> {
            LIVE_SESSION.get(ssrc).map(
                |c| {
                    let guard = c.buf.state.read();
                    let temp = guard.clone();
                    drop(guard);
                    (c.buf.counter.read().clone(), temp.sliding_window, temp.ts)
                })
        }


        /// 生产数据
        pub fn produce(ssrc: u32, sn: u16, ts: u32, raw: Vec<u8>) -> bool {
            match LIVE_SESSION.get(&ssrc) {
                None => {
                    info!("未注册的ssrc,抛弃");
                    //todo 未知ssrc 是否 每隔N秒回调信令无该SSRC
                    // 插入缓存，设置状态为0，开始计时，
                    // 计时结束后设置状态为1，进入下一轮计时，计时结束如果状态为1则移除，表示已无该SSRC;
                    // 当还有该SSRC插入则-回调，并改状态为0，重新计时；
                    false
                }
                Some(session) => {
                    println!("net data in ....");
                    session.buf.add_counter();
                    session.buf.update_inner_raw(sn, ts, raw);
                    //用于通知缓冲数据大小满足缓存滑动窗口设定大小
                    if *session.buf.counter.read() > session.buf.state.read().sliding_window {
                        session.buf.async_block.notify_one();
                    }
                    true
                }
            }
        }

        pub async fn async_consume(ssrc: &u32) -> Option<Vec<u8>> {
            let res = timeout(Duration::from_secs(EXPIRE_SEC), async {
                loop {
                    let session_opt = LIVE_SESSION.get(ssrc);
                    match session_opt {
                        None => { return None; }
                        Some(session) => {
                            let counter = *session.buf.counter.read();
                            let sw = session.buf.state.read().sliding_window;
                            if counter >= sw {
                                let state_guard = session.buf.state.read();
                                let mut state = state_guard.clone();
                                drop(state_guard);
                                //丢包处理，获取下一个值
                                let mut inx: usize = state.index;
                                for i in 0..BUFFER_SIZE {
                                    let inner_guard = unsafe { session.buf.inner.get_unchecked(inx).read() };
                                    let inner = inner_guard.clone();
                                    drop(inner_guard);
                                    //判断回绕 【可能 BUG:倒放 时间差值?】
                                    if inner.0 < state.sn && inner.1 > state.ts && State::check_sn_abs_more_32767(inner.0, state.sn) {
                                        state.round_back = true;
                                    } else {
                                        state.round_back = false;
                                        // 非回绕与首次读取计数器减少：首次读取时state.index为0会循环查找第一个有效下标，造成计数器错误减小；同理回绕时sn下标是随机的，插入缓冲区时下标不会从0开始
                                        if state.sn != 0 && state.ts != 0 {
                                            session.buf.sub_counter();
                                        }
                                    }
                                    inx += 1;
                                    if inx >= BUFFER_SIZE {
                                        inx = 0;
                                    }
                                    //(序号增加||起始状态||序号回绕)&&值有效。【可能 BUG:倒放 时间差值?】
                                    if (inner.0 > state.sn || state.sn == 0 || state.round_back) && inner.1 >= state.ts && !inner.2.is_empty() {
                                        if i == 0 {//首次就命中则网络良好，减小缓冲区窗口
                                            state.down_sliding_window();
                                        }
                                        state.ts = inner.1;
                                        state.sn = inner.0;
                                        state.index = inx;
                                        let mut guard = session.buf.state.write();
                                        *guard = state.clone();
                                        return Some(inner.2);
                                    }
                                }
                                //出现脏数据时：如严重超时乱序导致无有效数据,增加缓冲区大小
                                let mut state_guard = session.buf.state.write();
                                if state_guard.sn != 0 && state_guard.ts != 0 && !state_guard.round_back {
                                    (*state_guard).up_sliding_window()
                                }
                            }
                            //阻塞等待 计数器>=缓冲区窗口 唤醒
                            session.buf.async_block.notified().await;
                        }
                    }
                }
            }).await;
            match res {
                Ok(data) => {
                    match data {
                        None => {
                            //无该SSRC
                            info!("无该SSRC <-- {}", ssrc);
                            None
                        }
                        data => { data }
                    }
                }
                Err(e) => {
                    //超时删除该SSRC
                    Self::remove(&ssrc);
                    info!("{e},超时删除SSRC <-- {}", ssrc);
                    None
                }
            }
        }
    }
}