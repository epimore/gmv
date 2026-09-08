use std::{collections::HashSet, path::PathBuf, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode, header},
    response::Response,
    routing::post,
};
use base::{
    base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD},
    sha2::{Digest, Sha256},
    tokio::io::AsyncWriteExt,
};
use base_db::{
    dbx::{DatabasePoolConfig, sqlitex::SqliteConnectionConfig},
    sqlx::{Row, SqlitePool},
};
use gmv_protocol::{
    avai::v1::{
        FinalizeImageUploadRequest, FinalizeImageUploadResponse, ImageMetadata, ImageUploadTicket,
        OwnedImageRef, PrepareImageUploadRequest, PrepareImageUploadResponse,
    },
    common::v1::{
        AccessGrant, DataEndpoint, ErrorDetail, NodeIdentity, ResourceRef, TransportCapabilities,
        TransportMode,
    },
};
use url::Url;
use uuid::Uuid;

const DEFAULT_UPLOAD_TTL_MS: i64 = 5 * 60 * 1_000;

#[derive(Debug, Clone)]
pub struct UploadManagerConfig {
    pub database_path: PathBuf,
    pub object_root: PathBuf,
    pub public_url: String,
    pub max_image_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadError {
    pub code: &'static str,
    pub message: String,
}

impl UploadError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn internal(stage: &str, error: impl std::fmt::Display) -> Self {
        base::log::error!("Avai upload failed: action=image_upload, stage={stage}, error={error}");
        Self::new("internal_failure", "Avai upload storage operation failed")
    }
}

#[derive(Clone)]
pub struct UploadManager {
    identity: NodeIdentity,
    capabilities: Arc<HashSet<String>>,
    pool: SqlitePool,
    object_root: Arc<PathBuf>,
    public_url: Arc<String>,
    max_image_bytes: usize,
}

#[derive(Debug, Clone)]
struct UploadRecord {
    upload_id: String,
    idempotency_key: String,
    capability: String,
    content_type: String,
    max_bytes: usize,
    proof: Vec<u8>,
    expires_at_ms: i64,
    state: i64,
    size_bytes: u64,
    sha256: String,
    width: u32,
    height: u32,
}

