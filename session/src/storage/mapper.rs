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
