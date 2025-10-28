use crate::gb::handler::parser;
use rsip::Request;

use base::chrono::{Local, NaiveDateTime};
use base::constructor::{Get, New, Set};
use base::dbx::mysqlx::get_conn_by_pool;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use base::serde::{Deserialize, Serialize};
use base::sqlx;
use sqlx::FromRow;

//CREATE TABLE `GMV_RECORD` (
//   `BIZ_ID` varchar(128) NOT NULL COMMENT '业务ID',
//   `DEVICE_ID` varchar(20) NOT NULL COMMENT '设备编号',
//   `CHANNEL_ID` varchar(20) NOT NULL COMMENT '通道编号',
//   `USER_ID` varchar(32) DEFAULT NULL COMMENT '用户ID',
//   `ST` datetime DEFAULT NULL COMMENT '录像开始时间',
//   `ET` datetime DEFAULT NULL COMMENT '录像结束时间',
//   `SPEED` int DEFAULT NULL COMMENT '倍速',
//   `CT` datetime DEFAULT NULL COMMENT '创建时间',
//   `STATE` int DEFAULT NULL COMMENT '录制状态：0=进行，1=完成，2=录制部分，3=失败',
//   `LT` datetime DEFAULT NULL ON UPDATE CURRENT_TIMESTAMP COMMENT '最后更新时间',
//   `STREAM_APP_NAME` varchar(64) CHARACTER SET utf8mb4 COLLATE utf8mb4_0900_ai_ci DEFAULT NULL COMMENT '流媒体名称',
//   PRIMARY KEY (`BIZ_ID`)
// ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci COMMENT='云端录像';
#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Get, Set, New, FromRow)]
#[serde(crate = "base::serde")]
pub struct GmvRecord {
    pub biz_id: String,
    pub device_id: String,
    pub channel_id: String,
    pub user_id: Option<String>,
    pub st: NaiveDateTime,
    pub et: NaiveDateTime,
    pub speed: u8,
    pub ct: NaiveDateTime,
    pub state: u8,
    pub lt: NaiveDateTime,
    pub stream_app_name: String,
}

impl GmvRecord {
    pub async fn rm_gmv_record_by_biz_id(biz_id: &str) -> GlobalResult<()> {
        let pool = get_conn_by_pool();
        sqlx::query("delete from GMV_RECORD where biz_id=?")
            .bind(biz_id)
            .execute(pool)
            .await.hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }

    pub async fn insert_single_gmv_record(&self) -> GlobalResult<()> {
        let pool = get_conn_by_pool();
        sqlx::query("insert into GMV_RECORD (BIZ_ID,DEVICE_ID,CHANNEL_ID,USER_ID,ST,ET,SPEED,CT,STATE,LT,STREAM_APP_NAME) values (?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&self.biz_id)
            .bind(&self.device_id)
            .bind(&self.channel_id)
            .bind(&self.user_id)
            .bind(&self.st)
            .bind(&self.et)
            .bind(&self.speed)
            .bind(&self.ct)
            .bind(&self.state)
            .bind(&self.lt)
            .bind(&self.stream_app_name)
            .execute(pool)
            .await.hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }

    pub async fn query_gmv_record_run_by_device_id_channel_id(device_id: &str, channel_id: &str) -> GlobalResult<Option<GmvRecord>> {
        let pool = get_conn_by_pool();
        let res = sqlx::query_as::<_, GmvRecord>("select biz_id,device_id,channel_id,user_id,st,et,speed,ct,state,lt,stream_app_name from GMV_RECORD where state=0 and device_id=? and channel_id=?")
            .bind(device_id).bind(channel_id).fetch_optional(pool).await.hand_log(|msg| error!("{msg}"))?;
        Ok(res)
    }

    pub async fn query_gmv_record_by_biz_id(biz_id: &str) -> GlobalResult<Option<GmvRecord>> {
        let pool = get_conn_by_pool();
        let res = sqlx::query_as::<_, GmvRecord>("select biz_id,device_id,channel_id,user_id,st,et,speed,ct,state,lt,stream_app_name from GMV_RECORD where biz_id=?")
            .bind(biz_id).fetch_optional(pool).await.hand_log(|msg| error!("{msg}"))?;
        Ok(res)
    }

    pub async fn update_gmv_record_by_biz_id(&self) -> GlobalResult<()> {
        let pool = get_conn_by_pool();
        sqlx::query("update GMV_RECORD set state=?,lt=? where biz_id=?")
            .bind(&self.state)
            .bind(&self.lt)
            .bind(&self.biz_id)
            .execute(pool)
            .await.hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Get, Set, New, FromRow)]
#[serde(crate = "base::serde")]
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
        let pool = get_conn_by_pool();
        let res = sqlx::query_as::<_, GmvOauth>("select device_id,domain_id,domain,pwd,pwd_check,alias,status,heartbeat_sec from GMV_OAUTH where device_id=?")
            .bind(device_id).fetch_optional(pool).await.hand_log(|msg| error!("{msg}"))?;
        Ok(res)
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Get, Set, New, FromRow)]
#[serde(crate = "base::serde")]
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
        let pool = get_conn_by_pool();
        let res = sqlx::query_as::<_, Self>(r#"select device_id,transport,register_expires,
        register_time,local_addr,sip_from,sip_to,status,gb_version from GMV_DEVICE where device_id=?"#)
            .bind(device_id).fetch_optional(pool).await.hand_log(|msg| error!("{msg}"))?;
        Ok(res)
    }

    pub async fn insert_single_gmv_device_by_register(&self) -> GlobalResult<()> {
        let pool = get_conn_by_pool();
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
        let pool = get_conn_by_pool();
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
        let pool = get_conn_by_pool();
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
    pub alias_name: Option<String>,
}

