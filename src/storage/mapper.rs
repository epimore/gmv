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

#[cfg(test)]
mod test {
    use common::once_cell::sync::OnceCell;
    use common::Tripe;
    use crate::storage::mapper::get_device_channel_status;

    fn init_mysql() {
        static cell: OnceCell<Tripe> = OnceCell::new();
        cell.get_or_init(|| {
            let tripe = common::init();
            idb::init_mysql(tripe.get_cfg());
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
}
