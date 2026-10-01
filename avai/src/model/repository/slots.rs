use super::{
    CapabilityRecovery, ModelError, ModelIdentity, ModelRepository, ModelResult, ModelState,
    PersistedCapabilitySlot, decode_error,
};

use base_db::sqlx::Row;

impl ModelRepository {
    pub(crate) async fn max_generation(&self) -> ModelResult<u64> {
        let value: i64 = base_db::sqlx::query_scalar(
            "SELECT COALESCE(MAX(generation), 0) FROM (\
             SELECT active_generation AS generation FROM avai_model_capability_slot \
             UNION ALL SELECT previous_generation FROM avai_model_capability_slot\
             )",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|error| ModelError::io("query model generation", error))?;
        u64::try_from(value)
            .map_err(|_| ModelError::new("model_generation_invalid", "negative generation"))
    }

    pub(crate) async fn set_state(
        &self,
        identity: &ModelIdentity,
        state: ModelState,
        generation: Option<u64>,
        now_epoch_ms: i64,
    ) -> ModelResult<()> {
        let generation = generation
            .map(i64::try_from)
            .transpose()
            .map_err(|_| ModelError::new("model_generation_invalid", "generation is too large"))?;
        let updated = base_db::sqlx::query(
            "UPDATE avai_model_revision SET state=?, active_generation=?, activated_at_ms=CASE WHEN ? IS NULL THEN activated_at_ms ELSE ? END, last_error=NULL WHERE model_id=? AND version=? AND revision=?",
        )
        .bind(state as i32)
        .bind(generation)
        .bind(generation)
        .bind(now_epoch_ms)
        .bind(&identity.model_id)
        .bind(&identity.version)
        .bind(&identity.revision)
        .execute(&self.pool)
        .await
        .map_err(|error| ModelError::io("update model state", error))?;
        if updated.rows_affected() != 1 {
            return Err(ModelError::new(
                "model_not_found",
                "installed model does not exist",
            ));
        }
        Ok(())
    }

