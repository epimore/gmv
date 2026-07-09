use crate::storage::db;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
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
    let res: Option<(String,)> = db::fetch_optional_as!(
        (String,),
        "SELECT COALESCE(c.status,'ONLY') FROM gb28181_device d LEFT JOIN gb28181_device_channel c on d.device_id=c.device_id and c.channel_id=? WHERE d.device_id=?",
        channel_id,
        device_id,
    )
    .hand_log(|msg| error!("{msg}"))?;
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
    // 多个语音输出子通道暂按 channel_id 取第一条，待真实设备接入后再决定最终策略。
    let res: Option<(String, String, String)> = db::fetch_optional_as!(
        (String, String, String),
        "SELECT a.device_id,a.channel_id,b.channel_id FROM gb28181_device_channel a \
         INNER JOIN gb28181_device_channel b \
         ON a.device_id=b.device_id AND a.channel_id=b.parent_id \
         WHERE a.device_id=? AND a.channel_id=? \
         ORDER BY b.channel_id LIMIT 1",
        device_id,
        channel_id,
    )
    .hand_log(|msg| error!("{msg}"))?;
    Ok(res.map_or_else(|| channel_id.to_string(), |(_, _, target_id)| target_id))
}
