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
    pub snapshot_to_mode: i64,
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
    pub snapshot_to_mode: i64,
    pub del: i64,
    pub create_time: Option<NaiveDateTime>,
    pub tenant_id: Option<String>,
    pub sys_org_code: Option<String>,
    pub create_by: Option<String>,
    pub update_by: Option<String>,
    pub update_time: Option<NaiveDateTime>,
    pub monitor_status: i64,
    pub device_type: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub firmware: Option<String>,
    pub gb_version: Option<String>,
    pub max_camera: i64,
    pub camera_in_count: i64,
    pub camera_off_count: i64,
    pub register_time: Option<NaiveDateTime>,
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
            r#"INSERT INTO gb28181_oauth (device_id,domain_id,domain,longitude,latitude,address,pwd,pwd_check,alias,status,heartbeat_sec,snapshot_to_mode,del,create_time,tenant_id,sys_org_code,create_by,update_by,update_time)
            VALUES (?,?,?,?,?,?,?,?,?,?,?,?,0,CURRENT_TIMESTAMP,?,?,?,?,CURRENT_TIMESTAMP)"#,
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
            request.snapshot_to_mode,
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

    pub async fn update(request: GbDeviceCreate) -> GlobalResult<Option<Self>> {
        let longitude = empty_string_to_none(request.longitude);
        let latitude = empty_string_to_none(request.latitude);
        let address = empty_string_to_none(request.address);
        let pwd = empty_string_to_none(request.pwd);
        let alias = empty_string_to_none(request.alias);
        let tenant_id = empty_string_to_i64(request.tenant_id);
        let sys_org_code = empty_string_to_none(request.sys_org_code);
        let update_by = empty_string_to_none(request.update_by);
        let affected = db::execute!(
            r#"UPDATE gb28181_oauth SET domain_id=?,domain=?,longitude=?,latitude=?,address=?,pwd=?,pwd_check=?,alias=?,status=?,heartbeat_sec=?,snapshot_to_mode=?,tenant_id=?,sys_org_code=?,update_by=?,update_time=CURRENT_TIMESTAMP WHERE COALESCE(del,0)=0 AND device_id=?"#,
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
            request.snapshot_to_mode,
            tenant_id,
            sys_org_code,
            update_by,
            &request.device_id,
        )
        .hand_log(|msg| error!("{msg}"))?;
        if affected == 0 {
            return Ok(None);
        }
        Self::get(&request.device_id).await
    }

    pub async fn list(registered_only: bool) -> GlobalResult<Vec<Self>> {
        let sql = match db::backend() {
            db::SessionDatabaseBackend::Mysql => GB_DEVICE_LIST_MYSQL,
            db::SessionDatabaseBackend::Sqlite => GB_DEVICE_LIST_SQLITE,
        };
        db::fetch_all_as!(Self, sql, registered_only_param(registered_only))
            .hand_log(|msg| error!("{msg}"))
    }

    pub async fn list_page(
        registered_only: bool,
        offset: u32,
        limit: u32,
    ) -> GlobalResult<Vec<Self>> {
        let sql = match db::backend() {
            db::SessionDatabaseBackend::Mysql => GB_DEVICE_LIST_PAGE_MYSQL,
            db::SessionDatabaseBackend::Sqlite => GB_DEVICE_LIST_PAGE_SQLITE,
        };
        db::fetch_all_as!(
            Self,
            sql,
            registered_only_param(registered_only),
            limit,
            offset
        )
        .hand_log(|msg| error!("{msg}"))
    }

    pub async fn list_page_by_domain(
        domain_id: &str,
        device_id: &str,
        device_name: &str,
        registered_only: bool,
        offset: u32,
        limit: u32,
    ) -> GlobalResult<Vec<Self>> {
        let sql = match db::backend() {
            db::SessionDatabaseBackend::Mysql => GB_DEVICE_LIST_PAGE_BY_DOMAIN_FILTER_MYSQL,
            db::SessionDatabaseBackend::Sqlite => GB_DEVICE_LIST_PAGE_BY_DOMAIN_FILTER_SQLITE,
        };
        let device_id = device_id.trim();
        let device_id_like = format!("%{device_id}%");
        let device_name = device_name.trim();
        let device_name_like = format!("%{device_name}%");
        db::fetch_all_as!(
            Self,
            sql,
            domain_id,
            device_id,
            &device_id_like,
            device_name,
            &device_name_like,
            registered_only_param(registered_only),
            limit,
            offset
        )
        .hand_log(|msg| error!("{msg}"))
    }
    pub async fn count(registered_only: bool) -> GlobalResult<u64> {
        let row: Option<(i64,)> = db::fetch_optional_as!(
            (i64,),
            "SELECT COUNT(*) FROM gb28181_oauth o LEFT JOIN gb28181_device d ON d.device_id=o.device_id WHERE COALESCE(o.del,0)=0 AND (?=0 OR d.register_time IS NOT NULL)",
            registered_only_param(registered_only),
        )
        .hand_log(|msg| error!("{msg}"))?;
        Ok(row
            .and_then(|(count,)| u64::try_from(count).ok())
            .unwrap_or_default())
    }

    pub async fn count_by_domain(
        domain_id: &str,
        device_id: &str,
        device_name: &str,
        registered_only: bool,
    ) -> GlobalResult<u64> {
        let device_id = device_id.trim();
        let device_id_like = format!("%{device_id}%");
        let device_name = device_name.trim();
        let device_name_like = format!("%{device_name}%");
        let row: Option<(i64,)> = db::fetch_optional_as!(
            (i64,),
            "SELECT COUNT(*) FROM gb28181_oauth o LEFT JOIN gb28181_device d ON d.device_id=o.device_id WHERE COALESCE(o.del,0)=0 AND o.domain_id=? AND (?='' OR o.device_id LIKE ?) AND (?='' OR o.alias LIKE ?) AND (?=0 OR d.register_time IS NOT NULL)",
            domain_id,
            device_id,
            &device_id_like,
            device_name,
            &device_name_like,
            registered_only_param(registered_only),
        )
        .hand_log(|msg| error!("{msg}"))?;
        Ok(row
            .and_then(|(count,)| u64::try_from(count).ok())
            .unwrap_or_default())
    }
    pub async fn get(device_id: &str) -> GlobalResult<Option<Self>> {
        let sql = match db::backend() {
            db::SessionDatabaseBackend::Mysql => GB_DEVICE_GET_MYSQL,
            db::SessionDatabaseBackend::Sqlite => GB_DEVICE_GET_SQLITE,
        };
        db::fetch_optional_as!(Self, sql, device_id).hand_log(|msg| error!("{msg}"))
    }

    pub async fn delete(device_id: &str, domain_id: &str) -> GlobalResult<bool> {
        let affected = db::execute!(
            "UPDATE gb28181_oauth SET del=1,update_time=CURRENT_TIMESTAMP WHERE COALESCE(del,0)=0 AND device_id=? AND domain_id=?",
            device_id,
            domain_id,
        )
        .hand_log(|msg| error!("{msg}"))?;
        Ok(affected > 0)
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
        "SELECT device_id FROM gb28181_oauth WHERE device_id LIKE ? ORDER BY device_id DESC LIMIT 1",
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

fn registered_only_param(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

const GB_DEVICE_COLUMNS_MYSQL: &str = r#"
    o.device_id AS device_id,
    o.domain_id AS domain_id,
    o.domain AS domain,
    CAST(o.longitude AS CHAR) AS longitude,
    CAST(o.latitude AS CHAR) AS latitude,
    o.address AS address,
    o.pwd AS pwd,
    COALESCE(o.pwd_check,0) AS pwd_check,
    o.alias AS alias,
    COALESCE(o.status,1) AS status,
    COALESCE(o.heartbeat_sec,60) AS heartbeat_sec,
    COALESCE(o.snapshot_to_mode,0) AS snapshot_to_mode,
    COALESCE(o.del,0) AS del,
    o.create_time AS create_time,
    CAST(o.tenant_id AS CHAR) AS tenant_id,
    o.sys_org_code AS sys_org_code,
    o.create_by AS create_by,
    o.update_by AS update_by,
    o.update_time AS update_time,
    CASE
        WHEN COALESCE(o.status,1)=1 AND d.online_expire_time IS NOT NULL AND d.online_expire_time > NOW() THEN 1
        ELSE 0
    END AS monitor_status,
    d.device_type AS device_type,
    d.manufacturer AS manufacturer,
    d.model AS model,
    d.firmware AS firmware,
    d.gb_version AS gb_version,
    COALESCE(CAST(d.max_camera AS SIGNED),0) AS max_camera,
    COALESCE(cs.camera_in_count,0) AS camera_in_count,
    CASE
        WHEN COALESCE(o.status,1)=0 OR d.online_expire_time IS NULL OR NOW() >= d.online_expire_time THEN COALESCE(cs.camera_in_count,0)
        ELSE COALESCE(cs.camera_off_count,0)
    END AS camera_off_count,
    d.register_time AS register_time
"#;
const GB_DEVICE_COLUMNS_SQLITE: &str = r#"
    o.device_id AS device_id,
    o.domain_id AS domain_id,
    o.domain AS domain,
    CAST(o.longitude AS TEXT) AS longitude,
    CAST(o.latitude AS TEXT) AS latitude,
    o.address AS address,
    o.pwd AS pwd,
    COALESCE(o.pwd_check,0) AS pwd_check,
    o.alias AS alias,
    COALESCE(o.status,1) AS status,
    COALESCE(o.heartbeat_sec,60) AS heartbeat_sec,
    COALESCE(o.snapshot_to_mode,0) AS snapshot_to_mode,
    COALESCE(o.del,0) AS del,
    o.create_time AS create_time,
    CAST(o.tenant_id AS TEXT) AS tenant_id,
    o.sys_org_code AS sys_org_code,
    o.create_by AS create_by,
    o.update_by AS update_by,
    o.update_time AS update_time,
    CASE
        WHEN COALESCE(o.status,1)=1 AND d.online_expire_time IS NOT NULL AND d.online_expire_time > CURRENT_TIMESTAMP THEN 1
        ELSE 0
    END AS monitor_status,
    d.device_type AS device_type,
    d.manufacturer AS manufacturer,
    d.model AS model,
    d.firmware AS firmware,
    d.gb_version AS gb_version,
    COALESCE(d.max_camera,0) AS max_camera,
    COALESCE(cs.camera_in_count,0) AS camera_in_count,
    CASE
        WHEN COALESCE(o.status,1)=0 OR d.online_expire_time IS NULL OR CURRENT_TIMESTAMP >= d.online_expire_time THEN COALESCE(cs.camera_in_count,0)
        ELSE COALESCE(cs.camera_off_count,0)
    END AS camera_off_count,
    d.register_time AS register_time
"#;
const GB_DEVICE_LIST_MYSQL: &str = "SELECT \n    o.device_id AS device_id,\n    o.domain_id AS domain_id,\n    o.domain AS domain,\n    CAST(o.longitude AS CHAR) AS longitude,\n    CAST(o.latitude AS CHAR) AS latitude,\n    o.address AS address,\n    o.pwd AS pwd,\n    COALESCE(o.pwd_check,0) AS pwd_check,\n    o.alias AS alias,\n    COALESCE(o.status,1) AS status,\n    COALESCE(o.heartbeat_sec,60) AS heartbeat_sec,\n    COALESCE(o.snapshot_to_mode,0) AS snapshot_to_mode,\n    COALESCE(o.del,0) AS del,\n    o.create_time AS create_time,\n    CAST(o.tenant_id AS CHAR) AS tenant_id,\n    o.sys_org_code AS sys_org_code,\n    o.create_by AS create_by,\n    o.update_by AS update_by,\n    o.update_time AS update_time,\n    CASE\n        WHEN COALESCE(o.status,1)=1 AND d.online_expire_time IS NOT NULL AND d.online_expire_time > NOW() THEN 1\n        ELSE 0\n    END AS monitor_status,\n    d.device_type AS device_type,\n    d.manufacturer AS manufacturer,\n    d.model AS model,\n    d.firmware AS firmware,\n    d.gb_version AS gb_version,\n    COALESCE(CAST(d.max_camera AS SIGNED),0) AS max_camera,\n    COALESCE(cs.camera_in_count,0) AS camera_in_count,\n    CASE\n        WHEN COALESCE(o.status,1)=0 OR d.online_expire_time IS NULL OR NOW() >= d.online_expire_time THEN COALESCE(cs.camera_in_count,0)\n        ELSE COALESCE(cs.camera_off_count,0)\n    END AS camera_off_count,\n    d.register_time AS register_time\n FROM gb28181_oauth o LEFT JOIN gb28181_device d ON d.device_id=o.device_id LEFT JOIN (SELECT device_id,CAST(COUNT(channel_id) AS SIGNED) AS camera_in_count,CAST(SUM(CASE WHEN status IN ('OFF','OFFLINE') THEN 1 ELSE 0 END) AS SIGNED) AS camera_off_count FROM gb28181_device_channel ch WHERE COALESCE((SELECT rc.resource_kind FROM gb28181_resource_confirmation rc WHERE rc.device_id=ch.device_id AND rc.resource_id=ch.channel_id AND rc.status=1),CASE WHEN SUBSTR(ch.channel_id,11,3) IN ('131','132') THEN 'video' ELSE 'other' END)='video' GROUP BY ch.device_id) cs ON cs.device_id=o.device_id WHERE COALESCE(o.del,0)=0 AND (?=0 OR d.register_time IS NOT NULL) ORDER BY o.device_id";
const GB_DEVICE_GET_MYSQL: &str = "SELECT \n    o.device_id AS device_id,\n    o.domain_id AS domain_id,\n    o.domain AS domain,\n    CAST(o.longitude AS CHAR) AS longitude,\n    CAST(o.latitude AS CHAR) AS latitude,\n    o.address AS address,\n    o.pwd AS pwd,\n    COALESCE(o.pwd_check,0) AS pwd_check,\n    o.alias AS alias,\n    COALESCE(o.status,1) AS status,\n    COALESCE(o.heartbeat_sec,60) AS heartbeat_sec,\n    COALESCE(o.snapshot_to_mode,0) AS snapshot_to_mode,\n    COALESCE(o.del,0) AS del,\n    o.create_time AS create_time,\n    CAST(o.tenant_id AS CHAR) AS tenant_id,\n    o.sys_org_code AS sys_org_code,\n    o.create_by AS create_by,\n    o.update_by AS update_by,\n    o.update_time AS update_time,\n    CASE\n        WHEN COALESCE(o.status,1)=1 AND d.online_expire_time IS NOT NULL AND d.online_expire_time > NOW() THEN 1\n        ELSE 0\n    END AS monitor_status,\n    d.device_type AS device_type,\n    d.manufacturer AS manufacturer,\n    d.model AS model,\n    d.firmware AS firmware,\n    d.gb_version AS gb_version,\n    COALESCE(CAST(d.max_camera AS SIGNED),0) AS max_camera,\n    COALESCE(cs.camera_in_count,0) AS camera_in_count,\n    CASE\n        WHEN COALESCE(o.status,1)=0 OR d.online_expire_time IS NULL OR NOW() >= d.online_expire_time THEN COALESCE(cs.camera_in_count,0)\n        ELSE COALESCE(cs.camera_off_count,0)\n    END AS camera_off_count,\n    d.register_time AS register_time\n FROM gb28181_oauth o LEFT JOIN gb28181_device d ON d.device_id=o.device_id LEFT JOIN (SELECT device_id,CAST(COUNT(channel_id) AS SIGNED) AS camera_in_count,CAST(SUM(CASE WHEN status IN ('OFF','OFFLINE') THEN 1 ELSE 0 END) AS SIGNED) AS camera_off_count FROM gb28181_device_channel ch WHERE COALESCE((SELECT rc.resource_kind FROM gb28181_resource_confirmation rc WHERE rc.device_id=ch.device_id AND rc.resource_id=ch.channel_id AND rc.status=1),CASE WHEN SUBSTR(ch.channel_id,11,3) IN ('131','132') THEN 'video' ELSE 'other' END)='video' GROUP BY ch.device_id) cs ON cs.device_id=o.device_id WHERE COALESCE(o.del,0)=0 AND o.device_id=?";
const GB_DEVICE_LIST_SQLITE: &str = "SELECT \n    o.device_id AS device_id,\n    o.domain_id AS domain_id,\n    o.domain AS domain,\n    CAST(o.longitude AS TEXT) AS longitude,\n    CAST(o.latitude AS TEXT) AS latitude,\n    o.address AS address,\n    o.pwd AS pwd,\n    COALESCE(o.pwd_check,0) AS pwd_check,\n    o.alias AS alias,\n    COALESCE(o.status,1) AS status,\n    COALESCE(o.heartbeat_sec,60) AS heartbeat_sec,\n    COALESCE(o.snapshot_to_mode,0) AS snapshot_to_mode,\n    COALESCE(o.del,0) AS del,\n    o.create_time AS create_time,\n    CAST(o.tenant_id AS TEXT) AS tenant_id,\n    o.sys_org_code AS sys_org_code,\n    o.create_by AS create_by,\n    o.update_by AS update_by,\n    o.update_time AS update_time,\n    CASE\n        WHEN COALESCE(o.status,1)=1 AND d.online_expire_time IS NOT NULL AND d.online_expire_time > CURRENT_TIMESTAMP THEN 1\n        ELSE 0\n    END AS monitor_status,\n    d.device_type AS device_type,\n    d.manufacturer AS manufacturer,\n    d.model AS model,\n    d.firmware AS firmware,\n    d.gb_version AS gb_version,\n    COALESCE(d.max_camera,0) AS max_camera,\n    COALESCE(cs.camera_in_count,0) AS camera_in_count,\n    CASE\n        WHEN COALESCE(o.status,1)=0 OR d.online_expire_time IS NULL OR CURRENT_TIMESTAMP >= d.online_expire_time THEN COALESCE(cs.camera_in_count,0)\n        ELSE COALESCE(cs.camera_off_count,0)\n    END AS camera_off_count,\n    d.register_time AS register_time\n FROM gb28181_oauth o LEFT JOIN gb28181_device d ON d.device_id=o.device_id LEFT JOIN (SELECT device_id,COUNT(channel_id) AS camera_in_count,SUM(CASE WHEN status IN ('OFF','OFFLINE') THEN 1 ELSE 0 END) AS camera_off_count FROM gb28181_device_channel ch WHERE COALESCE((SELECT rc.resource_kind FROM gb28181_resource_confirmation rc WHERE rc.device_id=ch.device_id AND rc.resource_id=ch.channel_id AND rc.status=1),CASE WHEN SUBSTR(ch.channel_id,11,3) IN ('131','132') THEN 'video' ELSE 'other' END)='video' GROUP BY ch.device_id) cs ON cs.device_id=o.device_id WHERE COALESCE(o.del,0)=0 AND (?=0 OR d.register_time IS NOT NULL) ORDER BY o.device_id";
const GB_DEVICE_GET_SQLITE: &str = "SELECT \n    o.device_id AS device_id,\n    o.domain_id AS domain_id,\n    o.domain AS domain,\n    CAST(o.longitude AS TEXT) AS longitude,\n    CAST(o.latitude AS TEXT) AS latitude,\n    o.address AS address,\n    o.pwd AS pwd,\n    COALESCE(o.pwd_check,0) AS pwd_check,\n    o.alias AS alias,\n    COALESCE(o.status,1) AS status,\n    COALESCE(o.heartbeat_sec,60) AS heartbeat_sec,\n    COALESCE(o.snapshot_to_mode,0) AS snapshot_to_mode,\n    COALESCE(o.del,0) AS del,\n    o.create_time AS create_time,\n    CAST(o.tenant_id AS TEXT) AS tenant_id,\n    o.sys_org_code AS sys_org_code,\n    o.create_by AS create_by,\n    o.update_by AS update_by,\n    o.update_time AS update_time,\n    CASE\n        WHEN COALESCE(o.status,1)=1 AND d.online_expire_time IS NOT NULL AND d.online_expire_time > CURRENT_TIMESTAMP THEN 1\n        ELSE 0\n    END AS monitor_status,\n    d.device_type AS device_type,\n    d.manufacturer AS manufacturer,\n    d.model AS model,\n    d.firmware AS firmware,\n    d.gb_version AS gb_version,\n    COALESCE(d.max_camera,0) AS max_camera,\n    COALESCE(cs.camera_in_count,0) AS camera_in_count,\n    CASE\n        WHEN COALESCE(o.status,1)=0 OR d.online_expire_time IS NULL OR CURRENT_TIMESTAMP >= d.online_expire_time THEN COALESCE(cs.camera_in_count,0)\n        ELSE COALESCE(cs.camera_off_count,0)\n    END AS camera_off_count,\n    d.register_time AS register_time\n FROM gb28181_oauth o LEFT JOIN gb28181_device d ON d.device_id=o.device_id LEFT JOIN (SELECT device_id,COUNT(channel_id) AS camera_in_count,SUM(CASE WHEN status IN ('OFF','OFFLINE') THEN 1 ELSE 0 END) AS camera_off_count FROM gb28181_device_channel ch WHERE COALESCE((SELECT rc.resource_kind FROM gb28181_resource_confirmation rc WHERE rc.device_id=ch.device_id AND rc.resource_id=ch.channel_id AND rc.status=1),CASE WHEN SUBSTR(ch.channel_id,11,3) IN ('131','132') THEN 'video' ELSE 'other' END)='video' GROUP BY ch.device_id) cs ON cs.device_id=o.device_id WHERE COALESCE(o.del,0)=0 AND o.device_id=?";
const GB_DEVICE_LIST_PAGE_MYSQL: &str = "SELECT \n    o.device_id AS device_id,\n    o.domain_id AS domain_id,\n    o.domain AS domain,\n    CAST(o.longitude AS CHAR) AS longitude,\n    CAST(o.latitude AS CHAR) AS latitude,\n    o.address AS address,\n    o.pwd AS pwd,\n    COALESCE(o.pwd_check,0) AS pwd_check,\n    o.alias AS alias,\n    COALESCE(o.status,1) AS status,\n    COALESCE(o.heartbeat_sec,60) AS heartbeat_sec,\n    COALESCE(o.snapshot_to_mode,0) AS snapshot_to_mode,\n    COALESCE(o.del,0) AS del,\n    o.create_time AS create_time,\n    CAST(o.tenant_id AS CHAR) AS tenant_id,\n    o.sys_org_code AS sys_org_code,\n    o.create_by AS create_by,\n    o.update_by AS update_by,\n    o.update_time AS update_time,\n    CASE\n        WHEN COALESCE(o.status,1)=1 AND d.online_expire_time IS NOT NULL AND d.online_expire_time > NOW() THEN 1\n        ELSE 0\n    END AS monitor_status,\n    d.device_type AS device_type,\n    d.manufacturer AS manufacturer,\n    d.model AS model,\n    d.firmware AS firmware,\n    d.gb_version AS gb_version,\n    COALESCE(CAST(d.max_camera AS SIGNED),0) AS max_camera,\n    COALESCE(cs.camera_in_count,0) AS camera_in_count,\n    CASE\n        WHEN COALESCE(o.status,1)=0 OR d.online_expire_time IS NULL OR NOW() >= d.online_expire_time THEN COALESCE(cs.camera_in_count,0)\n        ELSE COALESCE(cs.camera_off_count,0)\n    END AS camera_off_count,\n    d.register_time AS register_time\n FROM gb28181_oauth o LEFT JOIN gb28181_device d ON d.device_id=o.device_id LEFT JOIN (SELECT device_id,CAST(COUNT(channel_id) AS SIGNED) AS camera_in_count,CAST(SUM(CASE WHEN status IN ('OFF','OFFLINE') THEN 1 ELSE 0 END) AS SIGNED) AS camera_off_count FROM gb28181_device_channel ch WHERE COALESCE((SELECT rc.resource_kind FROM gb28181_resource_confirmation rc WHERE rc.device_id=ch.device_id AND rc.resource_id=ch.channel_id AND rc.status=1),CASE WHEN SUBSTR(ch.channel_id,11,3) IN ('131','132') THEN 'video' ELSE 'other' END)='video' GROUP BY ch.device_id) cs ON cs.device_id=o.device_id WHERE COALESCE(o.del,0)=0 AND (?=0 OR d.register_time IS NOT NULL) ORDER BY o.device_id LIMIT ? OFFSET ?";
const GB_DEVICE_LIST_PAGE_SQLITE: &str = "SELECT \n    o.device_id AS device_id,\n    o.domain_id AS domain_id,\n    o.domain AS domain,\n    CAST(o.longitude AS TEXT) AS longitude,\n    CAST(o.latitude AS TEXT) AS latitude,\n    o.address AS address,\n    o.pwd AS pwd,\n    COALESCE(o.pwd_check,0) AS pwd_check,\n    o.alias AS alias,\n    COALESCE(o.status,1) AS status,\n    COALESCE(o.heartbeat_sec,60) AS heartbeat_sec,\n    COALESCE(o.snapshot_to_mode,0) AS snapshot_to_mode,\n    COALESCE(o.del,0) AS del,\n    o.create_time AS create_time,\n    CAST(o.tenant_id AS TEXT) AS tenant_id,\n    o.sys_org_code AS sys_org_code,\n    o.create_by AS create_by,\n    o.update_by AS update_by,\n    o.update_time AS update_time,\n    CASE\n        WHEN COALESCE(o.status,1)=1 AND d.online_expire_time IS NOT NULL AND d.online_expire_time > CURRENT_TIMESTAMP THEN 1\n        ELSE 0\n    END AS monitor_status,\n    d.device_type AS device_type,\n    d.manufacturer AS manufacturer,\n    d.model AS model,\n    d.firmware AS firmware,\n    d.gb_version AS gb_version,\n    COALESCE(d.max_camera,0) AS max_camera,\n    COALESCE(cs.camera_in_count,0) AS camera_in_count,\n    CASE\n        WHEN COALESCE(o.status,1)=0 OR d.online_expire_time IS NULL OR CURRENT_TIMESTAMP >= d.online_expire_time THEN COALESCE(cs.camera_in_count,0)\n        ELSE COALESCE(cs.camera_off_count,0)\n    END AS camera_off_count,\n    d.register_time AS register_time\n FROM gb28181_oauth o LEFT JOIN gb28181_device d ON d.device_id=o.device_id LEFT JOIN (SELECT device_id,COUNT(channel_id) AS camera_in_count,SUM(CASE WHEN status IN ('OFF','OFFLINE') THEN 1 ELSE 0 END) AS camera_off_count FROM gb28181_device_channel ch WHERE COALESCE((SELECT rc.resource_kind FROM gb28181_resource_confirmation rc WHERE rc.device_id=ch.device_id AND rc.resource_id=ch.channel_id AND rc.status=1),CASE WHEN SUBSTR(ch.channel_id,11,3) IN ('131','132') THEN 'video' ELSE 'other' END)='video' GROUP BY ch.device_id) cs ON cs.device_id=o.device_id WHERE COALESCE(o.del,0)=0 AND (?=0 OR d.register_time IS NOT NULL) ORDER BY o.device_id LIMIT ? OFFSET ?";
const GB_DEVICE_LIST_PAGE_BY_DOMAIN_FILTER_MYSQL: &str = "SELECT \n    o.device_id AS device_id,\n    o.domain_id AS domain_id,\n    o.domain AS domain,\n    CAST(o.longitude AS CHAR) AS longitude,\n    CAST(o.latitude AS CHAR) AS latitude,\n    o.address AS address,\n    o.pwd AS pwd,\n    COALESCE(o.pwd_check,0) AS pwd_check,\n    o.alias AS alias,\n    COALESCE(o.status,1) AS status,\n    COALESCE(o.heartbeat_sec,60) AS heartbeat_sec,\n    COALESCE(o.snapshot_to_mode,0) AS snapshot_to_mode,\n    COALESCE(o.del,0) AS del,\n    o.create_time AS create_time,\n    CAST(o.tenant_id AS CHAR) AS tenant_id,\n    o.sys_org_code AS sys_org_code,\n    o.create_by AS create_by,\n    o.update_by AS update_by,\n    o.update_time AS update_time,\n    CASE\n        WHEN COALESCE(o.status,1)=1 AND d.online_expire_time IS NOT NULL AND d.online_expire_time > NOW() THEN 1\n        ELSE 0\n    END AS monitor_status,\n    d.device_type AS device_type,\n    d.manufacturer AS manufacturer,\n    d.model AS model,\n    d.firmware AS firmware,\n    d.gb_version AS gb_version,\n    COALESCE(CAST(d.max_camera AS SIGNED),0) AS max_camera,\n    COALESCE(cs.camera_in_count,0) AS camera_in_count,\n    CASE\n        WHEN COALESCE(o.status,1)=0 OR d.online_expire_time IS NULL OR NOW() >= d.online_expire_time THEN COALESCE(cs.camera_in_count,0)\n        ELSE COALESCE(cs.camera_off_count,0)\n    END AS camera_off_count,\n    d.register_time AS register_time\n FROM gb28181_oauth o LEFT JOIN gb28181_device d ON d.device_id=o.device_id LEFT JOIN (SELECT device_id,CAST(COUNT(channel_id) AS SIGNED) AS camera_in_count,CAST(SUM(CASE WHEN status IN ('OFF','OFFLINE') THEN 1 ELSE 0 END) AS SIGNED) AS camera_off_count FROM gb28181_device_channel ch WHERE COALESCE((SELECT rc.resource_kind FROM gb28181_resource_confirmation rc WHERE rc.device_id=ch.device_id AND rc.resource_id=ch.channel_id AND rc.status=1),CASE WHEN SUBSTR(ch.channel_id,11,3) IN ('131','132') THEN 'video' ELSE 'other' END)='video' GROUP BY ch.device_id) cs ON cs.device_id=o.device_id WHERE COALESCE(o.del,0)=0 AND o.domain_id=? AND (?='' OR o.device_id LIKE ?) AND (?='' OR o.alias LIKE ?) AND (?=0 OR d.register_time IS NOT NULL) ORDER BY o.device_id LIMIT ? OFFSET ?";
const GB_DEVICE_LIST_PAGE_BY_DOMAIN_FILTER_SQLITE: &str = "SELECT \n    o.device_id AS device_id,\n    o.domain_id AS domain_id,\n    o.domain AS domain,\n    CAST(o.longitude AS TEXT) AS longitude,\n    CAST(o.latitude AS TEXT) AS latitude,\n    o.address AS address,\n    o.pwd AS pwd,\n    COALESCE(o.pwd_check,0) AS pwd_check,\n    o.alias AS alias,\n    COALESCE(o.status,1) AS status,\n    COALESCE(o.heartbeat_sec,60) AS heartbeat_sec,\n    COALESCE(o.snapshot_to_mode,0) AS snapshot_to_mode,\n    COALESCE(o.del,0) AS del,\n    o.create_time AS create_time,\n    CAST(o.tenant_id AS TEXT) AS tenant_id,\n    o.sys_org_code AS sys_org_code,\n    o.create_by AS create_by,\n    o.update_by AS update_by,\n    o.update_time AS update_time,\n    CASE\n        WHEN COALESCE(o.status,1)=1 AND d.online_expire_time IS NOT NULL AND d.online_expire_time > CURRENT_TIMESTAMP THEN 1\n        ELSE 0\n    END AS monitor_status,\n    d.device_type AS device_type,\n    d.manufacturer AS manufacturer,\n    d.model AS model,\n    d.firmware AS firmware,\n    d.gb_version AS gb_version,\n    COALESCE(d.max_camera,0) AS max_camera,\n    COALESCE(cs.camera_in_count,0) AS camera_in_count,\n    CASE\n        WHEN COALESCE(o.status,1)=0 OR d.online_expire_time IS NULL OR CURRENT_TIMESTAMP >= d.online_expire_time THEN COALESCE(cs.camera_in_count,0)\n        ELSE COALESCE(cs.camera_off_count,0)\n    END AS camera_off_count,\n    d.register_time AS register_time\n FROM gb28181_oauth o LEFT JOIN gb28181_device d ON d.device_id=o.device_id LEFT JOIN (SELECT device_id,COUNT(channel_id) AS camera_in_count,SUM(CASE WHEN status IN ('OFF','OFFLINE') THEN 1 ELSE 0 END) AS camera_off_count FROM gb28181_device_channel ch WHERE COALESCE((SELECT rc.resource_kind FROM gb28181_resource_confirmation rc WHERE rc.device_id=ch.device_id AND rc.resource_id=ch.channel_id AND rc.status=1),CASE WHEN SUBSTR(ch.channel_id,11,3) IN ('131','132') THEN 'video' ELSE 'other' END)='video' GROUP BY ch.device_id) cs ON cs.device_id=o.device_id WHERE COALESCE(o.del,0)=0 AND o.domain_id=? AND (?='' OR o.device_id LIKE ?) AND (?='' OR o.alias LIKE ?) AND (?=0 OR d.register_time IS NOT NULL) ORDER BY o.device_id LIMIT ? OFFSET ?";

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
    pub broadcast_enable: i64,
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

    pub async fn update_config(channel: GbChannelConfigUpdate) -> GlobalResult<Option<Self>> {
        if Self::get(&channel.device_id, &channel.channel_id)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        match db::backend() {
            db::SessionDatabaseBackend::Mysql => {
                db::execute!(
                    r#"INSERT INTO gb28181_device_channel_conf
                    (device_id,channel_id,alias_name,ptz_enable,broadcast_enable,audio_enable,snapshot_enable,record_enable,playback_enable,alarm_enable,biz_enable,sort_no,over_pic_id,create_time,update_time)
                    VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)
                    ON DUPLICATE KEY UPDATE
                    alias_name=VALUES(alias_name),
                    ptz_enable=VALUES(ptz_enable),
                    broadcast_enable=VALUES(broadcast_enable),
                    audio_enable=VALUES(audio_enable),
                    snapshot_enable=VALUES(snapshot_enable),
                    record_enable=VALUES(record_enable),
                    playback_enable=VALUES(playback_enable),
                    alarm_enable=VALUES(alarm_enable),
                    biz_enable=VALUES(biz_enable),
                    sort_no=VALUES(sort_no),
                    over_pic_id=VALUES(over_pic_id),
                    update_time=CURRENT_TIMESTAMP"#,
                    &channel.device_id,
                    &channel.channel_id,
                    empty_string_to_none(channel.alias_name),
                    channel.ptz_enable,
                    channel.broadcast_enable,
                    channel.audio_enable,
                    channel.snapshot,
                    channel.record_enable,
                    channel.playback_enable,
                    channel.alarm_enable,
                    channel.biz_enable,
                    channel.sort_no,
                    empty_string_to_i64(channel.over_pic_id),
                )
                .hand_log(|msg| error!("{msg}"))?;
            }
            db::SessionDatabaseBackend::Sqlite => {
                db::execute!(
                    r#"INSERT INTO gb28181_device_channel_conf
                    (device_id,channel_id,alias_name,ptz_enable,broadcast_enable,audio_enable,snapshot_enable,record_enable,playback_enable,alarm_enable,biz_enable,sort_no,over_pic_id,create_time,update_time)
                    VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)
                    ON CONFLICT(device_id, channel_id) DO UPDATE SET
                    alias_name=excluded.alias_name,
                    ptz_enable=excluded.ptz_enable,
                    broadcast_enable=excluded.broadcast_enable,
                    audio_enable=excluded.audio_enable,
                    snapshot_enable=excluded.snapshot_enable,
                    record_enable=excluded.record_enable,
                    playback_enable=excluded.playback_enable,
                    alarm_enable=excluded.alarm_enable,
                    biz_enable=excluded.biz_enable,
                    sort_no=excluded.sort_no,
                    over_pic_id=excluded.over_pic_id,
                    update_time=CURRENT_TIMESTAMP"#,
                    &channel.device_id,
                    &channel.channel_id,
                    empty_string_to_none(channel.alias_name),
                    channel.ptz_enable,
                    channel.broadcast_enable,
                    channel.audio_enable,
                    channel.snapshot,
                    channel.record_enable,
                    channel.playback_enable,
                    channel.alarm_enable,
                    channel.biz_enable,
                    channel.sort_no,
                    empty_string_to_i64(channel.over_pic_id),
                )
                .hand_log(|msg| error!("{msg}"))?;
            }
        }
        Self::get(&channel.device_id, &channel.channel_id).await
    }

    pub async fn set_cover_image(
        device_id: &str,
        channel_id: &str,
        image_id: &str,
    ) -> GlobalResult<Option<Self>> {
        if Self::get(device_id, channel_id).await?.is_none() {
            return Ok(None);
        }
        let image_id = image_id.parse::<i64>().map_err(|_| {
            GlobalError::new_sys_error("invalid GB28181 image id", |msg| error!("{msg}"))
        })?;
        match db::backend() {
            db::SessionDatabaseBackend::Mysql => {
                db::execute!(
                    r#"INSERT INTO gb28181_device_channel_conf
                    (device_id,channel_id,over_pic_id,create_time,update_time)
                    VALUES (?,?,?,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)
                    ON DUPLICATE KEY UPDATE over_pic_id=VALUES(over_pic_id),update_time=CURRENT_TIMESTAMP"#,
                    device_id,
                    channel_id,
                    image_id,
                )
                .hand_log(|msg| error!("{msg}"))?;
            }
            db::SessionDatabaseBackend::Sqlite => {
                db::execute!(
                    r#"INSERT INTO gb28181_device_channel_conf
                    (device_id,channel_id,over_pic_id,create_time,update_time)
                    VALUES (?,?,?,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)
                    ON CONFLICT(device_id, channel_id) DO UPDATE SET
                    over_pic_id=excluded.over_pic_id,update_time=CURRENT_TIMESTAMP"#,
                    device_id,
                    channel_id,
                    image_id,
                )
                .hand_log(|msg| error!("{msg}"))?;
            }
        }
        Self::get(device_id, channel_id).await
    }
}