impl UploadManager {
    pub async fn open(
        identity: NodeIdentity,
        capabilities: Vec<String>,
        config: UploadManagerConfig,
    ) -> Result<Self, UploadError> {
        if config.max_image_bytes == 0 {
            return Err(UploadError::new(
                "invalid_upload_config",
                "max_image_bytes must be greater than zero",
            ));
        }
        let public_url = validate_public_url(&config.public_url)?;
        std::fs::create_dir_all(&config.object_root)
            .map_err(|error| UploadError::internal("create_object_root", error))?;
        let object_root = config
            .object_root
            .canonicalize()
            .map_err(|error| UploadError::internal("resolve_object_root", error))?;
        if let Some(parent) = config.database_path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|error| UploadError::internal("create_database_directory", error))?;
        }
        let pool_config = DatabasePoolConfig {
            max_size: 4,
            min_idle: Some(1),
            ..Default::default()
        };
        let pool = base_db::dbx::sqlitex::build_sqlite_pool(
            SqliteConnectionConfig::new(&config.database_path),
            pool_config,
        )
        .map_err(|error| UploadError::internal("configure_database", error))?;
        base_db::sqlx::query(
            "CREATE TABLE IF NOT EXISTS avai_image_upload (\
             upload_id TEXT PRIMARY KEY NOT NULL,\
             idempotency_key TEXT NOT NULL UNIQUE,\
             capability TEXT NOT NULL,\
             content_type TEXT NOT NULL,\
             max_bytes INTEGER NOT NULL,\
             proof BLOB NOT NULL,\
             expires_at_ms INTEGER NOT NULL,\
             state INTEGER NOT NULL,\
             size_bytes INTEGER NOT NULL DEFAULT 0,\
             sha256 TEXT NOT NULL DEFAULT '',\
             width INTEGER NOT NULL DEFAULT 0,\
             height INTEGER NOT NULL DEFAULT 0,\
             created_at_ms INTEGER NOT NULL,\
             completed_at_ms INTEGER NULL\
             )",
        )
        .execute(&pool)
        .await
        .map_err(|error| UploadError::internal("initialize_schema", error))?;
        let manager = Self {
            identity,
            capabilities: Arc::new(capabilities.into_iter().collect()),
            pool,
            object_root: Arc::new(object_root),
            public_url: Arc::new(public_url),
            max_image_bytes: config.max_image_bytes,
        };
        manager.cleanup_expired(now_epoch_ms()).await?;
        Ok(manager)
    }

    pub async fn prepare(
        &self,
        request: PrepareImageUploadRequest,
        now_ms: i64,
    ) -> PrepareImageUploadResponse {
        match self.prepare_inner(request, now_ms).await {
            Ok(record) => PrepareImageUploadResponse {
                ticket: Some(self.ticket(&record)),
                error: None,
            },
            Err(error) => PrepareImageUploadResponse {
                ticket: None,
                error: Some(error_detail(error.code, &error.message)),
            },
        }
    }

    async fn prepare_inner(
        &self,
        request: PrepareImageUploadRequest,
        now_ms: i64,
    ) -> Result<UploadRecord, UploadError> {
        self.validate_expected(request.expected_avai.as_ref())?;
        let operation = request
            .operation
            .as_ref()
            .ok_or_else(|| UploadError::new("invalid_request", "upload operation is required"))?;
        if operation.idempotency_key.trim().is_empty()
            || !self.capabilities.contains(request.capability.trim())
        {
            return Err(UploadError::new(
                "invalid_request",
                "upload idempotency key and available capability are required",
            ));
        }
        let content_type = normalize_content_type(&request.content_type)?;
        let requested_max = usize::try_from(request.max_bytes)
            .unwrap_or(usize::MAX)
            .min(self.max_image_bytes);
        let max_bytes = if requested_max == 0 {
            self.max_image_bytes
        } else {
            requested_max
        };
        let requested_deadline = if request.deadline_epoch_ms > now_ms {
            request.deadline_epoch_ms
        } else {
            now_ms.saturating_add(DEFAULT_UPLOAD_TTL_MS)
        };
        let expires_at_ms = requested_deadline.min(now_ms.saturating_add(DEFAULT_UPLOAD_TTL_MS));
        if let Some(record) = self.get_by_idempotency(&operation.idempotency_key).await? {
            if record.capability == request.capability
                && record.content_type == content_type
                && record.max_bytes == max_bytes
                && record.expires_at_ms > now_ms
            {
                return Ok(record);
            }
            return Err(UploadError::new(
                "upload_conflict",
                "idempotency key is already used by another upload request",
            ));
        }
        let record = UploadRecord {
            upload_id: Uuid::new_v4().to_string(),
            idempotency_key: operation.idempotency_key.clone(),
            capability: request.capability,
            content_type,
            max_bytes,
            proof: Uuid::new_v4().to_string().into_bytes(),
            expires_at_ms,
            state: 0,
            size_bytes: 0,
            sha256: String::new(),
            width: 0,
            height: 0,
        };
        let inserted = base_db::sqlx::query(
            "INSERT INTO avai_image_upload(upload_id,idempotency_key,capability,content_type,max_bytes,proof,expires_at_ms,state,created_at_ms) VALUES(?,?,?,?,?,?,?,?,?) ON CONFLICT(idempotency_key) DO NOTHING",
        )
        .bind(&record.upload_id)
        .bind(&record.idempotency_key)
        .bind(&record.capability)
        .bind(&record.content_type)
        .bind(record.max_bytes as i64)
        .bind(&record.proof)
        .bind(record.expires_at_ms)
        .bind(record.state)
        .bind(now_ms)
        .execute(&self.pool)
        .await
        .map_err(|error| UploadError::internal("insert_upload", error))?;
        if inserted.rows_affected() == 0 {
            let existing = self
                .get_by_idempotency(&record.idempotency_key)
                .await?
                .ok_or_else(|| {
                    UploadError::internal(
                        "resolve_insert_conflict",
                        "ignored upload insert has no conflicting row",
                    )
                })?;
            if existing.capability == record.capability
                && existing.content_type == record.content_type
                && existing.max_bytes == record.max_bytes
                && existing.expires_at_ms > now_ms
            {
                return Ok(existing);
            }
            return Err(UploadError::new(
                "upload_conflict",
                "idempotency key is already used by another upload request",
            ));
        }
        Ok(record)
    }

    pub async fn finalize(
        &self,
        request: FinalizeImageUploadRequest,
        now_ms: i64,
    ) -> FinalizeImageUploadResponse {
        match self.finalize_inner(request, now_ms).await {
            Ok(source) => FinalizeImageUploadResponse {
                source: Some(source),
                error: None,
            },
            Err(error) => FinalizeImageUploadResponse {
                source: None,
                error: Some(error_detail(error.code, &error.message)),
            },
        }
    }

    async fn finalize_inner(
        &self,
        request: FinalizeImageUploadRequest,
        now_ms: i64,
    ) -> Result<OwnedImageRef, UploadError> {
        self.validate_expected(request.expected_avai.as_ref())?;
        let record = self
            .get(&request.upload_id)
            .await?
            .ok_or_else(|| UploadError::new("upload_not_found", "image upload does not exist"))?;
        if record.capability != request.capability {
            return Err(UploadError::new(
                "upload_conflict",
                "image upload capability does not match task capability",
            ));
        }
        if record.expires_at_ms <= now_ms {
            return Err(UploadError::new(
                "upload_expired",
                "image upload has expired",
            ));
        }
        if record.state != 1 {
            return Err(UploadError::new(
                "upload_incomplete",
                "image upload has not completed",
            ));
        }
        Ok(OwnedImageRef {
            owner: Some(self.identity.clone()),
            resource: Some(ResourceRef {
                resource_id: record.upload_id.clone(),
                resource_type: "avai_upload".to_string(),
            }),
            metadata: Some(ImageMetadata {
                content_type: record.content_type,
                size_bytes: record.size_bytes,
                sha256: record.sha256,
                width: record.width,
                height: record.height,
            }),
            access: Some(AccessGrant {
                grant_id: format!("local-{}", record.upload_id),
                expected_consumer: Some(self.identity.clone()),
                purpose: record.capability,
                expires_at_epoch_ms: record.expires_at_ms,
                endpoints: vec![DataEndpoint {
                    name: "avai-local-object".to_string(),
                    uri: format!("gmv-object://{}", record.upload_id),
                    capabilities: Some(TransportCapabilities {
                        reliable: true,
                        ordered: true,
                        preserves_message_boundary: true,
                        encrypted: false,
                        congestion_controlled: false,
                        local_only: true,
                        max_message_size: record.max_bytes as u64,
                        mode: TransportMode::Stream as i32,
                    }),
                    labels: Default::default(),
                }],
                proof: record.proof,
            }),
        })
    }

    pub async fn accept(
        &self,
        upload_id: &str,
        proof: &str,
        content_type: &str,
        bytes: &[u8],
        now_ms: i64,
    ) -> Result<(), UploadError> {
        let record = self
            .get(upload_id)
            .await?
            .ok_or_else(|| UploadError::new("upload_not_found", "image upload does not exist"))?;
        if record.expires_at_ms <= now_ms {
            return Err(UploadError::new(
                "upload_expired",
                "image upload has expired",
            ));
        }
        if !proof_matches(&record.proof, proof) {
            return Err(UploadError::new(
                "upload_denied",
                "image upload proof is invalid",
            ));
        }
        if normalize_content_type(content_type)? != record.content_type {
            return Err(UploadError::new(
                "upload_type_mismatch",
                "image upload content type does not match the ticket",
            ));
        }
        if bytes.is_empty() || bytes.len() > record.max_bytes {
            return Err(UploadError::new(
                "upload_size_invalid",
                "image upload exceeds the ticket size boundary",
            ));
        }
        let format = image::guess_format(bytes)
            .map_err(|_| UploadError::new("upload_decode_failed", "upload is not an image"))?;
        let actual_type = match format {
            image::ImageFormat::Jpeg => "image/jpeg",
            image::ImageFormat::Png => "image/png",
            image::ImageFormat::WebP => "image/webp",
            _ => {
                return Err(UploadError::new(
                    "upload_decode_failed",
                    "only JPEG, PNG and WebP uploads are supported",
                ));
            }
        };
        if actual_type != record.content_type {
            return Err(UploadError::new(
                "upload_type_mismatch",
                "image bytes do not match the declared content type",
            ));
        }
        let decoded = image::load_from_memory_with_format(bytes, format)
            .map_err(|_| UploadError::new("upload_decode_failed", "image decoding failed"))?;
        let sha256 = format!("{:x}", Sha256::digest(bytes));
        if record.state == 1 {
            return if record.sha256 == sha256 {
                Ok(())
            } else {
                Err(UploadError::new(
                    "upload_conflict",
                    "image upload is already completed with different content",
                ))
            };
        }
        let final_path = self.object_root.join(&record.upload_id);
        match base::tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&final_path)
            .await
        {
            Ok(mut file) => {
                if let Err(error) = file.write_all(bytes).await {
                    drop(file);
                    remove_upload_file_best_effort(&final_path, &record.upload_id).await;
                    return Err(UploadError::internal("write_upload", error));
                }
                if let Err(error) = file.sync_all().await {
                    drop(file);
                    remove_upload_file_best_effort(&final_path, &record.upload_id).await;
                    return Err(UploadError::internal("flush_upload", error));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = base::tokio::fs::read(&final_path)
                    .await
                    .map_err(|error| UploadError::internal("read_existing_upload", error))?;
                if format!("{:x}", Sha256::digest(&existing)) != sha256 {
                    return Err(UploadError::new(
                        "upload_conflict",
                        "image upload storage already contains different content",
                    ));
                }
            }
            Err(error) => return Err(UploadError::internal("create_upload", error)),
        }
        let updated = base_db::sqlx::query(
            "UPDATE avai_image_upload SET state=1,size_bytes=?,sha256=?,width=?,height=?,completed_at_ms=? WHERE upload_id=? AND state=0",
        )
        .bind(bytes.len() as i64)
        .bind(&sha256)
        .bind(i64::from(decoded.width()))
        .bind(i64::from(decoded.height()))
        .bind(now_ms)
        .bind(upload_id)
        .execute(&self.pool)
        .await
        .map_err(|error| UploadError::internal("complete_upload", error))?;
        if updated.rows_affected() == 0 {
            let Some(current) = self.get(upload_id).await? else {
                remove_upload_file_best_effort(&final_path, &record.upload_id).await;
                return Err(UploadError::new(
                    "upload_not_found",
                    "image upload disappeared",
                ));
            };
            if current.state == 1 && current.sha256 == sha256 {
                return Ok(());
            }
            return Err(UploadError::new(
                "upload_conflict",
                "image upload is already completed with different content",
            ));
        }
        Ok(())
    }

    pub async fn authorize_max_bytes(
        &self,
        upload_id: &str,
        proof: &str,
        now_ms: i64,
    ) -> Result<usize, UploadError> {
        let record = self
            .get(upload_id)
            .await?
            .ok_or_else(|| UploadError::new("upload_not_found", "image upload does not exist"))?;
        if record.expires_at_ms <= now_ms {
            return Err(UploadError::new(
                "upload_expired",
                "image upload has expired",
            ));
        }
        if !proof_matches(&record.proof, proof) {
            return Err(UploadError::new(
                "upload_denied",
                "image upload proof is invalid",
            ));
        }
        Ok(record.max_bytes)
    }

    pub async fn cleanup_expired(&self, now_ms: i64) -> Result<usize, UploadError> {
        let rows = base_db::sqlx::query(
            "SELECT upload_id FROM avai_image_upload WHERE expires_at_ms<=? ORDER BY expires_at_ms,upload_id",
        )
        .bind(now_ms)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| UploadError::internal("list_expired_uploads", error))?;
        let mut cleaned = 0usize;
        for row in rows {
            let upload_id: String = row
                .try_get("upload_id")
                .map_err(|error| UploadError::internal("decode_expired_upload", error))?;
            let path = self.object_root.join(&upload_id);
            match base::tokio::fs::remove_file(&path).await {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    base::log::warn!(
                        "Avai expired upload file cleanup failed: action=image_upload, stage=cleanup_file, upload_id={}, reason={error}",
                        upload_id
                    );
                    continue;
                }
            }
            let deleted = base_db::sqlx::query(
                "DELETE FROM avai_image_upload WHERE upload_id=? AND expires_at_ms<=?",
            )
            .bind(&upload_id)
            .bind(now_ms)
            .execute(&self.pool)
            .await
            .map_err(|error| UploadError::internal("delete_expired_upload", error))?;
            cleaned = cleaned.saturating_add(deleted.rows_affected() as usize);
        }
        Ok(cleaned)
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    fn ticket(&self, record: &UploadRecord) -> ImageUploadTicket {
        ImageUploadTicket {
            upload_id: record.upload_id.clone(),
            endpoint: Some(DataEndpoint {
                name: "avai-image-upload".to_string(),
                uri: format!("{}/internal/uploads/{}", self.public_url, record.upload_id),
                capabilities: Some(TransportCapabilities {
                    reliable: true,
                    ordered: true,
                    preserves_message_boundary: false,
                    encrypted: self.public_url.starts_with("https://"),
                    congestion_controlled: true,
                    local_only: false,
                    max_message_size: record.max_bytes as u64,
                    mode: TransportMode::Stream as i32,
                }),
                labels: Default::default(),
            }),
            proof: record.proof.clone(),
            expires_at_epoch_ms: record.expires_at_ms,
            max_bytes: record.max_bytes as u64,
            content_type: record.content_type.clone(),
            owner: Some(self.identity.clone()),
        }
    }

    fn validate_expected(&self, expected: Option<&NodeIdentity>) -> Result<(), UploadError> {
        if expected.is_some_and(|expected| {
            expected.node_id == self.identity.node_id
                && expected.instance_id == self.identity.instance_id
        }) {
            Ok(())
        } else {
            Err(UploadError::new(
                "stale_instance",
                "upload request targets another Avai instance",
            ))
        }
    }

    async fn get(&self, upload_id: &str) -> Result<Option<UploadRecord>, UploadError> {
        self.query("SELECT upload_id,idempotency_key,capability,content_type,max_bytes,proof,expires_at_ms,state,size_bytes,sha256,width,height FROM avai_image_upload WHERE upload_id=?", upload_id).await
    }

    async fn get_by_idempotency(&self, key: &str) -> Result<Option<UploadRecord>, UploadError> {
        self.query("SELECT upload_id,idempotency_key,capability,content_type,max_bytes,proof,expires_at_ms,state,size_bytes,sha256,width,height FROM avai_image_upload WHERE idempotency_key=?", key).await
    }

    async fn query(
        &self,
        sql: &'static str,
        value: &str,
    ) -> Result<Option<UploadRecord>, UploadError> {
        let row = base_db::sqlx::query(sql)
            .bind(value)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| UploadError::internal("query_upload", error))?;
        row.map(decode_record).transpose()
    }
}

