use base::chrono::NaiveDateTime;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use sqlx::FromRow;

use crate::storage::db;

#[derive(Debug, Clone, Default)]
pub struct GbDeviceCreate {
    pub device_id: String,
    pub domain_id: String,
    pub domain: String,
    pub longitude: String,
    pub latitude: String,
    pub address: String,
    pub pwd: String,
    pub pwd_check: i64,
    pub alias: String,
    pub status: i64,
    pub heartbeat_sec: i64,
    pub tenant_id: String,
    pub sys_org_code: String,
    pub create_by: String,
    pub update_by: String,
}

#[derive(Debug, Clone, Default, FromRow)]
pub struct GbDeviceView {
    pub device_id: String,
    pub domain_id: String,
    pub domain: String,
    pub longitude: Option<String>,
    pub latitude: Option<String>,
    pub address: Option<String>,
    pub pwd: Option<String>,
    pub pwd_check: i64,
    pub alias: Option<String>,
    pub status: i64,
    pub heartbeat_sec: i64,
    pub del: i64,
    pub create_time: Option<NaiveDateTime>,
    pub tenant_id: Option<String>,
    pub sys_org_code: Option<String>,
    pub create_by: Option<String>,
    pub update_by: Option<String>,
    pub update_time: Option<NaiveDateTime>,
    pub channel_count: i64,
}

impl GbDeviceView {
    pub async fn create(request: GbDeviceCreate) -> GlobalResult<Self> {
        let device_id = next_device_id(&request.domain, &request.device_id).await?;
        let longitude = empty_string_to_none(request.longitude);
        let latitude = empty_string_to_none(request.latitude);
        let address = empty_string_to_none(request.address);
        let pwd = empty_string_to_none(request.pwd);
        let alias = empty_string_to_none(request.alias);
        let tenant_id = empty_string_to_i64(request.tenant_id);
        let sys_org_code = empty_string_to_none(request.sys_org_code);
        let create_by = empty_string_to_none(request.create_by);
        let update_by = empty_string_to_none(request.update_by);
        db::execute!(
            r#"INSERT INTO GB28181_OAUTH (DEVICE_ID,DOMAIN_ID,DOMAIN,longitude,latitude,address,PWD,PWD_CHECK,ALIAS,STATUS,HEARTBEAT_SEC,DEL,CREATE_TIME,tenant_id,sys_org_code,create_by,update_by,update_time)
            VALUES (?,?,?,?,?,?,?,?,?,?,?,0,CURRENT_TIMESTAMP,?,?,?,?,CURRENT_TIMESTAMP)"#,
            &device_id,
            &request.domain_id,
            &request.domain,
            longitude,
            latitude,
            address,
            pwd,
            request.pwd_check,
            alias,
            request.status,
            request.heartbeat_sec,
            tenant_id,
            sys_org_code,
            create_by,
            update_by,
        )
        .hand_log(|msg| error!("{msg}"))?;
        Self::get(&device_id).await?.ok_or_else(|| {
            GlobalError::new_sys_error("created GB28181 device is missing", |msg| error!("{msg}"))
        })
    }

    pub async fn list() -> GlobalResult<Vec<Self>> {
        let sql = match db::backend() {
            db::SessionDatabaseBackend::Mysql => GB_DEVICE_LIST_MYSQL,
            db::SessionDatabaseBackend::Sqlite => GB_DEVICE_LIST_SQLITE,
        };
        db::fetch_all_as!(Self, sql).hand_log(|msg| error!("{msg}"))
    }

    pub async fn get(device_id: &str) -> GlobalResult<Option<Self>> {
        let sql = match db::backend() {
            db::SessionDatabaseBackend::Mysql => GB_DEVICE_GET_MYSQL,
            db::SessionDatabaseBackend::Sqlite => GB_DEVICE_GET_SQLITE,
        };
        db::fetch_optional_as!(Self, sql, device_id).hand_log(|msg| error!("{msg}"))
    }
}

