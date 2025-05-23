/// 数据读写会话：与网络协议交互
/// UDP：三次心跳超时则移除会话
/// TCP：连接断开或三次心跳超时则移除会话
pub mod rw {
    use std::collections::{BTreeSet, HashMap};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    use parking_lot::Mutex;
    use rsip::{Response, SipMessage};

    use common::anyhow::anyhow;
    use common::bytes::Bytes;
    use common::constructor::New;
    use common::exception::{GlobalResult, TransError};
    use common::exception::GlobalError::SysErr;
    use common::log::{error, warn};
    use common::net::state::{Association, Event, Package, Protocol, Zip};
    use common::once_cell::sync::Lazy;
    use common::tokio;
    use common::tokio::sync::{mpsc, Notify};
    use common::tokio::sync::mpsc::{Receiver, Sender};
    use common::tokio::time;
    use common::tokio::time::Instant;

    use crate::gb::shared::event::{Container, EventSession, EXPIRES, Ident};
    use crate::storage::entity::GmvDevice;

    static RW_SESSION: Lazy<RWSession> = Lazy::new(|| RWSession::init());

    pub struct RWSession {
        shared: Arc<Shared>,
        //更新设备状态
        db_task: Sender<String>,
    }

    impl RWSession {
        fn init() -> Self {
            let (tx, rx) = mpsc::channel(16);
            let session = RWSession {
                shared: Arc::new(
                    Shared {
                        state: Mutex::new(State { sessions: HashMap::new(), expirations: BTreeSet::new(), bill_map: HashMap::new() }),
                        background_task: Notify::new(),
                    }
                ),
                db_task: tx.clone(),
            };
            let shared = session.shared.clone();
            thread::Builder::new().name("Shared:rw".to_string()).spawn(|| {
                let rt = tokio::runtime::Builder::new_multi_thread().enable_time().thread_name("RW-SESSION").build().hand_log(|msg| error!("{msg}")).unwrap();
                rt.block_on(async {
                    let db_task = tokio::spawn(async move {
                        Self::do_update_device_status(rx).await;
                    });
                    let clean_task = tokio::spawn(async move {
                        let _ = Self::purge_expired_task(shared).await;
                    });
                    let _ = db_task.await.hand_log(|msg| error!("Session:{msg}"));
                    let _ = clean_task.await.hand_log(|msg| error!("WEB:{msg}"));
                });
            }).expect("Shared:rw background thread create failed");
            session
        }
        async fn do_update_device_status(mut rx: Receiver<String>) {
            while let Some(device_id) = rx.recv().await {
                let _ = GmvDevice::update_gmv_device_status_by_device_id(&device_id, 0).await;
            }
        }

        async fn purge_expired_task(shared: Arc<Shared>) -> GlobalResult<()> {
            loop {
                if let Some(when) = shared.purge_expired_state().await? {
                    tokio::select! {
                        _ = time::sleep_until(when) =>{},
                        _ = shared.background_task.notified() =>{},
                    }
                } else {
                    shared.background_task.notified().await;
                }
            }
        }

        pub fn insert(device_id: &String, tx: Sender<Zip>, heartbeat: u8, bill: &Association) {
            let expires = Duration::from_secs(heartbeat as u64 * 3);
            let when = Instant::now() + expires;

            let mut state = RW_SESSION.shared.state.lock();

            let notify = state.next_expiration().map(|ts| ts > when).unwrap_or(true);
            state.expirations.insert((when, device_id.clone()));
            //当插入时，已有该设备映射时，需删除老数据，插入新数据
            if let Some((_tx, when, _expires, old_bill)) = state.sessions.insert(device_id.clone(), (tx, when, expires, bill.clone())) {
                state.expirations.remove(&(when, device_id.clone()));
                state.bill_map.remove(&old_bill);
                state.bill_map.insert(bill.clone(), device_id.clone());
            }
            drop(state);

            if notify {
                RW_SESSION.shared.background_task.notify_one();
            }
        }

