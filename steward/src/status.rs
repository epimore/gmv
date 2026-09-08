use gmv_protocol::guard::v1::{CenterConnectionState, StewardStatusSnapshot};
use std::sync::{Arc, RwLock};

#[derive(Clone)]
pub struct SharedStatus {
    inner: Arc<RwLock<StewardStatusSnapshot>>,
}

impl SharedStatus {
    pub fn new(installation_id: String, steward_instance_id: String) -> Self {
        Self {
            inner: Arc::new(RwLock::new(StewardStatusSnapshot {
                installation_id,
                steward_instance_id,
                center_connection: CenterConnectionState::Disabled as i32,
                observed_at_epoch_ms: crate::now_ms(),
                ..StewardStatusSnapshot::default()
            })),
        }
    }

    pub fn snapshot(&self) -> StewardStatusSnapshot {
        self.inner
            .read()
            .expect("Steward status lock poisoned")
            .clone()
    }

    pub fn update(&self, update: impl FnOnce(&mut StewardStatusSnapshot)) {
        let mut status = self.inner.write().expect("Steward status lock poisoned");
        update(&mut status);
        status.observed_at_epoch_ms = crate::now_ms();
    }
}
