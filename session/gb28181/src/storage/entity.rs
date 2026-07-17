use std::sync::Arc;

use crate::storage::db;
use base::chrono::{Local, NaiveDateTime};
use base::constructor::New;
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use base::serde::{Deserialize, Serialize};
use base::serde_default;
use base_db::sqlx;
#[cfg(feature = "db-mysql")]
use base_db::sqlx::MySql;
#[cfg(feature = "db-sqlite")]
use base_db::sqlx::Sqlite;
use sqlx::FromRow;

#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::{Mutex, OnceLock};

#[cfg(test)]
static TEST_STORAGE_ENABLED: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
static TEST_STORAGE: OnceLock<Mutex<TestStorage>> = OnceLock::new();
#[cfg(test)]
pub(crate) static TEST_STORAGE_TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
#[derive(Default)]
struct TestStorage {
    oauths: HashMap<String, GmvOauth>,
    devices: HashMap<String, GmvDevice>,
    channels: Vec<GmvDeviceChannel>,
}

#[cfg(test)]
fn test_storage() -> &'static Mutex<TestStorage> {
    TEST_STORAGE.get_or_init(|| Mutex::new(TestStorage::default()))
}

#[cfg(test)]
fn use_test_storage() -> bool {
    TEST_STORAGE_ENABLED.load(Ordering::Acquire)
}

#[cfg(test)]
pub(crate) fn test_storage_enabled() -> bool {
    use_test_storage()
}

#[cfg(test)]
pub(crate) struct TestStorageGuard;

#[cfg(test)]
impl Drop for TestStorageGuard {
    fn drop(&mut self) {
        TEST_STORAGE_ENABLED.store(false, Ordering::Release);
    }
}

#[cfg(test)]
pub(crate) fn enable_test_storage(oauth: GmvOauth) -> TestStorageGuard {
    let mut storage = test_storage()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *storage = TestStorage::default();
    storage.oauths.insert(oauth.device_id.clone(), oauth);
    TEST_STORAGE_ENABLED.store(true, Ordering::Release);
    TestStorageGuard
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, New, FromRow)]
#[serde(crate = "base::serde")]
pub struct GmvOauth {
    pub device_id: String,
    pub domain_id: String,
    pub domain: String,
    pub pwd: Option<String>,
    //0-false,1-true
    pub pwd_check: i64,
    pub alias: Option<String>,
    //0-停用,1-启用
    pub status: i64,
    // 默认60
    #[serde(default = "default_heartbeat_sec")]
    pub heartbeat_sec: i64,
}
serde_default!(default_heartbeat_sec, i64, 60);

const GMV_OAUTH_SELECT_FIELDS: &str = "\
device_id,\
domain_id,\
domain,\
pwd,\
COALESCE(pwd_check,0) AS pwd_check,\
alias,\
COALESCE(status,1) AS status,\
COALESCE(heartbeat_sec,60) AS heartbeat_sec";

const GMV_OAUTH_SELECT_BY_DEVICE_ID: &str = "\
select \
device_id,\
domain_id,\
domain,\
pwd,\
COALESCE(pwd_check,0) AS pwd_check,\
alias,\
COALESCE(status,1) AS status,\
COALESCE(heartbeat_sec,60) AS heartbeat_sec \
from gb28181_oauth \
where device_id=? and COALESCE(del,0)=0 and COALESCE(status,1)=1";

impl GmvOauth {
    pub fn heartbeat_sec_u8(&self) -> GlobalResult<u8> {
        if self.heartbeat_sec <= 0 {
            return Err(GlobalError::new_biz_error(
                BaseErrorCode::InvalidRequest.code(),
                "heartbeat_sec must be greater than zero",
                |msg| error!("{msg}"),
            ));
        }
        u8::try_from(self.heartbeat_sec).map_err(|_| {
            GlobalError::new_biz_error(
                BaseErrorCode::InvalidRequest.code(),
                "heartbeat_sec must fit u8",
                |msg| error!("{msg}"),
            )
        })
    }

    pub async fn read_gmv_oauth_by_device_id(device_id: &str) -> GlobalResult<Option<GmvOauth>> {
        #[cfg(test)]
        if use_test_storage() {
            return Ok(test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .oauths
                .get(device_id)
                .cloned());
        }
        let res = db::fetch_optional_as!(GmvOauth, GMV_OAUTH_SELECT_BY_DEVICE_ID, device_id)
            .hand_log(|msg| error!("{msg}"))?;
        Ok(res)
    }

