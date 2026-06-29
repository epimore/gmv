use base::dashmap::DashMap;
use base::net::state::Association;
use base::tokio::time::Instant;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

pub struct DetachedAssociation {
    pub device_id: Arc<str>,
    pub generation: u64,
    pub timeout: Duration,
}

pub struct RebindResult {
    pub reconnect_generation: Option<u64>,
}

//transport
#[derive(Default)]
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
                    old_session.connected.store(false, Ordering::Relaxed);
                }
                return Some(old_device_id);
            }
        }
        None
    }

    /// 清理指定设备的旧网络映射（如果存在）
    pub(crate) fn remove_device_mapping(
        &self,
        device_id: &Arc<str>,
        old_association: &Association,
    ) {
        // 仅当旧三元组仍指向本设备时才清理
        let should_remove = self
            .net_device_map
            .get(old_association)
            .is_some_and(|mapped_id| *mapped_id == *device_id);
        if should_remove {
            self.net_device_map.remove(old_association);
        }
    }

    // 原 insert 方法改为调用公共方法
    pub fn insert(&self, device_id: Arc<str>, device_session: DeviceSession) {
        device_session
            .association_expire
            .store(false, Ordering::Relaxed);
        device_session.connected.store(true, Ordering::Relaxed);
        // 建立新三元组映射（自动处理旧设备顶替）
        self.update_net_device_mapping(&device_session.association, &device_id);
        // 插入/更新 session
        self.session.insert(device_id, device_session);
    }

    pub fn rebind(&self, device_id: &Arc<str>, association: Association) -> Option<RebindResult> {
        let (displaced_device_id, reconnect_generation) = {
            let mut session = self.session.get_mut(device_id)?;
            let association_changed = session.association != association;
            let was_connected = session.connected.swap(true, Ordering::AcqRel);
            let reconnect_generation = (!was_connected).then(|| {
                let generation = session.connection_generation.load(Ordering::Acquire);
                session.connection_generation.fetch_add(1, Ordering::AcqRel);
                generation
            });

            if association_changed {
                self.remove_device_mapping(device_id, &session.association);
                session.association = association.clone();
            }
            let displaced_device_id = self
                .net_device_map
                .insert(association, device_id.clone())
                .filter(|old_device_id| old_device_id != device_id);
            session.association_expire.store(false, Ordering::Release);
            session.last_seen = Instant::now();
            (displaced_device_id, reconnect_generation)
        };

        if let Some(displaced_device_id) = displaced_device_id {
            if let Some(displaced_session) = self.session.get(&displaced_device_id) {
                displaced_session
                    .association_expire
                    .store(true, Ordering::Release);
                displaced_session.connected.store(false, Ordering::Release);
                displaced_session
                    .connection_generation
                    .fetch_add(1, Ordering::AcqRel);
            }
        }

        Some(RebindResult {
            reconnect_generation,
        })
    }

    pub fn rebind_registration(
        &self,
        device_id: &Arc<str>,
        mut new_session: DeviceSession,
    ) -> Result<u64, DeviceSession> {
        let displaced_device_id;
        let cleanup_generation;
        {
            let Some(mut current_session) = self.session.get_mut(device_id) else {
                return Err(new_session);
            };
            if current_session.connected.load(Ordering::Acquire) {
                return Err(new_session);
            }

            cleanup_generation = current_session
                .connection_generation
                .load(Ordering::Acquire);
            new_session
                .connection_generation
                .store(cleanup_generation.saturating_add(1), Ordering::Release);
            self.remove_device_mapping(device_id, &current_session.association);
            displaced_device_id = self
                .net_device_map
                .insert(new_session.association.clone(), device_id.clone())
                .filter(|old_device_id| old_device_id != device_id);
            *current_session = new_session;
        }

        if let Some(displaced_device_id) = displaced_device_id {
            if let Some(displaced_session) = self.session.get(&displaced_device_id) {
                displaced_session
                    .association_expire
                    .store(true, Ordering::Release);
                displaced_session.connected.store(false, Ordering::Release);
                displaced_session
                    .connection_generation
                    .fetch_add(1, Ordering::AcqRel);
            }
        }
        Ok(cleanup_generation)
    }

    pub fn detach_association(&self, association: &Association) -> Option<DetachedAssociation> {
        let device_id = self.net_device_map.get(association)?.clone();
        let session = self.session.get_mut(&device_id)?;
        if session.association != *association {
            return None;
        }
        if !session.connected.swap(false, Ordering::AcqRel) {
            return None;
        }
        self.remove_device_mapping(&device_id, association);
        let generation = session
            .connection_generation
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1);
        Some(DetachedAssociation {
            device_id,
            generation,
            timeout: session.reconnect_timeout(Instant::now()),
        })
    }

    pub fn remove_disconnected(
        &self,
        device_id: &Arc<str>,
        generation: u64,
    ) -> Option<DeviceSession> {
        let (_, session) = self.session.remove_if(device_id, |_, session| {
            !session.connected.load(Ordering::Acquire)
                && session.connection_generation.load(Ordering::Acquire) == generation
        })?;
        self.remove_device_mapping(device_id, &session.association);
        Some(session)
    }

    pub fn connected_session(&self, device_id: &str) -> Option<DeviceSession> {
        self.session.get(device_id).and_then(|session| {
            if !session.connected.load(Ordering::Relaxed)
                || session.association_expire.load(Ordering::Relaxed)
            {
                return None;
            }
            Some(session.snapshot())
        })
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
}
pub struct DeviceSession {
    pub gb_version: Option<String>,
    pub contact_uri: String, // 来自 REGISTER Contact
    pub registration_call_id: String,
    pub registration_cseq: u32,
    pub association: Association,
    pub connected: AtomicBool,
    pub connection_generation: AtomicU64,
    pub association_expire: AtomicBool, //网络三元组是否过期，当其他设备占用association时
    pub support_lr: AtomicBool,         // Contact 是否有 lr
    pub heartbeat_sec: u8,              //心跳有效期 秒
    pub last_seen: Instant,             //上次注册时刻
    pub registration_duration: Duration, //注册有效期
    pub registration_expires_at: Instant,
}
impl DeviceSession {
    pub fn build(
        contact_uri: String,
        association: Association,
        heartbeat: u8,
        registration_duration: Duration,
    ) -> Self {
        Self {
            gb_version: None,
            contact_uri,
            registration_call_id: String::new(),
            registration_cseq: 0,
            association,
            connected: AtomicBool::new(true),
            connection_generation: AtomicU64::new(0),
            association_expire: AtomicBool::new(false),
            support_lr: AtomicBool::new(false),
            heartbeat_sec: heartbeat,
            last_seen: Instant::now(),
            registration_duration,
            registration_expires_at: Instant::now() + registration_duration,
        }
    }
    pub fn enable_lr(&mut self) {
        self.support_lr.store(true, Ordering::Relaxed)
    }

