use base::chrono::{Local, TimeZone};
use gmv_protocol::common::v1::ErrorDetail;
use gmv_protocol::guard::v1::guard_media_server::GuardMedia;
use gmv_protocol::guard::v1::{
    FinishRecordRequest, QueryRunningRecordRequest, QueryRunningRecordResponse,
    RecordMutationResponse, StartRecordRequest,
};
use tonic::{Request, Response, Status};

use crate::core::{GuardError, GuardResult};
use crate::store::model::{GmvRecordInsert, RecordFileInsert};
use crate::store::persistent::MediaRepository;

#[derive(Debug, Clone)]
pub struct GuardMediaRpc {
    repository: MediaRepository,
}

impl GuardMediaRpc {
    pub fn new(repository: MediaRepository) -> Self {
        Self { repository }
    }
}

#[tonic::async_trait]
impl GuardMedia for GuardMediaRpc {
    async fn query_running_record(
        &self,
        request: Request<QueryRunningRecordRequest>,
    ) -> Result<Response<QueryRunningRecordResponse>, Status> {
        let request = request.into_inner();
        validate_device_channel(&request.device_id, &request.channel_id)?;
        let exists = self
            .repository
            .running_record_exists(&request.device_id, &request.channel_id)
            .await
            .map_err(status)?;
        Ok(Response::new(QueryRunningRecordResponse { exists }))
    }

    async fn start_record(
        &self,
        request: Request<StartRecordRequest>,
    ) -> Result<Response<RecordMutationResponse>, Status> {
        let request = request.into_inner();
        let response = match self.start_record_inner(request).await {
            Ok(()) => RecordMutationResponse {
                accepted: true,
                error: None,
            },
            Err(error) => RecordMutationResponse {
                accepted: false,
                error: Some(error_detail("start_record_failed", &error.to_string())),
            },
        };
        Ok(Response::new(response))
    }

    async fn finish_record(
        &self,
        request: Request<FinishRecordRequest>,
    ) -> Result<Response<RecordMutationResponse>, Status> {
        let request = request.into_inner();
        let response = match self.finish_record_inner(request).await {
            Ok(true) => RecordMutationResponse {
                accepted: true,
                error: None,
            },
            Ok(false) => RecordMutationResponse {
                accepted: false,
                error: Some(error_detail("record_not_found", "record not found")),
            },
            Err(error) => RecordMutationResponse {
                accepted: false,
                error: Some(error_detail("finish_record_failed", &error.to_string())),
            },
        };
        Ok(Response::new(response))
    }
}

impl GuardMediaRpc {
    async fn start_record_inner(&self, request: StartRecordRequest) -> GuardResult<()> {
        if request.biz_id.is_empty() || request.stream_app_name.is_empty() {
            return Err(GuardError::InvalidConfig(
                "biz_id and stream_app_name are required".to_string(),
            ));
        }
        validate_device_channel(&request.device_id, &request.channel_id)
            .map_err(|status| GuardError::InvalidConfig(status.message().to_string()))?;
        let st = format_epoch(request.st_epoch_sec)?;
        let et = format_epoch(request.et_epoch_sec)?;
        let now = Local::now()
            .naive_local()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        self.repository
            .insert_record(&GmvRecordInsert {
                biz_id: request.biz_id,
                device_id: request.device_id,
                channel_id: request.channel_id,
                user_id: (!request.user_id.is_empty()).then_some(request.user_id),
                st,
                et,
                speed: request.speed,
                ct: now.clone(),
                state: 0,
                lt: now,
                stream_app_name: request.stream_app_name,
            })
            .await
    }

    async fn finish_record_inner(&self, request: FinishRecordRequest) -> GuardResult<bool> {
        if request.biz_id.is_empty() || request.dir_path.is_empty() {
            return Err(GuardError::InvalidConfig(
                "biz_id and dir_path are required".to_string(),
            ));
        }
        let now = Local::now()
            .naive_local()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        self.repository
            .finish_record(&RecordFileInsert {
                biz_id: request.biz_id,
                file_size: request.file_size,
                record_duration_sec: request.record_duration_sec,
                file_format: (!request.file_format.is_empty()).then_some(request.file_format),
                dir_path: request.dir_path,
                abs_path: (!request.abs_path.is_empty()).then_some(request.abs_path),
                now,
            })
            .await
    }
}

fn validate_device_channel(device_id: &str, channel_id: &str) -> Result<(), Status> {
    if device_id.is_empty() || channel_id.is_empty() {
        return Err(Status::invalid_argument(
            "device_id and channel_id are required",
        ));
    }
    Ok(())
}

fn format_epoch(epoch_sec: i64) -> GuardResult<String> {
    Local
        .timestamp_opt(epoch_sec, 0)
        .single()
        .map(|value| value.naive_local().format("%Y-%m-%d %H:%M:%S").to_string())
        .ok_or_else(|| GuardError::InvalidConfig("invalid record timestamp".to_string()))
}

fn error_detail(code: &str, message: &str) -> ErrorDetail {
    ErrorDetail {
        code: code.to_string(),
        message: message.to_string(),
        metadata: std::collections::HashMap::new(),
    }
}

fn status(error: GuardError) -> Status {
    match error {
        GuardError::Conflict(message) => Status::already_exists(message),
        GuardError::NotFound(message) => Status::not_found(message),
        other => Status::invalid_argument(other.to_string()),
    }
}
