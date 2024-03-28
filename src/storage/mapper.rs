use mysql::params;
use mysql::prelude::Queryable;
use common::err::{GlobalResult, TransError};
use common::log::error;

pub fn get_device_channel_status(device_id: &String, channel_id: &String) -> GlobalResult<Option<String>> {
    let sql = String::from("select `STATUS` from GMV_DEVICE_CHANNEL where device_id=:device_id and channel_id=:channel_id");
    let mut conn = idb::get_mysql_conn().unwrap();
    let option_status = conn.exec_first(sql, params! {device_id,channel_id}).hand_err(|msg| error!("{msg}"))?;
    Ok(option_status)
}

pub fn get_device_status_info(device_id: &String) -> GlobalResult<Option<(u8, u8, u32, u32, u8)>> {
    let sql = String::from("SELECT o.HEARTBEAT_SEC,o.`STATUS`,d.REGISTER_EXPIRES,d.REGISTER_TIME,d.`STATUS` FROM GMV_OAUTH o INNER JOIN GMV_DEVICE d ON o.DEVICE_ID = d.DEVICE_ID where d.device_id=:device_id");
    let mut conn = idb::get_mysql_conn().unwrap();
    let option_status = conn.exec_first(sql, params! {device_id})
        .hand_err(|msg| error!("{msg}"))?
        .map(|(heart, enable, expire, reg_ts, on)| (heart, enable, expire, reg_ts, on));
    Ok(option_status)
}

#[cfg(test)]
mod test {
    use common::once_cell::sync::OnceCell;
    use common::Tripe;
    use crate::storage::mapper::{get_device_channel_status, get_device_status_info};

    fn init_mysql() {
        static cell: OnceCell<Tripe> = OnceCell::new();
        cell.get_or_init(|| {
            let tripe = common::init();
            idb::init_mysql(tripe.get_cfg().get(0).unwrap());
            tripe
        });
    }

    #[test]
    fn test_get_device_channel_status() {
        init_mysql();
        let result = get_device_channel_status(&"1aa".to_string(), &"2ss".to_string());
        let result1 = get_device_channel_status(&"a".to_string(), &"2ss".to_string());
        println!("{result:?}");
        println!("{result1:?}");
    }

    #[test]
    fn test_get_device_status_info() {
        init_mysql();
        let status_info = get_device_status_info(&"34020000001110000001".to_string());
        println!("{status_info:?}");
    }
}
