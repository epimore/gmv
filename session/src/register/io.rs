use base::dashmap::DashMap;
use base::net::state::Association;
use base::tokio::time::Instant;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

//transport
pub struct Network {
    pub session: DashMap<Arc<str>, DeviceSession>,
    pub net_device_map: DashMap<Association, Arc<str>>,
}
impl Network {

    /// 核心方法：更新网络三元组 → 设备映射，处理设备顶替逻辑
    /// 返回被顶替的旧设备 ID（若有）
    pub(crate) fn update_net_device_mapping(
        &self,
        association: &Association,
        device_id: &Arc<str>,
    ) -> Option<Arc<str>> {
        // 若新三元组原来属于其他设备，标记其过期
        if let Some(old_device_id) = self
            .net_device_map
            .insert(association.clone(), device_id.clone())
        {
            if old_device_id != *device_id {
                // 不同设备抢占三元组：标记旧设备过期
                if let Some(old_session) = self.session.get(&old_device_id) {
                    old_session
                        .association_expire
                        .store(true, Ordering::Relaxed);
                }
                return Some(old_device_id);
            }
        }
        None
    }

    /// 清理指定设备的旧网络映射（如果存在）
    pub(crate) fn remove_device_mapping(&self, device_id: &Arc<str>, old_association: &Association) {
        // 仅当旧三元组仍指向本设备时才清理
        if let Some(mapped_id) = self.net_device_map.get(old_association) {
            if *mapped_id == *device_id {
                self.net_device_map.remove(old_association);
            }
        }
    }

    // 原 insert 方法改为调用公共方法
    pub fn insert(&self, device_id: Arc<str>, device_session: DeviceSession) {
        // 建立新三元组映射（自动处理旧设备顶替）
        self.update_net_device_mapping(&device_session.association, &device_id);
        // 插入/更新 session
        self.session.insert(device_id, device_session);
    }

    pub fn rm_by_association(&self, association: &Association) {
        if let Some((_, device_id)) = self.net_device_map.remove(association) {
            self.session.remove(&device_id);
        }
    }

    pub fn rm_by_device_id(&self, device_id: &Arc<str>) {
        if let Some((_, ds)) = self.session.remove(device_id) {
            self.net_device_map.remove(&ds.association);
        }
    }

    //todo 更新设备状态
}
pub struct DeviceSession {
    pub contact_uri: String, // 来自 REGISTER Contact
    pub association: Association,
    pub association_expire: AtomicBool, //网络三元组是否过期，当其他设备占用association时
    pub support_lr: AtomicBool,         // Contact 是否有 lr
    pub heartbeat_sec: u8,              //心跳有效期 秒
    pub last_seen: Instant,             //上次注册时刻
    pub registration_duration: Duration, //注册有效期
}
impl DeviceSession {
    pub fn build(
        contact_uri: String,
        association: Association,
        heartbeat: u8,
        registration_duration: Duration,
    ) -> Self {
        Self {
            contact_uri,
            association,
            association_expire: AtomicBool::new(false),
            support_lr: AtomicBool::new(false),
            heartbeat_sec: heartbeat,
            last_seen: Instant::now(),
            registration_duration,
        }
    }
    pub fn enable_lr(&mut self) {
        self.support_lr.store(true, Ordering::Relaxed)
    }
}