#[derive(Debug, Clone, Default, FromRow)]
pub struct GbChannelCoverView {
    pub channel_id: String,
    pub cover_image_id: String,
}

impl GbChannelCoverView {
    pub async fn list(device_id: &str) -> GlobalResult<Vec<Self>> {
        let sql = match db::backend() {
            db::SessionDatabaseBackend::Mysql => GB_CHANNEL_COVER_LIST_MYSQL,
            db::SessionDatabaseBackend::Sqlite => GB_CHANNEL_COVER_LIST_SQLITE,
        };
        db::fetch_all_as!(Self, sql, device_id).hand_log(|msg| error!("{msg}"))
    }
}

const GB_CHANNEL_COVER_LIST_MYSQL: &str = r#"SELECT c.channel_id,
COALESCE(
  (SELECT CAST(selected.id AS CHAR) FROM gb28181_file_info selected
   WHERE selected.id=conf.over_pic_id AND selected.device_id=c.device_id AND selected.channel_id=c.channel_id
     AND COALESCE(selected.is_del,0)=0 AND COALESCE(selected.file_type,0)=0 LIMIT 1),
  (SELECT CAST(latest.id AS CHAR) FROM gb28181_file_info latest
   WHERE latest.device_id=c.device_id AND latest.channel_id=c.channel_id
     AND COALESCE(latest.is_del,0)=0 AND COALESCE(latest.file_type,0)=0
   ORDER BY latest.create_time DESC,latest.id DESC LIMIT 1),
  '') AS cover_image_id
