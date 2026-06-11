//! GB28181 SIP session adapter.
//!
//! Boundary:
//! - `gmv_pjsip` owns SIP parsing, transaction cache, REGISTER/MESSAGE/INVITE/BYE
//!   context, dialog state, CSeq/tag/branch generation, and response/request bytes.
//! - `session/src/gb/sip` adapts those protocol events to GMV business code.
//! - Do not manually generate Via/From/To/Call-ID/CSeq/Contact headers here.
//!
//! Typical IO flow:
//!
//! ```text
//! io.rs receives SIP bytes
//!   -> GbSipRuntime::on_bytes()
//!   -> gmv_pjsip::SipContext
//!   -> GbSipRuntimeOutput { sends, event }
//!   -> io.rs sends all `sends`
//!   -> session handles `event`
//! ```

pub mod adapter;
pub mod auth;
pub mod bye;
pub mod command;
pub mod dialog;
pub mod invite;
pub mod message;
pub mod register;
pub mod runtime_cache;
pub mod sdp;
pub mod subscription;
pub mod xml;

pub use adapter::{
    GbSipConfig, GbSipEvent, GbSipRuntime, GbSipRuntimeOutput, apply_business_event,
    base_association_from_pjsip, base_protocol_from_pjsip, pjsip_association_from_base,
    pjsip_protocol_from_base,
};
pub use bye::GbByeEvent;
pub use invite::{
    GbIncomingInviteEvent, GbInviteAcceptedEvent, InvitePlayRequest, InviteStopRequest,
    InviteTalkRequest,
};
pub use message::{CreateDeviceMessageRequest, GbMessageEvent, GbMessageKind};
pub use register::GbRegisterEvent;

/// Periodically clean PJSIP protocol caches and session-level SIP waiters.
///
/// This replaces the old rsip transaction timeout path. Device heartbeat /
/// registration timers still live in register::TimeScheduler because they are
/// business state, not SIP protocol transactions.
pub async fn run_cleanup_task(cancel_token: base::tokio_util::sync::CancellationToken) {
    use std::time::Duration;

    use base::log::debug;
    use base::tokio::time;

    let mut ticker = time::interval(Duration::from_secs(1));
    loop {
        base::tokio::select! {
            _ = ticker.tick() => {
                if let Some(runtime) = GbSipRuntime::global() {
                    let report = runtime.cleanup_expired_with(Duration::from_secs(32));
                    let waiter_report = runtime_cache::SipRuntimeCache::global().cleanup_expired();
                    if report != gmv_pjsip::CleanupReport::default()
                        || waiter_report.invite_waiters > 0
                        || waiter_report.bye_waiters > 0
                        || waiter_report.response_waiters > 0
                    {
                        debug!(
                            "sip cleanup: pjsip={:?}, session_waiters={:?}",
                            report, waiter_report
                        );
                    }
                }
            }
            _ = cancel_token.cancelled() => break,
        }
    }
}
