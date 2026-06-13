#[derive(Clone, Debug)]
pub struct GbByeEvent {
    pub call_id: String,
    pub stream_id: Option<String>,
    pub device_id: Option<String>,
}
