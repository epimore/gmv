pub mod rw {
    use base::net::state::Association;

    use crate::register::core::Register;

    pub struct RWContext;

    impl RWContext {
        pub fn clean_rw_session_by_bill(bill: &Association) {
            Register::detach_device_association(bill);
        }

        pub fn has_session_by_device_id(device_id: &String) -> bool {
            Register::has_session(device_id.as_str())
        }
    }
}
