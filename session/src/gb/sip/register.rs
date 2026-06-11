use gmv_pjsip::{RegisterEvent, SipAssociation};

#[derive(Clone, Debug)]
pub struct GbRegisterEvent {
    pub device_id: String,
    pub contact: Option<String>,
    pub support_lr: bool,
    pub expires: u32,
    pub call_id: String,
    pub cseq: u32,
    pub authorized: bool,
    pub username: Option<String>,
    pub association: SipAssociation,
    pub user_agent: Option<String>,
    pub gb_version: Option<String>,
}

impl From<RegisterEvent> for GbRegisterEvent {
    fn from(event: RegisterEvent) -> Self {
        Self {
            device_id: event.device_id,
            contact: event.contact,
            support_lr: event.support_lr,
            expires: event.expires,
            call_id: event.call_id,
            cseq: event.cseq,
            authorized: event.authorized,
            username: event.username,
            association: event.association,
            user_agent: event.user_agent,
            gb_version: event.gb_version,
        }
    }
}

impl GbRegisterEvent {
    pub fn is_unregister(&self) -> bool {
        self.expires == 0
    }

    pub fn is_register(&self) -> bool {
        self.expires != 0
    }
}