async fn next_device_id(domain: &str, requested_device_id: &str) -> GlobalResult<String> {
    let requested_device_id = requested_device_id.trim();
    if !requested_device_id.is_empty() {
        return Ok(requested_device_id.to_string());
    }
    let prefix = device_id_prefix(domain);
    let like = format!("{prefix}%");
    let max_row: Option<(String,)> = db::fetch_optional_as!(
        (String,),
        "SELECT DEVICE_ID FROM GB28181_OAUTH WHERE DEVICE_ID LIKE ? ORDER BY DEVICE_ID DESC LIMIT 1",
        &like,
    )
    .hand_log(|msg| error!("{msg}"))?;
    let next = next_device_id_number(&prefix, max_row.as_ref().map(|(value,)| value.as_str()));
    Ok(format_device_id(&prefix, next))
}

fn device_id_prefix(domain: &str) -> String {
    format!("{}1327", domain.trim())
}

fn next_device_id_number(prefix: &str, max_device_id: Option<&str>) -> u64 {
    max_device_id
        .and_then(|value| value.get(prefix.len()..))
        .and_then(|suffix| suffix.parse::<u64>().ok())
        .unwrap_or(0)
        + 1
}

fn format_device_id(prefix: &str, next: u64) -> String {
    let suffix_width = 20usize.saturating_sub(prefix.len()).max(1);
    format!("{prefix}{next:0suffix_width$}")
}

fn empty_string_to_none(value: String) -> Option<String> {
    let value = value.trim().to_string();
    if value.is_empty() { None } else { Some(value) }
}

fn empty_string_to_i64(value: String) -> Option<i64> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        value.parse().ok()
    }
}

const GB_DEVICE_COLUMNS_MYSQL: &str = r#"
    o.DEVICE_ID AS device_id,
    o.DOMAIN_ID AS domain_id,
    o.DOMAIN AS domain,
    CAST(o.longitude AS CHAR) AS longitude,
    CAST(o.latitude AS CHAR) AS latitude,
    o.address AS address,
    o.PWD AS pwd,
    COALESCE(o.PWD_CHECK,0) AS pwd_check,
    o.ALIAS AS alias,
    COALESCE(o.STATUS,1) AS status,
    COALESCE(o.HEARTBEAT_SEC,60) AS heartbeat_sec,
    COALESCE(o.DEL,0) AS del,
    o.CREATE_TIME AS create_time,
    CAST(o.tenant_id AS CHAR) AS tenant_id,
    o.sys_org_code AS sys_org_code,
    o.create_by AS create_by,
    o.update_by AS update_by,
    o.update_time AS update_time,
    (SELECT COUNT(*) FROM GB28181_DEVICE_CHANNEL c WHERE c.DEVICE_ID=o.DEVICE_ID) AS channel_count
"#;
const GB_DEVICE_COLUMNS_SQLITE: &str = r#"
    o.DEVICE_ID AS device_id,
    o.DOMAIN_ID AS domain_id,
    o.DOMAIN AS domain,
    CAST(o.longitude AS TEXT) AS longitude,
    CAST(o.latitude AS TEXT) AS latitude,
    o.address AS address,
    o.PWD AS pwd,
    COALESCE(o.PWD_CHECK,0) AS pwd_check,
    o.ALIAS AS alias,
    COALESCE(o.STATUS,1) AS status,
    COALESCE(o.HEARTBEAT_SEC,60) AS heartbeat_sec,
    COALESCE(o.DEL,0) AS del,
    o.CREATE_TIME AS create_time,
    CAST(o.tenant_id AS TEXT) AS tenant_id,
    o.sys_org_code AS sys_org_code,
    o.create_by AS create_by,
    o.update_by AS update_by,
    o.update_time AS update_time,
    (SELECT COUNT(*) FROM GB28181_DEVICE_CHANNEL c WHERE c.DEVICE_ID=o.DEVICE_ID) AS channel_count
