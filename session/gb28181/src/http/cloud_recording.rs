use std::sync::LazyLock;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use axum::routing::get;
use base::chrono::Local;
use base::dashmap::DashMap;
use base::err::BaseErrorCode;
use base::exception::GlobalError;
use base::serde::Deserialize;
use base::tokio::fs::File;
use base::tokio::io::{AsyncReadExt, AsyncSeekExt};
use base::tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::http::Http;
use crate::service::cloud_recording::{resolve_file_path, storage_root};
use crate::state::DownloadConf;
use crate::storage::recording::{self, FILE_READY};

#[derive(Clone)]
struct AccessTicket {
    task_id: String,
    mode: String,
    idle_expires_at_ms: i64,
    absolute_expires_at_ms: i64,
}

#[derive(Debug, Deserialize)]
#[serde(crate = "base::serde")]
struct FileQuery {
    token: String,
}

pub struct IssuedAccess {
    pub url: String,
    pub expires_at_ms: i64,
    pub content_type: String,
    pub file_name: String,
    pub file_size: u64,
}

static ACCESS_TICKETS: LazyLock<DashMap<String, AccessTicket>> = LazyLock::new(DashMap::new);

pub(crate) fn routes() -> Router {
    Router::new().route("/cloud-recordings/{task_id}/file", get(serve_recording))
}

pub async fn issue_ticket(task_id: &str, mode: &str) -> Result<IssuedAccess, tonic::Status> {
    let record = recording::get(task_id)
        .await
        .map_err(crate::guard_integration::storage_status_public)?
        .ok_or_else(|| ticket_status(BaseErrorCode::NotFound, "CLOUD_RECORDING_NOT_FOUND"))?;
    if record.file_state.as_deref() != Some(FILE_READY) {
        return Err(ticket_status(
            BaseErrorCode::InvalidState,
            "CLOUD_RECORDING_FILE_NOT_READY",
        ));
    }
    let file = recording::file(task_id)
        .await
        .map_err(crate::guard_integration::storage_status_public)?
        .ok_or_else(|| ticket_status(BaseErrorCode::NotFound, "CLOUD_RECORDING_FILE_MISSING"))?;
    let root =
        storage_root().map_err(|_| ticket_status(BaseErrorCode::Network, "STORAGE_UNAVAILABLE"))?;
    let file_format = file.file_format.as_deref().unwrap_or("mp4");
    let path = resolve_file_path(&root, &file.dir_path, &file.file_name, file_format)
        .map_err(|_| ticket_status(BaseErrorCode::InvalidRequest, "STORAGE_PATH_INVALID"))?;
    let metadata = match base::tokio::fs::metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let _ = recording::mark_file_missing(task_id).await;
            return Err(ticket_status(
                BaseErrorCode::NotFound,
                "CLOUD_RECORDING_FILE_MISSING",
            ));
        }
        Err(_) => {
            return Err(ticket_status(BaseErrorCode::Network, "STORAGE_UNAVAILABLE"));
        }
    };
    let mode = if mode.eq_ignore_ascii_case("attachment") {
        "attachment"
    } else {
        "inline"
    };
    let conf = DownloadConf::get_download_conf();
    let now = Local::now().timestamp_millis();
    let idle_expires_at_ms = now.saturating_add(
        i64::try_from(conf.access_ticket_idle_ttl_secs.saturating_mul(1_000)).unwrap_or(i64::MAX),
    );
    let absolute_expires_at_ms = now.saturating_add(
        i64::try_from(conf.access_ticket_max_ttl_secs.saturating_mul(1_000)).unwrap_or(i64::MAX),
    );
    let token = Uuid::new_v4().to_string();
    ACCESS_TICKETS.insert(
        token.clone(),
        AccessTicket {
            task_id: task_id.to_string(),
            mode: mode.to_string(),
            idle_expires_at_ms,
            absolute_expires_at_ms,
        },
    );
    let http = Http::get_http_by_conf();
    Ok(IssuedAccess {
        url: build_access_url(&conf.public_base_url, &http.public_url, task_id, &token),
        expires_at_ms: idle_expires_at_ms,
        content_type: "video/mp4".to_string(),
        file_name: format!("{}.{}", file.file_name, file_format.trim_start_matches('.')),
        file_size: metadata.len(),
    })
}

fn build_access_url(
    public_base_url: &str,
    http_public_url: &str,
    task_id: &str,
    token: &str,
) -> String {
    let configured_base = public_base_url.trim();
    let base_url = if configured_base.is_empty() {
        http_public_url.trim_end_matches('/').to_string()
    } else {
        configured_base.trim_end_matches('/').to_string()
    };
    format!("{base_url}/cloud-recordings/{task_id}/file?token={token}")
}

