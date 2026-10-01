use super::{
    ClaimOperation, ModelError, ModelRepository, ModelResult, OperationClaimRequest,
    OperationReceipt, OperationReceiptLimits, OperationReceiptState, decode_error,
};

use base_db::sqlx::Row;

impl ModelRepository {
    pub(crate) async fn claim_operation(
        &self,
        request: OperationClaimRequest<'_>,
        limits: OperationReceiptLimits,
    ) -> ModelResult<ClaimOperation> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| ModelError::io("begin model operation receipt", error))?;
        let cleanup_before = request.now_epoch_ms.saturating_sub(limits.retention_ms);
        base_db::sqlx::query(
            "DELETE FROM avai_model_management_operation WHERE \
             (CASE WHEN terminal_at_ms IS NOT NULL AND terminal_at_ms>deadline_epoch_ms \
              THEN terminal_at_ms ELSE deadline_epoch_ms END) < ?",
        )
        .bind(cleanup_before)
        .execute(&mut *transaction)
        .await
        .map_err(|error| ModelError::io("clean model operation receipts", error))?;
        if let Some(receipt) = base_db::sqlx::query(SELECT_OPERATION)
            .bind(request.operation_id)
            .bind(request.idempotency_key)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(|error| ModelError::io("query model operation receipt", error))?
            .map(decode_operation)
            .transpose()?
        {
            if receipt.operation_id != request.operation_id
                || receipt.idempotency_key != request.idempotency_key
                || receipt.operation_kind != request.operation_kind
                || receipt.request_hash != request.request_hash
                || receipt.deadline_epoch_ms != request.deadline_epoch_ms
            {
                return Err(ModelError::new(
                    "model_operation_conflict",
                    "operation or idempotency identity was reused with a different request",
                ));
            }
            transaction
                .commit()
                .await
                .map_err(|error| ModelError::io("commit model operation replay", error))?;
            return Ok(ClaimOperation::Existing(receipt));
        }
        let count: i64 =
            base_db::sqlx::query_scalar("SELECT COUNT(*) FROM avai_model_management_operation")
                .fetch_one(&mut *transaction)
                .await
                .map_err(|error| ModelError::io("count model operation receipts", error))?;
        if usize::try_from(count).unwrap_or(usize::MAX) >= limits.capacity {
            return Err(ModelError::new(
                "model_operation_capacity_exceeded",
                "model operation receipt capacity is exhausted",
            ));
        }
        base_db::sqlx::query(
            "INSERT INTO avai_model_management_operation(\
             operation_id,idempotency_key,operation_kind,request_hash,state,created_at_ms,\
             updated_at_ms,deadline_epoch_ms) VALUES(?,?,?,?,?,?,?,?)",
        )
        .bind(request.operation_id)
        .bind(request.idempotency_key)
        .bind(request.operation_kind)
        .bind(request.request_hash)
        .bind(OperationReceiptState::Pending as i32)
        .bind(request.now_epoch_ms)
        .bind(request.now_epoch_ms)
        .bind(request.deadline_epoch_ms)
        .execute(&mut *transaction)
        .await
        .map_err(|error| ModelError::io("insert model operation receipt", error))?;
        transaction
            .commit()
            .await
            .map_err(|error| ModelError::io("commit model operation receipt", error))?;
        Ok(ClaimOperation::New(OperationReceipt {
            operation_id: request.operation_id.to_string(),
            idempotency_key: request.idempotency_key.to_string(),
            operation_kind: request.operation_kind.to_string(),
            request_hash: request.request_hash.to_string(),
            state: OperationReceiptState::Pending,
            stable_error_code: None,
            deadline_epoch_ms: request.deadline_epoch_ms,
        }))
    }

    pub(crate) async fn find_operation(
        &self,
        request: &OperationClaimRequest<'_>,
    ) -> ModelResult<Option<OperationReceipt>> {
        let receipt = base_db::sqlx::query(SELECT_OPERATION)
            .bind(request.operation_id)
            .bind(request.idempotency_key)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| ModelError::io("query model operation receipt", error))?
            .map(decode_operation)
            .transpose()?;
        if let Some(receipt) = &receipt
            && (receipt.operation_id != request.operation_id
                || receipt.idempotency_key != request.idempotency_key
                || receipt.operation_kind != request.operation_kind
                || receipt.request_hash != request.request_hash
                || receipt.deadline_epoch_ms != request.deadline_epoch_ms)
        {
            return Err(ModelError::new(
                "model_operation_conflict",
                "operation or idempotency identity was reused with a different request",
            ));
        }
        Ok(receipt)
    }

    pub(crate) async fn finish_operation(
        &self,
        operation_id: &str,
        state: OperationReceiptState,
        stable_error_code: Option<&str>,
        now_epoch_ms: i64,
    ) -> ModelResult<()> {
        if state == OperationReceiptState::Pending {
            return Err(ModelError::new(
                "model_operation_state_invalid",
                "terminal operation receipt cannot remain pending",
            ));
        }
        let updated = base_db::sqlx::query(
            "UPDATE avai_model_management_operation SET state=?,stable_error_code=?,\
             updated_at_ms=?,terminal_at_ms=? WHERE operation_id=? AND state=?",
        )
        .bind(state as i32)
        .bind(stable_error_code)
        .bind(now_epoch_ms)
        .bind(now_epoch_ms)
        .bind(operation_id)
        .bind(OperationReceiptState::Pending as i32)
        .execute(&self.pool)
        .await
        .map_err(|error| ModelError::io("finish model operation receipt", error))?;
        if updated.rows_affected() != 1 {
            return Err(ModelError::new(
                "model_operation_conflict",
                "model operation receipt is no longer pending",
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn reset_operation_pending_for_test(
        &self,
        operation_id: &str,
    ) -> ModelResult<()> {
        base_db::sqlx::query(
            "UPDATE avai_model_management_operation SET state=?,stable_error_code=NULL,\
             terminal_at_ms=NULL WHERE operation_id=?",
        )
        .bind(OperationReceiptState::Pending as i32)
        .bind(operation_id)
        .execute(&self.pool)
        .await
        .map_err(|error| ModelError::io("reset test operation receipt", error))?;
        Ok(())
    }
}

const SELECT_OPERATION: &str = "SELECT operation_id,idempotency_key,operation_kind,request_hash,state,stable_error_code,deadline_epoch_ms FROM avai_model_management_operation WHERE operation_id=? OR idempotency_key=? LIMIT 1";

fn decode_operation(row: base_db::sqlx::sqlite::SqliteRow) -> ModelResult<OperationReceipt> {
    let state: i32 = row.try_get("state").map_err(decode_error)?;
    let state = match state {
        1 => OperationReceiptState::Pending,
        2 => OperationReceiptState::Succeeded,
        3 => OperationReceiptState::Failed,
        _ => {
            return Err(ModelError::new(
                "model_operation_state_invalid",
                "persisted model operation state is invalid",
            ));
        }
    };
    Ok(OperationReceipt {
        operation_id: row.try_get("operation_id").map_err(decode_error)?,
        idempotency_key: row.try_get("idempotency_key").map_err(decode_error)?,
        operation_kind: row.try_get("operation_kind").map_err(decode_error)?,
        request_hash: row.try_get("request_hash").map_err(decode_error)?,
        state,
        stable_error_code: row.try_get("stable_error_code").map_err(decode_error)?,
        deadline_epoch_ms: row.try_get("deadline_epoch_ms").map_err(decode_error)?,
    })
}