"#;
const GB_DEVICE_LIST_MYSQL: &str = "SELECT \n    o.DEVICE_ID AS device_id,\n    o.DOMAIN_ID AS domain_id,\n    o.DOMAIN AS domain,\n    CAST(o.longitude AS CHAR) AS longitude,\n    CAST(o.latitude AS CHAR) AS latitude,\n    o.address AS address,\n    o.PWD AS pwd,\n    COALESCE(o.PWD_CHECK,0) AS pwd_check,\n    o.ALIAS AS alias,\n    COALESCE(o.STATUS,1) AS status,\n    COALESCE(o.HEARTBEAT_SEC,60) AS heartbeat_sec,\n    COALESCE(o.DEL,0) AS del,\n    o.CREATE_TIME AS create_time,\n    CAST(o.tenant_id AS CHAR) AS tenant_id,\n    o.sys_org_code AS sys_org_code,\n    o.create_by AS create_by,\n    o.update_by AS update_by,\n    o.update_time AS update_time,\n    (SELECT COUNT(*) FROM GB28181_DEVICE_CHANNEL c WHERE c.DEVICE_ID=o.DEVICE_ID) AS channel_count\n FROM GB28181_OAUTH o WHERE COALESCE(o.DEL,0)=0 ORDER BY o.DEVICE_ID";
const GB_DEVICE_GET_MYSQL: &str = "SELECT \n    o.DEVICE_ID AS device_id,\n    o.DOMAIN_ID AS domain_id,\n    o.DOMAIN AS domain,\n    CAST(o.longitude AS CHAR) AS longitude,\n    CAST(o.latitude AS CHAR) AS latitude,\n    o.address AS address,\n    o.PWD AS pwd,\n    COALESCE(o.PWD_CHECK,0) AS pwd_check,\n    o.ALIAS AS alias,\n    COALESCE(o.STATUS,1) AS status,\n    COALESCE(o.HEARTBEAT_SEC,60) AS heartbeat_sec,\n    COALESCE(o.DEL,0) AS del,\n    o.CREATE_TIME AS create_time,\n    CAST(o.tenant_id AS CHAR) AS tenant_id,\n    o.sys_org_code AS sys_org_code,\n    o.create_by AS create_by,\n    o.update_by AS update_by,\n    o.update_time AS update_time,\n    (SELECT COUNT(*) FROM GB28181_DEVICE_CHANNEL c WHERE c.DEVICE_ID=o.DEVICE_ID) AS channel_count\n FROM GB28181_OAUTH o WHERE COALESCE(o.DEL,0)=0 AND o.DEVICE_ID=?";
const GB_DEVICE_LIST_SQLITE: &str = "SELECT \n    o.DEVICE_ID AS device_id,\n    o.DOMAIN_ID AS domain_id,\n    o.DOMAIN AS domain,\n    CAST(o.longitude AS TEXT) AS longitude,\n    CAST(o.latitude AS TEXT) AS latitude,\n    o.address AS address,\n    o.PWD AS pwd,\n    COALESCE(o.PWD_CHECK,0) AS pwd_check,\n    o.ALIAS AS alias,\n    COALESCE(o.STATUS,1) AS status,\n    COALESCE(o.HEARTBEAT_SEC,60) AS heartbeat_sec,\n    COALESCE(o.DEL,0) AS del,\n    o.CREATE_TIME AS create_time,\n    CAST(o.tenant_id AS TEXT) AS tenant_id,\n    o.sys_org_code AS sys_org_code,\n    o.create_by AS create_by,\n    o.update_by AS update_by,\n    o.update_time AS update_time,\n    (SELECT COUNT(*) FROM GB28181_DEVICE_CHANNEL c WHERE c.DEVICE_ID=o.DEVICE_ID) AS channel_count\n FROM GB28181_OAUTH o WHERE COALESCE(o.DEL,0)=0 ORDER BY o.DEVICE_ID";
const GB_DEVICE_GET_SQLITE: &str = "SELECT \n    o.DEVICE_ID AS device_id,\n    o.DOMAIN_ID AS domain_id,\n    o.DOMAIN AS domain,\n    CAST(o.longitude AS TEXT) AS longitude,\n    CAST(o.latitude AS TEXT) AS latitude,\n    o.address AS address,\n    o.PWD AS pwd,\n    COALESCE(o.PWD_CHECK,0) AS pwd_check,\n    o.ALIAS AS alias,\n    COALESCE(o.STATUS,1) AS status,\n    COALESCE(o.HEARTBEAT_SEC,60) AS heartbeat_sec,\n    COALESCE(o.DEL,0) AS del,\n    o.CREATE_TIME AS create_time,\n    CAST(o.tenant_id AS TEXT) AS tenant_id,\n    o.sys_org_code AS sys_org_code,\n    o.create_by AS create_by,\n    o.update_by AS update_by,\n    o.update_time AS update_time,\n    (SELECT COUNT(*) FROM GB28181_DEVICE_CHANNEL c WHERE c.DEVICE_ID=o.DEVICE_ID) AS channel_count\n FROM GB28181_OAUTH o WHERE COALESCE(o.DEL,0)=0 AND o.DEVICE_ID=?";

