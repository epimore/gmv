use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path as AxumPath, Query};
use axum::http::{StatusCode, header};
use axum::response::Response;
use axum::routing::get;
use base::chrono::Local;
use base::dashmap::DashMap;
use base::err::BaseErrorCode;
use base::serde::Deserialize;
use base::tokio::fs::File;
use base::tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::http::Http;
use crate::storage::guard_query::GbChannelImageView;
use crate::storage::pics::Pics;

#[derive(Clone)]
struct AccessTicket {
    image_id: String,
    device_id: String,
    channel_id: String,
    mode: String,
    expires_at_ms: i64,
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

enum ResolvePathError {
    Invalid,
    Missing,
    StorageUnavailable,
}

pub(crate) fn routes() -> Router {
    Router::new().route("/images/{image_id}/file", get(serve_image))
}

pub async fn issue_ticket(
    image_id: &str,
    device_id: &str,
    channel_id: &str,
    mode: &str,
) -> Result<IssuedAccess, tonic::Status> {
    let image = GbChannelImageView::get(image_id, device_id, channel_id)
        .await
        .map_err(crate::guard_integration::storage_status_public)?
        .ok_or_else(|| ticket_status(BaseErrorCode::NotFound, "GB_CHANNEL_IMAGE_NOT_FOUND"))?;
    let path = resolve_file_path(&image)
        .await
        .map_err(|error| match error {
            ResolvePathError::Invalid => ticket_status(
                BaseErrorCode::InvalidRequest,
                "GB_CHANNEL_IMAGE_PATH_INVALID",
            ),
            ResolvePathError::Missing => {
                ticket_status(BaseErrorCode::NotFound, "GB_CHANNEL_IMAGE_FILE_MISSING")
            }
            ResolvePathError::StorageUnavailable => ticket_status(
                BaseErrorCode::Network,
                "GB_CHANNEL_IMAGE_STORAGE_UNAVAILABLE",
            ),
        })?;
    let metadata = match base::tokio::fs::metadata(&path).await {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => {
            return Err(ticket_status(
                BaseErrorCode::NotFound,
                "GB_CHANNEL_IMAGE_FILE_MISSING",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ticket_status(
                BaseErrorCode::NotFound,
                "GB_CHANNEL_IMAGE_FILE_MISSING",
            ));
        }
        Err(_) => {
            return Err(ticket_status(
                BaseErrorCode::Network,
                "GB_CHANNEL_IMAGE_STORAGE_UNAVAILABLE",
            ));
        }
    };
    let content_type = image_content_type(&image.file_format)
        .ok_or_else(|| ticket_status(BaseErrorCode::Unsupported, "GB_CHANNEL_IMAGE_UNSUPPORTED"))?;
    let file_name = image_file_name(&image).ok_or_else(|| {
        ticket_status(
            BaseErrorCode::InvalidRequest,
            "GB_CHANNEL_IMAGE_PATH_INVALID",
        )
    })?;
    let mode = if mode.eq_ignore_ascii_case("attachment") {
        "attachment"
    } else {
        "inline"
    };
    let conf = Pics::get_pics_by_conf();
    let now = Local::now().timestamp_millis();
    ACCESS_TICKETS.retain(|_, ticket| ticket.expires_at_ms > now);
    let expires_at_ms = now.saturating_add(
        i64::try_from(conf.access_ticket_ttl_secs.saturating_mul(1_000)).unwrap_or(i64::MAX),
    );
    let token = Uuid::new_v4().to_string();
    ACCESS_TICKETS.insert(
        token.clone(),
        AccessTicket {
            image_id: image_id.to_string(),
            device_id: device_id.to_string(),
            channel_id: channel_id.to_string(),
            mode: mode.to_string(),
            expires_at_ms,
        },
    );
    let http = Http::get_http_by_conf();
    Ok(IssuedAccess {
        url: build_access_url(&conf.public_base_url, &http.public_url, image_id, &token),
        expires_at_ms,
        content_type: content_type.to_string(),
        file_name,
        file_size: metadata.len(),
    })
}

fn build_access_url(
    public_base_url: &str,
    http_public_url: &str,
    image_id: &str,
    token: &str,
) -> String {
    let configured_base = public_base_url.trim();
    let base_url = if configured_base.is_empty() {
        http_public_url.trim_end_matches('/').to_string()
    } else {
        configured_base.trim_end_matches('/').to_string()
    };
    format!("{base_url}/images/{image_id}/file?token={token}")
}

async fn serve_image(
    AxumPath(image_id): AxumPath<String>,
    Query(query): Query<FileQuery>,
) -> Response<Body> {
    let now = Local::now().timestamp_millis();
    let Some(ticket) = ACCESS_TICKETS.get(&query.token) else {
        return status(StatusCode::UNAUTHORIZED);
    };
    if ticket.image_id != image_id || now >= ticket.expires_at_ms {
        drop(ticket);
        ACCESS_TICKETS.remove(&query.token);
        return status(StatusCode::UNAUTHORIZED);
    }
    let device_id = ticket.device_id.clone();
    let channel_id = ticket.channel_id.clone();
    let mode = ticket.mode.clone();
    drop(ticket);

    let Ok(Some(image)) = GbChannelImageView::get(&image_id, &device_id, &channel_id).await else {
        return status(StatusCode::NOT_FOUND);
    };
    let Some(content_type) = image_content_type(&image.file_format) else {
        return status(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    };
    let Some(file_name) = image_file_name(&image) else {
        return status(StatusCode::NOT_FOUND);
    };
    let path = match resolve_file_path(&image).await {
        Ok(path) => path,
        Err(ResolvePathError::Invalid | ResolvePathError::Missing) => {
            return status(StatusCode::NOT_FOUND);
        }
        Err(ResolvePathError::StorageUnavailable) => {
            return status(StatusCode::SERVICE_UNAVAILABLE);
        }
    };
    let Ok(file) = File::open(path).await else {
        return status(StatusCode::NOT_FOUND);
    };
    let Ok(metadata) = file.metadata().await else {
        return status(StatusCode::SERVICE_UNAVAILABLE);
    };
    if !metadata.is_file() {
        return status(StatusCode::NOT_FOUND);
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, metadata.len().to_string())
        .header(
            header::CONTENT_DISPOSITION,
            format!("{mode}; filename=\"{file_name}\""),
        )
        .header(header::CACHE_CONTROL, "private, no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from_stream(ReaderStream::new(file)))
        .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR))
}

async fn resolve_file_path(image: &GbChannelImageView) -> Result<PathBuf, ResolvePathError> {
    let file_name = image_file_name(image).ok_or(ResolvePathError::Invalid)?;
    let root = base::tokio::fs::canonicalize(Pics::get_pics_by_conf().storage_path)
        .await
        .map_err(|_| ResolvePathError::StorageUnavailable)?;
    let directory = image.abs_path.as_deref().unwrap_or(&image.dir_path);
    let path = base::tokio::fs::canonicalize(Path::new(directory).join(file_name))
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ResolvePathError::Missing
            } else {
                ResolvePathError::StorageUnavailable
            }
        })?;
    if !path.starts_with(&root) {
        return Err(ResolvePathError::Invalid);
    }
    Ok(path)
}