FROM gb28181_device_channel c
LEFT JOIN gb28181_device_channel_conf conf ON conf.device_id=c.device_id AND conf.channel_id=c.channel_id
WHERE c.device_id=?"#;
const GB_CHANNEL_COVER_LIST_SQLITE: &str = r#"SELECT c.channel_id,
COALESCE(
  (SELECT CAST(selected.id AS TEXT) FROM gb28181_file_info selected
   WHERE selected.id=conf.over_pic_id AND selected.device_id=c.device_id AND selected.channel_id=c.channel_id
     AND COALESCE(selected.is_del,0)=0 AND COALESCE(selected.file_type,0)=0 LIMIT 1),
  (SELECT CAST(latest.id AS TEXT) FROM gb28181_file_info latest
   WHERE latest.device_id=c.device_id AND latest.channel_id=c.channel_id
     AND COALESCE(latest.is_del,0)=0 AND COALESCE(latest.file_type,0)=0
   ORDER BY latest.create_time DESC,latest.id DESC LIMIT 1),
  '') AS cover_image_id
FROM gb28181_device_channel c
LEFT JOIN gb28181_device_channel_conf conf ON conf.device_id=c.device_id AND conf.channel_id=c.channel_id
WHERE c.device_id=?"#;

#[derive(Debug, Clone, Default)]
pub struct GbChannelConfigUpdate {
    pub device_id: String,
    pub channel_id: String,
    pub alias_name: String,
    pub snapshot: i64,
    pub over_pic_id: String,
    pub ptz_enable: i64,
    pub broadcast_enable: i64,
    pub audio_enable: i64,
    pub record_enable: i64,
    pub playback_enable: i64,
    pub alarm_enable: i64,
    pub biz_enable: i64,
    pub sort_no: i64,
}