#[derive(Debug, Clone, Default, FromRow)]
pub struct GbChannelView {
    pub device_id: String,
    pub channel_id: String,
    pub name: String,
    pub manufacturer: String,
    pub model: String,
    pub owner: String,
    pub status: String,
    pub civil_code: String,
    pub address: String,
    pub parent_id: String,
    pub ip_address: String,
    pub port: i64,
    pub longitude: String,
    pub latitude: String,
    pub ptz_type: String,
    pub alias_name: String,
    pub pic_url: String,
    pub snapshot: i64,
    pub over_pic_id: String,
    pub ptz_enable: i64,
    pub talk_enable: i64,
    pub audio_enable: i64,
    pub record_enable: i64,
    pub playback_enable: i64,
    pub alarm_enable: i64,
    pub biz_enable: i64,
    pub sort_no: i64,
    pub created_at: Option<NaiveDateTime>,
    pub updated_at: Option<NaiveDateTime>,
}

impl GbChannelView {
    pub async fn list(device_id: &str) -> GlobalResult<Vec<Self>> {
        let sql = match db::backend() {
            db::SessionDatabaseBackend::Mysql => GB_CHANNEL_LIST_MYSQL,
            db::SessionDatabaseBackend::Sqlite => GB_CHANNEL_LIST_SQLITE,
        };
        db::fetch_all_as!(Self, sql, device_id).hand_log(|msg| error!("{msg}"))
    }

    pub async fn get(device_id: &str, channel_id: &str) -> GlobalResult<Option<Self>> {
        let sql = match db::backend() {
            db::SessionDatabaseBackend::Mysql => GB_CHANNEL_GET_MYSQL,
            db::SessionDatabaseBackend::Sqlite => GB_CHANNEL_GET_SQLITE,
        };
        db::fetch_optional_as!(Self, sql, device_id, channel_id).hand_log(|msg| error!("{msg}"))
    }
}

