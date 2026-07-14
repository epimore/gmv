use std::collections::{HashMap, HashSet};

use base::chrono::NaiveDateTime;
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use sqlx::FromRow;

use crate::storage::db;

pub const RESOURCE_KIND_VIDEO: &str = "video";
pub const RESOURCE_KIND_AUDIO_INPUT: &str = "audio_input";
pub const RESOURCE_KIND_AUDIO_OUTPUT: &str = "audio_output";
pub const RESOURCE_KIND_OTHER: &str = "other";

#[derive(Debug, Clone, Default)]
pub struct ResourceConfirmationInput {
    pub device_id: String,
    pub resource_id: String,
    pub resource_kind: String,
    pub owner_scope: String,
    pub owner_id: String,
    pub suggested_enum_id: String,
    pub source_parent_id: String,
    pub confirmed_by: String,
    pub remark: String,
}

#[derive(Debug, Clone, Default)]
pub struct ResourceConfirmationView {
    pub status: i64,
    pub resource_kind: String,
    pub owner_scope: String,
    pub owner_id: String,
    pub suggested_enum_id: String,
    pub source_parent_id: String,
    pub confirmed_by: String,
    pub confirmed_at_ms: i64,
    pub remark: String,
}

#[derive(Debug, Clone, Default)]
pub struct GbResourceView {
    pub device_id: String,
    pub resource_id: String,
    pub name: String,
    pub status: String,
    pub parent_id: String,
    pub type_code: String,
    pub enum_id: String,
    pub enum_name: String,
    pub suggested_kind: String,
    pub classification_mode: String,
    pub effective_kind: String,
    pub effective_owner_scope: String,
    pub effective_owner_id: String,
    pub warning: String,
    pub biz_enable: i64,
    pub owner_biz_enable: i64,
    pub supported: bool,
    pub available: bool,
    pub unavailable_reason: String,
    pub confirmation: Option<ResourceConfirmationView>,
}

#[derive(Debug, Clone, FromRow)]
struct ResourceRow {
    device_id: String,
    resource_id: String,
    name: String,
    status: String,
    parent_id: String,
    biz_enable: i64,
}

#[derive(Debug, Clone, FromRow)]
struct ConfirmationRow {
    device_id: String,
    resource_id: String,
    resource_kind: String,
    owner_scope: String,
    owner_id: String,
    status: i64,
    suggested_enum_id: Option<String>,
    source_parent_id: Option<String>,
    confirmed_by: String,
    confirmed_at: NaiveDateTime,
    remark: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
struct EnumRow {
    id: String,
    name: String,
    value_start: String,
    value_end: String,
}

impl GbResourceView {
    pub async fn list(device_id: &str) -> GlobalResult<Vec<Self>> {
        let resources = db::fetch_all_as!(
            ResourceRow,
            r#"SELECT c.device_id AS device_id,
                      c.channel_id AS resource_id,
                      COALESCE(c.name,'') AS name,
                      COALESCE(c.status,'UNKNOWN') AS status,
                      COALESCE(c.parent_id,'') AS parent_id,
                      COALESCE(conf.biz_enable,1) AS biz_enable
                 FROM gb28181_device_channel c
                 LEFT JOIN gb28181_device_channel_conf conf
                   ON conf.device_id=c.device_id AND conf.channel_id=c.channel_id
                WHERE c.device_id=?
                ORDER BY c.channel_id"#,
            device_id,
        )
        .hand_log(|msg| error!("{msg}"))?;
        let confirmations = ConfirmationRow::list(device_id).await?;
        let confirmation_map = confirmations
            .iter()
            .cloned()
            .map(|row| (row.resource_id.clone(), row))
            .collect::<HashMap<_, _>>();
        let enums = db::fetch_all_as!(
            EnumRow,
            "SELECT id,name,value_start,value_end FROM gb28181_enum_code WHERE status=1",
        )
        .hand_log(|msg| error!("{msg}"))?;
        let exact_enums = enums
            .iter()
            .filter(|row| row.value_start == row.value_end)
            .map(|row| (row.value_start.clone(), row.clone()))
            .collect::<HashMap<_, _>>();
        let resource_ids = resources
            .iter()
            .map(|row| row.resource_id.clone())
            .collect::<HashSet<_>>();
        let policy = resources
            .iter()
            .map(|row| (row.resource_id.clone(), row.biz_enable))
            .collect::<HashMap<_, _>>();

        let mut views = resources
            .into_iter()
            .map(|row| {
                let confirmation = confirmation_map.get(&row.resource_id);
                classify_resource(row, confirmation, &exact_enums, &resource_ids, &policy)
            })
            .collect::<Vec<_>>();

        for confirmation in confirmations {
            if resource_ids.contains(&confirmation.resource_id) {
                continue;
            }
            views.push(orphan_resource(confirmation));
        }
        views.sort_by(|left, right| left.resource_id.cmp(&right.resource_id));
        Ok(views)
    }