const GB_CHANNEL_COLUMNS_MYSQL: &str = r#"
    c.device_id AS device_id,
    c.channel_id AS channel_id,
    COALESCE(c.name,'') AS name,
    COALESCE(c.manufacturer,'') AS manufacturer,
    COALESCE(c.model,'') AS model,
    COALESCE(c.owner,'') AS owner,
    COALESCE(c.status,'UNKNOWN') AS status,
    COALESCE(c.civil_code,'') AS civil_code,
    COALESCE(c.address,'') AS address,
    COALESCE(c.parent_id,'') AS parent_id,
    COALESCE(c.ip_address,'') AS ip_address,
    COALESCE(c.port,0) AS port,
    COALESCE(CAST(c.longitude AS CHAR),'') AS longitude,
    COALESCE(CAST(c.latitude AS CHAR),'') AS latitude,
    COALESCE(c.ptz_type,'') AS ptz_type,
    COALESCE(conf.alias_name,'') AS alias_name,
    '' AS pic_url,
    COALESCE(conf.snapshot_enable,0) AS snapshot,
    COALESCE(CAST(conf.over_pic_id AS CHAR),'') AS over_pic_id,
    COALESCE(conf.ptz_enable,0) AS ptz_enable,
    COALESCE(conf.broadcast_enable,0) AS broadcast_enable,
    COALESCE(conf.audio_enable,0) AS audio_enable,
    COALESCE(conf.record_enable,0) AS record_enable,
    COALESCE(conf.playback_enable,0) AS playback_enable,
    COALESCE(conf.alarm_enable,0) AS alarm_enable,
    COALESCE(conf.biz_enable,0) AS biz_enable,
    COALESCE(conf.sort_no,0) AS sort_no,
    conf.create_time AS created_at,
    conf.update_time AS updated_at
