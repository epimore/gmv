pub mod rw {
    use std::sync::OnceLock;

    use base::bytes::Bytes;
    use base::exception::{GlobalResult, GlobalResultExt};
    use base::log::error;
    use base::net::state::{Association, Package, Zip};
    use base::tokio::sync::mpsc::Sender;

    use crate::register::core::Register;

    static RW_CTX: OnceLock<RWContext> = OnceLock::new();

    /// Network writer facade used by business code.
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

        pub fn clean_rw_session_by_bill(bill: &Association) {
            Register::detach_device_association(bill);
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
    }
}
