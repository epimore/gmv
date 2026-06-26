use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use base_db::dbx::mysqlx::get_conn_by_pool;
use base_db::sqlx;

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

pub async fn resolve_broadcast_target_id(
    device_id: &str,
    channel_id: &str,
) -> GlobalResult<String> {
    #[cfg(test)]
    if crate::storage::entity::test_storage_enabled() {
        return Ok(channel_id.to_string());
    }
    let pool = get_conn_by_pool();
    // 多个语音输出子通道暂按 CHANNEL_ID 取第一条，待真实设备接入后再决定最终策略。
    let res: Option<(String, String, String)> = sqlx::query_as(
        "SELECT a.DEVICE_ID,a.CHANNEL_ID,b.CHANNEL_ID FROM GMV_DEVICE_CHANNEL a \
         INNER JOIN GMV_DEVICE_CHANNEL b \
         ON a.DEVICE_ID=b.DEVICE_ID AND a.CHANNEL_ID=b.PARENT_ID \
         WHERE a.DEVICE_ID=? AND a.CHANNEL_ID=? \
         ORDER BY b.CHANNEL_ID LIMIT 1",
    )
    .bind(device_id)
    .bind(channel_id)
    .fetch_optional(pool)
    .await
    .hand_log(|msg| error!("{msg}"))?;
    Ok(res.map_or_else(|| channel_id.to_string(), |(_, _, target_id)| target_id))
}
