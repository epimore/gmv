use base::bytes::Bytes;
use gmv_pjsip::{CreateMessage, MessageEvent, MessageKind, SipAssociation, SipContext, SipTransportProtocol};

use super::xml;

pub const GB_XML_CONTENT_TYPE: &str = "Application/MANSCDP+xml";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GbMessageKind {
    Keepalive,
    Catalog,
    DeviceInfo,
    Alarm,
    RecordInfo,
    Unknown,
}

impl From<MessageKind> for GbMessageKind {
    fn from(kind: MessageKind) -> Self {
        match kind {
            MessageKind::Keepalive => Self::Keepalive,
            MessageKind::Catalog => Self::Catalog,
            MessageKind::DeviceInfo => Self::DeviceInfo,
            MessageKind::Alarm => Self::Alarm,
            MessageKind::Unknown => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GbMessageEvent {
    pub kind: GbMessageKind,
    pub device_id: Option<String>,
    pub call_id: Option<String>,
    pub cseq: Option<u32>,
    pub association: SipAssociation,
    pub content_type: Option<String>,
    pub body: Bytes,
    pub xml_cmd_type: Option<String>,
    pub xml_sn: Option<String>,
    pub xml_device_id: Option<String>,
}

impl From<MessageEvent> for GbMessageEvent {
    fn from(event: MessageEvent) -> Self {
        let text = String::from_utf8_lossy(&event.body);
        let xml_cmd_type = xml::cmd_type(&text);
        let xml_sn = xml::sn(&text);
        let xml_device_id = xml::device_id(&text);
        let kind = match xml_cmd_type.as_deref() {
            Some("Keepalive") => GbMessageKind::Keepalive,
            Some("Catalog") => GbMessageKind::Catalog,
            Some("DeviceInfo") => GbMessageKind::DeviceInfo,
            Some("Alarm") => GbMessageKind::Alarm,
            Some("RecordInfo") => GbMessageKind::RecordInfo,
            _ => GbMessageKind::from(event.kind),
        };

        Self {
            kind,
            device_id: event.device_id.or_else(|| xml_device_id.clone()),
            call_id: event.call_id,
            cseq: event.cseq,
            association: event.association,
            content_type: event.content_type,
            body: event.body,
            xml_cmd_type,
            xml_sn,
            xml_device_id,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CreateDeviceMessageRequest {
    pub device_id: String,
    pub device_host: String,
    pub device_port: u16,
    pub protocol: SipTransportProtocol,
    pub body: Bytes,
    pub content_type: String,
    pub call_id: Option<String>,
    pub cseq: Option<u32>,
}

impl CreateDeviceMessageRequest {
    pub fn target_uri(&self) -> String {
        target_uri(&self.device_id, &self.device_host, self.device_port, self.protocol)
    }

    pub fn catalog_query(
        device_id: impl Into<String>,
        device_host: impl Into<String>,
        device_port: u16,
        protocol: SipTransportProtocol,
        sn: u32,
    ) -> Self {
        let device_id = device_id.into();
        let body = xml::build_catalog_query(sn, &device_id);
        Self::xml(device_id, device_host, device_port, protocol, body)
    }

    pub fn device_info_query(
        device_id: impl Into<String>,
        device_host: impl Into<String>,
        device_port: u16,
        protocol: SipTransportProtocol,
        sn: u32,
    ) -> Self {
        let device_id = device_id.into();
        let body = xml::build_device_info_query(sn, &device_id);
        Self::xml(device_id, device_host, device_port, protocol, body)
    }

    pub fn record_info_query(
        device_id: impl Into<String>,
        device_host: impl Into<String>,
        device_port: u16,
        protocol: SipTransportProtocol,
        sn: u32,
        start_time: &str,
        end_time: &str,
    ) -> Self {
        let device_id = device_id.into();
        let body = xml::build_record_info_query(sn, &device_id, start_time, end_time);
        Self::xml(device_id, device_host, device_port, protocol, body)
    }

    pub fn xml(
        device_id: impl Into<String>,
        device_host: impl Into<String>,
        device_port: u16,
        protocol: SipTransportProtocol,
        body: impl Into<String>,
    ) -> Self {
        Self {
            device_id: device_id.into(),
            device_host: device_host.into(),
            device_port,
            protocol,
            body: Bytes::from(body.into().into_bytes()),
            content_type: GB_XML_CONTENT_TYPE.to_string(),
            call_id: None,
            cseq: None,
        }
    }
}

pub fn create_device_message(ctx: &SipContext, req: CreateDeviceMessageRequest) -> gmv_pjsip::Result<Bytes> {
    ctx.create_message(CreateMessage {
        target_uri: req.target_uri(),
        body: req.body,
        content_type: req.content_type,
        protocol: req.protocol,
        call_id: req.call_id,
        cseq: req.cseq,
    })
}

pub fn target_uri(device_id: &str, host: &str, port: u16, protocol: SipTransportProtocol) -> String {
    match protocol {
        SipTransportProtocol::Udp => format!("sip:{device_id}@{host}:{port}"),
        SipTransportProtocol::Tcp => format!("sip:{device_id}@{host}:{port};transport=tcp"),
        SipTransportProtocol::Tls => format!("sips:{device_id}@{host}:{port};transport=tls"),
    }
}