    pub async fn get(device_id: &str, resource_id: &str) -> GlobalResult<Option<Self>> {
        Ok(Self::list(device_id)
            .await?
            .into_iter()
            .find(|resource| resource.resource_id == resource_id))
    }

    pub async fn save_confirmation(input: ResourceConfirmationInput) -> GlobalResult<Self> {
        validate_confirmation(&input).await?;
        let suggested_enum_id = empty_to_none(input.suggested_enum_id);
        let source_parent_id = empty_to_none(input.source_parent_id);
        let remark = empty_to_none(input.remark);
        match db::backend() {
            db::SessionDatabaseBackend::Mysql => {
                db::execute!(
                    r#"INSERT INTO gb28181_resource_confirmation
                       (device_id,resource_id,resource_kind,owner_scope,owner_id,status,suggested_enum_id,source_parent_id,confirmed_by,confirmed_at,remark,create_time,update_time)
                       VALUES (?,?,?,?,?,1,?,?,?,CURRENT_TIMESTAMP,?,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)
                       ON DUPLICATE KEY UPDATE
                       resource_kind=VALUES(resource_kind),owner_scope=VALUES(owner_scope),owner_id=VALUES(owner_id),status=1,
                       suggested_enum_id=VALUES(suggested_enum_id),source_parent_id=VALUES(source_parent_id),confirmed_by=VALUES(confirmed_by),
                       confirmed_at=CURRENT_TIMESTAMP,remark=VALUES(remark),update_time=CURRENT_TIMESTAMP"#,
                    &input.device_id,
                    &input.resource_id,
                    &input.resource_kind,
                    &input.owner_scope,
                    &input.owner_id,
                    suggested_enum_id,
                    source_parent_id,
                    &input.confirmed_by,
                    remark,
                )
                .hand_log(|msg| error!("{msg}"))?;
            }
            db::SessionDatabaseBackend::Sqlite => {
                db::execute!(
                    r#"INSERT INTO gb28181_resource_confirmation
                       (device_id,resource_id,resource_kind,owner_scope,owner_id,status,suggested_enum_id,source_parent_id,confirmed_by,confirmed_at,remark,create_time,update_time)
                       VALUES (?,?,?,?,?,1,?,?,?,CURRENT_TIMESTAMP,?,CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)
                       ON CONFLICT(device_id,resource_id) DO UPDATE SET
                       resource_kind=excluded.resource_kind,owner_scope=excluded.owner_scope,owner_id=excluded.owner_id,status=1,
                       suggested_enum_id=excluded.suggested_enum_id,source_parent_id=excluded.source_parent_id,confirmed_by=excluded.confirmed_by,
                       confirmed_at=CURRENT_TIMESTAMP,remark=excluded.remark,update_time=CURRENT_TIMESTAMP"#,
                    &input.device_id,
                    &input.resource_id,
                    &input.resource_kind,
                    &input.owner_scope,
                    &input.owner_id,
                    suggested_enum_id,
                    source_parent_id,
                    &input.confirmed_by,
                    remark,
                )
                .hand_log(|msg| error!("{msg}"))?;
            }
        }
        Self::get(&input.device_id, &input.resource_id)
            .await?
            .ok_or_else(|| not_found("GB28181 resource"))
    }

