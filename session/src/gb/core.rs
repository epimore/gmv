pub mod rw {
    use std::sync::{Arc, OnceLock};
    use std::time::Duration;

    use base::bytes::Bytes;
    use base::exception::{GlobalResult, GlobalResultExt};
    use base::log::error;
    use base::net::state::{Association, Package, Zip};
    use base::tokio::sync::mpsc::Sender;

    use crate::register::core::{DeviceSession, Register};

    static RW_CTX: OnceLock<RWContext> = OnceLock::new();

    /// Network writer facade used by business code.
    ///
    /// The old implementation also owned a `SipPackage` channel used by the
    /// rsip transaction layer. The PJSIP mid-term architecture removes that
    /// channel completely: all SIP bytes are produced by `gb::sip`/`gmv_pjsip`
    /// and sent directly to the socket writer.
    pub struct RWContext {
        io_tx: Sender<Zip>,
    }

    impl RWContext {
        pub fn get_ctx() -> &'static RWContext {
            RW_CTX.get().expect("RWContext not initialized")
        }

        pub fn init(io_tx: Sender<Zip>) {
            let _ = RW_CTX.set(RWContext { io_tx });
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

        pub async fn send_sip_bytes(association: Association, bytes: Bytes) -> GlobalResult<()> {
            RWContext::get_ctx()
                .io_tx
                .send(Zip::build_data(Package::new(association, bytes)))
                .await
                .hand_log(|msg| error!("{msg}"))
        }

        pub fn try_send_sip_bytes(association: Association, bytes: Bytes) -> GlobalResult<()> {
            RWContext::get_ctx()
                .io_tx
                .try_send(Zip::build_data(Package::new(association, bytes)))
                .hand_log(|msg| error!("{msg}"))
        }
    }
}