    pub async fn read_gmv_oauth_by_device_ids(
        device_ids: &[String],
    ) -> GlobalResult<Vec<GmvOauth>> {
        if device_ids.is_empty() {
            return Ok(Vec::new());
        }

        #[cfg(test)]
        if use_test_storage() {
            let storage = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            return Ok(device_ids
                .iter()
                .filter_map(|device_id| storage.oauths.get(device_id).cloned())
                .collect());
        }

        match db::backend() {
            #[cfg(feature = "db-mysql")]
            db::SessionDatabaseBackend::Mysql => {
                let mut builder = sqlx::QueryBuilder::<MySql>::new("select ");
                builder.push(GMV_OAUTH_SELECT_FIELDS).push(
                    " from gb28181_oauth where COALESCE(del,0)=0 \
                         and COALESCE(status,1)=1 and device_id in (",
                );
                let mut separated = builder.separated(", ");
                for device_id in device_ids {
                    separated.push_bind(device_id);
                }
                separated.push_unseparated(")");
                builder
                    .build_query_as::<GmvOauth>()
                    .fetch_all(db::mysql_pool())
                    .await
                    .hand_log(|msg| error!("{msg}"))
            }
            #[cfg(feature = "db-sqlite")]
            db::SessionDatabaseBackend::Sqlite => {
                let mut builder = sqlx::QueryBuilder::<Sqlite>::new("select ");
                builder.push(GMV_OAUTH_SELECT_FIELDS).push(
                    " from gb28181_oauth where COALESCE(del,0)=0 \
                         and COALESCE(status,1)=1 and device_id in (",
                );
                let mut separated = builder.separated(", ");
                for device_id in device_ids {
                    separated.push_bind(device_id);
                }
                separated.push_unseparated(")");
                builder
                    .build_query_as::<GmvOauth>()
                    .fetch_all(db::sqlite_pool())
                    .await
                    .hand_log(|msg| error!("{msg}"))
            }
            backend => Err(db::backend_not_enabled_global(backend)),
        }
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, New, FromRow)]
#[serde(crate = "base::serde")]
pub struct GmvDevice {
    pub device_id: String,
    pub transport: String,
    pub register_expires: u32,
    pub register_time: NaiveDateTime,
    pub online_expire_time: Option<NaiveDateTime>,
    pub local_addr: String,
    pub contact_uri: String,
    pub enable_lr: u8,
    pub gb_version: Option<String>,
}

#[derive(Debug, FromRow)]
struct GmvDeviceRow {
    device_id: String,
    transport: String,
    register_expires: i64,
    register_time: NaiveDateTime,
    online_expire_time: Option<NaiveDateTime>,
    local_addr: String,
    contact_uri: String,
    enable_lr: i64,
    gb_version: Option<String>,
}

pub struct GmvDeviceRecoveryPage {
    pub devices: Vec<GmvDevice>,
    pub next_device_id: Option<String>,
    pub scanned: usize,
    pub invalid: usize,
}

const GMV_DEVICE_SELECT_FIELDS: &str = "\
device_id,\
COALESCE(transport,'UDP') AS transport,\
COALESCE(register_expires,3600) AS register_expires,\
COALESCE(register_time,CURRENT_TIMESTAMP) AS register_time,\
online_expire_time,\
COALESCE(local_addr,'') AS local_addr,\
COALESCE(contact_uri,'') AS contact_uri,\
COALESCE(enable_lr,0) AS enable_lr,\
gb_version";

const GMV_DEVICE_RECOVERY_SELECT_FIELDS: &str = "\
d.device_id AS device_id,\
COALESCE(d.transport,'UDP') AS transport,\
COALESCE(d.register_expires,3600) AS register_expires,\
COALESCE(d.register_time,CURRENT_TIMESTAMP) AS register_time,\
d.online_expire_time AS online_expire_time,\
COALESCE(d.local_addr,'') AS local_addr,\
COALESCE(d.contact_uri,'') AS contact_uri,\
COALESCE(d.enable_lr,0) AS enable_lr,\
d.gb_version AS gb_version";