    pub async fn reset_confirmation(
        device_id: &str,
        resource_id: &str,
        confirmed_by: &str,
    ) -> GlobalResult<Self> {
        if confirmed_by.trim().is_empty() {
            return Err(invalid("confirmed_by is required"));
        }
        db::execute!(
            "UPDATE gb28181_resource_confirmation SET status=0,confirmed_by=?,confirmed_at=CURRENT_TIMESTAMP,update_time=CURRENT_TIMESTAMP WHERE device_id=? AND resource_id=?",
            confirmed_by,
            device_id,
            resource_id,
        )
        .hand_log(|msg| error!("{msg}"))?;
        Self::get(device_id, resource_id)
            .await?
            .ok_or_else(|| not_found("GB28181 resource"))
    }
}

impl ConfirmationRow {
    async fn list(device_id: &str) -> GlobalResult<Vec<Self>> {
        db::fetch_all_as!(
            Self,
            r#"SELECT device_id,resource_id,resource_kind,owner_scope,owner_id,status,
                      suggested_enum_id,source_parent_id,confirmed_by,confirmed_at,remark
                 FROM gb28181_resource_confirmation
                WHERE device_id=?"#,
            device_id,
        )
        .hand_log(|msg| error!("{msg}"))
    }
}

fn classify_resource(
    row: ResourceRow,
    confirmation: Option<&ConfirmationRow>,
    exact_enums: &HashMap<String, EnumRow>,
    resource_ids: &HashSet<String>,
    policy: &HashMap<String, i64>,
) -> GbResourceView {
    let type_code = resource_type_code(&row.resource_id).unwrap_or_default();
    let enum_row = exact_enums.get(&type_code);
    let suggested_kind = default_kind(&type_code).to_string();
    let mut mode = "default".to_string();
    let mut effective_kind = suggested_kind.clone();
    let (mut owner_scope, mut owner_id) = default_owner(&row, resource_ids);
    let mut warning = String::new();
    let mut confirmation_view = None;

    if let Some(confirmation) = confirmation {
        confirmation_view = Some(confirmation_view_from(confirmation));
        if confirmation.status == 1 {
            mode = "manual".to_string();
            effective_kind = confirmation.resource_kind.clone();
            owner_scope = confirmation.owner_scope.clone();
            owner_id = confirmation.owner_id.clone();
            if confirmation.source_parent_id.as_deref().unwrap_or_default() != row.parent_id {
                mode = "manual_stale".to_string();
                warning = "MANUAL_OVERRIDE_STALE".to_string();
            }
        }
    }

    if effective_kind == "unknown" {
        mode = "unknown".to_string();
    }
    let owner_valid = match owner_scope.as_str() {
        "device" => owner_id == row.device_id,
        "resource" => owner_id != row.resource_id && resource_ids.contains(&owner_id),
        _ => false,
    };
    if effective_kind != "unknown" && !owner_valid {
        mode = "conflict".to_string();
    }
    let owner_biz_enable = if owner_scope == "resource" {
        policy.get(&owner_id).copied().unwrap_or(0)
    } else {
        1
    };
    let supported = effective_kind == RESOURCE_KIND_AUDIO_OUTPUT && mode != "conflict";
    let online = matches!(row.status.to_ascii_uppercase().as_str(), "ON" | "ONLINE");
    let available = supported && online && row.biz_enable == 1 && owner_biz_enable == 1;
    let unavailable_reason = if available {
        String::new()
    } else if mode == "unknown" {
        "UNKNOWN_RESOURCE_KIND".to_string()
    } else if mode == "conflict" {
        "RESOURCE_CONFLICT".to_string()
    } else if !supported {
        "NO_AUDIO_OUTPUT".to_string()
    } else if !online {
        "OUTPUT_OFFLINE".to_string()
    } else {
        "BUSINESS_DISABLED".to_string()
    };

    GbResourceView {
        device_id: row.device_id,
        resource_id: row.resource_id,
        name: row.name,
        status: row.status,
        parent_id: row.parent_id,
        type_code,
        enum_id: enum_row.map(|item| item.id.clone()).unwrap_or_default(),
        enum_name: enum_row.map(|item| item.name.clone()).unwrap_or_default(),
        suggested_kind,
        classification_mode: mode,
        effective_kind,
        effective_owner_scope: owner_scope,
        effective_owner_id: owner_id,
        warning,
        biz_enable: row.biz_enable,
        owner_biz_enable,
        supported,
        available,
        unavailable_reason,
        confirmation: confirmation_view,
    }
}

fn orphan_resource(confirmation: ConfirmationRow) -> GbResourceView {
    GbResourceView {
        device_id: confirmation.device_id.clone(),
        resource_id: confirmation.resource_id.clone(),
        classification_mode: "orphan".to_string(),
        effective_kind: confirmation.resource_kind.clone(),
        effective_owner_scope: confirmation.owner_scope.clone(),
        effective_owner_id: confirmation.owner_id.clone(),
        warning: "RESOURCE_ORPHAN".to_string(),
        unavailable_reason: "RESOURCE_ORPHAN".to_string(),
        confirmation: Some(confirmation_view_from(&confirmation)),
        ..GbResourceView::default()
    }
}

fn confirmation_view_from(row: &ConfirmationRow) -> ResourceConfirmationView {
    ResourceConfirmationView {
        status: row.status,
        resource_kind: row.resource_kind.clone(),
        owner_scope: row.owner_scope.clone(),
        owner_id: row.owner_id.clone(),
        suggested_enum_id: row.suggested_enum_id.clone().unwrap_or_default(),
        source_parent_id: row.source_parent_id.clone().unwrap_or_default(),
        confirmed_by: row.confirmed_by.clone(),
        confirmed_at_ms: row.confirmed_at.and_utc().timestamp_millis(),
        remark: row.remark.clone().unwrap_or_default(),
    }
}

fn resource_type_code(resource_id: &str) -> Option<String> {
    (resource_id.len() == 20 && resource_id.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| resource_id[10..13].to_string())
}

fn default_kind(type_code: &str) -> &'static str {
    match type_code {
        "131" | "132" => RESOURCE_KIND_VIDEO,
        "136" => RESOURCE_KIND_AUDIO_INPUT,
        "137" => RESOURCE_KIND_AUDIO_OUTPUT,
        "111" | "118" => RESOURCE_KIND_OTHER,
        _ => "unknown",
    }
}

