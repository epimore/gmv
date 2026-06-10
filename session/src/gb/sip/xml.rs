//! Small GB28181 XML helpers.
//!
//! This module is intentionally dependency-free so it can be dropped into the
//! existing `session` crate without introducing a parser dependency. If the
//! session crate already uses `quick-xml`/`roxmltree`, replace these helpers
//! with the project-wide XML parser.

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