pub fn routes(manager: UploadManager) -> Router {
    Router::new()
        .route("/internal/uploads/{upload_id}", post(upload_image))
        .with_state(manager)
}

async fn upload_image(
    State(manager): State<UploadManager>,
    AxumPath(upload_id): AxumPath<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let now_ms = now_epoch_ms();
    let proof = match headers
        .get("x-gmv-upload-proof")
        .and_then(|value| value.to_str().ok())
    {
        Some(proof) => proof,
        None => {
            return upload_error_response(UploadError::new(
                "upload_denied",
                "image upload proof is required",
            ));
        }
    };
    let max_bytes = match manager.authorize_max_bytes(&upload_id, proof, now_ms).await {
        Ok(max_bytes) => max_bytes,
        Err(error) => return upload_error_response(error),
    };
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    let bytes = match axum::body::to_bytes(body, max_bytes).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return upload_error_response(UploadError::new(
                "upload_size_invalid",
                "image upload exceeds the ticket size boundary",
            ));
        }
    };
    match manager
        .accept(&upload_id, proof, content_type, &bytes, now_ms)
        .await
    {
        Ok(()) => Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Body::empty())
            .unwrap(),
        Err(error) => upload_error_response(error),
    }
}

fn upload_error_response(error: UploadError) -> Response {
    let status = match error.code {
        "upload_denied" => StatusCode::UNAUTHORIZED,
        "upload_not_found" => StatusCode::NOT_FOUND,
        "upload_expired" | "upload_conflict" => StatusCode::CONFLICT,
        "internal_failure" => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::BAD_REQUEST,
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(
            base::serde_json::to_vec(&base::serde_json::json!({
                "code": error.code,
                "message": error.message,
            }))
            .unwrap_or_default(),
        ))
        .unwrap()
}