const GMV_DEVICE_SELECT_BY_DEVICE_ID: &str = "\
select \
device_id,\
COALESCE(transport,'UDP') AS transport,\
COALESCE(register_expires,3600) AS register_expires,\
COALESCE(register_time,CURRENT_TIMESTAMP) AS register_time,\
online_expire_time,\
COALESCE(local_addr,'') AS local_addr,\
COALESCE(contact_uri,'') AS contact_uri,\
COALESCE(enable_lr,0) AS enable_lr,\
gb_version \
from gb28181_device where device_id=?";

impl TryFrom<GmvDeviceRow> for GmvDevice {
    type Error = GlobalError;

    fn try_from(row: GmvDeviceRow) -> Result<Self, Self::Error> {
        Ok(Self {
            device_id: row.device_id,
            transport: row.transport,
            register_expires: decode_u32(row.register_expires, "register_expires")?,
            register_time: row.register_time,
            online_expire_time: row.online_expire_time,
            local_addr: row.local_addr,
            contact_uri: row.contact_uri,
            enable_lr: decode_u8(row.enable_lr, "enable_lr")?,
            gb_version: row.gb_version,
        })
    }
}

impl GmvDevice {
    pub async fn page_recovery_candidates(
        after_device_id: Option<&str>,
        limit: u32,
    ) -> GlobalResult<GmvDeviceRecoveryPage> {
        if limit == 0 || limit > 10_000 {
            return Err(GlobalError::new_biz_error(
                BaseErrorCode::InvalidRequest.code(),
                "recovery candidate page limit must be between 1 and 10000",
                |msg| error!("{msg}"),
            ));
        }

        #[cfg(test)]
        if use_test_storage() {
            let storage = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut devices = storage
                .devices
                .values()
                .filter(|device| {
                    storage.oauths.get(&device.device_id).is_some_and(|oauth| {
                        oauth.status == 1
                            && after_device_id
                                .is_none_or(|cursor| device.device_id.as_str() > cursor)
                    })
                })
                .cloned()
                .collect::<Vec<_>>();
            devices.sort_by(|left, right| left.device_id.cmp(&right.device_id));
            devices.truncate(limit as usize);
            return Ok(GmvDeviceRecoveryPage {
                next_device_id: devices.last().map(|device| device.device_id.clone()),
                scanned: devices.len(),
                invalid: 0,
                devices,
            });
        }

        let rows = match db::backend() {
            #[cfg(feature = "db-mysql")]
            db::SessionDatabaseBackend::Mysql => {
                let mut builder = sqlx::QueryBuilder::<MySql>::new("select ");
                builder.push(GMV_DEVICE_RECOVERY_SELECT_FIELDS).push(
                    " from gb28181_device d inner join gb28181_oauth o \
                     on o.device_id=d.device_id where COALESCE(o.del,0)=0 \
                     and COALESCE(o.status,1)=1",
                );
                if let Some(cursor) = after_device_id {
                    builder.push(" and d.device_id>").push_bind(cursor);
                }
                builder
                    .push(" order by d.device_id limit ")
                    .push_bind(limit);
                builder
                    .build_query_as::<GmvDeviceRow>()
                    .fetch_all(db::mysql_pool())
                    .await
            }
            #[cfg(feature = "db-sqlite")]
            db::SessionDatabaseBackend::Sqlite => {
                let mut builder = sqlx::QueryBuilder::<Sqlite>::new("select ");
                builder.push(GMV_DEVICE_RECOVERY_SELECT_FIELDS).push(
                    " from gb28181_device d inner join gb28181_oauth o \
                     on o.device_id=d.device_id where COALESCE(o.del,0)=0 \
                     and COALESCE(o.status,1)=1",
                );
                if let Some(cursor) = after_device_id {
                    builder.push(" and d.device_id>").push_bind(cursor);
                }
                builder
                    .push(" order by d.device_id limit ")
                    .push_bind(limit);
                builder
                    .build_query_as::<GmvDeviceRow>()
                    .fetch_all(db::sqlite_pool())
                    .await
            }
            backend => return Err(db::backend_not_enabled_global(backend)),
        }
        .hand_log(|msg| error!("{msg}"))?;
        let scanned = rows.len();
        let next_device_id = rows.last().map(|row| row.device_id.clone());
        let mut invalid = 0usize;
        let devices = rows
            .into_iter()
            .filter_map(|row| {
                if u32::try_from(row.register_expires).is_err()
                    || u8::try_from(row.enable_lr).is_err()
                {
                    invalid += 1;
                    return None;
                }
                Some(GmvDevice {
                    device_id: row.device_id,
                    transport: row.transport,
                    register_expires: row.register_expires as u32,
                    register_time: row.register_time,
                    online_expire_time: row.online_expire_time,
                    local_addr: row.local_addr,
                    contact_uri: row.contact_uri,
                    enable_lr: row.enable_lr as u8,
                    gb_version: row.gb_version,
                })
            })
            .collect();
        Ok(GmvDeviceRecoveryPage {
            devices,
            next_device_id,
            scanned,
            invalid,
        })
    }

