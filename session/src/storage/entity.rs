use rsip::Request;
use serde::{Deserialize, Serialize};
use common::chrono::Local;
use common::err::GlobalResult;
use constructor::{Get, New, Set};
use ezsql::crud;
use crate::gb::handler::parser;

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Get, Set, New)]
#[crud(table_name = "GMV_OAUTH",
funs = [
{fn_name = "read_single_gmv_oauth", sql_type = "read:single", condition = "device_id:="},
])]
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
    pub fn read_gmv_oauth_by_device_id(device_id: &String) -> GlobalResult<Option<GmvOauth>> {
        let mut conn = idb::get_mysql_conn().unwrap();
        GmvOauth::read_single_gmv_oauth(&mut conn, device_id)
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Get, Set, New)]
#[crud(table_name = "GMV_DEVICE",
funs = [
{fn_name = "insert_single_gmv_device", sql_type = "create:single", exist_update = "true"},
{fn_name = "update_gmv_device_status", sql_type = "update", fields = "status", condition = "device_id:="},
{fn_name = "query_single_gmv_device_by_device_id", sql_type = "read:single", condition = "device_id:="},
])]
pub struct GmvDevice {
    device_id: String,
    transport: String,
    register_expires: u32,
    register_time: u32,
    local_addr: String,
    sip_from: String,
    sip_to: String,
    status: u8,
    gb_version: Option<String>,
}


impl GmvDevice {
    pub fn query_gmv_device_by_device_id(device_id: &String) -> GlobalResult<Option<GmvDevice>> {
        let mut conn = idb::get_mysql_conn().unwrap();
        GmvDevice::query_single_gmv_device_by_device_id(&mut conn, device_id)
    }

    pub fn insert_single_gmv_device_by_register(&self) {
        let mut conn = idb::get_mysql_conn().unwrap();
        self.insert_single_gmv_device(&mut conn);
    }
    pub fn update_gmv_device_status_by_device_id(device_id: &String, status: u8) {
        let mut conn = idb::get_mysql_conn().unwrap();
        let mut oauth = GmvDevice::default();
        oauth.set_status(status);
        oauth.update_gmv_device_status(&mut conn, device_id);
    }

    pub fn build_gmv_device(req: &Request) -> GlobalResult<Self> {
        let device = Self {
            device_id: parser::header::get_device_id_by_request(req)?,
            transport: parser::header::get_transport(req)?,
            register_expires: parser::header::get_expires(req)?,
            register_time: Local::now().timestamp() as u32,
            local_addr: parser::header::get_local_addr(req)?,
            sip_from: parser::header::get_from(req)?,
            sip_to: parser::header::get_to(req)?,
            status: 1,
            gb_version: parser::header::get_gb_version(req),
        };
        Ok(device)
    }
}

#[derive(Default, Debug, Clone, Get)]
#[crud(table_name = "GMV_DEVICE",
funs = [
{fn_name = "update_gmv_device_ext", sql_type = "update", condition = "device_id:="},
])]
pub struct GmvDeviceExt {
    device_id: String,
    device_type: Option<String>,
    manufacturer: String,
    model: String,
    firmware: String,
    max_camera: Option<u8>,
}

impl GmvDeviceExt {
    pub fn update_gmv_device_ext_info(vs: Vec<(String, String)>) {
        let mut conn = idb::get_mysql_conn().unwrap();
        let ext = Self::build(vs);
        let device_id = ext.get_device_id();
        ext.update_gmv_device_ext(&mut conn, device_id);
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

#[derive(Debug, Clone, Default, Get)]
#[crud(table_name = "GMV_DEVICE_CHANNEL",
funs = [
{fn_name = "insert_batch_gmv_device_channel", sql_type = "create:batch", exist_update = "true"},
])]
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
    pub fn insert_gmv_device_channel(device_id: &String, vs: Vec<(String, String)>) {
        let dc_ls = Self::build(device_id, vs);
        let mut conn = idb::get_mysql_conn().unwrap();
        Self::insert_batch_gmv_device_channel(dc_ls, &mut conn);
    }
    pub fn build(device_id: &String, vs: Vec<(String, String)>) -> Vec<GmvDeviceChannel> {
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
mod tests {
    use common::once_cell::sync::OnceCell;
    use common::Tripe;
    use super::*;

    fn init_mysql() {
        static cell: OnceCell<Tripe> = OnceCell::new();
        cell.get_or_init(|| {
            let tripe = common::init();
            idb::init_mysql(tripe.get_cfg().get(0).unwrap());
            tripe
        });
    }

    #[test]
    fn test_read_gmv_oauth_by_device_id() {
        init_mysql();
        let res = GmvOauth::read_gmv_oauth_by_device_id(&"device_id_1".to_string());
        println!("{res:?}");
    }

    #[test]
    fn test_update_gmv_device_status_by_device_id() {
        init_mysql();
        let _ = GmvDevice::update_gmv_device_status_by_device_id(&"device_id_1".to_string(), 1);
    }

    #[test]
    fn test_query_single_gmv_device_by_device_id() {
        init_mysql();
        GmvDevice::default().insert_single_gmv_device_by_register();
        let result = GmvDevice::query_gmv_device_by_device_id(&"aaa".to_string());
        println!("{result:?}");
    }
}