async fn serve_recording(
    Path(task_id): Path<String>,
    Query(query): Query<FileQuery>,
    headers: HeaderMap,
) -> Response<Body> {
    let now = Local::now().timestamp_millis();
    let Some(mut ticket) = ACCESS_TICKETS.get_mut(&query.token) else {
        return status(StatusCode::UNAUTHORIZED);
    };
    if ticket.task_id != task_id
        || now >= ticket.idle_expires_at_ms
        || now >= ticket.absolute_expires_at_ms
    {
        drop(ticket);
        ACCESS_TICKETS.remove(&query.token);
        return status(StatusCode::UNAUTHORIZED);
    }
    let range_header = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());
    let mode = ticket.mode.clone();
    drop(ticket);

    let Ok(Some(file_meta)) = recording::file(&task_id).await else {
        return status(StatusCode::NOT_FOUND);
    };
    let Ok(root) = storage_root() else {
        return status(StatusCode::SERVICE_UNAVAILABLE);
    };
    let format = file_meta.file_format.as_deref().unwrap_or("mp4");
    let Ok(path) = resolve_file_path(&root, &file_meta.dir_path, &file_meta.file_name, format)
    else {
        return status(StatusCode::NOT_FOUND);
    };
    let Ok(mut file) = File::open(path).await else {
        let _ = recording::mark_file_missing(&task_id).await;
        return status(StatusCode::NOT_FOUND);
    };
    let Ok(metadata) = file.metadata().await else {
        return status(StatusCode::SERVICE_UNAVAILABLE);
    };
    let total = metadata.len();
    if total == 0 {
        return status(StatusCode::NO_CONTENT);
    }
    let (response_status, start, end) = match range_header {
        Some(value) => match parse_range(value, total) {
            Some(range) => (StatusCode::PARTIAL_CONTENT, range.0, range.1),
            None => {
                return Response::builder()
                    .status(StatusCode::RANGE_NOT_SATISFIABLE)
                    .header(header::CONTENT_RANGE, format!("bytes */{total}"))
                    .body(Body::empty())
                    .unwrap();
            }
        },
        None => (StatusCode::OK, 0, total - 1),
    };
    if range_header.is_some()
        && let Some(mut ticket) = ACCESS_TICKETS.get_mut(&query.token)
    {
        let ttl_ms = i64::try_from(
            DownloadConf::get_download_conf()
                .access_ticket_idle_ttl_secs
                .saturating_mul(1_000),
        )
        .unwrap_or(i64::MAX);
        ticket.idle_expires_at_ms = now
            .saturating_add(ttl_ms)
            .min(ticket.absolute_expires_at_ms);
    }
    if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return status(StatusCode::SERVICE_UNAVAILABLE);
    }
    let length = end - start + 1;
    let stream = ReaderStream::new(file.take(length));
    let safe_name = format!("{}.{}", file_meta.file_name, format.trim_start_matches('.'));
    let mut builder = Response::builder()
        .status(response_status)
        .header(header::CONTENT_TYPE, "video/mp4")
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, length.to_string())
        .header(
            header::CONTENT_DISPOSITION,
            format!("{mode}; filename=\"{safe_name}\""),
        );
    if response_status == StatusCode::PARTIAL_CONTENT {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{total}"),
        );
    }
    builder.body(Body::from_stream(stream)).unwrap()
}

fn parse_range(value: &str, total: u64) -> Option<(u64, u64)> {
    let spec = value.strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None;
    }
    let (start, end) = spec.split_once('-')?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().ok()?.min(total);
        return (suffix > 0).then_some((total - suffix, total - 1));
    }
    let start = start.parse::<u64>().ok()?;
    if start >= total {
        return None;
    }
    let end = if end.is_empty() {
        total - 1
    } else {
        end.parse::<u64>().ok()?.min(total - 1)
    };
    (start <= end).then_some((start, end))
}

fn status(status: StatusCode) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(Body::empty())
        .unwrap()
}

fn ticket_status(code: BaseErrorCode, message: &str) -> tonic::Status {
    crate::guard_integration::storage_status_public(GlobalError::new_biz_error(
        code.code(),
        message,
        |_| {},
    ))
}

#[cfg(test)]
mod tests {
    use super::{build_access_url, parse_range};

    #[test]
    fn builds_access_url_from_public_base_url() {
        assert_eq!(
            build_access_url(
                "https://gmv.example.com/recordings/session-1/",
                "http://192.0.2.10:28567",
                "task-1",
                "token-1",
            ),
            "https://gmv.example.com/recordings/session-1/cloud-recordings/task-1/file?token=token-1"
        );
    }

    #[test]
    fn falls_back_to_session_http_public_url() {
        assert_eq!(
            build_access_url(
                "",
                "https://gmv.example.com/session-1/",
                "task-1",
                "token-1"
            ),
            "https://gmv.example.com/session-1/cloud-recordings/task-1/file?token=token-1"
        );
    }

    #[test]
    fn parses_single_byte_ranges() {
        assert_eq!(parse_range("bytes=0-9", 100), Some((0, 9)));
        assert_eq!(parse_range("bytes=90-", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
        assert_eq!(parse_range("bytes=100-", 100), None);
        assert_eq!(parse_range("bytes=0-1,4-5", 100), None);
    }
}
