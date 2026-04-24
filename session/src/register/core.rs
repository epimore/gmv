use std::sync::Arc;
use base::cache::c100k;
use base::net::state::Zip;
use base::once_cell::sync::Lazy;
use base::tokio::sync::mpsc::Sender;
use crate::gb::depot::SipPackage;

static REGISTER: Lazy<Register> = Lazy::new(|| init());

#[derive(Clone, Hash, Eq, PartialEq)]
pub enum TimeScheduleKey {
    RtpGateway(u32),
    OutSession(u64),
}

#[derive(Clone, Eq, PartialEq)]
pub enum Event {
    DeviceOffline(Arc<str>), //设备心跳超时下线
    ServerHeart(Arc<str>),//服务自身心跳
    OutSession(u64),
}
pub struct Register {
    pub inner: Arc<Inner>,
}

pub struct Inner {
    pub time_schedule: c100k::Cache<TimeScheduleKey>,
    pub io_tx: Sender<Zip>,
    pub sip_tx: Sender<SipPackage>,
    pub event_tx: Sender<Event>,
}
fn init() -> Register {
    unimplemented!()
}