pub(crate) fn image_file_name(image: &GbChannelImageView) -> Option<String> {
    let name = image.file_name.trim();
    let format = image.file_format.trim().trim_start_matches('.');
    if name.is_empty()
        || format.is_empty()
        || Path::new(name).file_name().and_then(|value| value.to_str()) != Some(name)
        || !format.chars().all(|ch| ch.is_ascii_alphanumeric())
    {
        return None;
    }
    if Path::new(name).extension().is_some() {
        Some(name.to_string())
    } else {
        Some(format!("{name}.{format}"))
    }
}

pub(crate) fn image_content_type(format: &str) -> Option<&'static str> {
    match format
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase()
        .as_str()
    {
        "jpeg" | "jpg" => Some("image/jpeg"),
        "png" => Some("image/png"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        _ => None,
    }
}

fn status(code: StatusCode) -> Response<Body> {
    Response::builder()
        .status(code)
        .body(Body::empty())
        .unwrap()
}

fn ticket_status(code: BaseErrorCode, message: &str) -> tonic::Status {
    crate::guard_integration::storage_status_public(base::exception::GlobalError::new_biz_error(
        code.code(),
        message,
        |_| {},
    ))
}

#[cfg(test)]
mod tests {
    use super::{build_access_url, image_content_type, image_file_name};
    use crate::storage::guard_query::GbChannelImageView;

    #[test]
    fn builds_access_url_from_public_base_url() {
        assert_eq!(
            build_access_url(
                "https://gmv.example.com/session-1/",
                "http://192.0.2.10:28567",
                "16873",
                "token-1",
            ),
            "https://gmv.example.com/session-1/images/16873/file?token=token-1"
        );
    }

    #[test]
    fn falls_back_to_session_http_public_url() {
        assert_eq!(
            build_access_url("", "https://gmv.example.com/session-1/", "16873", "token-1",),
            "https://gmv.example.com/session-1/images/16873/file?token=token-1"
        );
    }

    #[test]
    fn maps_supported_image_content_types() {
        assert_eq!(image_content_type("jpeg"), Some("image/jpeg"));
        assert_eq!(image_content_type(".png"), Some("image/png"));
        assert_eq!(image_content_type("svg"), None);
    }

    #[test]
    fn rejects_unsafe_image_file_names() {
        let valid = GbChannelImageView {
            file_name: "snapshot-1".to_string(),
            file_format: "jpeg".to_string(),
            ..Default::default()
        };
        assert_eq!(image_file_name(&valid).as_deref(), Some("snapshot-1.jpeg"));

        let escaped = GbChannelImageView {
            file_name: "../snapshot-1".to_string(),
            file_format: "jpeg".to_string(),
            ..Default::default()
        };
        assert_eq!(image_file_name(&escaped), None);
    }
}