        //用于收到网络出口对端连接断开时，清理rw_session数据
        pub fn clean_rw_session_by_bill(bill: &Association) {
            let mut guard = RW_SESSION.shared.state.lock();

            let state = &mut *guard;
            state.bill_map.remove(bill).map(|device_id| {
                state.sessions.remove(&device_id).map(|(_tx, when, _expires, _bill)| {
                    state.expirations.remove(&(when, device_id));
                });
            });
        }

        pub fn get_device_id_by_association(bill: &Association) -> Option<String> {
            let guard = RW_SESSION.shared.state.lock();
            let res = guard.bill_map.get(bill).map(|device_id| device_id.clone());


            res
        }

        //用于清理rw_session数据及端口TCP网络连接
        //todo 禁用设备时需调用
        pub async fn clean_rw_session_and_net(device_id: &String) {
            let res = {
                let mut guard = RW_SESSION.shared.state.lock();

                let state = &mut *guard;
                if let Some((tx, when, _expires, bill)) = state.sessions.remove(device_id) {
                    state.expirations.remove(&(when, device_id.clone()));
                    state.bill_map.remove(&bill);
                    //通知网络出口关闭TCP连接
                    if &Protocol::TCP == bill.get_protocol() {
                        Some((tx, bill))
                    } else { None }
                } else { None }
            };

            if let Some((tx, bill)) = res {
                let _ = tx.try_send(Zip::build_event(Event::new(bill, 0))).hand_log(|msg| warn!("{msg}"));
            }
        }

        pub fn heart(device_id: &String, new_bill: Association) {
            let mut guard = RW_SESSION.shared.state.lock();

            let state = &mut *guard;
            state.sessions.get_mut(device_id).map(|(_tx, when, expires, bill)| {
                //UDP的无连接状态，需根据心跳实时刷新其网络三元组
                if &Protocol::UDP == bill.get_protocol() {
                    state.bill_map.remove(bill);
                    state.bill_map.insert(bill.clone(), device_id.clone());
                    *bill = new_bill;
                }
                state.expirations.remove(&(*when, device_id.clone()));
                let ct = Instant::now() + *expires;
                *when = ct;
                state.expirations.insert((ct, device_id.clone()));
            });
        }

        pub fn get_bill_by_device_id(device_id: &String) -> Option<Association> {
            let guard = RW_SESSION.shared.state.lock();

            let option_bill = guard.sessions.get(device_id).map(|(_tx, _when, _expires, bill)| bill.clone());

            option_bill
        }

        pub fn get_expires_by_device_id(device_id: &String) -> Option<Duration> {
            let guard = RW_SESSION.shared.state.lock();
            let option_expires = guard.sessions.get(device_id).map(|(_tx, _when, expires, _bill)| *expires);


            option_expires
        }

        fn get_output_sender_by_device_id(device_id: &String) -> Option<(Sender<Zip>, Association)> {
            let guard = RW_SESSION.shared.state.lock();
            let opt = guard.sessions.get(device_id).map(|(sender, _, _, bill)| (sender.clone(), bill.clone()));


            opt
        }

        pub fn has_session_by_device_id(device_id: &String) -> bool {
            let guard = RW_SESSION.shared.state.lock();


            guard.sessions.contains_key(device_id)
        }
    }

    #[derive(New)]
    pub struct RequestOutput {
        ident: Ident,
        msg: SipMessage,
        event_sender: Option<Sender<(Option<Response>, Instant)>>,
    }

    impl RequestOutput {
        pub fn do_send_off(device_id: &String, msg: SipMessage) -> GlobalResult<()> {
            let (request_sender, bill) = RWSession::get_output_sender_by_device_id(device_id).ok_or(SysErr(anyhow!("设备 {device_id},已下线")))?;
            let _ = request_sender.try_send(Zip::build_data(Package::new(bill, Bytes::from(msg)))).hand_log(|msg| warn!("{msg}"));
            Ok(())
        }

