use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

use axum::Router;
use axum::body::Body;
use axum::extract::{Path as AxumPath, Query};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use axum::routing::get;
use base::chrono::Local;
use base::dashmap::DashMap;
use base::err::BaseErrorCode;
use base::serde::Deserialize;
use base::sha2::{Digest, Sha256};
use base::tokio::fs::File;
use base::tokio_util::io::ReaderStream;
use base::utils::rt::GlobalRuntime;
#[cfg(unix)]
use base::{
    bytes::Bytes,
    net::{
        transport::MessageTransport,
        uds::{ManagedUnixStream, ManagedUnixStreamListener, UnixTransportConfig},
    },
};
use gmv_protocol::common::v1::ErrorDetail;
#[cfg(unix)]
use gmv_protocol::session::v1::{ReadGrantedImageRequest, ReadGrantedImageResponse};
#[cfg(unix)]
use prost::Message;
use uuid::Uuid;

use crate::http::{Http, ImageSourceUdsConf};
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

#[derive(Clone)]
struct SourceAccessGrant {
    grant_id: String,
    image_id: String,
    device_id: String,
    channel_id: String,
    expected_node_id: String,
    expected_instance_id: String,
    task_id: String,
    purpose: String,
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

pub struct IssuedSourceAccess {
    pub grant_id: String,
    pub url: String,
    pub uds_url: Option<String>,
    pub uds_max_message_size: usize,
    pub proof: Vec<u8>,
    pub expires_at_ms: i64,
    pub content_type: String,
    pub file_name: String,
    pub file_size: u64,
    pub sha256: String,
}

static ACCESS_TICKETS: LazyLock<DashMap<String, AccessTicket>> = LazyLock::new(DashMap::new);
static SOURCE_ACCESS_GRANTS: LazyLock<DashMap<String, SourceAccessGrant>> =
    LazyLock::new(DashMap::new);
static ACTIVE_SOURCE_UDS: LazyLock<RwLock<Option<(PathBuf, usize)>>> =
    LazyLock::new(|| RwLock::new(None));

enum ResolvePathError {
    Invalid,
    Missing,
    StorageUnavailable,
}

pub(crate) fn routes() -> Router {
    Router::new()
        .route("/images/{image_id}/file", get(serve_image))
        .route(
            "/internal/images/{image_id}/source",
            get(serve_image_source),
        )
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

pub async fn issue_source_grant(
    image_id: &str,
    device_id: &str,
    channel_id: &str,
    expected_node_id: &str,
    expected_instance_id: &str,
    task_id: &str,
    purpose: &str,
    requested_deadline_ms: i64,
) -> Result<IssuedSourceAccess, tonic::Status> {
    if expected_node_id.trim().is_empty()
        || expected_instance_id.trim().is_empty()
        || task_id.trim().is_empty()
        || purpose.trim().is_empty()
    {
        return Err(ticket_status(
            BaseErrorCode::InvalidRequest,
            "GB_CHANNEL_IMAGE_SOURCE_GRANT_INVALID",
        ));
    }
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
    let bytes = base::tokio::fs::read(&path)
        .await
        .map_err(|_| ticket_status(BaseErrorCode::Network, "GB_CHANNEL_IMAGE_READ_FAILED"))?;
    let content_type = image_content_type(&image.file_format)
        .ok_or_else(|| ticket_status(BaseErrorCode::Unsupported, "GB_CHANNEL_IMAGE_UNSUPPORTED"))?;
    let file_name = image_file_name(&image).ok_or_else(|| {
        ticket_status(
            BaseErrorCode::InvalidRequest,
            "GB_CHANNEL_IMAGE_PATH_INVALID",
        )
    })?;
    let now = Local::now().timestamp_millis();
    if requested_deadline_ms <= now {
        return Err(ticket_status(
            BaseErrorCode::InvalidRequest,
            "GB_CHANNEL_IMAGE_SOURCE_GRANT_EXPIRED",
        ));
    }
    let conf = Pics::get_pics_by_conf();
    let configured_deadline = now.saturating_add(
        i64::try_from(conf.access_ticket_ttl_secs.saturating_mul(1_000)).unwrap_or(i64::MAX),
    );
    let expires_at_ms = requested_deadline_ms.min(configured_deadline);
    SOURCE_ACCESS_GRANTS.retain(|_, grant| grant.expires_at_ms > now);
    let grant_id = Uuid::new_v4().to_string();
    let proof = Uuid::new_v4().to_string().into_bytes();
    let proof_header = proof_header(&proof);
    SOURCE_ACCESS_GRANTS.insert(
        proof_header,
        SourceAccessGrant {
            grant_id: grant_id.clone(),
            image_id: image_id.to_string(),
            device_id: device_id.to_string(),
            channel_id: channel_id.to_string(),
            expected_node_id: expected_node_id.to_string(),
            expected_instance_id: expected_instance_id.to_string(),
            task_id: task_id.to_string(),
            purpose: purpose.to_string(),
            expires_at_ms,
        },
    );
    let http = Http::get_http_by_conf();
    Ok(IssuedSourceAccess {
        grant_id,
        url: build_source_access_url(&http.public_url, image_id),
        uds_url: active_source_uds_uri(),
        uds_max_message_size: http.image_source_uds.max_message_size,
        proof,
        expires_at_ms,
        content_type: content_type.to_string(),
        file_name,
        file_size: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
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

fn build_source_access_url(http_public_url: &str, image_id: &str) -> String {
    format!(
        "{}/internal/images/{image_id}/source",
        http_public_url.trim_end_matches('/')
    )
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

async fn serve_image_source(
    AxumPath(image_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response<Body> {
    let Some(proof) = headers
        .get("x-gmv-access-proof")
        .and_then(|value| value.to_str().ok())
    else {
        return status(StatusCode::UNAUTHORIZED);
    };
    let Some(grant) = consume_source_grant(&image_id, None, proof, Local::now().timestamp_millis())
    else {
        return status(StatusCode::UNAUTHORIZED);
    };
    let Ok(Some(image)) =
        GbChannelImageView::get(&image_id, &grant.device_id, &grant.channel_id).await
    else {
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
    base::log::info!(
        "internal image grant consumed: action=image_source, stage=read, outcome=accepted, grant_id={}, task_id={}, purpose={}, expected_node_id={}, expected_instance_id={}, image_id={}",
        grant.grant_id,
        grant.task_id,
        grant.purpose,
        grant.expected_node_id,
        grant.expected_instance_id,
        image_id
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, metadata.len().to_string())
        .header(
            header::CONTENT_DISPOSITION,
            format!("inline; filename=\"{file_name}\""),
        )
        .header(header::CACHE_CONTROL, "private, no-store")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from_stream(ReaderStream::new(file)))
        .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR))
}

fn proof_header(proof: &[u8]) -> String {
    use base::base64::Engine;
    base::base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(proof)
}

fn consume_source_grant(
    image_id: &str,
    grant_id: Option<&str>,
    proof: &str,
    now_epoch_ms: i64,
) -> Option<SourceAccessGrant> {
    let (_, grant) = SOURCE_ACCESS_GRANTS.remove(proof)?;
    (grant.image_id == image_id
        && grant_id.is_none_or(|grant_id| grant.grant_id == grant_id)
        && now_epoch_ms < grant.expires_at_ms)
        .then_some(grant)
}

fn active_source_uds_uri() -> Option<String> {
    ACTIVE_SOURCE_UDS.read().ok().and_then(|active| {
        active
            .as_ref()
            .map(|(path, _)| format!("unix://{}", path.display()))
    })
}

#[cfg(unix)]
pub async fn start_source_uds_service(
    runtime: &GlobalRuntime,
    conf: &ImageSourceUdsConf,
) -> Result<(), base::exception::GlobalError> {
    if !conf.enabled {
        return Ok(());
    }
    std::fs::create_dir_all(&conf.socket_root).map_err(|error| {
        base::exception::GlobalError::new_sys_error(
            &format!("create image source UDS root failed: {error}"),
            |_| {},
        )
    })?;
    let root = conf.socket_root.canonicalize().map_err(|error| {
        base::exception::GlobalError::new_sys_error(
            &format!("resolve image source UDS root failed: {error}"),
            |_| {},
        )
    })?;
    let file_name = conf.socket_path.file_name().ok_or_else(|| {
        base::exception::GlobalError::new_sys_error(
            "image source UDS socket_path has no file name",
            |_| {},
        )
    })?;
    let socket_path = if conf.socket_path.is_absolute() {
        conf.socket_path.clone()
    } else {
        let parent = conf.socket_path.parent().unwrap_or_else(|| Path::new("."));
        let parent = parent.canonicalize().map_err(|error| {
            base::exception::GlobalError::new_sys_error(
                &format!("resolve image source UDS parent failed: {error}"),
                |_| {},
            )
        })?;
        parent.join(file_name)
    };
    let mut transport_conf = UnixTransportConfig::new(root, socket_path.clone());
    transport_conf.max_message_size = conf.max_message_size;
    let listener = ManagedUnixStreamListener::bind(transport_conf)
        .await
        .map_err(source_uds_global_error)?;
    if let Ok(mut active) = ACTIVE_SOURCE_UDS.write() {
        *active = Some((socket_path.clone(), conf.max_message_size));
    }
    let task_runtime = runtime.clone();
    let task_cancel = runtime.cancel.clone();
    runtime
        .spawn("session-image-source-uds", async move {
            let mut sequence = 0u64;
            loop {
                sequence = sequence.wrapping_add(1);
                let accepted = base::tokio::select! {
                    _ = task_cancel.cancelled() => break,
                    accepted = listener.accept(
                        &task_runtime,
                        format!("session-image-source-uds-io-{sequence}"),
                    ) => accepted,
                };
                let (connection, credentials) = match accepted {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        base::log::error!("image source UDS accept failed: {error}");
                        GlobalRuntime::request_shutdown_with_error();
                        break;
                    }
                };
                base::log::debug!(
                    "image source UDS peer accepted: pid={:?}, uid={}, gid={}",
                    credentials.process_id,
                    credentials.user_id,
                    credentials.group_id
                );
                if let Err(error) = task_runtime.spawn(
                    format!("session-image-source-uds-request-{sequence}"),
                    handle_source_uds_connection(connection),
                ) {
                    base::log::error!("spawn image source UDS request failed: {error}");
                    GlobalRuntime::request_shutdown_with_error();
                    break;
                }
            }
            listener.close();
            if let Err(error) = listener.close_and_wait().await {
                base::log::error!("close image source UDS listener failed: {error}");
                GlobalRuntime::request_shutdown_with_error();
            }
            if let Ok(mut active) = ACTIVE_SOURCE_UDS.write() {
                if active
                    .as_ref()
                    .is_some_and(|(path, _)| path == &socket_path)
                {
                    *active = None;
                }
            }
        })
        .map_err(|error| {
            if let Ok(mut active) = ACTIVE_SOURCE_UDS.write() {
                *active = None;
            }
            error
        })?;
    Ok(())
}

#[cfg(not(unix))]
pub async fn start_source_uds_service(
    _runtime: &GlobalRuntime,
    conf: &ImageSourceUdsConf,
) -> Result<(), base::exception::GlobalError> {
    if conf.enabled {
        return Err(base::exception::GlobalError::new_sys_error(
            "image source UDS is unsupported on this platform",
            |_| {},
        ));
    }
    Ok(())
}

#[cfg(unix)]
async fn handle_source_uds_connection(connection: ManagedUnixStream) {
    let response = match connection.receive().await {
        Ok(message) => match ReadGrantedImageRequest::decode(message.payload) {
            Ok(request) => read_granted_image(request).await,
            Err(_) => source_uds_error("invalid_request", "image source request is invalid"),
        },
        Err(error) => {
            base::log::debug!("image source UDS receive ended: {error}");
            connection.close();
            let _ = connection.close_and_wait().await;
            return;
        }
    };
    if let Err(error) = connection.send(Bytes::from(response.encode_to_vec())).await {
        base::log::debug!("image source UDS response failed: {error}");
    } else {
        let _ = base::tokio::time::timeout(std::time::Duration::from_secs(1), connection.receive())
            .await;
    }
    if let Err(error) = connection.close_and_wait().await {
        base::log::debug!("image source UDS connection close failed: {error}");
    }
}

#[cfg(unix)]
async fn read_granted_image(request: ReadGrantedImageRequest) -> ReadGrantedImageResponse {
    let proof = proof_header(&request.proof);
    let Some(grant) = consume_source_grant(
        &request.image_id,
        Some(&request.grant_id),
        &proof,
        Local::now().timestamp_millis(),
    ) else {
        return source_uds_error("source_fetch_denied", "image source grant is invalid");
    };
    let image =
        match GbChannelImageView::get(&request.image_id, &grant.device_id, &grant.channel_id).await
        {
            Ok(Some(image)) => image,
            Ok(None) => return source_uds_error("source_not_found", "image source does not exist"),
            Err(_) => return source_uds_error("source_unavailable", "image source lookup failed"),
        };
    let Some(content_type) = image_content_type(&image.file_format) else {
        return source_uds_error("source_type_unsupported", "image format is unsupported");
    };
    let path = match resolve_file_path(&image).await {
        Ok(path) => path,
        Err(_) => {
            return source_uds_error("source_unavailable", "image source path is unavailable");
        }
    };
    let bytes = match base::tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(_) => return source_uds_error("source_unavailable", "image source read failed"),
    };
    base::log::info!(
        "internal image grant consumed: action=image_source, transport=uds, stage=read, outcome=accepted, grant_id={}, task_id={}, purpose={}, expected_node_id={}, expected_instance_id={}, image_id={}",
        grant.grant_id,
        grant.task_id,
        grant.purpose,
        grant.expected_node_id,
        grant.expected_instance_id,
        request.image_id
    );
    ReadGrantedImageResponse {
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        image: bytes,
        content_type: content_type.to_string(),
        error: None,
    }
}

#[cfg(unix)]
fn source_uds_error(code: &str, message: &str) -> ReadGrantedImageResponse {
    ReadGrantedImageResponse {
        error: Some(ErrorDetail {
            code: code.to_string(),
            message: message.to_string(),
            metadata: Default::default(),
        }),
        ..ReadGrantedImageResponse::default()
    }
}

#[cfg(unix)]
fn source_uds_global_error(
    error: base::net::transport::TransportError,
) -> base::exception::GlobalError {
    base::exception::GlobalError::new_sys_error(
        &format!("start image source UDS failed: {error}"),
        |_| {},
    )
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
    use super::{
        SOURCE_ACCESS_GRANTS, SourceAccessGrant, build_access_url, build_source_access_url,
        consume_source_grant, image_content_type, image_file_name, proof_header,
    };
    use crate::storage::guard_query::GbChannelImageView;
    use base::chrono::Local;

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
    fn builds_internal_source_url_without_bearer_proof() {
        let url = build_source_access_url("http://127.0.0.1:28567/", "image-1");
        assert_eq!(url, "http://127.0.0.1:28567/internal/images/image-1/source");
        assert!(!url.contains("token"));
    }

    #[test]
    fn source_grant_is_consumed_once_even_when_the_image_is_missing() {
        let proof = proof_header(b"one-time-proof");
        let now = Local::now().timestamp_millis();
        SOURCE_ACCESS_GRANTS.insert(
            proof.clone(),
            SourceAccessGrant {
                grant_id: "grant-1".to_string(),
                image_id: "image-1".to_string(),
                device_id: "device-1".to_string(),
                channel_id: "channel-1".to_string(),
                expected_node_id: "avai-1".to_string(),
                expected_instance_id: "instance-1".to_string(),
                task_id: "task-1".to_string(),
                purpose: "image.metadata.inspect".to_string(),
                expires_at_ms: now + 60_000,
            },
        );
        assert!(consume_source_grant("image-1", Some("grant-1"), &proof, now).is_some());
        assert!(consume_source_grant("image-1", Some("grant-1"), &proof, now).is_none());
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