fn default_owner(row: &ResourceRow, resource_ids: &HashSet<String>) -> (String, String) {
    if row.parent_id == row.device_id || row.parent_id.is_empty() {
        ("device".to_string(), row.device_id.clone())
    } else if resource_ids.contains(&row.parent_id) {
        ("resource".to_string(), row.parent_id.clone())
    } else {
        ("resource".to_string(), row.parent_id.clone())
    }
}

async fn validate_confirmation(input: &ResourceConfirmationInput) -> GlobalResult<()> {
    if !matches!(
        input.resource_kind.as_str(),
        RESOURCE_KIND_VIDEO
            | RESOURCE_KIND_AUDIO_INPUT
            | RESOURCE_KIND_AUDIO_OUTPUT
            | RESOURCE_KIND_OTHER
    ) {
        return Err(invalid("unsupported resource_kind"));
    }
    let resource = db::fetch_optional_as!(
        (String, String),
        "SELECT channel_id,COALESCE(parent_id,'') FROM gb28181_device_channel WHERE device_id=? AND channel_id=?",
        &input.device_id,
        &input.resource_id,
    )
    .hand_log(|msg| error!("{msg}"))?;
    if resource.is_none() {
        return Err(not_found("GB28181 resource"));
    }
    match input.owner_scope.as_str() {
        "device" if input.owner_id == input.device_id => {}
        "resource" if input.owner_id != input.resource_id => {
            let owner = db::fetch_optional_as!(
                (String,),
                "SELECT channel_id FROM gb28181_device_channel WHERE device_id=? AND channel_id=?",
                &input.device_id,
                &input.owner_id,
            )
            .hand_log(|msg| error!("{msg}"))?;
            if owner.is_none() {
                return Err(invalid("resource owner must belong to the same device"));
            }
            ensure_no_manual_owner_cycle(&input.device_id, &input.resource_id, &input.owner_id)
                .await?;
        }
        _ => return Err(invalid("invalid owner_scope or owner_id")),
    }
    if input.confirmed_by.trim().is_empty() {
        return Err(invalid("confirmed_by is required"));
    }
    Ok(())
}

