use gmv_pjsip::{RegisterEvent, SipAssociation, SipRuntimeEvent, SipRuntimeEventKind};

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
    pub fn from_native(event: &SipRuntimeEvent) -> Option<Self> {
        if !matches!(
            event.kind,
            SipRuntimeEventKind::Registered | SipRuntimeEventKind::Unregistered
        ) {
            return None;
        }
        let device_id = event.device_id.clone()?;
        let association = SipAssociation {
            local_addr: event.local_addr?,
            remote_addr: event.remote_addr?,
            protocol: event.protocol?,
        };
        Some(Self {
            device_id: device_id.clone(),
            contact: event.contact.clone(),
            support_lr: event.contact.as_deref().is_some_and(|contact| {
                contact
                    .split(';')
                    .skip(1)
                    .any(|parameter| parameter.eq_ignore_ascii_case("lr"))
            }),
            expires: event.expires_seconds.unwrap_or_default(),
            call_id: event.call_id.clone()?,
            cseq: event.cseq?,
            authorized: true,
            username: Some(device_id),
            association,
            user_agent: event.user_agent.clone(),
            gb_version: event.gb_version.clone(),
        })
    }

    pub fn is_unregister(&self) -> bool {
        self.expires == 0
    }

    pub fn is_register(&self) -> bool {
        self.expires != 0
    }
}
