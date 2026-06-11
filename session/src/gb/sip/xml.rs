//! Small GB28181 XML helpers.
//!
//! This module is intentionally dependency-free so it can be dropped into the
//! existing `session` crate without introducing a parser dependency. If the
//! session crate already uses `quick-xml`/`roxmltree`, replace these helpers
//! with the project-wide XML parser.

use anyhow::anyhow;
use base::exception::GlobalError::SysErr;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use encoding_rs::{Encoding, GB18030, GBK, UTF_8};
use quick_xml::Reader;
use quick_xml::events::Event;

#[derive(Clone, Copy)]
enum XmlEncoding {
    Utf8,
    Gb2312,
    Gb18030,
}

impl XmlEncoding {
    fn encoding(self) -> &'static Encoding {
        match self {
            Self::Utf8 => UTF_8,
            Self::Gb2312 => GBK,
            Self::Gb18030 => GB18030,
        }
    }
}

fn detect_encoding(xml: &[u8]) -> XmlEncoding {
    let declaration = String::from_utf8_lossy(&xml[..xml.len().min(256)]).to_ascii_uppercase();
    if declaration.contains("GB18030") {
        XmlEncoding::Gb18030
    } else if declaration.contains("GB2312") || declaration.contains("GBK") {
        XmlEncoding::Gb2312
    } else if std::str::from_utf8(xml).is_ok() {
        XmlEncoding::Utf8
    } else {
        XmlEncoding::Gb18030
    }
}

pub fn parse_items(xml: &[u8]) -> GlobalResult<Vec<(String, String)>> {
    let encoding = detect_encoding(xml);
    let (decoded, _, had_errors) = encoding.encoding().decode(xml);
    if had_errors {
        return Err(SysErr(anyhow!("invalid GB28181 XML encoding")));
    }

    let mut reader = Reader::from_str(&decoded);
    reader.trim_text(true);
    reader.expand_empty_elements(true);
    let mut path = Vec::<String>::new();
    let mut items = Vec::<(String, String)>::new();
    let mut last_depth = 0usize;

    loop {
        match reader.read_event() {
            Ok(Event::Start(tag)) => {
                let tag_name = tag.name();
                let name =
                    std::str::from_utf8(tag_name.as_ref()).hand_log(|msg| error!("{msg}"))?;
                path.push(name.to_string());
            }
            Ok(Event::Text(text)) => {
                let value = text
                    .unescape()
                    .hand_log(|msg| error!("{msg}"))?
                    .trim()
                    .to_string();
                if !value.is_empty() {
                    if last_depth != path.len() {
                        last_depth = path.len();
                        items.push((SPLIT_CLASS.to_string(), last_depth.to_string()));
                    }
                    items.push((path.join(","), value));
                }
            }
            Ok(Event::End(_)) => {
                path.pop();
            }
            Ok(Event::Eof) => break,
            Err(err) => {
                return Err(SysErr(anyhow!(
                    "invalid GB28181 XML at position {}: {err}",
                    reader.buffer_position()
                )));
            }
            _ => {}
        }
    }
    Ok(items)
}

pub fn value<'a>(items: &'a [(String, String)], key: &str) -> Option<&'a str> {
    items
        .iter()
        .find_map(|(item_key, value)| (item_key == key).then_some(value.as_str()))
}

pub fn tag_value(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(xml[start..end].trim().to_string())
}

pub fn cmd_type(xml: &str) -> Option<String> {
    tag_value(xml, "CmdType")
}

pub fn sn(xml: &str) -> Option<String> {
    tag_value(xml, "SN")
}

pub fn device_id(xml: &str) -> Option<String> {
    tag_value(xml, "DeviceID")
}

pub fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn build_catalog_query(sn: u32, device_id: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"GB2312\"?>\r\n\
<Query>\r\n\
<CmdType>Catalog</CmdType>\r\n\
<SN>{}</SN>\r\n\
<DeviceID>{}</DeviceID>\r\n\
</Query>\r\n",
        sn,
        escape(device_id)
    )
}

pub fn build_device_info_query(sn: u32, device_id: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"GB2312\"?>\r\n\
<Query>\r\n\
<CmdType>DeviceInfo</CmdType>\r\n\
<SN>{}</SN>\r\n\
<DeviceID>{}</DeviceID>\r\n\
</Query>\r\n",
        sn,
        escape(device_id)
    )
}

