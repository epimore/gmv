//! LEGACY RSIP PIPELINE
//!
//! The medium-term SIP stack has moved to `crate::gb::sip` + `gmv_pjsip`.
//! This file is kept temporarily for compatibility with existing service APIs
//! and for migration reference. New code must not add SIP parsing, transaction,
//! dialog, CSeq/tag/branch, or header-generation logic here.
//!

mod builder;
pub mod cmd;
pub mod parser;
pub mod requester;