    pub async fn query_gmv_device_by_device_id(
        device_id: &String,
    ) -> GlobalResult<Option<GmvDevice>> {
        #[cfg(test)]
        if use_test_storage() {
            return Ok(test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .devices
                .get(device_id)
                .cloned());
        }
        let res = db::fetch_optional_as!(GmvDeviceRow, GMV_DEVICE_SELECT_BY_DEVICE_ID, device_id,)
            .hand_log(|msg| error!("{msg}"))?
            .map(GmvDevice::try_from)
            .transpose()?;
        Ok(res)
    }

    pub async fn insert_single_gmv_device_by_register(&self) -> GlobalResult<()> {
        #[cfg(test)]
        if use_test_storage() {
            test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .devices
                .insert(self.device_id.clone(), self.clone());
            return Ok(());
        }
        let sql = match db::backend() {
            db::SessionDatabaseBackend::Mysql => {
                r#"insert into gb28181_device (device_id,transport,register_expires,
        register_time,online_expire_time,local_addr,contact_uri,enable_lr,gb_version) values (?,?,?,?,?,?,?,?,?)
        ON DUPLICATE KEY UPDATE transport=VALUES(transport),register_expires=VALUES(register_expires),
        register_time=VALUES(register_time),online_expire_time=VALUES(online_expire_time),local_addr=VALUES(local_addr),
        contact_uri=VALUES(contact_uri),enable_lr=VALUES(enable_lr),gb_version=VALUES(gb_version)"#
            }
            db::SessionDatabaseBackend::Sqlite => {
                r#"insert into gb28181_device (device_id,transport,register_expires,
        register_time,online_expire_time,local_addr,contact_uri,enable_lr,gb_version) values (?,?,?,?,?,?,?,?,?)
        ON CONFLICT(device_id) DO UPDATE SET transport=excluded.transport,register_expires=excluded.register_expires,
        register_time=excluded.register_time,online_expire_time=excluded.online_expire_time,local_addr=excluded.local_addr,
        contact_uri=excluded.contact_uri,enable_lr=excluded.enable_lr,gb_version=excluded.gb_version"#
            }
        };
        db::execute!(
            sql,
            &self.device_id,
            &self.transport,
            self.register_expires,
            &self.register_time,
            &self.online_expire_time,
            &self.local_addr,
            &self.contact_uri,
            self.enable_lr,
            &self.gb_version,
        )
        .hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }

    pub async fn expire_online_by_device_id(device_id: &str) -> GlobalResult<()> {
        #[cfg(test)]
        if use_test_storage() {
            if let Some(device) = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .devices
                .get_mut(device_id)
            {
                device.online_expire_time = Some(Local::now().naive_local());
            }
            return Ok(());
        }
        db::execute!(
            "update gb28181_device set online_expire_time=? where device_id=?",
            Local::now().naive_local(),
            device_id,
        )
        .hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }

    pub async fn refresh_online_expire_time_by_device_id(device_id: &str) -> GlobalResult<()> {
        #[cfg(test)]
        if use_test_storage() {
            if let Some(device) = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .devices
                .get_mut(device_id)
            {
                device.online_expire_time = Some(Local::now().naive_local());
            }
            return Ok(());
        }
        match db::backend() {
            db::SessionDatabaseBackend::Mysql => db::execute!(
                r#"update gb28181_device d
            inner join gb28181_oauth o on o.device_id=d.device_id
            set d.online_expire_time=timestampadd(second,o.heartbeat_sec * 3 + 1,now())
            where d.device_id=?"#,
                device_id,
            ),
            db::SessionDatabaseBackend::Sqlite => db::execute!(
                "UPDATE gb28181_device SET online_expire_time=datetime('now','localtime','+' || (SELECT heartbeat_sec * 3 + 1 FROM gb28181_oauth WHERE gb28181_oauth.device_id=gb28181_device.device_id) || ' seconds') WHERE device_id=?",
                device_id,
            ),
        }
        .hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }
}

