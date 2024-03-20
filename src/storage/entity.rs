use rsip::Request;
use serde::{Deserialize, Serialize};
use common::chrono::Local;
use common::err::GlobalResult;
use common::net::shard::Bill;
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

    pub fn build_gmv_device(req: &Request, bill: &Bill) -> GlobalResult<Self> {
        let device = Self {
            device_id: parser::header::get_device_id(req)?,
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

pub struct GmvDeviceExt {
    device_id: String,
    device_type: Option<String>,
    manufacturer: String,
    model: String,
    firmware: String,
    max_camera: Option<u8>,
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
            idb::init_mysql(tripe.get_cfg());
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