impl GmvDeviceChannel {
    pub async fn insert_gmv_device_channel(device_id: &String, vs: Vec<(String, String)>) -> GlobalResult<Vec<GmvDeviceChannel>> {
        let dc_ls = Self::build(device_id, vs);
        let pool = get_conn_by_pool();
        let mut builder = sqlx::query_builder::QueryBuilder::new("INSERT INTO GMV_DEVICE_CHANNEL (device_id, channel_id, name, manufacturer,
         model, owner, status, civil_code, address, parental, block, parent_id, ip_address, port,password,
         longitude,latitude,ptz_type,supply_light_type,alias_name) ");
        builder.push_values(&dc_ls, |mut b, dc| {
            b.push_bind(&dc.device_id)
                .push_bind(&dc.channel_id)
                .push_bind(&dc.name)
                .push_bind(&dc.manufacturer)
                .push_bind(&dc.model)
                .push_bind(&dc.owner)
                .push_bind(&dc.status)
                .push_bind(&dc.civil_code)
                .push_bind(&dc.address)
                .push_bind(&dc.parental)
                .push_bind(&dc.block)
                .push_bind(&dc.parent_id)
                .push_bind(&dc.ip_address)
                .push_bind(&dc.port)
                .push_bind(&dc.password)
                .push_bind(&dc.longitude)
                .push_bind(&dc.latitude)
                .push_bind(&dc.ptz_type)
                .push_bind(&dc.supply_light_type)
                .push_bind(&dc.alias_name);
        });
        builder.push(" ON DUPLICATE KEY UPDATE device_id=VALUES(device_id),channel_id=VALUES(channel_id),name=VALUES(name),
        manufacturer=VALUES(manufacturer),model=VALUES(model),owner=VALUES(owner),status=VALUES(status),civil_code=VALUES(civil_code),
        address=VALUES(address),parental=VALUES(parental),block=VALUES(block),parent_id=VALUES(parent_id),ip_address=VALUES(ip_address),
        port=VALUES(port),password=VALUES(password),longitude=VALUES(longitude),latitude=VALUES(latitude),ptz_type=VALUES(ptz_type),
        supply_light_type=VALUES(supply_light_type),alias_name=VALUES(alias_name)");
        builder.build().execute(pool).await.hand_log(|msg| error!("{msg}"))?;
        Ok(dc_ls)
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

#[derive(Debug, FromRow, Default)]
pub struct GmvFileInfo {
    pub id: Option<i64>,
    pub device_id: String,
    pub channel_id: String,
    pub biz_time: Option<NaiveDateTime>,
    pub biz_id: String,
    pub file_type: Option<i32>,
    pub file_size: Option<u64>,
    pub file_name: String,
    pub file_format: Option<String>,
    pub dir_path: String,
    pub abs_path: String,
    pub note: Option<String>,
    pub is_del: Option<i32>,
    pub create_time: Option<NaiveDateTime>,
}

impl GmvFileInfo {
    pub async fn query_gmv_file_info_by_id(id: i64) -> GlobalResult<GmvFileInfo> {
        let pool = get_conn_by_pool();
        let res = sqlx::query_as::<_, GmvFileInfo>("select id,device_id,channel_id,biz_time,biz_id,file_type,file_size,file_name,file_format,dir_path,abs_path,note,is_del,create_time from GMV_FILE_INFO where id=?")
            .bind(id)
            .fetch_one(pool)
            .await.hand_log(|msg| error!("{msg}"))?;
        Ok(res)
    }

    pub async fn rm_gmv_file_info_by_id(biz_id: i64) -> GlobalResult<()> {
        let pool = get_conn_by_pool();
        sqlx::query("delete from GMV_FILE_INFO where id=?")
            .bind(biz_id)
            .execute(pool)
            .await.hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }

    pub async fn insert_gmv_file_info(arr: Vec<Self>) -> GlobalResult<()> {
        if arr.is_empty() {
            return Ok(());
        }
        let pool = get_conn_by_pool();
        let mut builder = sqlx::query_builder::QueryBuilder::new("INSERT INTO GMV_FILE_INFO
                (DEVICE_ID, CHANNEL_ID, BIZ_TIME, BIZ_ID, FILE_TYPE, FILE_SIZE,
                 FILE_NAME, FILE_FORMAT, DIR_PATH,ABS_PATH, NOTE, IS_DEL, CREATE_TIME) ");
        builder.push_values(arr.iter(), |mut b, info| {
            b.push_bind(&info.device_id)
                .push_bind(&info.channel_id)
                .push_bind(&info.biz_time)
                .push_bind(&info.biz_id)
                .push_bind(&info.file_type)
                .push_bind(&info.file_size)
                .push_bind(&info.file_name)
                .push_bind(&info.file_format)
                .push_bind(&info.dir_path)
                .push_bind(&info.abs_path)
                .push_bind(&info.note)
                .push_bind(&info.is_del)
                .push_bind(&info.create_time);
        });
        builder.build().execute(pool).await.hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }
}


#[cfg(test)]
#[allow(dead_code, unused_imports)]
mod tests {
    use super::*;
    use base::cfg_lib::conf::init_cfg;
    use base::chrono::TimeZone;
    use base::dbx::mysqlx;
    use base::tokio;

    // #[tokio::test]
    async fn test_batch_insert_gmv_file_info() {
        let files = vec![
            GmvFileInfo {
                id: None,
                device_id: "D001".into(),
                channel_id: "C001".into(),
                biz_time: Some(Local::now().naive_local()),
                biz_id: "BIZ123".into(),
                file_type: Some(0),
                file_size: Some(1024),
                file_name: "file1".into(),
                file_format: Some("jpg".into()),
                dir_path: "/path/to/file1".into(),
                abs_path: "".to_string(),
                note: Some("test1".into()),
                is_del: Some(0),
                create_time: Some(Local::now().naive_local()),
            },
            GmvFileInfo {
                id: None,
                device_id: "D002".into(),
                channel_id: "C002".into(),
                biz_time: Some(Local::now().naive_local()),
                biz_id: "BIZ124".into(),
                file_type: Some(1),
                file_size: Some(2048),
                file_name: "file2".into(),
                file_format: Some("mp4".into()),
                dir_path: "/path/to/file2".into(),
                abs_path: "".to_string(),
                note: Some("test2".into()),
                is_del: Some(0),
                create_time: Some(Local::now().naive_local()),
            },
            GmvFileInfo {
                id: None,
                device_id: "D003".into(),
                channel_id: "C003".into(),
                biz_time: Some(Local::now().naive_local()),
                biz_id: "BIZ125".into(),
                file_type: Some(2),
                file_size: Some(512),
                file_name: "file3".into(),
                file_format: Some("mp3".into()),
                dir_path: "/path/to/file3".into(),
                abs_path: "".to_string(),
                note: Some("test3".into()),
                is_del: Some(0),
                create_time: Some(Local::now().naive_local()),
            },
            GmvFileInfo {
                id: None,
                device_id: "D004".into(),
                channel_id: "C004".into(),
                biz_time: Some(Local::now().naive_local()),
                biz_id: "BIZ126".into(),
                file_type: Some(0),
                file_size: Some(3072),
                file_name: "file4".into(),
                file_format: Some("png".into()),
                dir_path: "/path/to/file4".into(),
                abs_path: "".to_string(),
                note: Some("test4".into()),
                is_del: Some(0),
                create_time: Some(Local::now().naive_local()),
            },
        ];
        init();
        let res = GmvFileInfo::insert_gmv_file_info(files).await;
        println!("{res:?}");
    }

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
        let res = GmvDevice::query_gmv_device_by_device_id(&"34020000001320000004".to_string()).await;
        if let Ok(Some(gd)) = res {
            let a = GmvDevice { device_id: "34020000001320000004".to_string(), register_time: Local::now().naive_local(), ..gd };
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
        let pool = get_conn_by_pool();
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
        let pool = get_conn_by_pool();
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

    fn init() {
        init_cfg("/home/ubuntu20/code/rs/mv/github/epimore/gmv/session/config.yml".to_string());
    }

    // #[tokio::test]
    async fn test_record() {
        init();
        let g = GmvRecord::query_gmv_record_by_biz_id("q2qcqqz1hqqu4qhqqs7Fqzsqqz6f2R6gc6Bs5I").await;
        println!("{:?}", g);
        if let Ok(Some(mut gr)) = g {
            gr.state = 2;
            gr.lt = Local::now().naive_local();
            let res = gr.update_gmv_record_by_biz_id().await;
            println!("{:?}", res);
        }

        // let r = GmvRecord::query_gmv_record_run_by_device_id_channel_id("34020000001110000009", "34020000001320000101").await;
        // println!("{:?}", r);
        // 
        // let result = GmvFileInfo::query_gmv_file_info_by_id(32).await;
        // println!("{:?}", result);
        // let _ = GmvFileInfo::rm_gmv_file_info_by_id(39).await;
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
