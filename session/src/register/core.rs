use crate::gb::SessionConf;
use crate::gb::depot::SipPackage;
use crate::register::event::Event;
use crate::register::io::{DeviceSession, Network};
use base::cache::c100k;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{error, info, warn};
use base::net::state::{Association, Zip};
use base::once_cell::sync::Lazy;
use base::tokio::sync::mpsc::Sender;
use base::tokio::time::Instant;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

static REGISTER: Lazy<Register> = Lazy::new(|| Register::init());
pub const DEFAULT_EXPIRES: Duration = Duration::from_secs(8);
pub const SERVER_HEART_SECOND: u64 = 60;
const SERVER_HEART_EXPIRE: Duration = Duration::from_secs(SERVER_HEART_SECOND);

#[derive(Clone, Hash, Eq, PartialEq)]
pub enum TimeScheduleKey {
    Device3Heart(Arc<str>),
    DeviceRegistration(Arc<str>),
    OutSession(u64),
    ServerHeart(Arc<str>),
}

pub struct Register {
    pub inner: Arc<Inner>,
}

pub struct Inner {
    pub time_schedule: c100k::Cache<TimeScheduleKey>,
    pub server_conf: SessionConf,
    pub io_tx: Sender<Zip>,
    pub sip_tx: Sender<SipPackage>,
    pub event_tx: Sender<Event>,
    pub io_map: Network,
}
impl Register {
    fn init() -> Register {
        unimplemented!()
    }

    pub fn device_heart(device_id: Arc<str>, association: Association) -> GlobalResult<()> {
        let arc = REGISTER.inner.clone();

        // 1. 仅通过 session 判断设备是否已注册
        let Some(mut session) = arc.io_map.session.get_mut(&device_id) else {
            return Err(GlobalError::new_sys_error(
                "未注册设备，拒绝心跳响应",
                |msg| warn!("device_id={}; {msg}", device_id),
            ));
        };

        // 2. 若 association 未变化，仅刷新心跳
        if session.association == association {
            arc.time_schedule
                .refresh(TimeScheduleKey::Device3Heart(device_id.clone()))?;
            return Ok(());
        }

        // === association 发生变化：网络漂移 / NAT重建 / 重注册 ===
        info!(
            "设备 {device_id} 网络三元组变更: {} -> {}",
            session.association, association
        );

        // 3. 清理旧三元组映射
        arc.io_map
            .remove_device_mapping(&device_id, &session.association);

        // 4. 建立新三元组映射（自动处理旧设备顶替）
        arc.io_map
            .update_net_device_mapping(&association, &device_id);

        // 5. 更新本设备关联的三元组
        session.association = association;

        // 6. 显式释放 session 写锁
        drop(session);

        // 7. 刷新心跳定时器
        arc.time_schedule
            .refresh(TimeScheduleKey::Device3Heart(device_id))?;

        Ok(())
    }

    pub fn register_device(device_id: Arc<str>, ds: DeviceSession) -> GlobalResult<()> {
        let arc = REGISTER.inner.clone();

        // 设置心跳超时（3 倍心跳间隔）
        let expires = Duration::from_secs((ds.heartbeat_sec * 3) as u64);
        arc.time_schedule
            .insert(TimeScheduleKey::Device3Heart(device_id.clone()), expires)
            .hand_log(|e| error!("插入心跳定时器失败: {e}"))?;

        // 设置注册有效期
        arc.time_schedule
            .insert(
                TimeScheduleKey::DeviceRegistration(device_id.clone()),
                ds.registration_duration,
            )
            .hand_log(|e| error!("插入注册定时器失败: {e}"))?;

        // 插入网络映射和设备会话（复用公共逻辑）
        arc.io_map.insert(device_id, ds);
        Ok(())
    }

    /// 统一设备移除逻辑
    pub fn remove_device(&self, device_id: &Arc<str>) {
        let arc = REGISTER.inner.clone();

        // 先获取 session 信息，获取关联的 association
        if let Some((_, session)) = arc.io_map.session.remove(device_id) {
            // 清理网络三元组映射
            arc.io_map
                .net_device_map
                .remove(&session.association);

            // 可选：清理该设备的所有定时器
            // arc.time_schedule.delete(TimeScheduleKey::Device3Heart(device_id.clone()));
            // arc.time_schedule.delete(TimeScheduleKey::DeviceRegistration(device_id.clone()));
        }
    }

    pub async fn server_keep_heart_update_db(domain_id: Arc<str>) -> GlobalResult<()> {
        REGISTER.inner.server_conf.heart_to_db().await?;
        let arc = REGISTER.inner.clone();
        arc.time_schedule
            .insert(TimeScheduleKey::ServerHeart(domain_id), SERVER_HEART_EXPIRE)
    }
    pub fn server_keep_heart(domain_id: Arc<str>) -> GlobalResult<()> {
        let arc = REGISTER.inner.clone();
        let _ = arc
            .event_tx
            .try_send(Event::ServerHeart(domain_id.clone()))
            .hand_log(|msg| error!("{msg}"));
        arc.time_schedule
            .insert(TimeScheduleKey::ServerHeart(domain_id), SERVER_HEART_EXPIRE)
    }
}