fn normalize_content_type(content_type: &str) -> Result<String, UploadError> {
    let content_type = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match content_type.as_str() {
        "image/jpeg" | "image/png" | "image/webp" => Ok(content_type),
        _ => Err(UploadError::new(
            "upload_type_unsupported",
            "only image/jpeg, image/png and image/webp uploads are supported",
        )),
    }
}

fn validate_public_url(value: &str) -> Result<String, UploadError> {
    let parsed = Url::parse(value.trim()).map_err(|_| {
        UploadError::new(
            "invalid_upload_config",
            "upload public_url must be an absolute HTTP(S) URL",
        )
    })?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(UploadError::new(
            "invalid_upload_config",
            "upload public_url must be an HTTP(S) origin without credentials, query or fragment",
        ));
    }
    Ok(value.trim().trim_end_matches('/').to_string())
}

fn proof_matches(expected: &[u8], actual: &str) -> bool {
    URL_SAFE_NO_PAD
        .decode(actual)
        .is_ok_and(|actual| actual == expected)
}

async fn remove_upload_file_best_effort(path: &std::path::Path, upload_id: &str) {
    match base::tokio::fs::remove_file(path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            base::log::warn!(
                "Avai partial upload cleanup failed: action=image_upload, stage=cleanup_partial, upload_id={}, reason={error}",
                upload_id
            );
        }
    }
}