    pub(crate) async fn activate(
        &self,
        identity: &ModelIdentity,
        no_longer_active: &[ModelIdentity],
        slots: &[PersistedCapabilitySlot],
        generation: u64,
        now_epoch_ms: i64,
    ) -> ModelResult<()> {
        let generation = i64::try_from(generation)
            .map_err(|_| ModelError::new("model_generation_invalid", "generation is too large"))?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| ModelError::io("begin model activation", error))?;
        for previous in no_longer_active {
            base_db::sqlx::query(
                "UPDATE avai_model_revision SET state=?, active_generation=NULL WHERE model_id=? AND version=? AND revision=?",
            )
            .bind(ModelState::Ready as i32)
            .bind(&previous.model_id)
            .bind(&previous.version)
            .bind(&previous.revision)
            .execute(&mut *transaction)
            .await
            .map_err(|error| ModelError::io("persist previous model state", error))?;
        }
        let updated = base_db::sqlx::query(
            "UPDATE avai_model_revision SET state=?, active_generation=?, activated_at_ms=?, last_error=NULL WHERE model_id=? AND version=? AND revision=?",
        )
        .bind(ModelState::Active as i32)
        .bind(generation)
        .bind(now_epoch_ms)
        .bind(&identity.model_id)
        .bind(&identity.version)
        .bind(&identity.revision)
        .execute(&mut *transaction)
        .await
        .map_err(|error| ModelError::io("persist active model state", error))?;
        if updated.rows_affected() != 1 {
            return Err(ModelError::new(
                "model_not_found",
                "installed model does not exist",
            ));
        }
        for slot in slots {
            let active_generation = i64::try_from(slot.active_generation).map_err(|_| {
                ModelError::new("model_generation_invalid", "generation is too large")
            })?;
            let previous_generation = slot
                .previous_generation
                .map(i64::try_from)
                .transpose()
                .map_err(|_| {
                    ModelError::new("model_generation_invalid", "generation is too large")
                })?;
            base_db::sqlx::query(
                "INSERT INTO avai_model_capability_slot(\
                 capability,active_model_id,active_version,active_revision,active_generation,\
                 previous_model_id,previous_version,previous_revision,previous_generation\
                 ) VALUES(?,?,?,?,?,?,?,?,?) ON CONFLICT(capability) DO UPDATE SET \
                 active_model_id=excluded.active_model_id,active_version=excluded.active_version,\
                 active_revision=excluded.active_revision,active_generation=excluded.active_generation,\
                 previous_model_id=excluded.previous_model_id,previous_version=excluded.previous_version,\
                 previous_revision=excluded.previous_revision,previous_generation=excluded.previous_generation",
            )
            .bind(&slot.capability)
            .bind(&slot.active_identity.model_id)
            .bind(&slot.active_identity.version)
            .bind(&slot.active_identity.revision)
            .bind(active_generation)
            .bind(
                slot.previous_identity
                    .as_ref()
                    .map(|identity| &identity.model_id),
            )
            .bind(
                slot.previous_identity
                    .as_ref()
                    .map(|identity| &identity.version),
            )
            .bind(
                slot.previous_identity
                    .as_ref()
                    .map(|identity| &identity.revision),
            )
            .bind(previous_generation)
            .execute(&mut *transaction)
            .await
            .map_err(|error| ModelError::io("persist model capability slot", error))?;
        }
        transaction
            .commit()
            .await
            .map_err(|error| ModelError::io("commit model activation", error))
    }

    pub(crate) async fn list_slots(&self) -> ModelResult<Vec<PersistedCapabilitySlot>> {
        let rows = base_db::sqlx::query(
            "SELECT capability,active_model_id,active_version,active_revision,active_generation,\
             previous_model_id,previous_version,previous_revision,previous_generation \
             FROM avai_model_capability_slot ORDER BY capability",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| ModelError::io("list model capability slots", error))?;
        rows.into_iter().map(decode_slot).collect()
    }

    pub(crate) async fn clear_previous(&self, capability: &str) -> ModelResult<()> {
        base_db::sqlx::query(
            "UPDATE avai_model_capability_slot SET previous_model_id=NULL,previous_version=NULL,\
             previous_revision=NULL,previous_generation=NULL WHERE capability=?",
        )
        .bind(capability)
        .execute(&self.pool)
        .await
        .map_err(|error| ModelError::io("retire previous model slot", error))?;
        Ok(())
    }

    pub(crate) async fn recover_failed_active(
        &self,
        failed: &ModelIdentity,
        recoveries: &[CapabilityRecovery],
        now_epoch_ms: i64,
        reason: &str,
    ) -> ModelResult<()> {
        if recoveries.is_empty() {
            return Err(ModelError::new(
                "model_recovery_invalid",
                "failed model has no active capability slots to recover",
            ));
        }
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| ModelError::io("begin failed model recovery", error))?;
        for recovery in recoveries {
            let expected_generation =
                i64::try_from(recovery.expected_active_generation).map_err(|_| {
                    ModelError::new("model_generation_invalid", "generation is too large")
                })?;
            let changed = if let Some((replacement, generation)) = &recovery.replacement {
                let generation = i64::try_from(*generation).map_err(|_| {
                    ModelError::new("model_generation_invalid", "generation is too large")
                })?;
                let changed = base_db::sqlx::query(
                    "UPDATE avai_model_capability_slot SET \
                     active_model_id=?,active_version=?,active_revision=?,active_generation=?,\
                     previous_model_id=NULL,previous_version=NULL,previous_revision=NULL,previous_generation=NULL \
                     WHERE capability=? AND active_model_id=? AND active_version=? AND active_revision=? AND active_generation=?",
                )
                .bind(&replacement.model_id)
                .bind(&replacement.version)
                .bind(&replacement.revision)
                .bind(generation)
                .bind(&recovery.capability)
                .bind(&failed.model_id)
                .bind(&failed.version)
                .bind(&failed.revision)
                .bind(expected_generation)
                .execute(&mut *transaction)
                .await
                .map_err(|error| ModelError::io("promote recovery model slot", error))?;
                base_db::sqlx::query(
                    "UPDATE avai_model_revision SET state=?,active_generation=?,activated_at_ms=?,last_error=NULL \
                     WHERE model_id=? AND version=? AND revision=?",
                )
                .bind(ModelState::Active as i32)
                .bind(generation)
                .bind(now_epoch_ms)
                .bind(&replacement.model_id)
                .bind(&replacement.version)
                .bind(&replacement.revision)
                .execute(&mut *transaction)
                .await
                .map_err(|error| ModelError::io("persist recovery model state", error))?;
                changed
            } else {
                base_db::sqlx::query(
                    "DELETE FROM avai_model_capability_slot WHERE capability=? AND \
                     active_model_id=? AND active_version=? AND active_revision=? AND active_generation=?",
                )
                .bind(&recovery.capability)
                .bind(&failed.model_id)
                .bind(&failed.version)
                .bind(&failed.revision)
                .bind(expected_generation)
                .execute(&mut *transaction)
                .await
                .map_err(|error| ModelError::io("clear failed model slot", error))?
            };
            if changed.rows_affected() != 1 {
                return Err(ModelError::new(
                    "model_recovery_conflict",
                    "persisted capability slot changed during recovery",
                ));
            }
        }
        base_db::sqlx::query(
            "UPDATE avai_model_capability_slot SET previous_model_id=NULL,previous_version=NULL,\
             previous_revision=NULL,previous_generation=NULL WHERE previous_model_id=? AND \
             previous_version=? AND previous_revision=?",
        )
        .bind(&failed.model_id)
        .bind(&failed.version)
        .bind(&failed.revision)
        .execute(&mut *transaction)
        .await
        .map_err(|error| ModelError::io("clear failed previous model references", error))?;
        let remaining: i64 = base_db::sqlx::query_scalar(
            "SELECT COUNT(*) FROM avai_model_capability_slot WHERE active_model_id=? AND \
             active_version=? AND active_revision=?",
        )
        .bind(&failed.model_id)
        .bind(&failed.version)
        .bind(&failed.revision)
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| ModelError::io("verify failed model slot recovery", error))?;
        if remaining != 0 {
            return Err(ModelError::new(
                "model_recovery_conflict",
                "not every active capability of the failed model was recovered",
            ));
        }
        let updated = base_db::sqlx::query(
            "UPDATE avai_model_revision SET state=?,active_generation=NULL,last_error=? WHERE \
             model_id=? AND version=? AND revision=?",
        )
        .bind(ModelState::Failed as i32)
        .bind(reason)
        .bind(&failed.model_id)
        .bind(&failed.version)
        .bind(&failed.revision)
        .execute(&mut *transaction)
        .await
        .map_err(|error| ModelError::io("persist failed recovered model", error))?;
        if updated.rows_affected() != 1 {
            return Err(ModelError::new(
                "model_not_found",
                "failed model revision does not exist",
            ));
        }
        transaction
            .commit()
            .await
            .map_err(|error| ModelError::io("commit failed model recovery", error))
    }

    pub(crate) async fn reconcile_unreferenced_ready(&self) -> ModelResult<()> {
        base_db::sqlx::query(
            "UPDATE avai_model_revision SET state=?,active_generation=NULL WHERE state=? AND NOT EXISTS (\
             SELECT 1 FROM avai_model_capability_slot slot WHERE \
             slot.previous_model_id=avai_model_revision.model_id AND \
             slot.previous_version=avai_model_revision.version AND \
             slot.previous_revision=avai_model_revision.revision\
             )",
        )
        .bind(ModelState::Installed as i32)
        .bind(ModelState::Ready as i32)
        .execute(&self.pool)
        .await
        .map_err(|error| ModelError::io("reconcile ready model state", error))?;
        Ok(())
    }
}