pub fn build_record_info_query(
    sn: u32,
    device_id: &str,
    start_time: &str,
    end_time: &str,
) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"GB2312\"?>\r\n\
<Query>\r\n\
<CmdType>RecordInfo</CmdType>\r\n\
<SN>{}</SN>\r\n\
<DeviceID>{}</DeviceID>\r\n\
<StartTime>{}</StartTime>\r\n\
<EndTime>{}</EndTime>\r\n\
<Secrecy>0</Secrecy>\r\n\
<Type>all</Type>\r\n\
</Query>\r\n",
        sn,
        escape(device_id),
        escape(start_time),
        escape(end_time)
    )
}

// Legacy parser constants moved here so business models no longer depend on
// gb::handler::parser::xml / rsip.
pub const NOTIFY_DEVICE_ID: &str = "Notify,DeviceID";
pub const NOTIFY_TYPE: &str = "Notify,NotifyType";
pub const NOTIFY_ALARM_PRIORITY: &str = "Notify,AlarmPriority";
pub const NOTIFY_ALARM_TIME: &str = "Notify,AlarmTime";
pub const NOTIFY_ALARM_METHOD: &str = "Notify,AlarmMethod";
pub const NOTIFY_INFO_ALARM_TYPE: &str = "Notify,Info,AlarmType";

pub trait KV2Model: Sized {
    fn kv_to_model(arr: Vec<(String, String)>) -> base::exception::GlobalResult<Self>;
}

pub fn build_ptz_control(sn: u32, device_id: &str, command: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"GB2312\"?>\r\n\
<Control>\r\n\
<CmdType>DeviceControl</CmdType>\r\n\
<SN>{}</SN>\r\n\
<DeviceID>{}</DeviceID>\r\n\
<PTZCmd>{}</PTZCmd>\r\n\
<Info>\r\n\
<ControlPriority>5</ControlPriority>\r\n\
</Info>\r\n\
</Control>\r\n",
        sn,
        escape(device_id),
        escape(command),
    )
}

pub fn build_snapshot(
    sn: u32,
    device_id: &str,
    count: u8,
    interval: u8,
    url: &str,
    session_id: &str,
) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"GB2312\"?>\r\n\
<Control>\r\n\
<CmdType>DeviceControl</CmdType>\r\n\
<SN>{}</SN>\r\n\
<DeviceID>{}</DeviceID>\r\n\
<TeleBoot>Boot</TeleBoot>\r\n\
<Info>\r\n\
<ControlPriority>5</ControlPriority>\r\n\
<SnapNum>{}</SnapNum>\r\n\
<Interval>{}</Interval>\r\n\
<UploadURL>{}</UploadURL>\r\n\
<SessionID>{}</SessionID>\r\n\
</Info>\r\n\
</Control>\r\n",
        sn,
        escape(device_id),
        count,
        interval,
        escape(url),
        escape(session_id),
    )
}

pub const RESPONSE_DEVICE_ID: &str = "Response,DeviceID";
pub const RESPONSE_MANUFACTURER: &str = "Response,Manufacturer";
pub const RESPONSE_MODEL: &str = "Response,Model";
pub const RESPONSE_FIRMWARE: &str = "Response,Firmware";
pub const RESPONSE_DEVICE_TYPE: &str = "Response,DeviceType";
pub const RESPONSE_MAX_CAMERA: &str = "Response,MaxCamera";