"#;
const GB_CHANNEL_COLUMNS_SQLITE: &str = r#"
    c.device_id AS device_id,
    c.channel_id AS channel_id,
    COALESCE(c.name,'') AS name,
    COALESCE(c.manufacturer,'') AS manufacturer,
    COALESCE(c.model,'') AS model,
    COALESCE(c.owner,'') AS owner,
    COALESCE(c.status,'UNKNOWN') AS status,
    COALESCE(c.civil_code,'') AS civil_code,
    COALESCE(c.address,'') AS address,
    COALESCE(c.parent_id,'') AS parent_id,
    COALESCE(c.ip_address,'') AS ip_address,
    COALESCE(c.port,0) AS port,
    COALESCE(CAST(c.longitude AS TEXT),'') AS longitude,
    COALESCE(CAST(c.latitude AS TEXT),'') AS latitude,
    COALESCE(c.ptz_type,'') AS ptz_type,
    COALESCE(conf.alias_name,'') AS alias_name,
    '' AS pic_url,
    COALESCE(conf.snapshot_enable,0) AS snapshot,
    COALESCE(CAST(conf.over_pic_id AS TEXT),'') AS over_pic_id,
    COALESCE(conf.ptz_enable,0) AS ptz_enable,
    COALESCE(conf.broadcast_enable,0) AS broadcast_enable,
    COALESCE(conf.audio_enable,0) AS audio_enable,
    COALESCE(conf.record_enable,0) AS record_enable,
    COALESCE(conf.playback_enable,0) AS playback_enable,
    COALESCE(conf.alarm_enable,0) AS alarm_enable,
    COALESCE(conf.biz_enable,0) AS biz_enable,
    COALESCE(conf.sort_no,0) AS sort_no,
    conf.create_time AS created_at,
    conf.update_time AS updated_at