const GB_CHANNEL_COLUMNS_MYSQL: &str = r#"
    c.DEVICE_ID AS device_id,
    c.CHANNEL_ID AS channel_id,
    COALESCE(c.NAME,'') AS name,
    COALESCE(c.MANUFACTURER,'') AS manufacturer,
    COALESCE(c.MODEL,'') AS model,
    COALESCE(c.OWNER,'') AS owner,
    COALESCE(c.STATUS,'UNKNOWN') AS status,
    COALESCE(c.CIVIL_CODE,'') AS civil_code,
    COALESCE(c.ADDRESS,'') AS address,
    COALESCE(c.PARENT_ID,'') AS parent_id,
    COALESCE(c.IP_ADDRESS,'') AS ip_address,
    COALESCE(c.PORT,0) AS port,
    COALESCE(CAST(c.LONGITUDE AS CHAR),'') AS longitude,
    COALESCE(CAST(c.LATITUDE AS CHAR),'') AS latitude,
    COALESCE(c.PTZ_TYPE,'') AS ptz_type,
    COALESCE(conf.ALIAS_NAME,'') AS alias_name,
    '' AS pic_url,
    COALESCE(conf.SNAPSHOT_ENABLE,0) AS snapshot,
    COALESCE(CAST(conf.over_pic_id AS CHAR),'') AS over_pic_id,
    COALESCE(conf.PTZ_ENABLE,0) AS ptz_enable,
    COALESCE(conf.TALK_ENABLE,0) AS talk_enable,
    COALESCE(conf.AUDIO_ENABLE,0) AS audio_enable,
    COALESCE(conf.RECORD_ENABLE,0) AS record_enable,
    COALESCE(conf.PLAYBACK_ENABLE,0) AS playback_enable,
    COALESCE(conf.ALARM_ENABLE,0) AS alarm_enable,
    COALESCE(conf.BIZ_ENABLE,0) AS biz_enable,
    COALESCE(conf.SORT_NO,0) AS sort_no,
    conf.CREATE_TIME AS created_at,
    conf.UPDATE_TIME AS updated_at
"#;
const GB_CHANNEL_COLUMNS_SQLITE: &str = r#"
    c.DEVICE_ID AS device_id,
    c.CHANNEL_ID AS channel_id,
    COALESCE(c.NAME,'') AS name,
    COALESCE(c.MANUFACTURER,'') AS manufacturer,
    COALESCE(c.MODEL,'') AS model,
    COALESCE(c.OWNER,'') AS owner,
    COALESCE(c.STATUS,'UNKNOWN') AS status,
    COALESCE(c.CIVIL_CODE,'') AS civil_code,
    COALESCE(c.ADDRESS,'') AS address,
    COALESCE(c.PARENT_ID,'') AS parent_id,
    COALESCE(c.IP_ADDRESS,'') AS ip_address,
    COALESCE(c.PORT,0) AS port,
    COALESCE(CAST(c.LONGITUDE AS TEXT),'') AS longitude,
    COALESCE(CAST(c.LATITUDE AS TEXT),'') AS latitude,
    COALESCE(c.PTZ_TYPE,'') AS ptz_type,
    COALESCE(conf.ALIAS_NAME,'') AS alias_name,
    '' AS pic_url,
    COALESCE(conf.SNAPSHOT_ENABLE,0) AS snapshot,
    COALESCE(CAST(conf.over_pic_id AS TEXT),'') AS over_pic_id,
    COALESCE(conf.PTZ_ENABLE,0) AS ptz_enable,
    COALESCE(conf.TALK_ENABLE,0) AS talk_enable,
    COALESCE(conf.AUDIO_ENABLE,0) AS audio_enable,
    COALESCE(conf.RECORD_ENABLE,0) AS record_enable,
    COALESCE(conf.PLAYBACK_ENABLE,0) AS playback_enable,
    COALESCE(conf.ALARM_ENABLE,0) AS alarm_enable,
    COALESCE(conf.BIZ_ENABLE,0) AS biz_enable,
    COALESCE(conf.SORT_NO,0) AS sort_no,
    conf.CREATE_TIME AS created_at,
    conf.UPDATE_TIME AS updated_at