        pub fn do_send(self) -> GlobalResult<()> {
            let device_id = self.ident.get_device_id();
            let (request_sender, bill) = RWSession::get_output_sender_by_device_id(device_id).ok_or(SysErr(anyhow!("设备 {device_id},已下线")))?;
            let when = Instant::now() + Duration::from_secs(EXPIRES);
            EventSession::listen_event(&self.ident, when, Container::build_res(self.event_sender))?;
            let _ = request_sender.try_send(Zip::build_data(Package::new(bill, Bytes::from(self.msg)))).hand_log(|msg| error!("{msg}"));
            Ok(())
        }

        pub(super) fn _do_send(self) -> GlobalResult<()> {
            let device_id = self.ident.get_device_id();
            let (request_sender, bill) = RWSession::get_output_sender_by_device_id(device_id).ok_or(SysErr(anyhow!("设备 {device_id},已下线")))?;
            let _ = request_sender.try_send(Zip::build_data(Package::new(bill, Bytes::from(self.msg)))).hand_log(|msg| error!("{msg}"));
            Ok(())
        }
    }

    struct Shared {
        state: Mutex<State>,
        background_task: Notify,
    }

    impl Shared {
        //清理过期state,并返回下一个过期瞬间刻度
        async fn purge_expired_state(&self) -> GlobalResult<Option<Instant>> {
            let mut guard = RW_SESSION.shared.state.lock();

            let state = &mut *guard;
            let now = Instant::now();
            while let Some((when, device_id)) = state.expirations.iter().next() {
                if when > &now {
                    return Ok(Some(*when));
                }
                //放入队列中处理，避免阻塞导致锁长期占用:更新DB中设备状态为离线
                let _ = RW_SESSION.db_task.clone().try_send(device_id.clone()).hand_log(|msg| warn!("{msg}"));
                // GmvDevice::update_gmv_device_status_by_device_id(device_id, 0);
                //移除会话map
                if let Some((tx, when, _dur, bill)) = state.sessions.remove(device_id) {
                    state.bill_map.remove(&bill);
                    state.expirations.remove(&(when, device_id.to_string()));
                    //通知网络出口关闭TCP连接
                    if &Protocol::TCP == bill.get_protocol() {
                        let _ = tx.try_send(Zip::build_event(Event::new(bill, 0))).hand_log(|msg| warn!("{msg}"));
                    }
                }
            }

            Ok(None)
        }
    }

    struct State {
        //映射设备ID，会话发送端，过期瞬时，心跳周期，网络三元组，device_id,msg,dst_addr,time,duration,bill
        sessions: HashMap<String, (Sender<Zip>, Instant, Duration, Association)>,
        //标识设备状态过期时刻，instant,device_id
        expirations: BTreeSet<(Instant, String)>,
        //映射网络三元组与设备ID，bill,device_id
        bill_map: HashMap<Association, String>,
    }

    impl State {
        //获取下一个过期瞬间刻度
        fn next_expiration(&self) -> Option<Instant> {
            self.expirations.first().map(|expiration| expiration.0)
        }
    }
}

/// 会话事件：与业务事件交互
/// 定位：请求 <——> 回复
pub mod event {
    use std::collections::{BTreeSet, HashMap};
    use std::collections::hash_map::Entry;
    use std::sync::Arc;
    use std::thread;

    use parking_lot::Mutex;
    use rsip::{Response, SipMessage};

    use common::anyhow::anyhow;
    use common::constructor::{Get, New};
    use common::exception::{GlobalResult, TransError};
    use common::exception::GlobalError::SysErr;
    use common::log::{error, warn};
    use common::once_cell::sync::Lazy;
    use common::tokio;
    use common::tokio::sync::mpsc::Sender;
    use common::tokio::sync::Notify;
    use common::tokio::time;
    use common::tokio::time::Instant;

    use crate::gb::shared::rw::RequestOutput;

    /// 会话超时 8s
    pub const EXPIRES: u64 = 8;
    static EVENT_SESSION: Lazy<EventSession> = Lazy::new(|| EventSession::init());

    pub struct EventSession {
        shared: Arc<Shared>,
    }