"#;
const GB_CHANNEL_LIST_MYSQL: &str = "SELECT \n    c.device_id AS device_id,\n    c.channel_id AS channel_id,\n    COALESCE(c.name,'') AS name,\n    COALESCE(c.manufacturer,'') AS manufacturer,\n    COALESCE(c.model,'') AS model,\n    COALESCE(c.owner,'') AS owner,\n    CASE\n        WHEN COALESCE(o.status,1)=0 OR d.online_expire_time IS NULL OR NOW() >= d.online_expire_time THEN 'OFFLINE'\n        ELSE COALESCE(c.status,'UNKNOWN')\n    END AS status,\n    COALESCE(c.civil_code,'') AS civil_code,\n    COALESCE(c.address,'') AS address,\n    COALESCE(c.parent_id,'') AS parent_id,\n    COALESCE(c.ip_address,'') AS ip_address,\n    COALESCE(c.port,0) AS port,\n    COALESCE(CAST(c.longitude AS CHAR),'') AS longitude,\n    COALESCE(CAST(c.latitude AS CHAR),'') AS latitude,\n    COALESCE(c.ptz_type,'') AS ptz_type,\n    COALESCE(conf.alias_name,'') AS alias_name,\n    '' AS pic_url,\n    COALESCE(conf.snapshot_enable,0) AS snapshot,\n    COALESCE(CAST(conf.over_pic_id AS CHAR),'') AS over_pic_id,\n    COALESCE(conf.ptz_enable,0) AS ptz_enable,\n    COALESCE(conf.broadcast_enable,0) AS broadcast_enable,\n    COALESCE(conf.audio_enable,0) AS audio_enable,\n    COALESCE(conf.record_enable,0) AS record_enable,\n    COALESCE(conf.playback_enable,0) AS playback_enable,\n    COALESCE(conf.alarm_enable,0) AS alarm_enable,\n    COALESCE(conf.biz_enable,0) AS biz_enable,\n    COALESCE(conf.sort_no,0) AS sort_no,\n    conf.create_time AS created_at,\n    conf.update_time AS updated_at\n FROM gb28181_device_channel c LEFT JOIN gb28181_device_channel_conf conf ON conf.device_id=c.device_id AND conf.channel_id=c.channel_id LEFT JOIN gb28181_device d ON d.device_id=c.device_id LEFT JOIN gb28181_oauth o ON o.device_id=c.device_id WHERE c.device_id=? ORDER BY COALESCE(conf.sort_no,0),c.channel_id";
const GB_CHANNEL_GET_MYSQL: &str = "SELECT \n    c.device_id AS device_id,\n    c.channel_id AS channel_id,\n    COALESCE(c.name,'') AS name,\n    COALESCE(c.manufacturer,'') AS manufacturer,\n    COALESCE(c.model,'') AS model,\n    COALESCE(c.owner,'') AS owner,\n    CASE\n        WHEN COALESCE(o.status,1)=0 OR d.online_expire_time IS NULL OR NOW() >= d.online_expire_time THEN 'OFFLINE'\n        ELSE COALESCE(c.status,'UNKNOWN')\n    END AS status,\n    COALESCE(c.civil_code,'') AS civil_code,\n    COALESCE(c.address,'') AS address,\n    COALESCE(c.parent_id,'') AS parent_id,\n    COALESCE(c.ip_address,'') AS ip_address,\n    COALESCE(c.port,0) AS port,\n    COALESCE(CAST(c.longitude AS CHAR),'') AS longitude,\n    COALESCE(CAST(c.latitude AS CHAR),'') AS latitude,\n    COALESCE(c.ptz_type,'') AS ptz_type,\n    COALESCE(conf.alias_name,'') AS alias_name,\n    '' AS pic_url,\n    COALESCE(conf.snapshot_enable,0) AS snapshot,\n    COALESCE(CAST(conf.over_pic_id AS CHAR),'') AS over_pic_id,\n    COALESCE(conf.ptz_enable,0) AS ptz_enable,\n    COALESCE(conf.broadcast_enable,0) AS broadcast_enable,\n    COALESCE(conf.audio_enable,0) AS audio_enable,\n    COALESCE(conf.record_enable,0) AS record_enable,\n    COALESCE(conf.playback_enable,0) AS playback_enable,\n    COALESCE(conf.alarm_enable,0) AS alarm_enable,\n    COALESCE(conf.biz_enable,0) AS biz_enable,\n    COALESCE(conf.sort_no,0) AS sort_no,\n    conf.create_time AS created_at,\n    conf.update_time AS updated_at\n FROM gb28181_device_channel c LEFT JOIN gb28181_device_channel_conf conf ON conf.device_id=c.device_id AND conf.channel_id=c.channel_id LEFT JOIN gb28181_device d ON d.device_id=c.device_id LEFT JOIN gb28181_oauth o ON o.device_id=c.device_id WHERE c.device_id=? AND c.channel_id=?";
const GB_CHANNEL_LIST_SQLITE: &str = "SELECT \n    c.device_id AS device_id,\n    c.channel_id AS channel_id,\n    COALESCE(c.name,'') AS name,\n    COALESCE(c.manufacturer,'') AS manufacturer,\n    COALESCE(c.model,'') AS model,\n    COALESCE(c.owner,'') AS owner,\n    CASE\n        WHEN COALESCE(o.status,1)=0 OR d.online_expire_time IS NULL OR CURRENT_TIMESTAMP >= d.online_expire_time THEN 'OFFLINE'\n        ELSE COALESCE(c.status,'UNKNOWN')\n    END AS status,\n    COALESCE(c.civil_code,'') AS civil_code,\n    COALESCE(c.address,'') AS address,\n    COALESCE(c.parent_id,'') AS parent_id,\n    COALESCE(c.ip_address,'') AS ip_address,\n    COALESCE(c.port,0) AS port,\n    COALESCE(CAST(c.longitude AS TEXT),'') AS longitude,\n    COALESCE(CAST(c.latitude AS TEXT),'') AS latitude,\n    COALESCE(c.ptz_type,'') AS ptz_type,\n    COALESCE(conf.alias_name,'') AS alias_name,\n    '' AS pic_url,\n    COALESCE(conf.snapshot_enable,0) AS snapshot,\n    COALESCE(CAST(conf.over_pic_id AS TEXT),'') AS over_pic_id,\n    COALESCE(conf.ptz_enable,0) AS ptz_enable,\n    COALESCE(conf.broadcast_enable,0) AS broadcast_enable,\n    COALESCE(conf.audio_enable,0) AS audio_enable,\n    COALESCE(conf.record_enable,0) AS record_enable,\n    COALESCE(conf.playback_enable,0) AS playback_enable,\n    COALESCE(conf.alarm_enable,0) AS alarm_enable,\n    COALESCE(conf.biz_enable,0) AS biz_enable,\n    COALESCE(conf.sort_no,0) AS sort_no,\n    conf.create_time AS created_at,\n    conf.update_time AS updated_at\n FROM gb28181_device_channel c LEFT JOIN gb28181_device_channel_conf conf ON conf.device_id=c.device_id AND conf.channel_id=c.channel_id LEFT JOIN gb28181_device d ON d.device_id=c.device_id LEFT JOIN gb28181_oauth o ON o.device_id=c.device_id WHERE c.device_id=? ORDER BY COALESCE(conf.sort_no,0),c.channel_id";
const GB_CHANNEL_GET_SQLITE: &str = "SELECT \n    c.device_id AS device_id,\n    c.channel_id AS channel_id,\n    COALESCE(c.name,'') AS name,\n    COALESCE(c.manufacturer,'') AS manufacturer,\n    COALESCE(c.model,'') AS model,\n    COALESCE(c.owner,'') AS owner,\n    CASE\n        WHEN COALESCE(o.status,1)=0 OR d.online_expire_time IS NULL OR CURRENT_TIMESTAMP >= d.online_expire_time THEN 'OFFLINE'\n        ELSE COALESCE(c.status,'UNKNOWN')\n    END AS status,\n    COALESCE(c.civil_code,'') AS civil_code,\n    COALESCE(c.address,'') AS address,\n    COALESCE(c.parent_id,'') AS parent_id,\n    COALESCE(c.ip_address,'') AS ip_address,\n    COALESCE(c.port,0) AS port,\n    COALESCE(CAST(c.longitude AS TEXT),'') AS longitude,\n    COALESCE(CAST(c.latitude AS TEXT),'') AS latitude,\n    COALESCE(c.ptz_type,'') AS ptz_type,\n    COALESCE(conf.alias_name,'') AS alias_name,\n    '' AS pic_url,\n    COALESCE(conf.snapshot_enable,0) AS snapshot,\n    COALESCE(CAST(conf.over_pic_id AS TEXT),'') AS over_pic_id,\n    COALESCE(conf.ptz_enable,0) AS ptz_enable,\n    COALESCE(conf.broadcast_enable,0) AS broadcast_enable,\n    COALESCE(conf.audio_enable,0) AS audio_enable,\n    COALESCE(conf.record_enable,0) AS record_enable,\n    COALESCE(conf.playback_enable,0) AS playback_enable,\n    COALESCE(conf.alarm_enable,0) AS alarm_enable,\n    COALESCE(conf.biz_enable,0) AS biz_enable,\n    COALESCE(conf.sort_no,0) AS sort_no,\n    conf.create_time AS created_at,\n    conf.update_time AS updated_at\n FROM gb28181_device_channel c LEFT JOIN gb28181_device_channel_conf conf ON conf.device_id=c.device_id AND conf.channel_id=c.channel_id LEFT JOIN gb28181_device d ON d.device_id=c.device_id LEFT JOIN gb28181_oauth o ON o.device_id=c.device_id WHERE c.device_id=? AND c.channel_id=?";