fn decode_slot(row: base_db::sqlx::sqlite::SqliteRow) -> ModelResult<PersistedCapabilitySlot> {
    let active_generation: i64 = row.try_get("active_generation").map_err(decode_error)?;
    let previous_model_id: Option<String> =
        row.try_get("previous_model_id").map_err(decode_error)?;
    let previous_version: Option<String> = row.try_get("previous_version").map_err(decode_error)?;
    let previous_revision: Option<String> =
        row.try_get("previous_revision").map_err(decode_error)?;
    let previous_generation: Option<i64> =
        row.try_get("previous_generation").map_err(decode_error)?;
    let previous_identity = match (previous_model_id, previous_version, previous_revision) {
        (Some(model_id), Some(version), Some(revision)) => Some(ModelIdentity {
            model_id,
            version,
            revision,
        }),
        (None, None, None) => None,
        _ => {
            return Err(ModelError::new(
                "model_slot_invalid",
                "persisted previous model identity is incomplete",
            ));
        }
    };
    if previous_identity.is_some() != previous_generation.is_some() {
        return Err(ModelError::new(
            "model_slot_invalid",
            "persisted previous model generation is incomplete",
        ));
    }
    Ok(PersistedCapabilitySlot {
        capability: row.try_get("capability").map_err(decode_error)?,
        active_identity: ModelIdentity {
            model_id: row.try_get("active_model_id").map_err(decode_error)?,
            version: row.try_get("active_version").map_err(decode_error)?,
            revision: row.try_get("active_revision").map_err(decode_error)?,
        },
        active_generation: u64::try_from(active_generation)
            .map_err(|_| ModelError::new("model_generation_invalid", "negative generation"))?,
        previous_identity,
        previous_generation: previous_generation
            .map(u64::try_from)
            .transpose()
            .map_err(|_| ModelError::new("model_generation_invalid", "negative generation"))?,
    })
}