fn decode_record(row: base_db::sqlx::sqlite::SqliteRow) -> Result<UploadRecord, UploadError> {
    Ok(UploadRecord {
        upload_id: row.try_get("upload_id").map_err(decode_error)?,
        idempotency_key: row.try_get("idempotency_key").map_err(decode_error)?,
        capability: row.try_get("capability").map_err(decode_error)?,
        content_type: row.try_get("content_type").map_err(decode_error)?,
        max_bytes: usize::try_from(row.try_get::<i64, _>("max_bytes").map_err(decode_error)?)
            .map_err(|_| UploadError::new("internal_failure", "invalid upload size record"))?,
        proof: row.try_get("proof").map_err(decode_error)?,
        expires_at_ms: row.try_get("expires_at_ms").map_err(decode_error)?,
        state: row.try_get("state").map_err(decode_error)?,
        size_bytes: u64::try_from(row.try_get::<i64, _>("size_bytes").map_err(decode_error)?)
            .map_err(|_| UploadError::new("internal_failure", "invalid uploaded size record"))?,
        sha256: row.try_get("sha256").map_err(decode_error)?,
        width: u32::try_from(row.try_get::<i64, _>("width").map_err(decode_error)?)
            .map_err(|_| UploadError::new("internal_failure", "invalid upload width record"))?,
        height: u32::try_from(row.try_get::<i64, _>("height").map_err(decode_error)?)
            .map_err(|_| UploadError::new("internal_failure", "invalid upload height record"))?,
    })
}