#[derive(Debug, Clone, Default, FromRow)]
pub struct GbChannelImageView {
    pub image_id: String,
    pub device_id: String,
    pub channel_id: String,
    pub created_at: Option<NaiveDateTime>,
    pub file_name: String,
    pub file_format: String,
    pub file_size: i64,
    pub dir_path: String,
    pub abs_path: Option<String>,
}

#[derive(Debug, Clone, Default, FromRow)]
struct CountRow {
    total: i64,
}

impl GbChannelImageView {
    pub async fn list(
        device_id: &str,
        channel_id: &str,
        start_time: Option<NaiveDateTime>,
        end_time: Option<NaiveDateTime>,
        page: i64,
        page_size: i64,
    ) -> GlobalResult<(Vec<Self>, i64)> {
        let sql = match db::backend() {
            db::SessionDatabaseBackend::Mysql => GB_CHANNEL_IMAGE_LIST_MYSQL,
            db::SessionDatabaseBackend::Sqlite => GB_CHANNEL_IMAGE_LIST_SQLITE,
        };
        let count_sql = match db::backend() {
            db::SessionDatabaseBackend::Mysql => GB_CHANNEL_IMAGE_COUNT_MYSQL,
            db::SessionDatabaseBackend::Sqlite => GB_CHANNEL_IMAGE_COUNT_SQLITE,
        };
        let offset = (page - 1) * page_size;
        let start_enabled = i64::from(start_time.is_some());
        let end_enabled = i64::from(end_time.is_some());
        let default_time = NaiveDateTime::default();
        let start_time = start_time.unwrap_or(default_time);
        let end_time = end_time.unwrap_or(default_time);
        let images = db::fetch_all_as!(
            Self,
            sql,
            device_id,
            channel_id,
            start_enabled,
            start_time,
            end_enabled,
            end_time,
            page_size,
            offset
        )
        .hand_log(|msg| error!("{msg}"))?;
        let total = db::fetch_optional_as!(
            CountRow,
            count_sql,
            device_id,
            channel_id,
            start_enabled,
            start_time,
            end_enabled,
            end_time
        )
        .hand_log(|msg| error!("{msg}"))?
        .map(|row| row.total)
        .unwrap_or_default();
        Ok((images, total))
    }

