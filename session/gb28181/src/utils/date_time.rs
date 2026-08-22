use base::chrono;
use chrono::{DateTime, Local, TimeZone, Utc};

pub struct TimeFormatter;

impl TimeFormatter {
    pub fn local_time_ios_format(date_time: DateTime<Local>) -> String {
        date_time.format("%Y-%m-%dT%H:%M:%S").to_string()
    }

    pub fn local_time_format(date_time: DateTime<Local>, fmt: &str) -> String {
        date_time.format(fmt).to_string()
    }

    pub fn utc_time_format(date_time: DateTime<Utc>, fmt: &str) -> String {
        date_time.format(fmt).to_string()
    }

    /// 获取当前时间并格式化为 "2010-11-11T00:00:00" 格式
    pub fn now_iso_format() -> String {
        Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
    }

    /// 使用UTC时间
    pub fn now_utc_iso_format() -> String {
        Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string()
    }

    /// 格式化指定时间戳
    pub fn format_timestamp(timestamp: i64) -> String {
        let datetime = DateTime::from_timestamp(timestamp, 0)
            .unwrap_or_else(|| DateTime::from_timestamp(0, 0).unwrap());
        datetime.format("%Y-%m-%dT%H:%M:%S").to_string()
    }

    /// 解析ISO格式字符串为DateTime
    pub fn parse_iso_format(time_str: &str) -> Option<DateTime<Local>> {
        let naive = chrono::NaiveDateTime::parse_from_str(time_str, "%Y-%m-%dT%H:%M:%S").ok()?;
        Some(Local.from_local_datetime(&naive).unwrap())
    }

    /// 获取带毫秒的ISO格式
    pub fn now_iso_with_millis() -> String {
        Local::now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string()
    }

    /// 获取带时区的ISO格式
    pub fn now_iso_with_timezone() -> String {
        Local::now().to_rfc3339()
    }
}