"#;
const GB_CHANNEL_LIST_MYSQL: &str = "SELECT \n    c.DEVICE_ID AS device_id,\n    c.CHANNEL_ID AS channel_id,\n    COALESCE(c.NAME,'') AS name,\n    COALESCE(c.MANUFACTURER,'') AS manufacturer,\n    COALESCE(c.MODEL,'') AS model,\n    COALESCE(c.OWNER,'') AS owner,\n    COALESCE(c.STATUS,'UNKNOWN') AS status,\n    COALESCE(c.CIVIL_CODE,'') AS civil_code,\n    COALESCE(c.ADDRESS,'') AS address,\n    COALESCE(c.PARENT_ID,'') AS parent_id,\n    COALESCE(c.IP_ADDRESS,'') AS ip_address,\n    COALESCE(c.PORT,0) AS port,\n    COALESCE(CAST(c.LONGITUDE AS CHAR),'') AS longitude,\n    COALESCE(CAST(c.LATITUDE AS CHAR),'') AS latitude,\n    COALESCE(c.PTZ_TYPE,'') AS ptz_type,\n    COALESCE(conf.ALIAS_NAME,'') AS alias_name,\n    '' AS pic_url,\n    COALESCE(conf.SNAPSHOT_ENABLE,0) AS snapshot,\n    COALESCE(CAST(conf.over_pic_id AS CHAR),'') AS over_pic_id,\n    COALESCE(conf.PTZ_ENABLE,0) AS ptz_enable,\n    COALESCE(conf.TALK_ENABLE,0) AS talk_enable,\n    COALESCE(conf.AUDIO_ENABLE,0) AS audio_enable,\n    COALESCE(conf.RECORD_ENABLE,0) AS record_enable,\n    COALESCE(conf.PLAYBACK_ENABLE,0) AS playback_enable,\n    COALESCE(conf.ALARM_ENABLE,0) AS alarm_enable,\n    COALESCE(conf.BIZ_ENABLE,0) AS biz_enable,\n    COALESCE(conf.SORT_NO,0) AS sort_no,\n    conf.CREATE_TIME AS created_at,\n    conf.UPDATE_TIME AS updated_at\n FROM GB28181_DEVICE_CHANNEL c LEFT JOIN GB28181_DEVICE_CHANNEL_CONF conf ON conf.DEVICE_ID=c.DEVICE_ID AND conf.CHANNEL_ID=c.CHANNEL_ID  WHERE c.DEVICE_ID=? ORDER BY COALESCE(conf.SORT_NO,0),c.CHANNEL_ID";
const GB_CHANNEL_GET_MYSQL: &str = "SELECT \n    c.DEVICE_ID AS device_id,\n    c.CHANNEL_ID AS channel_id,\n    COALESCE(c.NAME,'') AS name,\n    COALESCE(c.MANUFACTURER,'') AS manufacturer,\n    COALESCE(c.MODEL,'') AS model,\n    COALESCE(c.OWNER,'') AS owner,\n    COALESCE(c.STATUS,'UNKNOWN') AS status,\n    COALESCE(c.CIVIL_CODE,'') AS civil_code,\n    COALESCE(c.ADDRESS,'') AS address,\n    COALESCE(c.PARENT_ID,'') AS parent_id,\n    COALESCE(c.IP_ADDRESS,'') AS ip_address,\n    COALESCE(c.PORT,0) AS port,\n    COALESCE(CAST(c.LONGITUDE AS CHAR),'') AS longitude,\n    COALESCE(CAST(c.LATITUDE AS CHAR),'') AS latitude,\n    COALESCE(c.PTZ_TYPE,'') AS ptz_type,\n    COALESCE(conf.ALIAS_NAME,'') AS alias_name,\n    '' AS pic_url,\n    COALESCE(conf.SNAPSHOT_ENABLE,0) AS snapshot,\n    COALESCE(CAST(conf.over_pic_id AS CHAR),'') AS over_pic_id,\n    COALESCE(conf.PTZ_ENABLE,0) AS ptz_enable,\n    COALESCE(conf.TALK_ENABLE,0) AS talk_enable,\n    COALESCE(conf.AUDIO_ENABLE,0) AS audio_enable,\n    COALESCE(conf.RECORD_ENABLE,0) AS record_enable,\n    COALESCE(conf.PLAYBACK_ENABLE,0) AS playback_enable,\n    COALESCE(conf.ALARM_ENABLE,0) AS alarm_enable,\n    COALESCE(conf.BIZ_ENABLE,0) AS biz_enable,\n    COALESCE(conf.SORT_NO,0) AS sort_no,\n    conf.CREATE_TIME AS created_at,\n    conf.UPDATE_TIME AS updated_at\n FROM GB28181_DEVICE_CHANNEL c LEFT JOIN GB28181_DEVICE_CHANNEL_CONF conf ON conf.DEVICE_ID=c.DEVICE_ID AND conf.CHANNEL_ID=c.CHANNEL_ID  WHERE c.DEVICE_ID=? AND c.CHANNEL_ID=?";
const GB_CHANNEL_LIST_SQLITE: &str = "SELECT \n    c.DEVICE_ID AS device_id,\n    c.CHANNEL_ID AS channel_id,\n    COALESCE(c.NAME,'') AS name,\n    COALESCE(c.MANUFACTURER,'') AS manufacturer,\n    COALESCE(c.MODEL,'') AS model,\n    COALESCE(c.OWNER,'') AS owner,\n    COALESCE(c.STATUS,'UNKNOWN') AS status,\n    COALESCE(c.CIVIL_CODE,'') AS civil_code,\n    COALESCE(c.ADDRESS,'') AS address,\n    COALESCE(c.PARENT_ID,'') AS parent_id,\n    COALESCE(c.IP_ADDRESS,'') AS ip_address,\n    COALESCE(c.PORT,0) AS port,\n    COALESCE(CAST(c.LONGITUDE AS TEXT),'') AS longitude,\n    COALESCE(CAST(c.LATITUDE AS TEXT),'') AS latitude,\n    COALESCE(c.PTZ_TYPE,'') AS ptz_type,\n    COALESCE(conf.ALIAS_NAME,'') AS alias_name,\n    '' AS pic_url,\n    COALESCE(conf.SNAPSHOT_ENABLE,0) AS snapshot,\n    COALESCE(CAST(conf.over_pic_id AS TEXT),'') AS over_pic_id,\n    COALESCE(conf.PTZ_ENABLE,0) AS ptz_enable,\n    COALESCE(conf.TALK_ENABLE,0) AS talk_enable,\n    COALESCE(conf.AUDIO_ENABLE,0) AS audio_enable,\n    COALESCE(conf.RECORD_ENABLE,0) AS record_enable,\n    COALESCE(conf.PLAYBACK_ENABLE,0) AS playback_enable,\n    COALESCE(conf.ALARM_ENABLE,0) AS alarm_enable,\n    COALESCE(conf.BIZ_ENABLE,0) AS biz_enable,\n    COALESCE(conf.SORT_NO,0) AS sort_no,\n    conf.CREATE_TIME AS created_at,\n    conf.UPDATE_TIME AS updated_at\n FROM GB28181_DEVICE_CHANNEL c LEFT JOIN GB28181_DEVICE_CHANNEL_CONF conf ON conf.DEVICE_ID=c.DEVICE_ID AND conf.CHANNEL_ID=c.CHANNEL_ID  WHERE c.DEVICE_ID=? ORDER BY COALESCE(conf.SORT_NO,0),c.CHANNEL_ID";
const GB_CHANNEL_GET_SQLITE: &str = "SELECT \n    c.DEVICE_ID AS device_id,\n    c.CHANNEL_ID AS channel_id,\n    COALESCE(c.NAME,'') AS name,\n    COALESCE(c.MANUFACTURER,'') AS manufacturer,\n    COALESCE(c.MODEL,'') AS model,\n    COALESCE(c.OWNER,'') AS owner,\n    COALESCE(c.STATUS,'UNKNOWN') AS status,\n    COALESCE(c.CIVIL_CODE,'') AS civil_code,\n    COALESCE(c.ADDRESS,'') AS address,\n    COALESCE(c.PARENT_ID,'') AS parent_id,\n    COALESCE(c.IP_ADDRESS,'') AS ip_address,\n    COALESCE(c.PORT,0) AS port,\n    COALESCE(CAST(c.LONGITUDE AS TEXT),'') AS longitude,\n    COALESCE(CAST(c.LATITUDE AS TEXT),'') AS latitude,\n    COALESCE(c.PTZ_TYPE,'') AS ptz_type,\n    COALESCE(conf.ALIAS_NAME,'') AS alias_name,\n    '' AS pic_url,\n    COALESCE(conf.SNAPSHOT_ENABLE,0) AS snapshot,\n    COALESCE(CAST(conf.over_pic_id AS TEXT),'') AS over_pic_id,\n    COALESCE(conf.PTZ_ENABLE,0) AS ptz_enable,\n    COALESCE(conf.TALK_ENABLE,0) AS talk_enable,\n    COALESCE(conf.AUDIO_ENABLE,0) AS audio_enable,\n    COALESCE(conf.RECORD_ENABLE,0) AS record_enable,\n    COALESCE(conf.PLAYBACK_ENABLE,0) AS playback_enable,\n    COALESCE(conf.ALARM_ENABLE,0) AS alarm_enable,\n    COALESCE(conf.BIZ_ENABLE,0) AS biz_enable,\n    COALESCE(conf.SORT_NO,0) AS sort_no,\n    conf.CREATE_TIME AS created_at,\n    conf.UPDATE_TIME AS updated_at\n FROM GB28181_DEVICE_CHANNEL c LEFT JOIN GB28181_DEVICE_CHANNEL_CONF conf ON conf.DEVICE_ID=c.DEVICE_ID AND conf.CHANNEL_ID=c.CHANNEL_ID  WHERE c.DEVICE_ID=? AND c.CHANNEL_ID=?";