    pub async fn get(
        image_id: &str,
        device_id: &str,
        channel_id: &str,
    ) -> GlobalResult<Option<Self>> {
        let sql = match db::backend() {
            db::SessionDatabaseBackend::Mysql => GB_CHANNEL_IMAGE_GET_MYSQL,
            db::SessionDatabaseBackend::Sqlite => GB_CHANNEL_IMAGE_GET_SQLITE,
        };
        db::fetch_optional_as!(Self, sql, image_id, device_id, channel_id)
            .hand_log(|msg| error!("{msg}"))
    }
}

const GB_CHANNEL_IMAGE_LIST_MYSQL: &str = "SELECT CAST(id AS CHAR) AS image_id,device_id,channel_id,create_time AS created_at,file_name,COALESCE(file_format,'') AS file_format,CAST(COALESCE(file_size,0) AS SIGNED) AS file_size,dir_path,abs_path FROM gb28181_file_info WHERE device_id=? AND channel_id=? AND COALESCE(is_del,0)=0 AND COALESCE(file_type,0)=0 AND (?=0 OR create_time>=?) AND (?=0 OR create_time<=?) ORDER BY create_time DESC,id DESC LIMIT ? OFFSET ?";
const GB_CHANNEL_IMAGE_LIST_SQLITE: &str = "SELECT CAST(id AS TEXT) AS image_id,device_id,channel_id,create_time AS created_at,file_name,COALESCE(file_format,'') AS file_format,COALESCE(file_size,0) AS file_size,dir_path,abs_path FROM gb28181_file_info WHERE device_id=? AND channel_id=? AND COALESCE(is_del,0)=0 AND COALESCE(file_type,0)=0 AND (?=0 OR create_time>=?) AND (?=0 OR create_time<=?) ORDER BY create_time DESC,id DESC LIMIT ? OFFSET ?";
const GB_CHANNEL_IMAGE_COUNT_MYSQL: &str = "SELECT CAST(COUNT(*) AS SIGNED) AS total FROM gb28181_file_info WHERE device_id=? AND channel_id=? AND COALESCE(is_del,0)=0 AND COALESCE(file_type,0)=0 AND (?=0 OR create_time>=?) AND (?=0 OR create_time<=?)";
const GB_CHANNEL_IMAGE_COUNT_SQLITE: &str = "SELECT COUNT(*) AS total FROM gb28181_file_info WHERE device_id=? AND channel_id=? AND COALESCE(is_del,0)=0 AND COALESCE(file_type,0)=0 AND (?=0 OR create_time>=?) AND (?=0 OR create_time<=?)";
const GB_CHANNEL_IMAGE_GET_MYSQL: &str = "SELECT CAST(id AS CHAR) AS image_id,device_id,channel_id,create_time AS created_at,file_name,COALESCE(file_format,'') AS file_format,CAST(COALESCE(file_size,0) AS SIGNED) AS file_size,dir_path,abs_path FROM gb28181_file_info WHERE id=? AND device_id=? AND channel_id=? AND COALESCE(is_del,0)=0 AND COALESCE(file_type,0)=0";
const GB_CHANNEL_IMAGE_GET_SQLITE: &str = "SELECT CAST(id AS TEXT) AS image_id,device_id,channel_id,create_time AS created_at,file_name,COALESCE(file_format,'') AS file_format,COALESCE(file_size,0) AS file_size,dir_path,abs_path FROM gb28181_file_info WHERE id=? AND device_id=? AND channel_id=? AND COALESCE(is_del,0)=0 AND COALESCE(file_type,0)=0";

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

    #[test]
    fn mysql_image_queries_decode_file_size_as_i64() {
        let signed_file_size = "CAST(COALESCE(file_size,0) AS SIGNED) AS file_size";

        assert!(GB_CHANNEL_IMAGE_LIST_MYSQL.contains(signed_file_size));
        assert!(GB_CHANNEL_IMAGE_GET_MYSQL.contains(signed_file_size));
    }
}
