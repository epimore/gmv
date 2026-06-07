pub mod rw {
    use std::sync::{Arc, OnceLock};
    use std::time::Duration;

    use rsip::Request;

    use crate::gb::depot::{Callback, SipPackage, default_log_callback};
    use crate::register::core::Register;

    use base::exception::{GlobalResult, GlobalResultExt};
    use base::log::{error, warn};
    use base::net::state::{Association, Zip};
    use base::tokio::sync::mpsc::Sender;

    static RW_CTX: OnceLock<RWContext> = OnceLock::new();

    pub struct RWContext {
        io_tx: Sender<Zip>,
        sip_tx: Sender<SipPackage>,
    }

    impl RWContext {
        pub fn get_ctx() -> &'static RWContext {
            RW_CTX.get().expect("RWContext not initialized")
        }

        pub fn init(io_tx: Sender<Zip>, sip_tx: Sender<SipPackage>) {
            let _ = RW_CTX.set(RWContext { io_tx, sip_tx });
        }

        pub fn insert(_device_id: &String, _device_session: DeviceSession) {}

        pub fn clean_rw_session_by_bill(bill: &Association) {
            Register::detach_device_association(bill);
        }

        pub fn get_device_id_by_association(bill: &Association) -> Option<String> {
            Register::get_device_id_by_association(bill).map(|device_id| device_id.to_string())
        }

        pub fn clean_rw_session_and_net(device_id: &String) {
            let device_id: Arc<str> = Arc::from(device_id.as_str());
            if let Some(session) = Register::get_device_session(device_id.as_ref()) {
                Register::remove_device(&device_id);
                Register::close_tcp_if_needed(&session);
            }
        }

        pub fn keep_alive(device_id: &String, new_bill: Association) {
            let device_id: Arc<str> = Arc::from(device_id.as_str());
            let _ = Register::device_heart(&device_id, new_bill);
        }

        pub fn get_expires_by_device_id(device_id: &String) -> Option<Duration> {
            Register::get_device_session(device_id.as_str()).map(|ds| ds.registration_duration)
        }

        pub fn get_gb_version_by_device_id(device_id: &str) -> Option<String> {
            Register::get_device_session(device_id).and_then(|ds| ds.gb_version)
        }

        pub fn get_ds_by_device_id(device_id: &String) -> Option<(String, Association, bool)> {
            Register::get_connected_device_session(device_id.as_str()).map(|ds| {
                (
                    ds.contact_uri,
                    ds.association,
                    ds.support_lr.load(std::sync::atomic::Ordering::Relaxed),
                )
            })
        }

        pub fn has_session_by_device_id(device_id: &String) -> bool {
            Register::has_session(device_id.as_str())
        }
    }

    pub struct SipRequestOutput<'a> {
        pub device_id: &'a String,
        pub association: Association,
        pub request: Request,
    }

    impl<'a> SipRequestOutput<'a> {
        pub fn new(device_id: &'a String, association: Association, request: Request) -> Self {
            Self {
                device_id,
                association,
                request,
            }
        }

        pub async fn send_log(self, log: &str) {
            let cb = default_log_callback(format!("{}:{}", log, self.device_id));
            let _ = self.send(cb).await;
        }

        pub async fn send(self, cb: Callback) -> GlobalResult<()> {
            let sip_pkg = SipPackage::build_request(self.request, self.association, cb);
            RWContext::get_ctx()
                .sip_tx
                .send(sip_pkg)
                .await
                .hand_log(|msg| error!("{msg}"))
        }
    }

    pub struct DeviceSession {
        pub contact_uri: String,
        pub association: Association,
        pub support_lr: bool,
        pub expires: Duration,
    }

    impl DeviceSession {
        pub fn build(contact_uri: String, association: Association, heartbeat: u8) -> Self {
            let expires = Duration::from_secs(heartbeat as u64 * 3);
            Self {
                contact_uri,
                association,
                support_lr: false,
                expires,
            }
        }

        pub fn enable_lr(&mut self) {
            self.support_lr = true;
        }
    }
}