#[derive(Debug, Clone, Default, FromRow)]
pub struct GbChannelImageView {
    pub image_id: String,
    pub device_id: String,
    pub channel_id: String,
    pub image_url: String,
    pub created_at: Option<NaiveDateTime>,
}

impl GbChannelImageView {
    pub async fn list(device_id: &str, channel_id: &str) -> GlobalResult<Vec<Self>> {
        let sql = match db::backend() {
            db::SessionDatabaseBackend::Mysql => GB_CHANNEL_IMAGE_LIST_MYSQL,
            db::SessionDatabaseBackend::Sqlite => GB_CHANNEL_IMAGE_LIST_SQLITE,
        };
        db::fetch_all_as!(Self, sql, device_id, channel_id).hand_log(|msg| error!("{msg}"))
    }
}

const GB_CHANNEL_IMAGE_LIST_MYSQL: &str = "SELECT CAST(ID AS CHAR) AS image_id,DEVICE_ID AS device_id,CHANNEL_ID AS channel_id,COALESCE(ABS_PATH,DIR_PATH) AS image_url,CREATE_TIME AS created_at FROM GB28181_FILE_INFO WHERE DEVICE_ID=? AND CHANNEL_ID=? AND COALESCE(IS_DEL,0)=0 AND COALESCE(FILE_TYPE,0)=0 ORDER BY ID DESC LIMIT 50";
const GB_CHANNEL_IMAGE_LIST_SQLITE: &str = "SELECT CAST(ID AS TEXT) AS image_id,DEVICE_ID AS device_id,CHANNEL_ID AS channel_id,COALESCE(ABS_PATH,DIR_PATH) AS image_url,CREATE_TIME AS created_at FROM GB28181_FILE_INFO WHERE DEVICE_ID=? AND CHANNEL_ID=? AND COALESCE(IS_DEL,0)=0 AND COALESCE(FILE_TYPE,0)=0 ORDER BY ID DESC LIMIT 50";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_device_id_uses_domain_1327_prefix() {
        let prefix = device_id_prefix("5101000000");

        assert_eq!(prefix, "51010000001327");
        assert_eq!(format_device_id(&prefix, 1), "51010000001327000001");
    }

    #[test]
    fn auto_device_id_increments_from_max_device_id() {
        let prefix = device_id_prefix("5101000000");
        let next = next_device_id_number(&prefix, Some("51010000001327000001"));

        assert_eq!(next, 2);
        assert_eq!(format_device_id(&prefix, next), "51010000001327000002");
    }
}