    impl EventSession {
        fn init() -> Self {
            let session = EventSession {
                shared: Arc::new(
                    Shared {
                        state: Mutex::new(State { expirations: BTreeSet::new(), ident_map: HashMap::new(), device_session: HashMap::new() }),
                        background_task: Notify::new(),
                    }
                ),
            };
            let shared = session.shared.clone();
            thread::Builder::new().name("Shared:rw".to_string()).spawn(|| {
                let rt = tokio::runtime::Builder::new_current_thread().enable_time().thread_name("EVENT-SESSION").build().hand_log(|msg| error!("{msg}")).unwrap();
                let _ = rt.block_on(Self::purge_expired_task(shared));
            }).expect("Shared:event background thread create failed");
            session
        }

        async fn purge_expired_task(shared: Arc<Shared>) -> GlobalResult<()> {
            loop {
                if let Some(when) = shared.purge_expired_state().await? {
                    tokio::select! {
                        _ = time::sleep_until(when) =>{},
                        _ = shared.background_task.notified() =>{},
                    }
                } else {
                    shared.background_task.notified().await;
                }
            }
        }

        //即时事件监听，延迟事件监听
        pub(crate) fn listen_event(ident: &Ident, when: Instant, container: Container) -> GlobalResult<()> {
            let mut guard = EVENT_SESSION.shared.state.lock();

            let state = &mut *guard;
            match state.device_session.entry(ident.call_id.clone()) {
                Entry::Occupied(_o) => {
                    Err(SysErr(anyhow!("new = {:?},事件重复-添加监听无效",ident)))
                }
                Entry::Vacant(en) => {
                    en.insert(ident.device_id.clone());
                    state.expirations.insert((when, ident.clone()));
                    state.ident_map.insert(ident.clone(), (when, container));

                    Ok(())
                }
            }
        }

        pub fn remove_event(ident: &Ident) {
            let mut guard = EVENT_SESSION.shared.state.lock();

            let state = &mut *guard;
            state.ident_map.remove(ident).map(|(when, _container)| {
                state.expirations.remove(&(when, ident.clone()));

                state.device_session.remove(ident.get_call_id())
            });
        }

        pub async fn handle_response(to_device_id: String, call_id: String, cs_eq: String, response: Response) -> GlobalResult<()> {
            let res = {
                let mut guard = EVENT_SESSION.shared.state.lock();

                let state = &mut *guard;
                match state.device_session.get(&call_id) {
                    None => {
                        warn!("丢弃：超时或未知响应。device_id={to_device_id},call_id={call_id},cs_eq={cs_eq}");
                        None
                    }
                    Some(device_id) => {
                        let ident: Ident = Ident::new(device_id.clone(), call_id, cs_eq);
                        //用于一次请求有多次响应：如点播时，有100-trying，再200-OK两次响应
                        //接收端确认无后继响应时，需调用remove_event()，清理会话
                        match state.ident_map.get(&ident) {
                            None => {
                                warn!("{:?},超时或未知响应",&ident);
                                None
                            }
                            Some((when, container)) => {
                                match container {
                                    Container::Res(res) => {
                                        //当tx为some时发送响应结果，不清理会话，由相应rx接收端根据自身业务清理
                                        if let Some(tx) = res {
                                            let when = *when;
                                            let sender = tx.clone();
                                            Some((sender, when))
                                            // drop(guard);
                                            // let _ = sender.send((Some(response), when)).await.hand_log(|msg| error!("{msg}"));
                                        } else {
                                            //清理会话
                                            state.ident_map.remove(&ident).map(|(when, _container)| {
                                                state.expirations.remove(&(when, ident.clone()));
                                                state.device_session.remove(ident.get_call_id())
                                            });
                                            None
                                        }
                                    }
                                    Container::Actor(..) => {
                                        Err(SysErr(anyhow!("{:?},无效事件",&ident)))?;
                                        None
                                    }
                                }
                            }
                        }
                    }
                }
            };

            if let Some((tx, when)) = res {
                let _ = tx.try_send((Some(response), when)).hand_log(|msg| error!("{msg}"));
            }
            Ok(())
        }
    }

    struct Shared {
        state: Mutex<State>,
        background_task: Notify,
    }

