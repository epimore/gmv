use crate::storage::db;
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
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
    let resources = crate::storage::resource::GbResourceView::list(device_id).await?;
    let mut outputs = resources
        .iter()
        .filter(|resource| {
            resource.effective_kind == crate::storage::resource::RESOURCE_KIND_AUDIO_OUTPUT
                && resource.available
                && (resource.resource_id == channel_id
                    || channel_id == device_id
                    || resource.effective_owner_id == channel_id)
        })
        .collect::<Vec<_>>();
    outputs.sort_by(|left, right| left.resource_id.cmp(&right.resource_id));
    match outputs.as_slice() {
        [] => Err(GlobalError::new_biz_error(
            BaseErrorCode::NotFound.code(),
            "no available GB28181 audio output resource",
            |msg| error!("{msg}: device_id={device_id}, scope_id={channel_id}"),
        )),
        [output] => Ok(output.resource_id.clone()),
        _ => Ok(channel_id.to_string()),
    }
}
