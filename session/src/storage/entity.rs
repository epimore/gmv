use rsip::Request;

use common::chrono::{Local, NaiveDateTime};
use common::constructor::{Get, New, Set};
use common::dbx::mysqlx::get_conn_by_pool;
use common::exception::{GlobalResult, TransError};
use common::log::error;
use common::serde::{Deserialize, Serialize};
use common::sqlx;
use common::sqlx::FromRow;

use crate::gb::handler::parser;

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Get, Set, New, FromRow)]
#[serde(crate = "common::serde")]
pub struct GmvOauth {
    device_id: String,
    domain_id: String,
    domain: String,
    pwd: Option<String>,
    //0-false,1-true
    pwd_check: u8,
    alias: Option<String>,
    //0-停用,1-启用
    status: u8,
    heartbeat_sec: u8,
}

impl GmvOauth {
    pub async fn read_gmv_oauth_by_device_id(device_id: &String) -> GlobalResult<Option<GmvOauth>> {
        let pool = get_conn_by_pool()?;
        let res = sqlx::query_as::<_, GmvOauth>("select device_id,domain_id,domain,pwd,pwd_check,alias,status,heartbeat_sec from GMV_OAUTH where device_id=?")
            .bind(device_id).fetch_optional(pool).await.hand_log(|msg| error!("{msg}"))?;
        Ok(res)
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Get, Set, New, FromRow)]
#[serde(crate = "common::serde")]
pub struct GmvDevice {
    device_id: String,
    transport: String,
    register_expires: u32,
    register_time: NaiveDateTime,
    local_addr: String,
    sip_from: String,
    sip_to: String,
    status: u8,
    gb_version: Option<String>,
}


impl GmvDevice {
    pub async fn query_gmv_device_by_device_id(device_id: &String) -> GlobalResult<Option<GmvDevice>> {
        let pool = get_conn_by_pool()?;
        let res = sqlx::query_as::<_, Self>(r#"select device_id,transport,register_expires,
        register_time,local_addr,sip_from,sip_to,status,gb_version from GMV_DEVICE where device_id=?"#)
            .bind(device_id).fetch_optional(pool).await.hand_log(|msg| error!("{msg}"))?;
        Ok(res)
    }

    pub async fn insert_single_gmv_device_by_register(&self) -> GlobalResult<()> {
        let pool = get_conn_by_pool()?;
        sqlx::query(r#"insert into GMV_DEVICE (device_id,transport,register_expires,
        register_time,local_addr,sip_from,sip_to,status,gb_version) values (?,?,?,?,?,?,?,?,?)
        ON DUPLICATE KEY UPDATE device_id=VALUES(device_id),transport=VALUES(transport),register_expires=VALUES(register_expires),
        register_time=VALUES(register_time),local_addr=VALUES(local_addr),sip_from=VALUES(sip_from),sip_to=VALUES(sip_to),status=VALUES(status),gb_version=VALUES(gb_version)"#)
            .bind(&self.device_id)
            .bind(&self.transport)
            .bind(&self.register_expires)
            .bind(&self.register_time)
            .bind(&self.local_addr)
            .bind(&self.sip_from)
            .bind(&self.sip_to)
            .bind(&self.status)
            .bind(&self.gb_version)
            .execute(pool)
            .await.hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }
    pub async fn update_gmv_device_status_by_device_id(device_id: &String, status: u8) -> GlobalResult<()> {
        let pool = get_conn_by_pool()?;
        sqlx::query("update GMV_DEVICE set status=? where device_id=?")
            .bind(status)
            .bind(device_id)
            .execute(pool).await.hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }

    pub fn build_gmv_device(req: &Request) -> GlobalResult<Self> {
        let device = Self {
            device_id: parser::header::get_device_id_by_request(req)?,
            transport: parser::header::get_transport(req)?,
            register_expires: parser::header::get_expires(req)?,
            register_time: Local::now().naive_local(),
            local_addr: parser::header::get_local_addr(req)?,
            sip_from: parser::header::get_from(req)?,
            sip_to: parser::header::get_to(req)?,
            status: 1,
            gb_version: parser::header::get_gb_version(req),
        };
        Ok(device)
    }
}

#[derive(Default, Debug, Clone, Get, FromRow)]
pub struct GmvDeviceExt {
    device_id: String,
    device_type: Option<String>,
    manufacturer: String,
    model: String,
    firmware: String,
    max_camera: Option<u8>,
}

impl GmvDeviceExt {
    pub async fn update_gmv_device_ext_info(vs: Vec<(String, String)>) -> GlobalResult<()> {
        let ext = Self::build(vs);
        let pool = get_conn_by_pool()?;
        sqlx::query("update GMV_DEVICE set device_type=?,manufacturer=?,model=?,firmware=?,max_camera=? where device_id=?")
            .bind(ext.device_type)
            .bind(ext.manufacturer)
            .bind(ext.model)
            .bind(ext.firmware)
            .bind(ext.max_camera)
            .bind(ext.device_id)
            .execute(pool)
            .await.hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }

    fn build(vs: Vec<(String, String)>) -> GmvDeviceExt {
        use crate::gb::handler::parser::xml::*;

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

#[derive(Debug, Clone, Default, Get, FromRow)]
pub struct GmvDeviceChannel {
    device_id: String,
    channel_id: String,
    name: Option<String>,
    manufacturer: Option<String>,
    model: Option<String>,
    owner: Option<String>,
    status: String,
    civil_code: Option<String>,
    address: Option<String>,
    parental: Option<u8>,
    block: Option<String>,
    parent_id: Option<String>,
    ip_address: Option<String>,
    port: Option<u16>,
    password: Option<String>,
    longitude: Option<f32>,
    latitude: Option<f32>,
    ptz_type: Option<u8>,
    supply_light_type: Option<u8>,
    alias_name: Option<String>,
}

impl GmvDeviceChannel {
    pub async fn insert_gmv_device_channel(device_id: &String, vs: Vec<(String, String)>) -> GlobalResult<()> {
        let dc_ls = Self::build(device_id, vs);
        let pool = get_conn_by_pool()?;
        let mut builder = sqlx::query_builder::QueryBuilder::new("INSERT INTO GMV_DEVICE_CHANNEL (device_id, channel_id, name, manufacturer,
         model, owner, status, civil_code, address, parental, block, parent_id, ip_address, port,password,
         longitude,latitude,ptz_type,supply_light_type,alias_name) ");
        builder.push_values(dc_ls, |mut b, dc| {
            b.push_bind(dc.device_id)
                .push_bind(dc.channel_id)
                .push_bind(dc.name)
                .push_bind(dc.manufacturer)
                .push_bind(dc.model)
                .push_bind(dc.owner)
                .push_bind(dc.status)
                .push_bind(dc.civil_code)
                .push_bind(dc.address)
                .push_bind(dc.parental)
                .push_bind(dc.block)
                .push_bind(dc.parent_id)
                .push_bind(dc.ip_address)
                .push_bind(dc.port)
                .push_bind(dc.password)
                .push_bind(dc.longitude)
                .push_bind(dc.latitude)
                .push_bind(dc.ptz_type)
                .push_bind(dc.supply_light_type)
                .push_bind(dc.alias_name);
        });
        builder.push(" ON DUPLICATE KEY UPDATE device_id=VALUES(device_id),channel_id=VALUES(channel_id),name=VALUES(name),
        manufacturer=VALUES(manufacturer),model=VALUES(model),owner=VALUES(owner),status=VALUES(status),civil_code=VALUES(civil_code),
        address=VALUES(address),parental=VALUES(parental),block=VALUES(block),parent_id=VALUES(parent_id),ip_address=VALUES(ip_address),
        port=VALUES(port),password=VALUES(password),longitude=VALUES(longitude),latitude=VALUES(latitude),ptz_type=VALUES(ptz_type),
        supply_light_type=VALUES(supply_light_type),alias_name=VALUES(alias_name)");
        builder.build().execute(pool).await.hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }
    fn build(device_id: &String, vs: Vec<(String, String)>) -> Vec<GmvDeviceChannel> {
        use crate::gb::handler::parser::xml::*;
        let mut dc = GmvDeviceChannel::default();
        dc.device_id = device_id.to_string();
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
                        dc.device_id = device_id.to_string();
                    }
                }
                &_ => {}
            }
        }
        dcs.push(dc);
        dcs
    }
}


#[cfg(test)]
#[allow(dead_code,unused_imports)]
mod tests {
    use common::cfg_lib::conf::init_cfg;
    use common::dbx::mysqlx;
    use common::tokio;
    use super::*;

    // #[tokio::test]
    async fn test_read_gmv_oauth_by_device_id() {
        init();
        let res = GmvOauth::read_gmv_oauth_by_device_id(&"34020000001320000003".to_string()).await;
        println!("{res:?}");
    }

