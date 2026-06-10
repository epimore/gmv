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
pub mod bye;
pub mod dialog;
pub mod invite;
pub mod message;
pub mod register;
pub mod sdp;
pub mod xml;

pub use adapter::{GbSipConfig, GbSipRuntime, GbSipRuntimeOutput};
pub use bye::GbByeEvent;
pub use invite::{GbIncomingInviteEvent, GbInviteAcceptedEvent, InvitePlayRequest, InviteStopRequest};
pub use message::{CreateDeviceMessageRequest, GbMessageEvent, GbMessageKind};
pub use register::GbRegisterEvent;