#[derive(Default, Debug, Clone, FromRow)]
pub struct GmvDeviceExt {
    pub device_id: String,
    pub device_type: Option<String>,
    pub manufacturer: String,
    pub model: String,
    pub firmware: String,
    pub max_camera: Option<u8>,
}

impl GmvDeviceExt {
    pub async fn update_gmv_device_ext_info(vs: Vec<(String, String)>) -> GlobalResult<()> {
        #[cfg(test)]
        if use_test_storage() {
            let _ = Self::build(vs);
            return Ok(());
        }
        let ext = Self::build(vs);
        db::execute!(
            "update gb28181_device set device_type=?,manufacturer=?,model=?,firmware=?,max_camera=? where device_id=?",
            ext.device_type,
            ext.manufacturer,
            ext.model,
            ext.firmware,
            ext.max_camera,
            ext.device_id,
        )
        .hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }

    fn build(vs: Vec<(String, String)>) -> GmvDeviceExt {
        use crate::gb::sip::xml::*;

        let mut de = GmvDeviceExt::default();
        for (k, v) in vs {
            match &k[..] {
                RESPONSE_DEVICE_ID => {
                    de.device_id = v.to_string();
                }
                RESPONSE_MANUFACTURER => {
                    de.manufacturer = v.to_string();
                }
                RESPONSE_MODEL => {
                    de.model = v.to_string();
                }
                RESPONSE_FIRMWARE => {
                    de.firmware = v.to_string();
                }
                RESPONSE_DEVICE_TYPE => {
                    de.device_type = Some(v.to_string());
                }
                RESPONSE_MAX_CAMERA => {
                    de.max_camera = v.parse::<u8>().ok();
                }
                _ => {}
            }
        }
        de
    }
}

#[derive(Debug, Clone, Default, FromRow)]
pub struct GmvDeviceChannel {
    pub device_id: String,
    pub channel_id: String,
    pub name: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub owner: Option<String>,
    pub status: String,
    pub civil_code: Option<String>,
    pub address: Option<String>,
    pub parental: Option<u8>,
    pub block: Option<String>,
    pub parent_id: Option<String>,
    pub ip_address: Option<String>,
    pub port: Option<u16>,
    pub password: Option<String>,
    pub longitude: Option<f32>,
    pub latitude: Option<f32>,
    pub ptz_type: Option<u8>,
    pub supply_light_type: Option<u8>,
}

impl GmvDeviceChannel {
    pub async fn insert_gmv_device_channel(
        device_id: &str,
        vs: Vec<(String, String)>,
    ) -> GlobalResult<Vec<GmvDeviceChannel>> {
        let dc_ls = Self::build(device_id, vs);
        #[cfg(test)]
        if use_test_storage() {
            test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .channels
                .extend(dc_ls.clone());
            return Ok(dc_ls);
        }
        for dc in &dc_ls {
            let sql = match db::backend() {
                db::SessionDatabaseBackend::Mysql => {
                    "INSERT INTO gb28181_device_channel (device_id, channel_id, name, manufacturer, model, owner, status, civil_code, address, parental, block, parent_id, ip_address, port,password, longitude,latitude,ptz_type,supply_light_type) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE name=VALUES(name),manufacturer=VALUES(manufacturer),model=VALUES(model),owner=VALUES(owner),status=VALUES(status),civil_code=VALUES(civil_code),address=VALUES(address),parental=VALUES(parental),block=VALUES(block),parent_id=VALUES(parent_id),ip_address=VALUES(ip_address),port=VALUES(port),password=VALUES(password),longitude=VALUES(longitude),latitude=VALUES(latitude),ptz_type=VALUES(ptz_type),supply_light_type=VALUES(supply_light_type)"
                }
                db::SessionDatabaseBackend::Sqlite => {
                    "INSERT INTO gb28181_device_channel (device_id, channel_id, name, manufacturer, model, owner, status, civil_code, address, parental, block, parent_id, ip_address, port,password, longitude,latitude,ptz_type,supply_light_type) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(device_id, channel_id) DO UPDATE SET name=excluded.name,manufacturer=excluded.manufacturer,model=excluded.model,owner=excluded.owner,status=excluded.status,civil_code=excluded.civil_code,address=excluded.address,parental=excluded.parental,block=excluded.block,parent_id=excluded.parent_id,ip_address=excluded.ip_address,port=excluded.port,password=excluded.password,longitude=excluded.longitude,latitude=excluded.latitude,ptz_type=excluded.ptz_type,supply_light_type=excluded.supply_light_type"
                }
            };
            db::execute!(
                sql,
                &dc.device_id,
                &dc.channel_id,
                &dc.name,
                &dc.manufacturer,
                &dc.model,
                &dc.owner,
                &dc.status,
                &dc.civil_code,
                &dc.address,
                &dc.parental,
                &dc.block,
                &dc.parent_id,
                &dc.ip_address,
                &dc.port,
                &dc.password,
                &dc.longitude,
                &dc.latitude,
                &dc.ptz_type,
                &dc.supply_light_type,
            )
            .hand_log(|msg| error!("{msg}"))?;
        }
        Self::insert_gmv_device_channel_conf(&dc_ls).await?;
        Ok(dc_ls)
    }

