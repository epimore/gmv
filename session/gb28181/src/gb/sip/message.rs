use base::bytes::Bytes;
use gmv_pjsip::message::extract_tag;
use gmv_pjsip::{
    SipAssociation, SipMethod, SipRuntimeEvent, SipRuntimeEventKind, SipTransportProtocol,
};

use super::xml;

pub const GB_XML_CONTENT_TYPE: &str = "Application/MANSCDP+xml";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GbMessageKind {
    Keepalive,
    Catalog,
    DeviceInfo,
    Alarm,
    RecordInfo,
    MediaStatus,
    DeviceStatus,
    DeviceControl,
    DeviceConfig,
    ConfigDownload,
    PresetQuery,
    Broadcast,
    PtzPosition,
    CruiseTrackListQuery,
    CruiseTrackQuery,
    UploadSnapshotFinished,
    Notify,
    Options,
    Update,
    Prack,
    Standard,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct GbMessageEvent {
    pub kind: GbMessageKind,
    pub method: Option<SipMethod>,
    pub device_id: Option<String>,
    pub call_id: Option<String>,
    pub cseq: Option<u32>,
    pub association: SipAssociation,
    pub content_type: Option<String>,
    pub event: Option<String>,
    pub from_tag: Option<String>,
    pub to_tag: Option<String>,
    pub subscription_state: Option<String>,
    pub body: Bytes,
    pub xml_cmd_type: Option<String>,
    pub xml_sn: Option<String>,
    pub xml_device_id: Option<String>,
    pub snapshot_session_id: Option<String>,
}

impl GbMessageEvent {
    fn from_parts(
        kind: GbMessageKind,
        method: Option<SipMethod>,
        device_id: Option<String>,
        call_id: Option<String>,
        cseq: Option<u32>,
        association: SipAssociation,
        content_type: Option<String>,
        event: Option<String>,
        from_tag: Option<String>,
        to_tag: Option<String>,
        subscription_state: Option<String>,
        body: Bytes,
        snapshot_session_id_hint: Option<String>,
    ) -> Self {
        let text = String::from_utf8_lossy(&body);
        let xml_cmd_type = xml::cmd_type(&text);
        let xml_sn = xml::sn(&text);
        let xml_device_id = xml::device_id(&text);
        let snapshot_session_id = snapshot_session_id_hint.or_else(|| xml::session_id(&text));
        let kind = match xml_cmd_type.as_deref() {
            Some("Keepalive") => GbMessageKind::Keepalive,
            Some("Catalog") => GbMessageKind::Catalog,
            Some("DeviceInfo") => GbMessageKind::DeviceInfo,
            Some("Alarm") => GbMessageKind::Alarm,
            Some("RecordInfo") => GbMessageKind::RecordInfo,
            Some("MediaStatus") => GbMessageKind::MediaStatus,
            Some("DeviceStatus") => GbMessageKind::DeviceStatus,
            Some("DeviceControl") => GbMessageKind::DeviceControl,
            Some("DeviceConfig") => GbMessageKind::DeviceConfig,
            Some("ConfigDownload") => GbMessageKind::ConfigDownload,
            Some("PresetQuery") => GbMessageKind::PresetQuery,
            Some("Broadcast") => GbMessageKind::Broadcast,
            Some("PTZPosition") => GbMessageKind::PtzPosition,
            Some("CruiseTrackListQuery") => GbMessageKind::CruiseTrackListQuery,
            Some("CruiseTrackQuery") => GbMessageKind::CruiseTrackQuery,
            Some("UploadSnapShotFinished" | "UploadSnapshotFinished") => {
                GbMessageKind::UploadSnapshotFinished
            }
            _ => kind,
        };

        Self {
            kind,
            method,
            device_id: device_id.or_else(|| xml_device_id.clone()),
            call_id,
            cseq,
            association,
            content_type,
            event,
            from_tag,
            to_tag,
            subscription_state,
            body,
            xml_cmd_type,
            xml_sn,
            xml_device_id,
            snapshot_session_id,
        }
    }

    pub fn from_native(event: &SipRuntimeEvent) -> Option<Self> {
        if event.kind != SipRuntimeEventKind::RequestReceived {
            return None;
        }
        let method = SipMethod::parse(event.method.as_deref()?);
        let kind = match method {
            SipMethod::Message => GbMessageKind::Unknown,
            SipMethod::Notify => GbMessageKind::Notify,
            SipMethod::Options => GbMessageKind::Options,
            SipMethod::Update => GbMessageKind::Update,
            SipMethod::Prack => GbMessageKind::Prack,
            _ => return None,
        };
        let association = SipAssociation {
            local_addr: event.local_addr?,
            remote_addr: event.remote_addr?,
            protocol: event.protocol?,
        };
        Some(Self::from_parts(
            kind,
            Some(method),
            None,
            event.call_id.clone(),
            event.cseq,
            association,
            event.content_type.clone(),
            event.event.clone(),
            event.from_header.as_deref().and_then(extract_tag),
            event.to_header.as_deref().and_then(extract_tag),
            event.subscription_state.clone(),
            Bytes::copy_from_slice(&event.body),
            None,
        ))
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
        target_uri(
            &self.device_id,
            &self.device_host,
            self.device_port,
            self.protocol,
        )
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

    pub fn device_status_query(
        device_id: impl Into<String>,
        device_host: impl Into<String>,
        device_port: u16,
        protocol: SipTransportProtocol,
        sn: u32,
    ) -> Self {
        let device_id = device_id.into();
        let body = xml::build_device_status_query(sn, &device_id);
        Self::xml(device_id, device_host, device_port, protocol, body)
    }

    pub fn config_download_query(
        device_id: impl Into<String>,
        device_host: impl Into<String>,
        device_port: u16,
        protocol: SipTransportProtocol,
        sn: u32,
        config_type: &str,
    ) -> Self {
        let device_id = device_id.into();
        let body = xml::build_config_download_query(sn, &device_id, config_type);
        Self::xml(device_id, device_host, device_port, protocol, body)
    }

    pub fn ptz_position_query(
        device_id: impl Into<String>,
        device_host: impl Into<String>,
        device_port: u16,
        protocol: SipTransportProtocol,
        sn: u32,
    ) -> Self {
        let device_id = device_id.into();
        let body = xml::build_ptz_position_query(sn, &device_id);
        Self::xml(device_id, device_host, device_port, protocol, body)
    }

    pub fn cruise_track_list_query(
        device_id: impl Into<String>,
        device_host: impl Into<String>,
        device_port: u16,
        protocol: SipTransportProtocol,
        sn: u32,
    ) -> Self {
        let device_id = device_id.into();
        let body = xml::build_cruise_track_list_query(sn, &device_id);
        Self::xml(device_id, device_host, device_port, protocol, body)
    }

    pub fn cruise_track_query(
        device_id: impl Into<String>,
        device_host: impl Into<String>,
        device_port: u16,
        protocol: SipTransportProtocol,
        sn: u32,
        number: u32,
    ) -> Self {
        let device_id = device_id.into();
        let body = xml::build_cruise_track_query(sn, &device_id, number);
        Self::xml(device_id, device_host, device_port, protocol, body)
    }

    pub fn broadcast_notify(
        target_id: impl Into<String>,
        device_host: impl Into<String>,
        device_port: u16,
        protocol: SipTransportProtocol,
        sn: u32,
        source_id: &str,
    ) -> Self {
        let target_id = target_id.into();
        let body = xml::build_broadcast_notify(sn, source_id, &target_id);
        Self::xml(target_id, device_host, device_port, protocol, body)
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

    pub fn preset_query(
        device_id: impl Into<String>,
        device_host: impl Into<String>,
        device_port: u16,
        protocol: SipTransportProtocol,
    ) -> Self {
        let device_id = device_id.into();
        let body = xml::build_preset_query_xml(&device_id);
        Self::xml(device_id, device_host, device_port, protocol, body)
    }

    pub fn snapshot_control(
        device_id: impl Into<String>,
        channel_id: &str,
        device_host: impl Into<String>,
        device_port: u16,
        protocol: SipTransportProtocol,
        count: u8,
        interval: u8,
        url: &str,
        session_id: &str,
    ) -> Self {
        let body = xml::build_snapshot_control_xml(channel_id, count, interval, url, session_id);
        Self::xml(device_id, device_host, device_port, protocol, body)
    }

    pub fn xml(
        device_id: impl Into<String>,
        device_host: impl Into<String>,
        device_port: u16,
        protocol: SipTransportProtocol,
        body: impl Into<String>,
    ) -> Self {
        let body = body.into();
        Self {
            device_id: device_id.into(),
            device_host: device_host.into(),
            device_port,
            protocol,
            body: xml::encode_document(&body),
            content_type: GB_XML_CONTENT_TYPE.to_string(),
            call_id: None,
            cseq: None,
        }
    }
}

pub fn target_uri(
    device_id: &str,
    host: &str,
    port: u16,
    protocol: SipTransportProtocol,
) -> String {
    match protocol {
        SipTransportProtocol::Udp => format!("sip:{device_id}@{host}:{port}"),
        SipTransportProtocol::Tcp => format!("sip:{device_id}@{host}:{port};transport=tcp"),
        SipTransportProtocol::Tls => format!("sips:{device_id}@{host}:{port};transport=tls"),
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use base::bytes::Bytes;
    use gmv_pjsip::{SipAssociation, SipMethod, SipTransportProtocol};

    use super::{GB_XML_CONTENT_TYPE, GbMessageEvent, GbMessageKind};

    fn association() -> SipAssociation {
        SipAssociation {
            local_addr: "192.0.2.10:5060".parse::<SocketAddr>().unwrap(),
            remote_addr: "198.51.100.20:5060".parse::<SocketAddr>().unwrap(),
            protocol: SipTransportProtocol::Udp,
        }
    }

    fn classify(cmd_type: &str) -> GbMessageKind {
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"GB2312\"?>\r\n\
<Response>\r\n\
<CmdType>{cmd_type}</CmdType>\r\n\
<SN>1</SN>\r\n\
<DeviceID>34020000001320000001</DeviceID>\r\n\
</Response>\r\n"
        );
        GbMessageEvent::from_parts(
            GbMessageKind::Unknown,
            Some(SipMethod::Message),
            None,
            Some("message-classify".into()),
            Some(1),
            association(),
            Some(GB_XML_CONTENT_TYPE.into()),
            None,
            None,
            None,
            None,
            Bytes::from(body),
            None,
        )
        .kind
    }

    #[test]
    fn classifies_reference_cmd_types() {
        for (cmd_type, expected) in [
            ("DeviceStatus", GbMessageKind::DeviceStatus),
            ("Broadcast", GbMessageKind::Broadcast),
            ("ConfigDownload", GbMessageKind::ConfigDownload),
            ("PTZPosition", GbMessageKind::PtzPosition),
            ("CruiseTrackListQuery", GbMessageKind::CruiseTrackListQuery),
            ("CruiseTrackQuery", GbMessageKind::CruiseTrackQuery),
            (
                "UploadSnapShotFinished",
                GbMessageKind::UploadSnapshotFinished,
            ),
        ] {
            assert_eq!(classify(cmd_type), expected, "{cmd_type}");
        }
    }
}
