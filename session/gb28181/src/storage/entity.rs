use std::sync::Arc;

use crate::storage::db;
use base::chrono::{Local, NaiveDateTime};
use base::constructor::New;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use base::serde::{Deserialize, Serialize};
use base::serde_default;
use base_db::sqlx::{self, MySql, Sqlite};
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
    pub pwd_check: u8,
    pub alias: Option<String>,
    //0-停用,1-启用
    pub status: u8,
    // 默认60
    #[serde(default = "default_heartbeat_sec")]
    pub heartbeat_sec: u8,
}
serde_default!(default_heartbeat_sec, u8, 60);
impl GmvOauth {
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
        let res = db::fetch_optional_as!(GmvOauth, "select device_id,domain_id,domain,pwd,pwd_check,alias,status,heartbeat_sec from GMV_OAUTH where device_id=? and DEL=0 and STATUS=1", device_id)
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
            db::SessionDatabaseBackend::Mysql => {
                let mut builder = sqlx::QueryBuilder::<MySql>::new(
                    "select device_id,domain_id,domain,pwd,pwd_check,alias,status,heartbeat_sec \
             from GMV_OAUTH where DEL=0 and STATUS=1 and device_id in (",
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
            db::SessionDatabaseBackend::Sqlite => {
                let mut builder = sqlx::QueryBuilder::<Sqlite>::new(
                    "select device_id,domain_id,domain,pwd,pwd_check,alias,status,heartbeat_sec \
             from GMV_OAUTH where DEL=0 and STATUS=1 and device_id in (",
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

impl GmvDevice {
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
        let res = db::fetch_optional_as!(
            Self,
            r#"select device_id,transport,register_expires,
        register_time,online_expire_time,local_addr,contact_uri,enable_lr,gb_version
        from GMV_DEVICE where device_id=?"#,
            device_id,
        )
        .hand_log(|msg| error!("{msg}"))?;
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
                r#"insert into GMV_DEVICE (device_id,transport,register_expires,
        register_time,online_expire_time,local_addr,contact_uri,enable_lr,gb_version) values (?,?,?,?,?,?,?,?,?)
        ON DUPLICATE KEY UPDATE transport=VALUES(transport),register_expires=VALUES(register_expires),
        register_time=VALUES(register_time),online_expire_time=VALUES(online_expire_time),local_addr=VALUES(local_addr),
        contact_uri=VALUES(contact_uri),enable_lr=VALUES(enable_lr),gb_version=VALUES(gb_version)"#
            }
            db::SessionDatabaseBackend::Sqlite => {
                r#"insert into GMV_DEVICE (device_id,transport,register_expires,
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
            "update GMV_DEVICE set online_expire_time=? where device_id=?",
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
                r#"update GMV_DEVICE d
            inner join GMV_OAUTH o on o.DEVICE_ID=d.DEVICE_ID
            set d.online_expire_time=timestampadd(second,o.heartbeat_sec * 3 + 1,now())
            where d.device_id=?"#,
                device_id,
            ),
            db::SessionDatabaseBackend::Sqlite => db::execute!(
                "UPDATE GMV_DEVICE SET ONLINE_EXPIRE_TIME=datetime('now','localtime','+' || (SELECT HEARTBEAT_SEC * 3 + 1 FROM GMV_OAUTH WHERE GMV_OAUTH.DEVICE_ID=GMV_DEVICE.DEVICE_ID) || ' seconds') WHERE DEVICE_ID=?",
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
            "update GMV_DEVICE set device_type=?,manufacturer=?,model=?,firmware=?,max_camera=? where device_id=?",
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
                    "INSERT INTO GMV_DEVICE_CHANNEL (device_id, channel_id, name, manufacturer, model, owner, status, civil_code, address, parental, block, parent_id, ip_address, port,password, longitude,latitude,ptz_type,supply_light_type) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON DUPLICATE KEY UPDATE name=VALUES(name),manufacturer=VALUES(manufacturer),model=VALUES(model),owner=VALUES(owner),status=VALUES(status),civil_code=VALUES(civil_code),address=VALUES(address),parental=VALUES(parental),block=VALUES(block),parent_id=VALUES(parent_id),ip_address=VALUES(ip_address),port=VALUES(port),password=VALUES(password),longitude=VALUES(longitude),latitude=VALUES(latitude),ptz_type=VALUES(ptz_type),supply_light_type=VALUES(supply_light_type)"
                }
                db::SessionDatabaseBackend::Sqlite => {
                    "INSERT INTO GMV_DEVICE_CHANNEL (device_id, channel_id, name, manufacturer, model, owner, status, civil_code, address, parental, block, parent_id, ip_address, port,password, longitude,latitude,ptz_type,supply_light_type) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(device_id, channel_id) DO UPDATE SET name=excluded.name,manufacturer=excluded.manufacturer,model=excluded.model,owner=excluded.owner,status=excluded.status,civil_code=excluded.civil_code,address=excluded.address,parental=excluded.parental,block=excluded.block,parent_id=excluded.parent_id,ip_address=excluded.ip_address,port=excluded.port,password=excluded.password,longitude=excluded.longitude,latitude=excluded.latitude,ptz_type=excluded.ptz_type,supply_light_type=excluded.supply_light_type"
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
                    "INSERT IGNORE INTO GMV_DEVICE_CHANNEL_CONF (device_id, channel_id) VALUES (?,?)"
                }
                db::SessionDatabaseBackend::Sqlite => {
                    "INSERT INTO GMV_DEVICE_CHANNEL_CONF (device_id, channel_id) VALUES (?,?) ON CONFLICT(device_id, channel_id) DO NOTHING"
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

#[derive(Debug, FromRow, Default)]
pub struct DeviceStatus {
    pub heartbeat: u8,
    pub enable: u8,
    pub expires: u32,
    pub online_expire_time: Option<NaiveDateTime>,
    pub contact_uri: String,
    pub lr: u8,
}
impl DeviceStatus {
    pub async fn get_device_status(device_id: &String) -> GlobalResult<Option<DeviceStatus>> {
        let res = db::fetch_optional_as!(
            DeviceStatus,
            "SELECT o.HEARTBEAT_SEC heartbeat,o.STATUS enable,d.REGISTER_EXPIRES expires,
            d.ONLINE_EXPIRE_TIME online_expire_time,d.CONTACT_URI contact_uri,d.ENABLE_LR lr
            FROM GMV_OAUTH o INNER JOIN GMV_DEVICE d ON o.DEVICE_ID = d.DEVICE_ID where d.device_id=?",
            device_id,
        )
        .hand_log(|msg| error!("{msg}"))?;
        Ok(res)
    }
}

#[cfg(test)]
#[allow(dead_code, unused_imports)]
mod tests {
    use super::*;
    use base::cfg_lib::conf::init_cfg;
    use base::chrono::TimeZone;
    use base::tokio;

    // #[tokio::test]
    async fn test_read_gmv_oauth_by_device_id() {
        init();
        let res = GmvOauth::read_gmv_oauth_by_device_id(&"34020000001320000003".to_string()).await;
        println!("{res:?}");
    }

    // #[tokio::test]
    async fn test_query_gmv_device_by_device_id() {
        init();
        let res =
            GmvDevice::query_gmv_device_by_device_id(&"34020000001320000003".to_string()).await;
        println!("{res:?}");
    }

    // #[tokio::test]
    async fn test_insert_single_gmv_device_by_register() {
        init();
        let res =
            GmvDevice::query_gmv_device_by_device_id(&"34020000001320000004".to_string()).await;
        if let Ok(Some(gd)) = res {
            let a = GmvDevice {
                device_id: "34020000001320000004".to_string(),
                register_time: Local::now().naive_local(),
                ..gd
            };
            println!("{a:?}");
            let result = a.insert_single_gmv_device_by_register().await;
            println!("{:?}", result)
        }
    }

    // #[tokio::test]
    async fn test_expire_online_by_device_id() {
        init();
        let res = GmvDevice::expire_online_by_device_id(&"34020000001320000003".to_string()).await;
        println!("{:?}", res);
    }

    fn init() {
        init_cfg(
            "/home/ubuntu20/code/rs/mv/github/epimore/gmv/session/gb28181/config.yml".to_string(),
        );
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
}