    impl Shared {
        //清理过期state,并返回下一个过期瞬间刻度
        async fn purge_expired_state(&self) -> GlobalResult<Option<Instant>> {
            let mut guard = EVENT_SESSION.shared.state.lock();

            let state = &mut *guard;
            let now = Instant::now();
            while let Some((when, expire_ident)) = state.expirations.iter().next() {
                if when > &now {
                    return Ok(Some(*when));
                }
                if let Some((ident, (when, container))) = state.ident_map.remove_entry(expire_ident) {
                    state.expirations.remove(&(when, expire_ident.clone()));
                    state.device_session.remove(ident.get_call_id());
                    match container {
                        Container::Res(res) => {
                            warn!("{:?},响应超时。",&ident);
                            //响应超时->发送None->接收端收到None,不需要再清理State
                            if let Some(tx) = res {
                                let _ = tx.try_send((None, when)).hand_log(|msg| error!("{msg}"));
                            }
                        }
                        //延迟事件触发后，添加延迟事件执行后的监听
                        Container::Actor(new_ident, msg, sender) => {
                            match state.device_session.entry(new_ident.call_id.clone()) {
                                Entry::Occupied(_o) => { Err(SysErr(anyhow!("{:?},事件重复监听",new_ident)))? }
                                //插入事件监听
                                Entry::Vacant(en) => {
                                    en.insert(new_ident.device_id.clone());
                                    let expires = time::Duration::from_secs(EXPIRES);
                                    let new_when = Instant::now() + expires;
                                    state.expirations.insert((new_when, new_ident.clone()));
                                    state.ident_map.insert(new_ident, (new_when, Container::build_res(sender)));
                                }
                            }
                            RequestOutput::new(ident, msg, None)._do_send()?
                        }
                    }
                }
            }

            Ok(None)
        }
    }

    #[derive(New, Get, Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
    pub struct Ident {
        device_id: String,
        call_id: String,
        cs_eq: String,
    }

    //Res : 请求响应，当需要做后继处理时，Sender不为None,接收端收到数据时如果后继不再接收数据，需调用清理state
    //Actor : 延时之后所做操作,对设备Bill发送请求数据
    #[derive(Debug)]
    pub enum Container {
        //Option<Response> 可能无响应
        //实时事件
        Res(Option<Sender<(Option<Response>, Instant)>>),
        //延时事件,执行时，会加入实时事件
        Actor(Ident, SipMessage, Option<Sender<(Option<Response>, Instant)>>),
    }

    impl Container {
        pub fn build_res(res: Option<Sender<(Option<Response>, Instant)>>) -> Self {
            Container::Res(res)
        }

        pub fn build_actor(ident: Ident, msg: SipMessage, sender: Option<Sender<(Option<Response>, Instant)>>) -> Self {
            Container::Actor(ident, msg, sender)
        }
    }

    struct State {
        expirations: BTreeSet<(Instant, Ident)>,
        ident_map: HashMap<Ident, (Instant, Container)>,
        //call_id:device_id
        device_session: HashMap<String, String>,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};
    use std::collections::hash_map::Entry;

    #[test]
    fn test_bt_set() {
        let mut set = BTreeSet::new();
        set.insert(2);
        set.insert(1);
        set.insert(6);
        set.insert(3);
        let mut iter = set.iter();
        assert_eq!(Some(&1), iter.next());
        assert_eq!(Some(&2), iter.next());
        assert_eq!(Some(&3), iter.next());
        assert_eq!(Some(&6), iter.next());
        assert_eq!(None, iter.next());
    }

    #[test]
    fn test_map_entry() {
        let mut map = HashMap::new();
        map.insert(1, 2);
        map.insert(3, 4);
        map.insert(5, 6);

        match map.entry(3) {
            Entry::Occupied(_) => { println!("repeat"); }
            Entry::Vacant(en) => {
                en.insert(10);
            }
        }
        match map.entry(7) {
            Entry::Occupied(_) => { println!("repeat"); }
            Entry::Vacant(en) => {
                en.insert(8);
            }
        }
        println!("{map:?}");
    }
}

