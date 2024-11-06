use common::chrono::NaiveDateTime;
use common::dbx::mysqlx::get_conn_by_pool;
use common::exception::{GlobalResult, TransError};
use common::log::error;
use common::sqlx;

pub async fn get_device_channel_status(device_id: &String, channel_id: &String) -> GlobalResult<Option<String>> {
    let pool = get_conn_by_pool()?;
    let res: Option<(String,)> = sqlx::query_as(
        "SELECT IFNULL(c.`STATUS`,'ONLY') FROM GMV_DEVICE d LEFT JOIN GMV_DEVICE_CHANNEL c on d.DEVICE_ID=c.DEVICE_ID and c.CHANNEL_ID=? WHERE d.DEVICE_ID=?"
    )
        .bind(channel_id)
        .bind(device_id)
        .fetch_optional(pool).await.hand_log(|msg| error!("{msg}"))?;
    Ok(res.map(|(v, )| v))
}

pub async fn get_device_status_info(device_id: &String) -> GlobalResult<Option<(u8, u8, u32, NaiveDateTime, u8)>> {
    let pool = get_conn_by_pool()?;
    let res = sqlx::query_as::<_, (u8, u8, u32, NaiveDateTime, u8)>(
        "SELECT o.HEARTBEAT_SEC,o.`STATUS`,d.REGISTER_EXPIRES,d.REGISTER_TIME,d.`STATUS` FROM GMV_OAUTH o INNER JOIN GMV_DEVICE d ON o.DEVICE_ID = d.DEVICE_ID where d.device_id=?",
    ).bind(device_id).fetch_optional(pool).await.hand_log(|msg| error!("{msg}"))?;
    Ok(res)
}

#[cfg(test)]
mod test {
    use common::dbx::mysqlx;
    use super::*;

    #[common::tokio::test]
    async fn test_get_device_channel_status() {
        mysqlx::init_conn_pool();
        let result = get_device_channel_status(&"34020000001110000001".to_string(), &"34020000001320000180".to_string()).await;
        println!("{:?}",result);
    }

    #[common::tokio::test]
    async fn test_get_device_status_info() {
        mysqlx::init_conn_pool();
        let status_info = get_device_status_info(&"34020000001110000001".to_string()).await;
        println!("{:?}",status_info);
    }
}
