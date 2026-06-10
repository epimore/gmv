use gmv_pjsip::{RegisterEvent, SipAssociation};

#[derive(Clone, Debug)]
pub struct GbRegisterEvent {
    pub device_id: String,
    pub contact: Option<String>,
    pub expires: u32,
    pub authorized: bool,
    pub username: Option<String>,
    pub association: SipAssociation,
    pub user_agent: Option<String>,
}

impl From<RegisterEvent> for GbRegisterEvent {
    fn from(event: RegisterEvent) -> Self {
        Self {
            device_id: event.device_id,
            contact: event.contact,
            expires: event.expires,
            authorized: event.authorized,
            username: event.username,
            association: event.association,
            user_agent: event.user_agent,
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
