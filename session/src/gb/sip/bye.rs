use gmv_pjsip::ByeEvent;

#[derive(Clone, Debug)]
pub struct GbByeEvent {
    pub call_id: String,
    pub stream_id: Option<String>,
    pub device_id: Option<String>,
}

impl From<ByeEvent> for GbByeEvent {
    fn from(event: ByeEvent) -> Self {
        Self {
            call_id: event.call_id,
            stream_id: event.stream_id,
            device_id: event.device_id,
        }
    }
}
