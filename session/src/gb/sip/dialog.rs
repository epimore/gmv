//! Session-side dialog mapping helpers.
//!
//! SIP dialog state is owned by `gmv_pjsip::SipContext`.
//! This file should only hold GMV business mappings, for example:
//! - `stream_id -> call_id`
//! - `call_id -> stream_id`
//! - `device_id/channel_id -> active stream_id`
//!
//! Do not manually generate Call-ID, CSeq, From tag, To tag, or Via branch here.

use std::collections::HashMap;

#[derive(Clone, Debug, Default)]
pub struct GbDialogIndex {
    stream_to_call: HashMap<String, String>,
    call_to_stream: HashMap<String, String>,
}

impl GbDialogIndex {
    pub fn insert(&mut self, stream_id: impl Into<String>, call_id: impl Into<String>) {
        let stream_id = stream_id.into();
        let call_id = call_id.into();
        self.stream_to_call.insert(stream_id.clone(), call_id.clone());
        self.call_to_stream.insert(call_id, stream_id);
    }

    pub fn call_id_by_stream(&self, stream_id: &str) -> Option<&str> {
        self.stream_to_call.get(stream_id).map(String::as_str)
    }

    pub fn stream_id_by_call(&self, call_id: &str) -> Option<&str> {
        self.call_to_stream.get(call_id).map(String::as_str)
    }

    pub fn remove_by_call(&mut self, call_id: &str) -> Option<String> {
        let stream_id = self.call_to_stream.remove(call_id)?;
        self.stream_to_call.remove(&stream_id);
        Some(stream_id)
    }

    pub fn remove_by_stream(&mut self, stream_id: &str) -> Option<String> {
        let call_id = self.stream_to_call.remove(stream_id)?;
        self.call_to_stream.remove(&call_id);
        Some(call_id)
    }
}