    pub fn set_gb_version(&mut self, gb_version: Option<String>) {
        self.gb_version = gb_version;
    }

    pub fn set_registration_identity(&mut self, call_id: String, cseq: u32) {
        self.registration_call_id = call_id;
        self.registration_cseq = cseq;
    }

    pub fn registration_generation_changed(&self, next: &Self) -> bool {
        let transport_changed =
            self.association != next.association || !self.connected.load(Ordering::Acquire);
        !self.registration_call_id.is_empty()
            && self.registration_call_id == next.registration_call_id
            && self.registration_cseq > 0
            && next.registration_cseq > 0
            && next.registration_cseq < self.registration_cseq
            && transport_changed
    }

    pub fn snapshot(&self) -> Self {
        Self {
            gb_version: self.gb_version.clone(),
            contact_uri: self.contact_uri.clone(),
            registration_call_id: self.registration_call_id.clone(),
            registration_cseq: self.registration_cseq,
            association: self.association.clone(),
            connected: AtomicBool::new(self.connected.load(Ordering::Relaxed)),
            connection_generation: AtomicU64::new(
                self.connection_generation.load(Ordering::Acquire),
            ),
            association_expire: AtomicBool::new(self.association_expire.load(Ordering::Relaxed)),
            support_lr: AtomicBool::new(self.support_lr.load(Ordering::Relaxed)),
            heartbeat_sec: self.heartbeat_sec,
            last_seen: self.last_seen,
            registration_duration: self.registration_duration,
            registration_expires_at: self.registration_expires_at,
        }
    }

    pub fn reconnect_timeout(&self, now: Instant) -> Duration {
        let heartbeat_timeout =
            Duration::from_secs(u64::from(self.heartbeat_sec).saturating_mul(3));
        let registration_remaining = self.registration_expires_at.saturating_duration_since(now);
        heartbeat_timeout.min(registration_remaining)
    }
}

#[cfg(test)]
mod tests {
    use super::{DeviceSession, Network};
    use base::net::state::{Association, Protocol};
    use base::tokio::time::Instant;
    use std::sync::Arc;
    use std::time::Duration;

    fn association(port: u16) -> Association {
        Association::new(
            "0.0.0.0:25600".parse().unwrap(),
            format!("127.0.0.1:{port}").parse().unwrap(),
            Protocol::TCP,
        )
    }

    fn session(association: Association) -> DeviceSession {
        DeviceSession::build(
            "sip:device@127.0.0.1:5060".to_string(),
            association,
            60,
            Duration::from_secs(3600),
        )
    }

    #[test]
    fn session_snapshot_preserves_gb_version() {
        let mut session = session(association(40000));
        session.set_gb_version(Some("3.0".to_string()));

        assert_eq!(session.snapshot().gb_version.as_deref(), Some("3.0"));
    }