    async fn insert_gmv_device_channel_conf(dc_ls: &[GmvDeviceChannel]) -> GlobalResult<()> {
        if dc_ls.is_empty() {
            return Ok(());
        }
        for dc in dc_ls {
            let sql = match db::backend() {
                db::SessionDatabaseBackend::Mysql => {
                    "INSERT IGNORE INTO gb28181_device_channel_conf (device_id, channel_id) VALUES (?,?)"
                }
                db::SessionDatabaseBackend::Sqlite => {
                    "INSERT INTO gb28181_device_channel_conf (device_id, channel_id) VALUES (?,?) ON CONFLICT(device_id, channel_id) DO NOTHING"
                }
            };
            db::execute!(sql, &dc.device_id, &dc.channel_id).hand_log(|msg| error!("{msg}"))?;
        }
        Ok(())
    }

    fn build(parent_device_id: &str, vs: Vec<(String, String)>) -> Vec<GmvDeviceChannel> {
        use crate::gb::sip::xml::*;
        let mut dc = GmvDeviceChannel::default();
        dc.device_id = parent_device_id.to_string();
        let mut dcs: Vec<GmvDeviceChannel> = Vec::new();
        for (k, v) in vs {
            match &k[..] {
                RESPONSE_DEVICE_LIST_ITEM_DEVICE_ID => {
                    dc.channel_id = v.to_string();
                }
                RESPONSE_DEVICE_LIST_ITEM_NAME => {
                    dc.name = v.parse::<String>().ok();
                }
                RESPONSE_DEVICE_LIST_ITEM_MANUFACTURER => {
                    dc.manufacturer = v.parse::<String>().ok();
                }
                RESPONSE_DEVICE_LIST_ITEM_MODEL => {
                    dc.model = v.parse::<String>().ok();
                }
                RESPONSE_DEVICE_LIST_ITEM_OWNER => {
                    dc.owner = v.parse::<String>().ok();
                }
                RESPONSE_DEVICE_LIST_ITEM_CIVIL_CODE => {
                    dc.civil_code = v.parse::<String>().ok();
                }
                RESPONSE_DEVICE_LIST_ITEM_BLOCK => {
                    dc.block = Some(v.to_string());
                }
                RESPONSE_DEVICE_LIST_ITEM_ADDRESS => {
                    dc.address = v.parse::<String>().ok();
                }
                RESPONSE_DEVICE_LIST_ITEM_PARENTAL => {
                    dc.parental = v.parse::<u8>().ok();
                }
                RESPONSE_DEVICE_LIST_ITEM_PARENT_ID => {
                    dc.parent_id = Some(v.to_string());
                }
                RESPONSE_DEVICE_LIST_ITEM_LONGITUDE => {
                    dc.longitude = v.parse::<f32>().ok();
                }
                RESPONSE_DEVICE_LIST_ITEM_LATITUDE => {
                    dc.latitude = v.parse::<f32>().ok();
                }
                RESPONSE_DEVICE_LIST_ITEM_PTZ_TYPE => {
                    dc.ptz_type = v.parse::<u8>().ok();
                }
                RESPONSE_DEVICE_LIST_ITEM_SUPPLY_LIGHT_TYPE => {
                    dc.supply_light_type = v.parse::<u8>().ok();
                }
                RESPONSE_DEVICE_LIST_ITEM_IP_ADDRESS => {
                    dc.ip_address = Some(v.to_string());
                }
                RESPONSE_DEVICE_LIST_ITEM_PORT => {
                    dc.port = v.parse::<u16>().ok();
                }
                RESPONSE_DEVICE_LIST_ITEM_PASSWORD => {
                    dc.password = Some(v.to_string());
                }
                RESPONSE_DEVICE_LIST_ITEM_STATUS => {
                    dc.status = v.to_string();
                }
                SPLIT_CLASS if "4".eq(&v) => {
                    if !dc.channel_id.is_empty() {
                        dcs.push(dc.clone());
                        dc = GmvDeviceChannel::default();
                        dc.device_id = parent_device_id.to_string();
                    }
                }
                &_ => {}
            }
        }
        dcs.push(dc);
        dcs
    }
}