async fn ensure_no_manual_owner_cycle(
    device_id: &str,
    resource_id: &str,
    owner_id: &str,
) -> GlobalResult<()> {
    let mut visited = HashSet::from([resource_id.to_string()]);
    let mut current = owner_id.to_string();
    loop {
        if !visited.insert(current.clone()) {
            return Err(invalid(
                "resource owner relationship must not contain a cycle",
            ));
        }
        let next = db::fetch_optional_as!(
            (String, String, i64),
            "SELECT owner_scope,owner_id,status FROM gb28181_resource_confirmation WHERE device_id=? AND resource_id=?",
            device_id,
            &current,
        )
        .hand_log(|msg| error!("{msg}"))?;
        match next {
            Some((scope, owner, 1)) if scope == "resource" => current = owner,
            _ => return Ok(()),
        }
    }
}

fn empty_to_none(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn invalid(message: &str) -> GlobalError {
    GlobalError::new_biz_error(BaseErrorCode::InvalidRequest.code(), message, |msg| {
        error!("{msg}")
    })
}

fn not_found(message: &str) -> GlobalError {
    GlobalError::new_biz_error(BaseErrorCode::NotFound.code(), message, |msg| {
        error!("{msg}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resource(resource_id: &str, parent_id: &str) -> ResourceRow {
        ResourceRow {
            device_id: "34020000001320000001".to_string(),
            resource_id: resource_id.to_string(),
            name: "resource".to_string(),
            status: "ON".to_string(),
            parent_id: parent_id.to_string(),
            biz_enable: 1,
        }
    }

    fn confirmation(resource_id: &str, kind: &str, status: i64) -> ConfirmationRow {
        ConfirmationRow {
            device_id: "34020000001320000001".to_string(),
            resource_id: resource_id.to_string(),
            resource_kind: kind.to_string(),
            owner_scope: "device".to_string(),
            owner_id: "34020000001320000001".to_string(),
            status,
            suggested_enum_id: None,
            source_parent_id: Some("34020000001320000001".to_string()),
            confirmed_by: "admin".to_string(),
            confirmed_at: base::chrono::Local::now().naive_local(),
            remark: None,
        }
    }

    fn classify(row: ResourceRow, confirmation: Option<&ConfirmationRow>) -> GbResourceView {
        let resource_ids = HashSet::from([row.resource_id.clone()]);
        let policy = HashMap::from([(row.resource_id.clone(), 1)]);
        classify_resource(row, confirmation, &HashMap::new(), &resource_ids, &policy)
    }

    #[test]
    fn extracts_standard_device_type_code() {
        assert_eq!(
            resource_type_code("34020000001370000001").as_deref(),
            Some("137")
        );
        assert_eq!(resource_type_code("vendor-resource"), None);
    }

    #[test]
    fn maps_audio_codes_without_forcing_unknown_codes() {
        assert_eq!(default_kind("136"), RESOURCE_KIND_AUDIO_INPUT);
        assert_eq!(default_kind("137"), RESOURCE_KIND_AUDIO_OUTPUT);
        assert_eq!(default_kind("199"), "unknown");
    }

    #[test]
    fn default_137_is_available_without_confirmation_row() {
        let view = classify(
            resource("34020000001370000001", "34020000001320000001"),
            None,
        );
        assert_eq!(view.classification_mode, "default");
        assert_eq!(view.effective_kind, RESOURCE_KIND_AUDIO_OUTPUT);
        assert!(view.available);
    }

    #[test]
    fn active_manual_other_disables_default_137() {
        let confirmation = confirmation("34020000001370000001", RESOURCE_KIND_OTHER, 1);
        let view = classify(
            resource("34020000001370000001", "34020000001320000001"),
            Some(&confirmation),
        );
        assert_eq!(view.classification_mode, "manual");
        assert_eq!(view.effective_kind, RESOURCE_KIND_OTHER);
        assert!(!view.supported);
    }

    #[test]
    fn reset_confirmation_falls_back_to_default_137() {
        let confirmation = confirmation("34020000001370000001", RESOURCE_KIND_OTHER, 0);
        let view = classify(
            resource("34020000001370000001", "34020000001320000001"),
            Some(&confirmation),
        );
        assert_eq!(view.classification_mode, "default");
        assert_eq!(view.effective_kind, RESOURCE_KIND_AUDIO_OUTPUT);
        assert!(view.available);
    }

    #[test]
    fn manual_audio_output_supports_nonstandard_resource_id() {
        let confirmation = confirmation("vendor-speaker", RESOURCE_KIND_AUDIO_OUTPUT, 1);
        let view = classify(
            resource("vendor-speaker", "34020000001320000001"),
            Some(&confirmation),
        );
        assert_eq!(view.classification_mode, "manual");
        assert_eq!(view.effective_kind, RESOURCE_KIND_AUDIO_OUTPUT);
        assert!(view.available);
    }

    #[test]
    fn parent_change_marks_manual_override_stale_but_keeps_valid_owner() {
        let mut confirmation = confirmation("34020000001370000001", RESOURCE_KIND_AUDIO_OUTPUT, 1);
        confirmation.source_parent_id = Some("34020000001320000002".to_string());
        let view = classify(
            resource("34020000001370000001", "34020000001320000001"),
            Some(&confirmation),
        );
        assert_eq!(view.classification_mode, "manual_stale");
        assert_eq!(view.warning, "MANUAL_OVERRIDE_STALE");
        assert!(view.available);
    }

    #[test]
    fn missing_resource_owner_blocks_manual_audio_output() {
        let mut confirmation = confirmation("34020000001370000001", RESOURCE_KIND_AUDIO_OUTPUT, 1);
        confirmation.owner_scope = "resource".to_string();
        confirmation.owner_id = "34020000001320000099".to_string();
        let view = classify(
            resource("34020000001370000001", "34020000001320000001"),
            Some(&confirmation),
        );
        assert_eq!(view.classification_mode, "conflict");
        assert_eq!(view.unavailable_reason, "RESOURCE_CONFLICT");
        assert!(!view.available);
    }

    #[test]
    fn output_business_policy_disables_broadcast_without_changing_kind() {
        let mut row = resource("34020000001370000001", "34020000001320000001");
        row.biz_enable = 0;
        let view = classify(row, None);
        assert_eq!(view.effective_kind, RESOURCE_KIND_AUDIO_OUTPUT);
        assert!(view.supported);
        assert!(!view.available);
        assert_eq!(view.unavailable_reason, "BUSINESS_DISABLED");
    }
}
