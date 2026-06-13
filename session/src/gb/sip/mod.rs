//! GB28181 business adapter for the single native `gmv_pjsip` runtime.

pub mod adapter;
pub mod auth;
pub mod bye;
pub mod command;
pub mod invite;
pub mod message;
pub mod native_runtime;
pub mod register;
pub mod runtime_cache;
pub mod sdp;
pub mod subscription;
pub mod xml;

pub use adapter::{
    GbSipEvent, apply_business_event, base_association_from_pjsip, base_protocol_from_pjsip,
    pjsip_protocol_from_base,
};
pub use bye::GbByeEvent;
pub use invite::{
    GbIncomingInviteEvent, GbInviteAcceptedEvent, InvitePlayRequest, InviteStopRequest,
    InviteTalkRequest,
};
pub use message::{CreateDeviceMessageRequest, GbMessageEvent, GbMessageKind};
pub use native_runtime::{NativeSipRuntimeHandle, NativeSipRuntimeService};
pub use register::GbRegisterEvent;

/// Periodically clean session-level business waiters.
pub async fn run_cleanup_task(cancel_token: base::tokio_util::sync::CancellationToken) {
    use std::time::Duration;

    use base::log::debug;
    use base::tokio::time;

    let mut ticker = time::interval(Duration::from_secs(1));
    loop {
        base::tokio::select! {
            _ = ticker.tick() => {
                let report = runtime_cache::SipRuntimeCache::global().cleanup_expired();
                if report.invite_waiters > 0
                    || report.bye_waiters > 0
                    || report.response_waiters > 0
                    || report.native_response_waiters > 0
                {
                    debug!("sip business waiter cleanup: {report:?}");
                }
            }
            _ = cancel_token.cancelled() => break,
        }
    }
}