#[derive(Debug, Clone, Default, FromRow)]
pub struct GmvFileInfo {
    pub device_id: String,
    pub channel_id: String,
    pub biz_time: Option<NaiveDateTime>,
    pub biz_id: String,
    pub file_type: Option<i32>,
    pub file_size: Option<i64>,
    pub file_name: String,
    pub file_format: Option<String>,
    pub dir_path: String,
    pub abs_path: Option<String>,
    pub note: Option<String>,
    pub is_del: Option<i32>,
    pub create_time: Option<NaiveDateTime>,
}

impl GmvFileInfo {
    pub async fn insert_gmv_file_info(files: Vec<GmvFileInfo>) -> GlobalResult<()> {
        for file in files {
            db::execute!(
                "INSERT INTO gb28181_file_info(device_id,channel_id,biz_time,biz_id,file_type,file_size,file_name,file_format,dir_path,abs_path,note,is_del,create_time) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
                &file.device_id,
                &file.channel_id,
                &file.biz_time,
                &file.biz_id,
                &file.file_type,
                &file.file_size,
                &file.file_name,
                &file.file_format,
                &file.dir_path,
                &file.abs_path,
                &file.note,
                &file.is_del,
                &file.create_time,
            )
            .hand_log(|msg| error!("{msg}"))?;
        }
        Ok(())
    }
}
#[derive(Debug, FromRow, Default)]
pub struct DeviceStatus {
    pub heartbeat: u8,
    pub enable: u8,
    pub expires: u32,
    pub online_expire_time: Option<NaiveDateTime>,
    pub contact_uri: String,
    pub lr: u8,
}

#[derive(Debug, FromRow)]
struct DeviceStatusRow {
    heartbeat: i64,
    enable: i64,
    expires: i64,
    online_expire_time: Option<NaiveDateTime>,
    contact_uri: String,
    lr: i64,
}

impl TryFrom<DeviceStatusRow> for DeviceStatus {
    type Error = GlobalError;

    fn try_from(row: DeviceStatusRow) -> Result<Self, Self::Error> {
        Ok(Self {
            heartbeat: decode_u8(row.heartbeat, "heartbeat")?,
            enable: decode_u8(row.enable, "enable")?,
            expires: decode_u32(row.expires, "expires")?,
            online_expire_time: row.online_expire_time,
            contact_uri: row.contact_uri,
            lr: decode_u8(row.lr, "lr")?,
        })
    }
}

impl DeviceStatus {
    pub async fn get_device_status(device_id: &String) -> GlobalResult<Option<DeviceStatus>> {
        let res = db::fetch_optional_as!(
            DeviceStatusRow,
            "SELECT COALESCE(o.heartbeat_sec,60) heartbeat,COALESCE(o.status,1) enable,COALESCE(d.register_expires,0) expires,
            d.online_expire_time online_expire_time,COALESCE(d.contact_uri,'') contact_uri,COALESCE(d.enable_lr,0) lr
            FROM gb28181_oauth o INNER JOIN gb28181_device d ON o.device_id = d.device_id where d.device_id=?",
            device_id,
        )
        .hand_log(|msg| error!("{msg}"))?
        .map(DeviceStatus::try_from)
        .transpose()?;
        Ok(res)
    }
}

fn decode_u32(value: i64, field: &str) -> GlobalResult<u32> {
    u32::try_from(value).map_err(|_| decode_range_error(field, "u32"))
}