    #[test]
    fn changed_register_call_id_does_not_mark_new_generation() {
        let mut current = session(association(40001));
        current.set_registration_identity("old-call-id".to_string(), 10);
        let mut next = session(association(40002));
        next.set_registration_identity("new-call-id".to_string(), 1);

        assert!(!current.registration_generation_changed(&next));
    }

    #[test]
    fn register_cseq_rollback_after_reconnect_marks_new_generation() {
        let mut current = session(association(40001));
        current.set_registration_identity("call-id".to_string(), 10);
        let mut next = session(association(40002));
        next.set_registration_identity("call-id".to_string(), 1);

        assert!(current.registration_generation_changed(&next));
    }

    #[test]
    fn register_cseq_rollback_on_same_connection_is_not_new_generation() {
        let mut current = session(association(40001));
        current.set_registration_identity("call-id".to_string(), 10);
        let mut next = session(association(40001));
        next.set_registration_identity("call-id".to_string(), 1);

        assert!(!current.registration_generation_changed(&next));
    }

    #[test]
    fn unknown_register_identity_does_not_mark_new_generation() {
        let current = session(association(40001));
        let mut next = session(association(40002));
        next.set_registration_identity("new-call-id".to_string(), 1);

        assert!(!current.registration_generation_changed(&next));
    }

    #[test]
    fn detach_keeps_registration_and_disables_outbound_association() {
        let network = Network::default();
        let device_id: Arc<str> = Arc::from("34020000001320000001");
        let old = association(40001);
        network.insert(device_id.clone(), session(old.clone()));

        assert!(network.detach_association(&old).is_some());
        assert!(network.session.contains_key(&device_id));
        assert!(network.connected_session(device_id.as_ref()).is_none());
    }

    #[test]
    fn delayed_old_close_does_not_detach_new_association() {
        let network = Network::default();
        let device_id: Arc<str> = Arc::from("34020000001320000001");
        let old = association(40001);
        let new = association(40002);
        network.insert(device_id.clone(), session(old.clone()));

        assert!(network.rebind(&device_id, new.clone()).is_some());
        assert!(network.detach_association(&old).is_none());
        let current = network
            .connected_session(device_id.as_ref())
            .expect("new association should remain connected");
        assert_eq!(current.association, new);
    }

    #[test]
    fn reconnect_timeout_uses_three_heartbeats_when_registration_is_longer() {
        let now = Instant::now();
        let mut device_session = session(association(40001));
        device_session.heartbeat_sec = 10;
        device_session.registration_expires_at = now + Duration::from_secs(120);

        assert_eq!(
            device_session.reconnect_timeout(now),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn reconnect_timeout_uses_remaining_registration_when_it_is_shorter() {
        let now = Instant::now();
        let mut device_session = session(association(40001));
        device_session.heartbeat_sec = 60;
        device_session.registration_expires_at = now + Duration::from_secs(25);

        assert_eq!(
            device_session.reconnect_timeout(now),
            Duration::from_secs(25)
        );
    }

    #[test]
    fn stale_reconnect_timeout_does_not_remove_rebound_session() {
        let network = Network::default();
        let device_id: Arc<str> = Arc::from("34020000001320000001");
        let old = association(40001);
        let new = association(40002);
        network.insert(device_id.clone(), session(old.clone()));

        let detached = network
            .detach_association(&old)
            .expect("old association should detach");
        assert!(network.rebind(&device_id, new.clone()).is_some());

        assert!(
            network
                .remove_disconnected(&device_id, detached.generation)
                .is_none()
        );
        assert_eq!(
            network
                .connected_session(device_id.as_ref())
                .expect("rebound session should remain")
                .association,
            new
        );
    }

    #[test]
    fn reconnect_timeout_removes_matching_disconnected_session() {
        let network = Network::default();
        let device_id: Arc<str> = Arc::from("34020000001320000001");
        let old = association(40001);
        network.insert(device_id.clone(), session(old.clone()));

        let detached = network
            .detach_association(&old)
            .expect("association should detach");
        let removed = network
            .remove_disconnected(&device_id, detached.generation)
            .expect("matching disconnected session should be removed");

        assert_eq!(removed.association, old);
        assert!(!network.session.contains_key(&device_id));
        assert!(!network.net_device_map.contains_key(&removed.association));
    }

    #[test]
    fn registration_rebinds_disconnected_session_and_invalidates_cleanup() {
        let network = Network::default();
        let device_id: Arc<str> = Arc::from("34020000001320000001");
        let old = association(40001);
        let new = association(40002);
        network.insert(device_id.clone(), session(old.clone()));
        let detached = network
            .detach_association(&old)
            .expect("association should detach");

        let cleanup_generation = network
            .rebind_registration(&device_id, session(new.clone()))
            .ok()
            .expect("registration should rebind disconnected session");

        assert_eq!(cleanup_generation, detached.generation);
        assert!(
            network
                .remove_disconnected(&device_id, detached.generation)
                .is_none()
        );
        assert_eq!(
            network
                .connected_session(device_id.as_ref())
                .expect("registration should restore the session")
                .association,
            new
        );
    }
}