    // #[tokio::test]
    async fn test_query_gmv_device_by_device_id() {
        init();
        let res = GmvDevice::query_gmv_device_by_device_id(&"34020000001320000003".to_string()).await;
        println!("{res:?}");
    }

    // #[tokio::test]
    async fn test_insert_single_gmv_device_by_register() {
        init();
        let res = GmvDevice::query_gmv_device_by_device_id(&"34020000001320000003".to_string()).await;
        if let Ok(Some(gd)) = res {
            let a = GmvDevice { device_id: "34020000001320000004".to_string(), ..gd };
            println!("{a:?}");
            let result = a.insert_single_gmv_device_by_register().await;
            println!("{:?}", result)
        }
    }

    // #[tokio::test]
    async fn test_update_gmv_device_status_by_device_id() {
        init();
        let res = GmvDevice::update_gmv_device_status_by_device_id(&"34020000001320000003".to_string(), 1).await;
        println!("{:?}", res);
    }

    // #[tokio::test]
    async fn test_update_gmv_device_ext_info() {
        init();
        let ext = GmvDeviceExt {
            device_id: "34020000001110000001".to_string(),
            ..Default::default()
        };
        let pool = get_conn_by_pool().unwrap();
        let res = sqlx::query("update GMV_DEVICE set device_type=?,manufacturer=?,model=?,firmware=?,max_camera=? where device_id=?")
            .bind(ext.device_type)
            .bind(ext.manufacturer)
            .bind(ext.model)
            .bind(ext.firmware)
            .bind(ext.max_camera)
            .bind(ext.device_id)
            .execute(pool)
            .await;
        println!("{:?}", res);
    }

    // #[tokio::test]
    async fn test_insert_gmv_device_channel() {
        init();
        let dc_ls = (0..10).map(|i| GmvDeviceChannel {
            device_id: "34020000001320000004".to_string(),
            channel_id: format!("3402000000132000010{}", i),
            ..Default::default()
        });

        let _ext = GmvDeviceExt {
            device_id: "34020000001110000001".to_string(),
            ..Default::default()
        };
        let pool = get_conn_by_pool().unwrap();
        let mut builder = sqlx::query_builder::QueryBuilder::new("INSERT INTO GMV_DEVICE_CHANNEL (device_id, channel_id, name, manufacturer,
         model, owner, status, civil_code, address, parental, block, parent_id, ip_address, port,password,
         longitude,latitude,ptz_type,supply_light_type,alias_name) ");
        builder.push_values(dc_ls, |mut b, dc| {
            b.push_bind(dc.device_id)
                .push_bind(dc.channel_id)
                .push_bind(dc.name)
                .push_bind(dc.manufacturer)
                .push_bind(dc.model)
                .push_bind(dc.owner)
                .push_bind(dc.status)
                .push_bind(dc.civil_code)
                .push_bind(dc.address)
                .push_bind(dc.parental)
                .push_bind(dc.block)
                .push_bind(dc.parent_id)
                .push_bind(dc.ip_address)
                .push_bind(dc.port)
                .push_bind(dc.password)
                .push_bind(dc.longitude)
                .push_bind(dc.latitude)
                .push_bind(dc.ptz_type)
                .push_bind(dc.supply_light_type)
                .push_bind(dc.alias_name);
        });
        builder.push(" ON DUPLICATE KEY UPDATE device_id=VALUES(device_id),channel_id=VALUES(channel_id),name=VALUES(name),
        manufacturer=VALUES(manufacturer),model=VALUES(model),owner=VALUES(owner),status=VALUES(status),civil_code=VALUES(civil_code),
        address=VALUES(address),parental=VALUES(parental),block=VALUES(block),parent_id=VALUES(parent_id),ip_address=VALUES(ip_address),
        port=VALUES(port),password=VALUES(password),longitude=VALUES(longitude),latitude=VALUES(latitude),ptz_type=VALUES(ptz_type),
        supply_light_type=VALUES(supply_light_type),alias_name=VALUES(alias_name)");
        let res = builder.build().execute(pool).await;
        println!("{:?}", res);
    }

    fn init(){
        init_cfg("/home/ubuntu20/code/rs/mv/github/epimore/gmv/session/config.yml".to_string());
        let _ = mysqlx::init_conn_pool();
    }
}
