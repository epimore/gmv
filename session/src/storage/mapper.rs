use base::dbx::mysqlx::get_conn_by_pool;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use base::sqlx;

pub async fn get_device_channel_status(
    device_id: &String,
    channel_id: &String,
) -> GlobalResult<Option<String>> {
    #[cfg(test)]
    if crate::storage::entity::test_storage_enabled() {
        let _ = (device_id, channel_id);
        return Ok(Some("ON".to_string()));
    }
    let pool = get_conn_by_pool();
    let res: Option<(String,)> = sqlx::query_as(
        "SELECT IFNULL(c.`STATUS`,'ONLY') FROM GMV_DEVICE d LEFT JOIN GMV_DEVICE_CHANNEL c on d.DEVICE_ID=c.DEVICE_ID and c.CHANNEL_ID=? WHERE d.DEVICE_ID=?"
    )
        .bind(channel_id)
        .bind(device_id)
        .fetch_optional(pool).await.hand_log(|msg| error!("{msg}"))?;
    Ok(res.map(|(v,)| v))
}

pub async fn get_snapshot_dc_by_limit(
    start: u32,
    count: u32,
) -> GlobalResult<Vec<(String, String)>> {
    #[cfg(test)]
    if crate::storage::entity::test_storage_enabled() {
        let _ = (start, count);
        return Ok(Vec::new());
    }
    let pool = get_conn_by_pool();
    let script = r"
    SELECT c.DEVICE_ID,c.CHANNEL_ID FROM GMV_OAUTH a
    INNER JOIN GMV_DEVICE b ON a.DEVICE_ID = b.DEVICE_ID
    INNER JOIN GMV_DEVICE_CHANNEL c ON a.DEVICE_ID=c.DEVICE_ID
    LEFT JOIN GMV_DEVICE_CHANNEL_CONF cc ON c.DEVICE_ID=cc.DEVICE_ID AND c.CHANNEL_ID=cc.CHANNEL_ID
    WHERE
    a.DEL = 0
    AND b.ONLINE_EXPIRE_TIME IS NOT NULL
    AND b.ONLINE_EXPIRE_TIME > NOW()
    AND LEFT(b.GB_VERSION, 1) >= '3'
    AND COALESCE(cc.BIZ_ENABLE, 1) = 1
    AND COALESCE(cc.SNAPSHOT_ENABLE, 2) = 1
    AND !(c.`status` = 'OFF' OR c.`status` = 'OFFLINE' )
    ORDER BY c.DEVICE_ID,c.CHANNEL_ID limit ?,?";
    let dcs: Vec<(String, String)> = sqlx::query_as(script)
        .bind(start)
        .bind(count)
        .fetch_all(pool)
        .await
        .hand_log(|msg| error!("{msg}"))?;
    Ok(dcs)
}

#[cfg(test)]
#[allow(dead_code, unused_imports)]
mod test {
    use super::*;
    use base::cfg_lib::conf::init_cfg;
    use base::dbx::mysqlx;
    use base::tokio;

    // #[tokio::test]
    async fn test_get_snapshot_dc_by_limit() {
        init();
        let result = get_snapshot_dc_by_limit(0, 5).await;
        println!("{:?}", result);
    }

    // #[tokio::test]
    async fn test_get_device_channel_status() {
        init();
        let result = get_device_channel_status(
            &"34020000001110000001".to_string(),
            &"34020000001320000180".to_string(),
        )
        .await;
        println!("{:?}", result);
    }

    fn init() {
        init_cfg("/home/ubuntu20/code/rs/mv/github/epimore/gmv/session/config.yml".to_string());
    }
}