fn decode_error(error: base_db::sqlx::Error) -> UploadError {
    UploadError::internal("decode_upload", error)
}

fn error_detail(code: &str, message: &str) -> ErrorDetail {
    ErrorDetail {
        code: code.to_string(),
        message: message.to_string(),
        metadata: Default::default(),
    }
}

fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gmv_protocol::{
        avai::v1::{FinalizeImageUploadRequest, PrepareImageUploadRequest},
        common::v1::{NodeKind, OperationRef},
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    static NEXT_ID: AtomicUsize = AtomicUsize::new(1);

    fn identity() -> NodeIdentity {
        NodeIdentity {
            node_id: "avai-upload-test".to_string(),
            instance_id: "instance-upload-test".to_string(),
            kind: NodeKind::Avai as i32,
        }
    }

    fn png_bytes() -> Vec<u8> {
        use base::base64::Engine;
        base::base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=")
            .unwrap()
    }

    #[tokio::test]
    async fn upload_is_idempotent_persistent_and_finalizes_to_local_source() {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("avai-upload-test-{}-{id}", std::process::id()));
        let config = UploadManagerConfig {
            database_path: root.join("avai.db"),
            object_root: root.join("objects"),
            public_url: "http://127.0.0.1:19081".to_string(),
            max_image_bytes: 1024,
        };
        let manager = UploadManager::open(
            identity(),
            vec!["image.metadata.inspect".to_string()],
            config.clone(),
        )
        .await
        .unwrap();
        let request = PrepareImageUploadRequest {
            operation: Some(OperationRef {
                operation_id: "prepare-1".to_string(),
                idempotency_key: "prepare-1".to_string(),
            }),
            expected_avai: Some(identity()),
            capability: "image.metadata.inspect".to_string(),
            content_type: "image/png".to_string(),
            max_bytes: 1024,
            deadline_epoch_ms: now_epoch_ms() + 60_000,
        };
        let (first, repeated) = base::tokio::join!(
            manager.prepare(request.clone(), now_epoch_ms()),
            manager.prepare(request, now_epoch_ms())
        );
        assert_eq!(
            first.ticket.as_ref().unwrap().upload_id,
            repeated.ticket.as_ref().unwrap().upload_id
        );
        let ticket = first.ticket.unwrap();
        let proof = URL_SAFE_NO_PAD.encode(&ticket.proof);
        let bytes = png_bytes();
        assert_eq!(
            manager
                .accept(
                    &ticket.upload_id,
                    "wrong",
                    "image/png",
                    &bytes,
                    now_epoch_ms()
                )
                .await
                .unwrap_err()
                .code,
            "upload_denied"
        );
        let response = routes(manager.clone())
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/internal/uploads/{}", ticket.upload_id))
                    .header("x-gmv-upload-proof", &proof)
                    .header(header::CONTENT_TYPE, "image/png")
                    .body(Body::from(bytes.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let finalize = FinalizeImageUploadRequest {
            operation: Some(OperationRef {
                operation_id: "finalize-1".to_string(),
                idempotency_key: "finalize-1".to_string(),
            }),
            expected_avai: Some(identity()),
            upload_id: ticket.upload_id.clone(),
            capability: "image.metadata.inspect".to_string(),
        };
        let source = manager.finalize(finalize.clone(), now_epoch_ms()).await;
        assert!(source.error.is_none());
        assert_eq!(
            source.source.unwrap().metadata.unwrap().sha256,
            format!("{:x}", Sha256::digest(&bytes))
        );
        manager.close().await;

        let reopened = UploadManager::open(
            identity(),
            vec!["image.metadata.inspect".to_string()],
            config,
        )
        .await
        .unwrap();
        assert!(
            reopened
                .finalize(finalize, now_epoch_ms())
                .await
                .error
                .is_none()
        );
        assert_eq!(
            reopened
                .cleanup_expired(ticket.expires_at_epoch_ms + 1)
                .await
                .unwrap(),
            1
        );
        assert!(!root.join("objects").join(&ticket.upload_id).exists());
        reopened.close().await;
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn invalid_public_url_is_rejected_before_startup() {
        let root = std::env::temp_dir().join(format!(
            "avai-invalid-upload-config-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let error = UploadManager::open(
            identity(),
            vec!["image.metadata.inspect".to_string()],
            UploadManagerConfig {
                database_path: root.join("avai.db"),
                object_root: root.join("objects"),
                public_url: "http://user:password@127.0.0.1:19081?token=secret".to_string(),
                max_image_bytes: 1024,
            },
        )
        .await
        .err()
        .unwrap();
        assert_eq!(error.code, "invalid_upload_config");
        assert!(!root.exists());
    }
}