pub const RESPONSE_DEVICE_LIST_ITEM_DEVICE_ID: &str = "Response,DeviceList,Item,DeviceID";
pub const RESPONSE_DEVICE_LIST_ITEM_NAME: &str = "Response,DeviceList,Item,Name";
pub const RESPONSE_DEVICE_LIST_ITEM_MANUFACTURER: &str = "Response,DeviceList,Item,Manufacturer";
pub const RESPONSE_DEVICE_LIST_ITEM_MODEL: &str = "Response,DeviceList,Item,Model";
pub const RESPONSE_DEVICE_LIST_ITEM_OWNER: &str = "Response,DeviceList,Item,Owner";
pub const RESPONSE_DEVICE_LIST_ITEM_CIVIL_CODE: &str = "Response,DeviceList,Item,CivilCode";
pub const RESPONSE_DEVICE_LIST_ITEM_BLOCK: &str = "Response,DeviceList,Item,Block";
pub const RESPONSE_DEVICE_LIST_ITEM_ADDRESS: &str = "Response,DeviceList,Item,Address";
pub const RESPONSE_DEVICE_LIST_ITEM_PARENTAL: &str = "Response,DeviceList,Item,Parental";
pub const RESPONSE_DEVICE_LIST_ITEM_PARENT_ID: &str = "Response,DeviceList,Item,ParentID";
pub const RESPONSE_DEVICE_LIST_ITEM_LONGITUDE: &str = "Response,DeviceList,Item,Longitude";
pub const RESPONSE_DEVICE_LIST_ITEM_LATITUDE: &str = "Response,DeviceList,Item,Latitude";
pub const RESPONSE_DEVICE_LIST_ITEM_PTZ_TYPE: &str = "Response,DeviceList,Item,Info,PTZType";
pub const RESPONSE_DEVICE_LIST_ITEM_SUPPLY_LIGHT_TYPE: &str =
    "Response,DeviceList,Item,SupplyLightType";
pub const RESPONSE_DEVICE_LIST_ITEM_IP_ADDRESS: &str = "Response,DeviceList,Item,IPAddress";
pub const RESPONSE_DEVICE_LIST_ITEM_PORT: &str = "Response,DeviceList,Item,Port";
pub const RESPONSE_DEVICE_LIST_ITEM_PASSWORD: &str = "Response,DeviceList,Item,Password";
pub const RESPONSE_DEVICE_LIST_ITEM_STATUS: &str = "Response,DeviceList,Item,Status";
pub const SPLIT_CLASS: &str = "?<-0_0->?";

pub fn session_id(xml: &str) -> Option<String> {
    tag_value(xml, "SessionID")
}

pub fn build_preset_query_xml(device_id: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"GB2312\"?>\r\n\
<Query>\r\n\
<CmdType>PresetQuery</CmdType>\r\n\
<SN>1</SN>\r\n\
<DeviceID>{}</DeviceID>\r\n\
</Query>\r\n",
        escape(device_id)
    )
}

pub fn build_snapshot_control_xml(
    channel_id: &str,
    count: u8,
    interval: u8,
    url: &str,
    session_id: &str,
) -> String {
    format!(
        "<?xml version=\"1.0\"?>\r\n\
<Control>\r\n\
<CmdType>DeviceConfig</CmdType>\r\n\
<SN>1</SN>\r\n\
<DeviceID>{}</DeviceID>\r\n\
<SnapShotConfig>\r\n\
<SnapNum>{}</SnapNum>\r\n\
<Interval>{}</Interval>\r\n\
<UploadURL>{}</UploadURL>\r\n\
<SessionID>{}</SessionID>\r\n\
</SnapShotConfig>\r\n\
</Control>\r\n",
        escape(channel_id),
        count,
        interval,
        escape(url),
        escape(session_id),
    )
}

#[cfg(test)]
mod tests {
    use super::{RESPONSE_DEVICE_LIST_ITEM_DEVICE_ID, SPLIT_CLASS, parse_items};
    use encoding_rs::GBK;

    #[test]
    fn parses_gb2312_catalog_items_with_paths() {
        let xml = "<?xml version=\"1.0\" encoding=\"GB2312\"?>\
            <Response><CmdType>Catalog</CmdType><DeviceList>\
            <Item><DeviceID>34020000001320000001</DeviceID><Name>摄像机一</Name></Item>\
            <Item><DeviceID>34020000001320000002</DeviceID><Name>摄像机二</Name></Item>\
            </DeviceList></Response>";
        let (bytes, _, had_errors) = GBK.encode(xml);
        assert!(!had_errors);

        let items = parse_items(&bytes).unwrap();
        assert_eq!(
            items
                .iter()
                .filter(|(key, _)| key == RESPONSE_DEVICE_LIST_ITEM_DEVICE_ID)
                .count(),
            2
        );
        assert!(items.iter().any(|(key, _)| key == SPLIT_CLASS));
    }
}