fn decode_u8(value: i64, field: &str) -> GlobalResult<u8> {
    u8::try_from(value).map_err(|_| decode_range_error(field, "u8"))
}

fn decode_range_error(field: &str, ty: &str) -> GlobalError {
    let message = format!("{field} must fit {ty}");
    GlobalError::new_biz_error(BaseErrorCode::InvalidRequest.code(), &message, |msg| {
        error!("{msg}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base::chrono::TimeZone;
    use base::tokio::runtime::Builder;

    #[test]
    fn oauth_select_fields_match_lowercase_schema_for_from_row() {
        for field in [
            "device_id",
            "domain_id",
            "domain",
            "pwd",
            "COALESCE(pwd_check,0) AS pwd_check",
            "alias",
            "COALESCE(status,1) AS status",
            "COALESCE(heartbeat_sec,60) AS heartbeat_sec",
        ] {
            assert!(GMV_OAUTH_SELECT_FIELDS.contains(field), "missing {field}");
        }
    }

    #[test]
    fn gmv_device_select_fields_match_lowercase_schema_for_from_row() {
        for field in [
            "device_id",
            "COALESCE(transport,'UDP') AS transport",
            "COALESCE(register_expires,3600) AS register_expires",
            "COALESCE(register_time,CURRENT_TIMESTAMP) AS register_time",
            "online_expire_time",
            "COALESCE(local_addr,'') AS local_addr",
            "COALESCE(contact_uri,'') AS contact_uri",
            "COALESCE(enable_lr,0) AS enable_lr",
            "gb_version",
        ] {
            assert!(GMV_DEVICE_SELECT_FIELDS.contains(field), "missing {field}");
            assert!(
                GMV_DEVICE_SELECT_BY_DEVICE_ID.contains(field),
                "query missing {field}"
            );
        }
    }

    #[test]
    fn test_datetime() {
        let now = Local::now();
        let ts = now.timestamp();
        println!("ts:{}", ts);
        let time = Local.timestamp_opt(ts, 0).unwrap().naive_local();
        let time_str1 = time.format("%Y-%m-%d %H:%M:%S").to_string();
        println!("{}", time_str1);
        let time_str2 = now.naive_local().format("%Y-%m-%d %H:%M:%S").to_string();
        println!("{}", time_str2);
    }

    #[test]
    fn recovery_candidates_use_stable_device_id_paging_and_enabled_auth() {
        let _test_lock = TEST_STORAGE_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let oauth = |device_id: &str, status| GmvOauth {
            device_id: device_id.to_string(),
            domain_id: "34020000002000000001".to_string(),
            domain: "3402000000".to_string(),
            status,
            heartbeat_sec: 60,
            ..GmvOauth::default()
        };
        let _guard = enable_test_storage(oauth("device-01", 1));
        let now = Local::now().naive_local();
        let device = |device_id: &str| GmvDevice {
            device_id: device_id.to_string(),
            transport: "UDP".to_string(),
            register_expires: 3600,
            register_time: now,
            online_expire_time: Some(now),
            local_addr: "127.0.0.1:5060".to_string(),
            ..GmvDevice::default()
        };
        {
            let mut storage = test_storage()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            storage
                .oauths
                .insert("device-02".to_string(), oauth("device-02", 1));
            storage
                .oauths
                .insert("device-03".to_string(), oauth("device-03", 1));
            storage
                .oauths
                .insert("device-04".to_string(), oauth("device-04", 0));
            for device_id in ["device-03", "device-01", "device-04", "device-02"] {
                storage
                    .devices
                    .insert(device_id.to_string(), device(device_id));
            }
        }

        Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(async {
                let first = GmvDevice::page_recovery_candidates(None, 2)
                    .await
                    .expect("first page");
                assert_eq!(
                    first
                        .devices
                        .iter()
                        .map(|device| device.device_id.as_str())
                        .collect::<Vec<_>>(),
                    ["device-01", "device-02"]
                );
                let second = GmvDevice::page_recovery_candidates(Some("device-02"), 2)
                    .await
                    .expect("second page");
                assert_eq!(
                    second
                        .devices
                        .iter()
                        .map(|device| device.device_id.as_str())
                        .collect::<Vec<_>>(),
                    ["device-03"]
                );
            });
    }
}
