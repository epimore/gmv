use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, FromRequestParts, MatchedPath, OriginalUri, Path, Query, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE, ORIGIN, REFERRER_POLICY,
    SET_COOKIE, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base::err;
use base::exception::GlobalError;
use base::log::debug;
use gmv_protocol::session::v1::{
    ActiveStreamManagementState, CloudRecordingFileState, CloudRecordingStatus,
    CloudRecordingSummary as RpcCloudRecordingSummary, CreateCloudRecordingRequest,
    GbChannel as RpcGbChannel, GbChannelImage as RpcGbChannelImage, GbDevice as RpcGbDevice,
    GbRecordQueryBatch as RpcGbRecordQueryBatch, GbRecordSegment as RpcGbRecordSegment,
    GbResource as RpcGbResource, GetGbChannelRecordsResponse as RpcGbChannelRecordsResponse,
    ListActiveStreamDialogsRequest, ListCloudRecordingsRequest, ListStreamHistoryRequest,
    PlaybackPresenceHeartbeat, ResetGbResourceConfirmationRequest,
    SaveGbResourceConfirmationRequest,
};
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::set_header::SetResponseHeaderLayer;
use uuid::Uuid;

use crate::api::v2::control::{
    BusinessControl, DeviceStreamOptions, GbDevicePage, GbSessionConfigSummary,
};
use crate::api::v2::model::{
    ActiveStreamDialogItem, ActiveStreamDialogPage, ActiveStreamManagementInfo,
    ActiveStreamMonitorItem, ActiveStreamViewerFormat, AiTaskSummary, AiTaskSummaryState,
    DeviceSummary, MediaOperationError, MediaOperationState, MediaOperationSummary,
    MediaTransportCapability, MonitoredStreamStopResponse, RuntimeStatus, StreamHistoryMonitorItem,
    StreamHistoryMonitorPage, StreamOutputSummary, StreamSummary, StreamSummaryState,
};
use crate::api::v2::paths;
use crate::api::v2::{ApiV2, CursorQuery, EventQuery};
use crate::auth::session::{SESSION_COOKIE, cookie_value};
use crate::auth::{
    AuthState, Role, UiSession, UserAccess, UserProfile, hash_password as hash_ui_password,
};
use crate::core::{
    ConnectionState, GmvGuardErrorCode, GuardError, GuardResult, HealthState, LeaseState,
    RouteState, SchedulingState,
};
use crate::integration::hmac::{HmacNonceCache, SignedRequest, body_sha256, verify_request};
use crate::integration::model::{
    CredentialPurpose, CredentialStatus, Integration, IntegrationAudit, IntegrationCredential,
    IntegrationCredentialSummary, IntegrationHttpConfig, IntegrationMapping, IntegrationMqttConfig,
    IntegrationTransport,
};
use crate::integration::secret::IntegrationSecretCipher;
use crate::operation::OperationRequest;
use crate::operation::{OperationRecord, OperationStatus};
use crate::outbox::OutboxRepository;
use crate::runtime::event_forwarder::EventForwarder;
use crate::store::model::{
    EventRecord, INTEGRATION_PLAYBACK_MAX_LIFETIME_MS, INTEGRATION_PLAYBACK_MAX_RENEWALS,
    INTEGRATION_PLAYBACK_TOKEN_TTL_MS, LeaseRecord, NodeRecord, OutboxDestinationKind,
    OutboxRecord, OutboxState, PLAYBACK_TOKEN_TTL_MS, PlaybackTicketRecord,
};
use crate::store::persistent::{IntegrationRepository, UserRepository};

const CSRF_HEADER: &str = "x-csrf-token";
const DEFAULT_GB_DEVICE_PAGE_SIZE: u32 = 20;
const MAX_GB_DEVICE_PAGE_SIZE: u32 = 500;
const MEDIA_CHECKPOINT_MS: u64 = 8_000;
const FIRST_PREVIEW_HARD_TIMEOUT_MS: u64 = 15_000;
const HMAC_ACCESS_KEY_HEADER: &str = "x-gmv-access-key";
const HMAC_TIMESTAMP_HEADER: &str = "x-gmv-timestamp";
const HMAC_NONCE_HEADER: &str = "x-gmv-nonce";
const HMAC_CONTENT_SHA256_HEADER: &str = "x-gmv-content-sha256";
const HMAC_SIGNATURE_HEADER: &str = "x-gmv-signature";
const MAX_OPEN_API_BODY_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct HttpState {
    pub api: ApiV2,
    pub auth: AuthState,
    pub outbox: OutboxRepository,
    pub users: Option<UserRepository>,
    pub integrations: Option<IntegrationRepository>,
    pub integration_secrets: Option<IntegrationSecretCipher>,
    pub integration_nonces: HmacNonceCache,
    pub mqtt_runtime_protocol_version: String,
    pub mqtt_runtime_enabled: bool,
    pub event_forwarder: Option<EventForwarder>,
    pub media_https_http2_verified: bool,
}

pub fn router(state: HttpState) -> Router {
    let root_state = state.clone();
    let open_api = open_business_router(state.clone());
    let session_renew_auth = state.auth.clone();
    let origins = state
        .auth
        .allowed_origins()
        .iter()
        .map(|origin| {
            HeaderValue::from_str(origin)
                .expect("validated UI allowed origin must be a valid header value")
        })
        .collect::<Vec<_>>();
    let csrf_header = HeaderName::from_static(CSRF_HEADER);
    let api = Router::new()
        .route("/auth/login", post(login))
        .route("/auth/session", get(current_session))
        .route("/auth/logout", post(logout))
        .route("/me", get(current_profile).post(update_profile))
        .route("/dashboard", get(dashboard))
        .route("/media/transport", get(media_transport))
        .route("/media/operations", get(media_operations))
        .route("/media/operations/{operation_id}", get(media_operation))
        .route(
            "/media/operations/{operation_id}/continue",
            post(continue_media_operation),
        )
        .route(
            "/media/operations/{operation_id}/cancel",
            post(cancel_media_operation),
        )
        .route("/nodes", get(nodes))
        .route("/leases", get(leases))
        .route("/events", get(events))
        .route("/users", get(list_users).post(create_user))
        .route("/users/{username}", post(update_user))
        .route(
            "/integrations",
            get(list_integrations).post(create_integration),
        )
        .route(
            "/integrations/{integration_id}",
            get(get_integration).post(update_integration),
        )
        .route(
            "/integrations/{integration_id}/credentials",
            get(list_integration_credentials).post(create_integration_credential),
        )
        .route(
            "/integrations/{integration_id}/credentials/{credential_id}/revoke",
            post(revoke_integration_credential),
        )
        .route(
            "/integrations/{integration_id}/http",
            get(get_integration_http).post(update_integration_http),
        )
        .route(
            "/integrations/{integration_id}/mqtt",
            get(get_integration_mqtt).post(update_integration_mqtt),
        )
        .route("/integrations/mqtt/runtime", get(integration_mqtt_runtime))
        .route(
            "/integrations/{integration_id}/mappings",
            get(list_integration_mappings).post(upsert_integration_mapping),
        )
        .route("/integrations/audits", get(list_integration_audits))
        .route("/integrations/outbox", get(outbox_records))
        .route("/integrations/outbox/{outbox_id}/retry", post(retry_outbox))
        .route(
            "/gb28181/session-nodes/{node_id}/config",
            get(gb_session_node_config),
        )
        .route("/gb28181/devices", get(gb_devices).post(create_gb_device))
        .route(
            "/gb28181/devices/{device_id}",
            get(gb_device).post(update_gb_device),
        )
        .route(
            "/gb28181/devices/{device_id}/delete",
            post(delete_gb_device),
        )
        .route("/gb28181/devices/{device_id}/channels", get(gb_channels))
        .route("/gb28181/devices/{device_id}/resources", get(gb_resources))
        .route(
            "/gb28181/devices/{device_id}/resources/{resource_id}/confirmation",
            post(save_gb_resource_confirmation),
        )
        .route(
            "/gb28181/devices/{device_id}/resources/{resource_id}/confirmation/reset",
            post(reset_gb_resource_confirmation),
        )
        .route(
            "/gb28181/devices/{device_id}/channels/{channel_id}",
            get(gb_channel).post(update_gb_channel),
        )
        .route(
            "/gb28181/devices/{device_id}/channels/{channel_id}/preview",
            post(gb_preview),
        )
        .route(
            "/gb28181/devices/{device_id}/channels/{channel_id}/playback",
            post(gb_playback),
        )
        .route(
            "/gb28181/devices/{device_id}/channels/{channel_id}/ptz",
            post(gb_ptz),
        )
        .route(
            "/gb28181/devices/{device_id}/channels/{channel_id}/images",
            get(gb_channel_images).post(gb_snapshot_image),
        )
        .route(
            "/gb28181/devices/{device_id}/channels/{channel_id}/images/{image_id}/access",
            post(issue_gb_channel_image_access),
        )
        .route(
            "/gb28181/devices/{device_id}/channels/{channel_id}/images/{image_id}/cover",
            post(set_gb_channel_cover),
        )
        .route(
            "/gb28181/devices/{device_id}/channels/{channel_id}/records",
            get(gb_channel_records),
        )
        .route(
            "/gb28181/devices/{device_id}/channels/{channel_id}/records/query",
            post(query_gb_channel_records),
        )
        .route(
            "/gb28181/devices/{device_id}/channels/{channel_id}/cloud-recordings",
            get(list_cloud_recordings).post(create_cloud_recording),
        )
        .route(
            "/gb28181/cloud-recordings/{task_id}",
            get(get_cloud_recording),
        )
        .route(
            "/gb28181/cloud-recordings/{task_id}/stop",
            post(stop_cloud_recording),
        )
        .route(
            "/gb28181/cloud-recordings/{task_id}/delete",
            post(delete_cloud_recording),
        )
        .route(
            "/gb28181/cloud-recordings/{task_id}/access",
            post(issue_cloud_recording_access),
        )
        .route(
            "/gb28181/devices/{device_id}/broadcast/start",
            post(gb_broadcast),
        )
        .route("/gb28181/broadcasts/{stream_id}/stop", post(stop_stream))
        .route("/devices", get(devices))
        .route("/devices/{device_id}/preview", post(preview))
        .route("/devices/{device_id}/playback", post(playback))
        .route("/devices/{device_id}/download", post(download))
        .route("/devices/{device_id}/talk", post(talk))
        .route("/devices/{device_id}/ptz", post(ptz))
        .route("/streams", get(streams))
        .route("/gb28181/streams", get(gb_active_streams))
        .route(
            "/gb28181/streams/{stream_id}/management",
            get(gb_active_stream_management),
        )
        .route("/gb28181/stream-history", get(gb_stream_history))
        .route(
            "/gb28181/streams/{stream_id}/stop",
            post(stop_gb_monitored_stream),
        )
        .route("/streams/{stream_id}/stop", post(stop_stream))
        .route("/streams/{stream_id}/release", post(release_stream))
        .route("/streams/{stream_id}/speed", post(set_playback_speed))
        .route("/playbacks/{playback_id}/seek", post(seek_playback))
        .route(
            "/playbacks/{playback_id}/speed",
            post(set_versioned_playback_speed),
        )
        .route("/playbacks/{playback_id}/state", post(set_playback_state))
        .route(
            "/playbacks/presence/heartbeat",
            post(heartbeat_playback_presence),
        )
        .route(
            "/streams/{stream_id}/outputs",
            get(list_stream_outputs).post(create_stream_output),
        )
        .route(
            "/streams/{stream_id}/outputs/{output_id}/close",
            post(close_stream_output),
        )
        .route("/ai/tasks", get(ai_tasks).post(start_ai_task))
        .route("/ai/tasks/{task_id}/cancel", post(cancel_ai_task))
        .route("/runtime/status", get(runtime_status))
        .with_state(state)
        .layer(middleware::from_fn_with_state(
            session_renew_auth,
            renew_ui_session,
        ))
        .layer(middleware::from_fn(debug_http_request))
        .layer(
            CorsLayer::new()
                .allow_origin(AllowOrigin::list(origins))
                .allow_credentials(true)
                .allow_methods([Method::GET, Method::POST])
                .allow_headers([CONTENT_TYPE, csrf_header]),
        )
        .layer(SetResponseHeaderLayer::if_not_present(
            CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ));

    Router::new()
        .route("/health/live", get(health_live))
        .route("/health/ready", get(health_ready))
        .route("/metrics", get(metrics))
        .route("/api-docs", get(api_docs_index))
        .route("/api-docs/http", get(api_docs_http))
        .route("/api-docs/mqtt", get(api_docs_mqtt))
        .route("/api-docs/assets/docs.css", get(api_docs_styles))
        .route("/api-docs/assets/docs.js", get(api_docs_script))
        .route("/api-docs/openapi.json", get(openapi_document))
        .route("/api-docs/asyncapi.json", get(asyncapi_document))
        .route("/api-docs/manifest.json", get(api_manifest))
        .nest("/openapi/v1", open_api)
        .nest(paths::API_PREFIX, api)
        .with_state(root_state)
        .layer(SetResponseHeaderLayer::overriding(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; script-src 'self'; worker-src 'self' blob:; style-src 'self' 'unsafe-inline'; img-src 'self' data:; media-src 'self' blob:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
            ),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
}

const OPEN_BUSINESS_OPERATIONS: &[(&str, &[&str])] = &[
    ("/dashboard", &["get"]),
    ("/media/transport", &["get"]),
    ("/media/operations", &["get"]),
    ("/media/operations/{operation_id}", &["get"]),
    ("/media/operations/{operation_id}/continue", &["post"]),
    ("/media/operations/{operation_id}/cancel", &["post"]),
    ("/nodes", &["get"]),
    ("/leases", &["get"]),
    ("/events", &["get"]),
    ("/gb28181/session-nodes/{node_id}/config", &["get"]),
    ("/gb28181/devices", &["get", "post"]),
    ("/gb28181/devices/{device_id}", &["get", "post"]),
    ("/gb28181/devices/{device_id}/delete", &["post"]),
    ("/gb28181/devices/{device_id}/channels", &["get"]),
    ("/gb28181/devices/{device_id}/resources", &["get"]),
    (
        "/gb28181/devices/{device_id}/resources/{resource_id}/confirmation",
        &["post"],
    ),
    (
        "/gb28181/devices/{device_id}/resources/{resource_id}/confirmation/reset",
        &["post"],
    ),
    (
        "/gb28181/devices/{device_id}/channels/{channel_id}",
        &["get", "post"],
    ),
    (
        "/gb28181/devices/{device_id}/channels/{channel_id}/preview",
        &["post"],
    ),
    (
        "/gb28181/devices/{device_id}/channels/{channel_id}/playback",
        &["post"],
    ),
    (
        "/gb28181/devices/{device_id}/channels/{channel_id}/ptz",
        &["post"],
    ),
    (
        "/gb28181/devices/{device_id}/channels/{channel_id}/images",
        &["get", "post"],
    ),
    (
        "/gb28181/devices/{device_id}/channels/{channel_id}/images/{image_id}/access",
        &["post"],
    ),
    (
        "/gb28181/devices/{device_id}/channels/{channel_id}/images/{image_id}/cover",
        &["post"],
    ),
    (
        "/gb28181/devices/{device_id}/channels/{channel_id}/records",
        &["get"],
    ),
    (
        "/gb28181/devices/{device_id}/channels/{channel_id}/records/query",
        &["post"],
    ),
    (
        "/gb28181/devices/{device_id}/channels/{channel_id}/cloud-recordings",
        &["get", "post"],
    ),
    ("/gb28181/cloud-recordings/{task_id}", &["get"]),
    ("/gb28181/cloud-recordings/{task_id}/stop", &["post"]),
    ("/gb28181/cloud-recordings/{task_id}/delete", &["post"]),
    ("/gb28181/cloud-recordings/{task_id}/access", &["post"]),
    ("/gb28181/devices/{device_id}/broadcast/start", &["post"]),
    ("/gb28181/broadcasts/{stream_id}/stop", &["post"]),
    ("/devices", &["get"]),
    ("/devices/{device_id}/preview", &["post"]),
    ("/devices/{device_id}/playback", &["post"]),
    ("/devices/{device_id}/download", &["post"]),
    ("/devices/{device_id}/talk", &["post"]),
    ("/devices/{device_id}/ptz", &["post"]),
    ("/streams", &["get"]),
    ("/gb28181/streams", &["get"]),
    ("/gb28181/streams/{stream_id}/management", &["get"]),
    ("/gb28181/stream-history", &["get"]),
    ("/gb28181/streams/{stream_id}/stop", &["post"]),
    ("/streams/{stream_id}/stop", &["post"]),
    ("/streams/{stream_id}/release", &["post"]),
    ("/streams/{stream_id}/speed", &["post"]),
    ("/playbacks/{playback_id}/seek", &["post"]),
    ("/playbacks/{playback_id}/speed", &["post"]),
    ("/playbacks/{playback_id}/state", &["post"]),
    ("/playbacks/presence/heartbeat", &["post"]),
    ("/playback-tickets/{token}/renew", &["post"]),
    ("/streams/{stream_id}/outputs", &["get", "post"]),
    ("/streams/{stream_id}/outputs/{output_id}/close", &["post"]),
    ("/ai/tasks", &["get", "post"]),
    ("/ai/tasks/{task_id}/cancel", &["post"]),
    ("/runtime/status", &["get"]),
];

async fn api_docs_index(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Html<&'static str>, HttpError> {
    require_role(&state.auth, &headers, Role::Admin)?;
    Ok(Html(
        r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>GMV 三方集成文档</title><link rel="stylesheet" href="/api-docs/assets/docs.css"></head><body><main class="shell"><header class="topbar"><div><p class="eyebrow">GMV GUARD · INTEGRATION CONTRACT</p><h1>三方集成在线文档</h1><p class="subtitle">HTTP 与 MQTT 契约均随 Guard Server 发布；在线页面用于阅读，JSON 地址继续作为机器可读的契约来源。</p></div></header><section class="overview-grid"><article class="overview-card"><span class="chip">OpenAPI 3.1</span><h2>HTTP 接入</h2><p>查看第三方调用 Guard 的业务接口、HMAC 鉴权、中文参数字段与 JSON 返回说明。</p><div class="link-row"><a class="button" href="/api-docs/http">打开在线文档</a><a class="button" href="/api-docs/openapi.json">查看 OpenAPI JSON</a></div></article><article class="overview-card"><span class="chip ok">AsyncAPI 3.0</span><h2>MQTT 接入</h2><p>查看 Guard 订阅/发布方向、Topic、QoS、消息字段与中文业务说明。</p><div class="link-row"><a class="button" href="/api-docs/mqtt">打开在线文档</a><a class="button" href="/api-docs/asyncapi.json">查看 AsyncAPI JSON</a></div></article></section><p class="subtitle"><a href="/api-docs/manifest.json">查看版本、鉴权和能力清单 JSON</a></p></main></body></html>"#,
    ))
}

async fn api_docs_http(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Html<String>, HttpError> {
    require_role(&state.auth, &headers, Role::Admin)?;
    Ok(Html(api_docs_contract_page(
        "http",
        "HTTP 三方接入文档",
        "OpenAPI 3.1 · 第三方调用 Guard Server · GMV-HMAC-SHA256-V1",
        "/api-docs/openapi.json",
    )))
}

async fn api_docs_mqtt(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Html<String>, HttpError> {
    require_role(&state.auth, &headers, Role::Admin)?;
    Ok(Html(api_docs_contract_page(
        "mqtt",
        "MQTT 三方接入文档",
        "AsyncAPI 3.0 · Guard 订阅命令并发布结果与事件 · QoS 1",
        "/api-docs/asyncapi.json",
    )))
}

fn api_docs_contract_page(mode: &str, title: &str, subtitle: &str, spec_url: &str) -> String {
    format!(
        r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{title}</title><link rel="stylesheet" href="/api-docs/assets/docs.css"><script src="/api-docs/assets/docs.js" defer></script></head><body><main class="shell" data-api-docs data-mode="{mode}" data-spec="{spec_url}"><header class="topbar"><div><p class="eyebrow">GMV GUARD · INTEGRATION CONTRACT</p><h1>{title}</h1><p class="subtitle">{subtitle}</p></div><nav class="nav" aria-label="文档导航"><a href="/api-docs">文档首页</a><a class="{http_active}" href="/api-docs/http">HTTP</a><a class="{mqtt_active}" href="/api-docs/mqtt">MQTT</a><a href="{spec_url}">原始 JSON</a></nav></header><section class="toolbar"><input id="contract-search" class="search" type="search" placeholder="搜索路径、Topic、方法或中文说明" disabled><div class="meta"><span id="contract-count">加载中</span><span class="chip ok">只读契约</span></div></section><div id="contract-content"><div class="empty">正在加载契约 JSON…</div></div></main></body></html>"#,
        http_active = if mode == "http" { "active" } else { "" },
        mqtt_active = if mode == "mqtt" { "active" } else { "" },
    )
}

async fn api_docs_styles() -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("api_docs.css"),
    )
}

async fn api_docs_script() -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("api_docs.js"),
    )
}

async fn openapi_document(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<base::serde_json::Value>, HttpError> {
    require_role(&state.auth, &headers, Role::Admin)?;
    let mut paths = base::serde_json::Map::new();
    for (path, methods) in OPEN_BUSINESS_OPERATIONS {
        let mut operations = base::serde_json::Map::new();
        for method in *methods {
            let summary = openapi_operation_summary(method, path);
            let required_scope = open_business_scope(
                if *method == "get" {
                    &Method::GET
                } else {
                    &Method::POST
                },
                path,
            );
            let mut operation = base::serde_json::json!({
                "tags": [openapi_operation_tag(path)],
                "summary": summary,
                "description": format!("{summary}。请求须完成 GMV-HMAC-SHA256-V1 签名校验，并具备所列权限。"),
                "x-gmv-required-scope": required_scope,
                "parameters": openapi_operation_parameters(method, path),
                "security": [{"GmvAccessKey": [], "GmvTimestamp": [], "GmvNonce": [], "GmvContentSha256": [], "GmvSignature": []}],
                "responses": openapi_responses(summary)
            });
            if let Some(request_body) = openapi_request_body(method, path) {
                operation
                    .as_object_mut()
                    .expect("OpenAPI operation must be an object")
                    .insert("requestBody".to_string(), request_body);
            }
            operations.insert((*method).to_string(), operation);
        }
        paths.insert(format!("/openapi/v1{path}"), operations.into());
    }
    Ok(Json(base::serde_json::json!({
        "openapi": "3.1.0",
        "info": {
            "title": "GMV Guard 三方 HTTP 开放接口",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "面向第三方业务系统的 Guard Server HTTP 接口契约。所有请求与响应正文均使用 JSON，写请求仅使用 POST。"
        },
        "tags": [
            {"name": "总览与运行状态", "description": "查询 Guard 汇总信息、运行状态和媒体传输能力。"},
            {"name": "节点与租约", "description": "查询节点、租约及媒体操作状态。"},
            {"name": "事件", "description": "分页查询 Guard 事件。"},
            {"name": "GB28181 设备与通道", "description": "管理 GB28181 设备、通道、资源确认与截图。"},
            {"name": "GB28181 录像", "description": "查询设备录像并管理云端录像任务。"},
            {"name": "预览与回放", "description": "创建和控制预览、回放、下载、语音及媒体输出。"},
            {"name": "流监控", "description": "查询和停止活动流。"},
            {"name": "智能分析", "description": "查询、启动和取消智能分析任务。"}
        ],
        "paths": paths,
        "components": {
            "securitySchemes": {
                "GmvAccessKey": {"type": "apiKey", "in": "header", "name": HMAC_ACCESS_KEY_HEADER, "description": "第三方应用的 Access Key。"},
                "GmvTimestamp": {"type": "apiKey", "in": "header", "name": HMAC_TIMESTAMP_HEADER, "description": "请求发起时间，单位为毫秒。"},
                "GmvNonce": {"type": "apiKey", "in": "header", "name": HMAC_NONCE_HEADER, "description": "一次性随机值，用于防止重放。"},
                "GmvContentSha256": {"type": "apiKey", "in": "header", "name": HMAC_CONTENT_SHA256_HEADER, "description": "JSON 请求正文的 SHA-256 十六进制摘要。"},
                "GmvSignature": {"type": "apiKey", "in": "header", "name": HMAC_SIGNATURE_HEADER, "description": "canonical_request 的 HMAC-SHA256 十六进制签名。"}
            },
            "schemas": {
                "ErrorResponse": {
                    "type": "object",
                    "description": "统一 JSON 错误返回。",
                    "properties": {
                        "code": {"type": "string", "description": "稳定的错误代码。"},
                        "message": {"type": "string", "description": "面向开发者的错误说明。"},
                        "user_message": {"type": "string", "description": "可直接展示给用户的中文提示。"},
                        "operation_id": {"type": ["string", "null"], "description": "关联的业务操作标识。"},
                        "trace_id": {"type": ["string", "null"], "description": "请求追踪标识。"},
                        "retryable": {"type": "boolean", "description": "是否适合由调用方重试。"},
                        "details": {"type": "object", "description": "受限的错误补充信息。"}
                    },
                    "required": ["code", "message", "user_message", "retryable", "details"]
                }
            }
        },
        "x-gmv-hmac": {
            "version": "GMV-HMAC-SHA256-V1",
            "description": "将以下字段按固定顺序组成 canonical_request 后计算 HMAC-SHA256。",
            "timestamp_unit": "毫秒",
            "clock_skew_ms": 300000,
            "canonical_fields": ["version", "access_key", "timestamp_ms", "nonce", "method", "path", "canonical_query", "body_sha256"]
        }
    })))
}

fn openapi_operation_parameters(method: &str, path: &str) -> Vec<base::serde_json::Value> {
    let mut parameters = path
        .split('{')
        .skip(1)
        .filter_map(|part| part.split_once('}').map(|(name, _)| name))
        .map(|name| {
            base::serde_json::json!({
                "name": name,
                "in": "path",
                "required": true,
                "description": openapi_field_description(name),
                "schema": {"type": "string"}
            })
        })
        .collect::<Vec<_>>();
    if method == "get" {
        parameters.extend(openapi_query_fields(path).iter().map(|(name, required)| {
            base::serde_json::json!({
                "name": name,
                "in": "query",
                "required": required,
                "description": openapi_field_description(name),
                "schema": {"type": openapi_field_type(name)}
            })
        }));
    }
    parameters
}

fn openapi_operation_tag(path: &str) -> &'static str {
    if path == "/dashboard" || path == "/media/transport" || path == "/runtime/status" {
        "总览与运行状态"
    } else if path.starts_with("/media/operations") || path == "/nodes" || path == "/leases" {
        "节点与租约"
    } else if path == "/events" {
        "事件"
    } else if path.contains("cloud-recordings") || path.contains("/records") {
        "GB28181 录像"
    } else if path.starts_with("/gb28181/devices")
        && !path.ends_with("/preview")
        && !path.ends_with("/playback")
        && !path.ends_with("/ptz")
    {
        "GB28181 设备与通道"
    } else if path.starts_with("/ai/") {
        "智能分析"
    } else if path.contains("stream-history") || path.contains("/streams/{stream_id}/management") {
        "流监控"
    } else {
        "预览与回放"
    }
}

fn openapi_operation_summary(method: &str, path: &str) -> &'static str {
    match (method, path) {
        ("get", "/dashboard") => "查询 Guard 业务总览",
        ("get", "/media/transport") => "查询媒体传输能力",
        ("get", "/media/operations") => "查询媒体操作列表",
        ("get", "/media/operations/{operation_id}") => "查询媒体操作详情",
        ("post", "/media/operations/{operation_id}/continue") => "继续等待媒体操作",
        ("post", "/media/operations/{operation_id}/cancel") => "取消媒体操作",
        ("get", "/nodes") => "查询已注册节点",
        ("get", "/leases") => "查询节点租约",
        ("get", "/events") => "分页查询事件",
        ("get", "/gb28181/session-nodes/{node_id}/config") => "查询 GB28181 会话节点配置",
        ("get", "/gb28181/devices") => "分页查询 GB28181 设备",
        ("post", "/gb28181/devices") => "创建 GB28181 设备",
        ("get", "/gb28181/devices/{device_id}") => "查询 GB28181 设备详情",
        ("post", "/gb28181/devices/{device_id}") => "更新 GB28181 设备",
        ("post", "/gb28181/devices/{device_id}/delete") => "删除 GB28181 设备",
        ("get", "/gb28181/devices/{device_id}/channels") => "查询设备通道",
        ("get", "/gb28181/devices/{device_id}/resources") => "查询设备资源",
        ("post", "/gb28181/devices/{device_id}/resources/{resource_id}/confirmation") => {
            "确认设备资源归属"
        }
        ("post", "/gb28181/devices/{device_id}/resources/{resource_id}/confirmation/reset") => {
            "重置设备资源确认"
        }
        ("get", "/gb28181/devices/{device_id}/channels/{channel_id}") => "查询通道详情",
        ("post", "/gb28181/devices/{device_id}/channels/{channel_id}") => "更新通道配置",
        ("post", "/gb28181/devices/{device_id}/channels/{channel_id}/preview") => {
            "创建通道实时预览"
        }
        ("post", "/gb28181/devices/{device_id}/channels/{channel_id}/playback") => {
            "创建通道录像回放"
        }
        ("post", "/gb28181/devices/{device_id}/channels/{channel_id}/ptz") => "控制通道云台",
        ("get", "/gb28181/devices/{device_id}/channels/{channel_id}/images") => "查询通道截图",
        ("post", "/gb28181/devices/{device_id}/channels/{channel_id}/images") => "触发通道截图",
        ("post", "/gb28181/devices/{device_id}/channels/{channel_id}/images/{image_id}/access") => {
            "签发通道截图访问地址"
        }
        ("post", "/gb28181/devices/{device_id}/channels/{channel_id}/images/{image_id}/cover") => {
            "设置通道封面截图"
        }
        ("get", "/gb28181/devices/{device_id}/channels/{channel_id}/records") => "查询通道录像片段",
        ("post", "/gb28181/devices/{device_id}/channels/{channel_id}/records/query") => {
            "发起通道录像查询"
        }
        ("get", "/gb28181/devices/{device_id}/channels/{channel_id}/cloud-recordings") => {
            "查询通道云端录像任务"
        }
        ("post", "/gb28181/devices/{device_id}/channels/{channel_id}/cloud-recordings") => {
            "创建通道云端录像任务"
        }
        ("get", "/gb28181/cloud-recordings/{task_id}") => "查询云端录像任务详情",
        ("post", "/gb28181/cloud-recordings/{task_id}/stop") => "停止云端录像任务",
        ("post", "/gb28181/cloud-recordings/{task_id}/delete") => "删除云端录像任务",
        ("post", "/gb28181/cloud-recordings/{task_id}/access") => "签发云端录像访问地址",
        ("post", "/gb28181/devices/{device_id}/broadcast/start") => "发起设备语音广播",
        ("post", "/gb28181/broadcasts/{stream_id}/stop") => "停止设备语音广播",
        ("get", "/devices") => "查询可用设备",
        ("post", "/devices/{device_id}/preview") => "创建设备实时预览",
        ("post", "/devices/{device_id}/playback") => "创建设备录像回放",
        ("post", "/devices/{device_id}/download") => "创建设备录像下载",
        ("post", "/devices/{device_id}/talk") => "创建设备语音对讲",
        ("post", "/devices/{device_id}/ptz") => "控制设备云台",
        ("get", "/streams") => "查询媒体流",
        ("get", "/gb28181/streams") => "分页查询 GB28181 活动流",
        ("get", "/gb28181/streams/{stream_id}/management") => "查询活动流管理信息",
        ("get", "/gb28181/stream-history") => "分页查询 GB28181 流历史",
        ("post", "/gb28181/streams/{stream_id}/stop") => "停止 GB28181 活动流",
        ("post", "/streams/{stream_id}/stop") => "停止媒体流",
        ("post", "/streams/{stream_id}/release") => "释放媒体流订阅",
        ("post", "/streams/{stream_id}/speed") => "设置回放流倍速",
        ("post", "/playbacks/{playback_id}/seek") => "跳转回放进度",
        ("post", "/playbacks/{playback_id}/speed") => "设置版本化回放倍速",
        ("post", "/playbacks/{playback_id}/state") => "暂停或继续回放",
        ("post", "/playbacks/presence/heartbeat") => "上报回放观看心跳",
        ("post", "/playback-tickets/{token}/renew") => "确认续期第三方播放票据",
        ("get", "/streams/{stream_id}/outputs") => "查询媒体流输出",
        ("post", "/streams/{stream_id}/outputs") => "创建媒体流输出",
        ("post", "/streams/{stream_id}/outputs/{output_id}/close") => "关闭媒体流输出",
        ("get", "/ai/tasks") => "查询智能分析任务",
        ("post", "/ai/tasks") => "启动智能分析任务",
        ("post", "/ai/tasks/{task_id}/cancel") => "取消智能分析任务",
        ("get", "/runtime/status") => "查询 Guard 运行状态",
        ("get", _) => "查询 Guard 业务数据",
        ("post", _) => "提交 Guard 业务操作",
        _ => "Guard 开放业务接口",
    }
}

fn openapi_query_fields(path: &str) -> &'static [(&'static str, bool)] {
    match path {
        "/media/operations" => &[("ids", false)],
        "/events" => &[
            ("after_id", false),
            ("limit", false),
            ("topic_prefix", false),
            ("min_priority", false),
        ],
        "/gb28181/devices" => &[
            ("page", false),
            ("page_size", false),
            ("session_node_id", false),
            ("domain_id", false),
            ("device_id", false),
            ("device_name", false),
            ("registered_only", false),
        ],
        "/gb28181/devices/{device_id}/channels"
        | "/gb28181/devices/{device_id}/resources"
        | "/gb28181/streams/{stream_id}/management" => &[("session_node_id", true)],
        "/gb28181/devices/{device_id}/channels/{channel_id}/records" => &[
            ("session_node_id", true),
            ("start_time_sec", false),
            ("end_time_sec", false),
            ("page", false),
            ("page_size", false),
        ],
        "/gb28181/devices/{device_id}/channels/{channel_id}/cloud-recordings" => &[
            ("session_node_id", true),
            ("page", false),
            ("page_size", false),
        ],
        "/gb28181/streams" => &[
            ("session_node_id", true),
            ("page", false),
            ("page_size", false),
            ("stream_id", false),
            ("stream_node_id", false),
            ("device_id", false),
            ("channel_id", false),
            ("ssrc", false),
            ("dialog_state", false),
        ],
        "/gb28181/stream-history" => &[
            ("session_node_id", true),
            ("page", false),
            ("page_size", false),
            ("stream_id", false),
            ("stream_node_id", false),
            ("device_id", false),
            ("channel_id", false),
            ("ssrc", false),
            ("state", false),
        ],
        _ => &[],
    }
}

fn openapi_request_fields(method: &str, path: &str) -> &'static [(&'static str, bool)] {
    if method != "post" {
        return &[];
    }
    match path {
        "/gb28181/devices" | "/gb28181/devices/{device_id}" => &[
            ("device_id", false),
            ("session_node_id", false),
            ("domain_id", false),
            ("domain", false),
            ("longitude", false),
            ("latitude", false),
            ("address", false),
            ("pwd", false),
            ("pwd_check", false),
            ("alias", false),
            ("status", false),
            ("heartbeat_sec", false),
            ("tenant_id", false),
            ("sys_org_code", false),
            ("create_by", false),
            ("update_by", false),
        ],
        "/gb28181/devices/{device_id}/delete" => &[("session_node_id", true), ("domain_id", true)],
        "/gb28181/devices/{device_id}/resources/{resource_id}/confirmation" => &[
            ("request_id", true),
            ("resource_kind", true),
            ("owner_scope", true),
            ("owner_id", true),
            ("remark", false),
        ],
        "/gb28181/devices/{device_id}/resources/{resource_id}/confirmation/reset" => {
            &[("request_id", true)]
        }
        "/gb28181/devices/{device_id}/channels/{channel_id}" => &[
            ("alias_name", false),
            ("snapshot", false),
            ("over_pic_id", false),
            ("ptz_enable", false),
            ("talk_enable", false),
            ("audio_enable", false),
            ("record_enable", false),
            ("playback_enable", false),
            ("alarm_enable", false),
            ("biz_enable", false),
            ("sort_no", false),
        ],
        "/gb28181/devices/{device_id}/channels/{channel_id}/preview"
        | "/gb28181/devices/{device_id}/channels/{channel_id}/playback" => &[
            ("request_id", true),
            ("session_node_id", false),
            ("token", false),
            ("start_time_sec", false),
            ("end_time_sec", false),
            ("trans_mode", false),
            ("output_type", false),
            ("audio_codec", false),
            ("startup_timeout_ms", false),
            ("playback_id", false),
        ],
        "/gb28181/devices/{device_id}/channels/{channel_id}/ptz" => &[
            ("deviceId", true),
            ("channelId", true),
            ("leftRight", true),
            ("upDown", true),
            ("inOut", true),
            ("horizonSpeed", true),
            ("verticalSpeed", true),
            ("zoomSpeed", true),
        ],
        "/gb28181/devices/{device_id}/channels/{channel_id}/images" => {
            &[("request_id", true), ("count", false), ("interval", false)]
        }
        "/gb28181/devices/{device_id}/channels/{channel_id}/images/{image_id}/access" => {
            &[("session_node_id", true), ("mode", false)]
        }
        "/gb28181/devices/{device_id}/channels/{channel_id}/images/{image_id}/cover" => {
            &[("session_node_id", true)]
        }
        "/gb28181/devices/{device_id}/channels/{channel_id}/records/query" => &[
            ("request_id", true),
            ("session_node_id", true),
            ("start_time_sec", true),
            ("end_time_sec", true),
        ],
        "/gb28181/devices/{device_id}/channels/{channel_id}/cloud-recordings" => &[
            ("request_id", true),
            ("session_node_id", true),
            ("start_time_sec", true),
            ("end_time_sec", true),
        ],
        "/gb28181/cloud-recordings/{task_id}/stop"
        | "/gb28181/cloud-recordings/{task_id}/delete" => &[("request_id", true)],
        "/gb28181/cloud-recordings/{task_id}/access" => &[("mode", false)],
        "/gb28181/devices/{device_id}/broadcast/start"
        | "/devices/{device_id}/preview"
        | "/devices/{device_id}/playback"
        | "/devices/{device_id}/download"
        | "/devices/{device_id}/talk" => &[
            ("request_id", true),
            ("channel_id", true),
            ("session_node_id", false),
            ("token", false),
            ("start_time_sec", false),
            ("end_time_sec", false),
            ("trans_mode", false),
            ("output_type", false),
            ("audio_codec", false),
            ("startup_timeout_ms", false),
            ("talk_codec", false),
            ("talk_sample_rate", false),
            ("talk_channel_count", false),
            ("talk_frame_duration_ms", false),
            ("playback_id", false),
        ],
        "/devices/{device_id}/ptz" => &[
            ("channel_id", true),
            ("leftRight", true),
            ("upDown", true),
            ("inOut", true),
            ("horizonSpeed", true),
            ("verticalSpeed", true),
            ("zoomSpeed", true),
        ],
        "/gb28181/streams/{stream_id}/stop" => &[
            ("session_node_id", true),
            ("request_id", true),
            ("stop_reason", true),
        ],
        "/streams/{stream_id}/release" => &[("request_id", true), ("subscription_id", true)],
        "/streams/{stream_id}/speed" => &[("speed_rate", true)],
        "/playbacks/{playback_id}/seek" => &[
            ("request_id", true),
            ("stream_id", true),
            ("position_sec", true),
            ("expected_generation", true),
        ],
        "/playbacks/{playback_id}/speed" => &[
            ("request_id", true),
            ("stream_id", true),
            ("speed_rate", true),
            ("expected_generation", true),
        ],
        "/playbacks/{playback_id}/state" => &[
            ("request_id", true),
            ("stream_id", true),
            ("paused", true),
            ("expected_generation", true),
        ],
        "/playbacks/presence/heartbeat" => &[("items", true)],
        "/playback-tickets/{token}/renew" => &[("renew", true)],
        "/streams/{stream_id}/outputs" => &[
            ("request_id", true),
            ("output_type", true),
            ("subscription_id", false),
            ("audio_codec", false),
            ("startup_timeout_ms", false),
        ],
        "/ai/tasks" => &[("request_id", true), ("stream_id", true), ("model", true)],
        _ => &[],
    }
}

fn openapi_request_body(method: &str, path: &str) -> Option<base::serde_json::Value> {
    let fields = openapi_request_fields(method, path);
    if fields.is_empty() {
        return None;
    }
    let properties = fields
        .iter()
        .map(|(name, _)| ((*name).to_string(), openapi_field_schema(name)))
        .collect::<base::serde_json::Map<_, _>>();
    let required = fields
        .iter()
        .filter_map(|(name, required)| required.then_some(*name))
        .collect::<Vec<_>>();
    Some(base::serde_json::json!({
        "required": true,
        "description": "JSON 请求正文。",
        "content": {
            "application/json": {
                "schema": {
                    "type": "object",
                    "properties": properties,
                    "required": required
                }
            }
        }
    }))
}

fn openapi_field_schema(name: &str) -> base::serde_json::Value {
    base::serde_json::json!({
        "type": openapi_field_type(name),
        "description": openapi_field_description(name)
    })
}

fn openapi_field_type(name: &str) -> &'static str {
    match name {
        "registered_only" | "paused" | "renew" => "boolean",
        "speed_rate" | "longitude" | "latitude" => "number",
        "items" => "array",
        "limit"
        | "min_priority"
        | "page"
        | "page_size"
        | "pwd_check"
        | "status"
        | "heartbeat_sec"
        | "snapshot"
        | "ptz_enable"
        | "talk_enable"
        | "audio_enable"
        | "record_enable"
        | "playback_enable"
        | "alarm_enable"
        | "biz_enable"
        | "sort_no"
        | "start_time_sec"
        | "end_time_sec"
        | "startup_timeout_ms"
        | "talk_sample_rate"
        | "talk_channel_count"
        | "talk_frame_duration_ms"
        | "leftRight"
        | "upDown"
        | "inOut"
        | "horizonSpeed"
        | "verticalSpeed"
        | "zoomSpeed"
        | "count"
        | "interval"
        | "position_sec"
        | "expected_generation" => "integer",
        _ => "string",
    }
}

fn openapi_field_description(name: &str) -> &'static str {
    match name {
        "operation_id" => "媒体或业务操作的唯一标识。",
        "node_id" => "Guard 注册节点的唯一标识。",
        "device_id" | "deviceId" => "设备唯一标识。",
        "channel_id" | "channelId" => "设备通道唯一标识。",
        "resource_id" => "设备资源唯一标识。",
        "stream_id" => "媒体流唯一标识。",
        "playback_id" => "回放会话唯一标识。",
        "token" => "播放票据或业务令牌。",
        "output_id" => "媒体输出唯一标识。",
        "task_id" => "任务唯一标识。",
        "request_id" => "调用方生成的幂等请求标识。",
        "session_node_id" => "负责该业务的 GB28181 会话节点标识。",
        "domain_id" => "GB28181 域标识。",
        "domain" => "GB28181 域名称。",
        "longitude" => "设备经度。",
        "latitude" => "设备纬度。",
        "address" => "设备安装或网络地址。",
        "pwd" => "设备接入密码；请按安全要求传输和保存。",
        "pwd_check" => "是否校验设备密码。",
        "alias" | "alias_name" => "便于识别的中文别名。",
        "over_pic_id" => "通道覆盖图资源标识。",
        "status" => "启停状态。",
        "heartbeat_sec" => "设备心跳间隔，单位为秒。",
        "tenant_id" => "业务租户标识。",
        "sys_org_code" => "业务组织机构编码。",
        "create_by" => "创建人标识。",
        "update_by" => "更新人标识。",
        "resource_kind" => "待确认的资源类型。",
        "owner_scope" => "资源归属范围。",
        "owner_id" => "资源归属对象标识。",
        "remark" => "资源确认备注。",
        "snapshot" => "是否启用截图能力。",
        "ptz_enable" => "是否启用云台控制。",
        "talk_enable" => "是否启用语音对讲。",
        "audio_enable" => "是否启用音频。",
        "record_enable" => "是否启用录像。",
        "playback_enable" => "是否启用回放。",
        "alarm_enable" => "是否启用告警。",
        "biz_enable" => "是否启用业务处理。",
        "sort_no" => "通道显示排序号。",
        "start_time_sec" => "开始时间，Unix 秒时间戳。",
        "end_time_sec" => "结束时间，Unix 秒时间戳。",
        "trans_mode" => "媒体传输模式。",
        "output_type" => "输出封装类型，例如 flv 或 hls。",
        "audio_codec" => "输出音频编码，例如 aac。",
        "startup_timeout_ms" => "等待媒体启动的超时时间，单位为毫秒。",
        "talk_codec" => "语音对讲编码。",
        "talk_sample_rate" => "语音对讲采样率。",
        "talk_channel_count" => "语音对讲声道数。",
        "talk_frame_duration_ms" => "语音对讲帧时长，单位为毫秒。",
        "leftRight" => "水平控制值：0 停止、1 向左、2 向右。",
        "upDown" => "垂直控制值：0 停止、1 向上、2 向下。",
        "inOut" => "变倍控制值：0 停止、1 缩小、2 放大。",
        "horizonSpeed" => "水平转动速度。",
        "verticalSpeed" => "垂直转动速度。",
        "zoomSpeed" => "变倍速度。",
        "count" => "本次截图数量。",
        "interval" => "连续截图间隔。",
        "mode" => "录像访问模式。",
        "stream_node_id" => "负责媒体流的流节点标识。",
        "ssrc" => "RTP 同步源标识。",
        "dialog_state" => "GB28181 SIP Dialog 状态筛选值。",
        "state" => "业务状态筛选值。",
        "stop_reason" => "停止媒体流的原因。",
        "subscription_id" => "媒体订阅唯一标识。",
        "speed_rate" => "回放倍速。",
        "position_sec" => "目标回放位置，Unix 秒时间戳。",
        "expected_generation" => "调用方期望的回放控制版本，用于防止并发覆盖。",
        "paused" => "是否暂停回放。",
        "items" => {
            "回放观看心跳条目数组，每项包含 playback_id、stream_id、subscription_id 和 generation。"
        }
        "renew" => "是否同意续期播放票据；仅 true 才执行续期。",
        "model" => "智能分析模型标识。",
        "ids" => "以英文逗号分隔的操作标识列表，最多 100 项。",
        "after_id" => "从该事件标识之后继续查询。",
        "limit" => "本次最多返回的记录数。",
        "topic_prefix" => "事件 Topic 前缀筛选值。",
        "min_priority" => "最低事件优先级。",
        "page" => "页码，从 1 开始。",
        "page_size" => "每页记录数。",
        "device_name" => "设备名称模糊筛选值。",
        "registered_only" => "是否只返回已注册设备。",
        _ => "业务字段。",
    }
}

fn openapi_responses(summary: &str) -> base::serde_json::Value {
    let success_schema = base::serde_json::json!({
        "type": ["object", "array"],
        "description": format!("{summary}成功时返回的 JSON 数据。具体字段由当前 Guard 版本的业务响应决定。")
    });
    let error_content = base::serde_json::json!({
        "application/json": {"schema": {"$ref": "#/components/schemas/ErrorResponse"}}
    });
    base::serde_json::json!({
        "200": {"description": "请求成功，返回 JSON 业务数据。", "content": {"application/json": {"schema": success_schema}}},
        "201": {"description": "资源创建成功，返回新建资源的 JSON 数据。", "content": {"application/json": {"schema": success_schema}}},
        "202": {"description": "异步操作已受理，返回操作状态或任务摘要 JSON。", "content": {"application/json": {"schema": success_schema}}},
        "400": {"description": "请求参数、字段格式或业务前置条件不正确。", "content": error_content},
        "401": {"description": "Access Key、时间戳、nonce、正文摘要或 HMAC 签名无效。", "content": error_content},
        "403": {"description": "第三方应用未获得该接口所需权限，或无权访问目标资源。", "content": error_content},
        "404": {"description": "目标设备、通道、流、任务或操作不存在。", "content": error_content},
        "409": {"description": "当前资源状态与请求冲突。", "content": error_content},
        "429": {"description": "超过调用频率或系统容量限制，可按返回信息决定是否重试。", "content": error_content},
        "500": {"description": "Guard 内部处理失败。", "content": error_content}
    })
}

async fn asyncapi_document(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<base::serde_json::Value>, HttpError> {
    require_role(&state.auth, &headers, Role::Admin)?;
    Ok(Json(base::serde_json::json!({
        "asyncapi": "3.0.0",
        "info": {
            "title": "GMV Guard 三方 MQTT 接入",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "面向第三方业务系统的 MQTT 消息契约。以 Guard 为观察方描述订阅和发布方向，Payload 统一使用 UTF-8 JSON。"
        },
        "servers": {
            "mqttV3": {"host": "{broker}", "protocol": "mqtt", "protocolVersion": "3.1.1", "description": "应用配置 protocol_version=v3 时使用 MQTT V3.1.1。"},
            "mqttV5": {"host": "{broker}", "protocol": "mqtt", "protocolVersion": "5.0", "description": "应用配置 protocol_version=v5 时使用 MQTT V5.0。"}
        },
        "channels": {
            "commands": {
                "address": "gmv/commands/{integration_id}",
                "description": "第三方发布命令，Guard 订阅并执行。",
                "parameters": {"integration_id": {"$ref": "#/components/parameters/integrationId"}},
                "messages": {"command": {"$ref": "#/components/messages/IntegrationCommand"}}
            },
            "commandResults": {
                "address": "gmv/command-results/{integration_id}",
                "description": "Guard 发布命令受理结果，第三方订阅。",
                "parameters": {"integration_id": {"$ref": "#/components/parameters/integrationId"}},
                "messages": {"result": {"$ref": "#/components/messages/CommandResult"}}
            },
            "events": {
                "address": "gmv/events/{integration_id}/{event_type}",
                "description": "Guard 发布业务事件或播放票据续期请求，第三方订阅。",
                "parameters": {
                    "integration_id": {"$ref": "#/components/parameters/integrationId"},
                    "event_type": {"$ref": "#/components/parameters/eventType"}
                },
                "messages": {
                    "event": {"$ref": "#/components/messages/EventEnvelope"},
                    "playbackRenewalRequest": {"$ref": "#/components/messages/PlaybackRenewalRequest"}
                }
            }
        },
        "components": {
            "parameters": {
                "integrationId": {"description": "第三方应用唯一标识。"},
                "eventType": {"description": "事件类型；Topic 中的点号按斜杠展开。"}
            },
            "messages": {
                "IntegrationCommand": {
                    "name": "IntegrationCommand",
                    "title": "第三方命令",
                    "summary": "第三方向 Guard 提交的版本化 JSON 命令。",
                    "contentType": "application/json",
                    "payload": {
                        "type": "object",
                        "required": ["integration_id", "command_id", "issued_at_ms", "expires_at_ms", "action", "target", "payload"],
                        "properties": {
                            "integration_id": {"type": "string", "description": "第三方应用唯一标识，必须与 Topic 一致。"},
                            "command_id": {"type": "string", "description": "第三方生成的全局唯一幂等命令标识。"},
                            "issued_at_ms": {"type": "integer", "description": "命令签发时间，Unix 毫秒时间戳。"},
                            "expires_at_ms": {"type": "integer", "description": "命令过期时间，Unix 毫秒时间戳。"},
                            "action": {"type": "string", "description": "已授权的命令动作，例如 stream.start。"},
                            "target": {"type": "string", "description": "命令目标设备、通道、流或任务标识。"},
                            "payload": {"type": "object", "description": "与 action 对应的 JSON 业务参数。"}
                        }
                    }
                },
                "CommandResult": {
                    "name": "CommandResult",
                    "title": "命令结果",
                    "summary": "Guard 对第三方命令的受理或执行状态。",
                    "contentType": "application/json",
                    "payload": {
                        "type": "object",
                        "required": ["command_id", "operation_id", "state"],
                        "properties": {
                            "command_id": {"type": "string", "description": "对应第三方命令的幂等标识。"},
                            "operation_id": {"type": "string", "description": "Guard 内部业务操作标识。"},
                            "state": {"type": "string", "description": "命令当前状态。"},
                            "error_code": {"type": "string", "description": "失败时的稳定错误代码。"},
                            "error_message": {"type": "string", "description": "失败时的中文错误说明。"}
                        }
                    }
                },
                "EventEnvelope": {
                    "name": "EventEnvelope",
                    "title": "业务事件",
                    "summary": "Guard 向第三方发布的统一 JSON 事件信封。",
                    "contentType": "application/json",
                    "payload": {
                        "type": "object",
                        "required": ["event_id", "event_type", "schema_version", "occurred_at_ms", "payload"],
                        "properties": {
                            "event_id": {"type": "string", "description": "事件唯一标识，消费方据此幂等。"},
                            "event_type": {"type": "string", "description": "事件类型。"},
                            "schema_version": {"type": "string", "description": "事件 Payload 的 Schema 版本。"},
                            "occurred_at_ms": {"type": "integer", "description": "事件发生时间，Unix 毫秒时间戳。"},
                            "trace_id": {"type": "string", "description": "跨 HTTP、MQTT 和内部操作的追踪标识。"},
                            "payload": {"type": "object", "description": "与 event_type 对应的 JSON 业务数据。"}
                        }
                    }
                },
                "PlaybackRenewalRequest": {
                    "name": "PlaybackRenewalRequest",
                    "title": "播放票据续期请求",
                    "summary": "Guard 在第三方播放票据到期前发出的续期询问。",
                    "contentType": "application/json",
                    "payload": {
                        "type": "object",
                        "required": ["token", "playback_id", "stream_id", "expires_at_ms", "response_action"],
                        "properties": {
                            "token": {"type": "string", "description": "待续期播放票据。"},
                            "playback_id": {"type": "string", "description": "回放会话唯一标识。"},
                            "stream_id": {"type": "string", "description": "媒体流唯一标识。"},
                            "expires_at_ms": {"type": "integer", "description": "当前票据过期时间，Unix 毫秒时间戳。"},
                            "response_action": {"type": "string", "description": "第三方返回续期决定时使用的命令动作。"}
                        }
                    }
                }
            }
        },
        "x-gmv-protocol-selection": {
            "allowed": ["v3", "v5"],
            "default": "v3",
            "qos": 1,
            "retain": false,
            "payload_compatible": true,
            "description": "应用选择的协议版本必须与 Guard 部署级 MQTT runtime 一致；两个版本使用相同 JSON Payload。"
        }
    })))
}

async fn api_manifest(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<base::serde_json::Value>, HttpError> {
    require_role(&state.auth, &headers, Role::Admin)?;
    Ok(Json(base::serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "http": {"base_path": "/openapi/v1", "methods": ["GET", "POST"], "auth": "GMV-HMAC-SHA256-V1"},
        "mqtt": {"protocol_versions": ["v3", "v5"], "default": "v3", "qos": 1, "command_actions": ["stream.start", "stream.stop", "stream.playback", "stream.download", "device.talk", "device.ptz", "ai.start", "ai.cancel", "playback.ticket.renew"]},
        "scopes": ["*", "devices:read", "devices:write", "devices:control", "images:read", "audio:control", "streams:read", "streams:write", "streams:preview", "streams:playback", "recordings:read", "recordings:write", "events:read", "ai:read", "ai:write", "nodes:read", "leases:read", "runtime:read"]
    })))
}

fn open_business_router(state: HttpState) -> Router<HttpState> {
    let auth_state = state.clone();
    Router::new()
        .route("/dashboard", get(dashboard))
        .route("/media/transport", get(media_transport))
        .route("/media/operations", get(media_operations))
        .route("/media/operations/{operation_id}", get(media_operation))
        .route(
            "/media/operations/{operation_id}/continue",
            post(continue_media_operation),
        )
        .route(
            "/media/operations/{operation_id}/cancel",
            post(cancel_media_operation),
        )
        .route("/nodes", get(nodes))
        .route("/leases", get(leases))
        .route("/events", get(events))
        .route(
            "/gb28181/session-nodes/{node_id}/config",
            get(gb_session_node_config),
        )
        .route("/gb28181/devices", get(gb_devices).post(create_gb_device))
        .route(
            "/gb28181/devices/{device_id}",
            get(gb_device).post(update_gb_device),
        )
        .route(
            "/gb28181/devices/{device_id}/delete",
            post(delete_gb_device),
        )
        .route("/gb28181/devices/{device_id}/channels", get(gb_channels))
        .route("/gb28181/devices/{device_id}/resources", get(gb_resources))
        .route(
            "/gb28181/devices/{device_id}/resources/{resource_id}/confirmation",
            post(save_gb_resource_confirmation),
        )
        .route(
            "/gb28181/devices/{device_id}/resources/{resource_id}/confirmation/reset",
            post(reset_gb_resource_confirmation),
        )
        .route(
            "/gb28181/devices/{device_id}/channels/{channel_id}",
            get(gb_channel).post(update_gb_channel),
        )
        .route(
            "/gb28181/devices/{device_id}/channels/{channel_id}/preview",
            post(gb_preview),
        )
        .route(
            "/gb28181/devices/{device_id}/channels/{channel_id}/playback",
            post(gb_playback),
        )
        .route(
            "/gb28181/devices/{device_id}/channels/{channel_id}/ptz",
            post(gb_ptz),
        )
        .route(
            "/gb28181/devices/{device_id}/channels/{channel_id}/images",
            get(gb_channel_images).post(gb_snapshot_image),
        )
        .route(
            "/gb28181/devices/{device_id}/channels/{channel_id}/images/{image_id}/access",
            post(issue_gb_channel_image_access),
        )
        .route(
            "/gb28181/devices/{device_id}/channels/{channel_id}/images/{image_id}/cover",
            post(set_gb_channel_cover),
        )
        .route(
            "/gb28181/devices/{device_id}/channels/{channel_id}/records",
            get(gb_channel_records),
        )
        .route(
            "/gb28181/devices/{device_id}/channels/{channel_id}/records/query",
            post(query_gb_channel_records),
        )
        .route(
            "/gb28181/devices/{device_id}/channels/{channel_id}/cloud-recordings",
            get(list_cloud_recordings).post(create_cloud_recording),
        )
        .route(
            "/gb28181/cloud-recordings/{task_id}",
            get(get_cloud_recording),
        )
        .route(
            "/gb28181/cloud-recordings/{task_id}/stop",
            post(stop_cloud_recording),
        )
        .route(
            "/gb28181/cloud-recordings/{task_id}/delete",
            post(delete_cloud_recording),
        )
        .route(
            "/gb28181/cloud-recordings/{task_id}/access",
            post(issue_cloud_recording_access),
        )
        .route(
            "/gb28181/devices/{device_id}/broadcast/start",
            post(gb_broadcast),
        )
        .route("/gb28181/broadcasts/{stream_id}/stop", post(stop_stream))
        .route("/devices", get(devices))
        .route("/devices/{device_id}/preview", post(preview))
        .route("/devices/{device_id}/playback", post(playback))
        .route("/devices/{device_id}/download", post(download))
        .route("/devices/{device_id}/talk", post(talk))
        .route("/devices/{device_id}/ptz", post(ptz))
        .route("/streams", get(streams))
        .route("/gb28181/streams", get(gb_active_streams))
        .route(
            "/gb28181/streams/{stream_id}/management",
            get(gb_active_stream_management),
        )
        .route("/gb28181/stream-history", get(gb_stream_history))
        .route(
            "/gb28181/streams/{stream_id}/stop",
            post(stop_gb_monitored_stream),
        )
        .route("/streams/{stream_id}/stop", post(stop_stream))
        .route("/streams/{stream_id}/release", post(release_stream))
        .route("/streams/{stream_id}/speed", post(set_playback_speed))
        .route("/playbacks/{playback_id}/seek", post(seek_playback))
        .route(
            "/playbacks/{playback_id}/speed",
            post(set_versioned_playback_speed),
        )
        .route("/playbacks/{playback_id}/state", post(set_playback_state))
        .route(
            "/playbacks/presence/heartbeat",
            post(heartbeat_playback_presence),
        )
        .route(
            "/playback-tickets/{token}/renew",
            post(renew_integration_playback_ticket),
        )
        .route(
            "/streams/{stream_id}/outputs",
            get(list_stream_outputs).post(create_stream_output),
        )
        .route(
            "/streams/{stream_id}/outputs/{output_id}/close",
            post(close_stream_output),
        )
        .route("/ai/tasks", get(ai_tasks).post(start_ai_task))
        .route("/ai/tasks/{task_id}/cancel", post(cancel_ai_task))
        .route("/runtime/status", get(runtime_status))
        .layer(middleware::from_fn_with_state(
            auth_state,
            authenticate_open_business_request,
        ))
        .layer(middleware::from_fn(debug_http_request))
        .layer(SetResponseHeaderLayer::if_not_present(
            CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
}

async fn authenticate_open_business_request(
    State(state): State<HttpState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    match authenticate_open_business_request_inner(&state, request).await {
        Ok((request, token)) => {
            let response = next.run(request).await;
            state.auth.logout(&token);
            response
        }
        Err(error) => error.into_response(),
    }
}

async fn authenticate_open_business_request_inner(
    state: &HttpState,
    request: Request<Body>,
) -> Result<(Request<Body>, String), HttpError> {
    let access_key = required_header(request.headers(), HMAC_ACCESS_KEY_HEADER)?;
    let timestamp = required_header(request.headers(), HMAC_TIMESTAMP_HEADER)?
        .parse::<i64>()
        .map_err(|_| HttpError::unauthorized())?;
    let nonce = required_header(request.headers(), HMAC_NONCE_HEADER)?;
    let signature = required_header(request.headers(), HMAC_SIGNATURE_HEADER)?;
    let content_sha256 = required_header(request.headers(), HMAC_CONTENT_SHA256_HEADER)?;
    let now_ms = http_now_ms()?;
    if now_ms.abs_diff(timestamp) > 300_000 {
        return Err(HttpError::unauthorized());
    }
    let repository = require_integration_repository(state)?;
    let credential = repository
        .find_credential(&access_key)
        .await?
        .filter(|credential| {
            credential.purpose == CredentialPurpose::HttpInboundVerify
                && credential.is_active_at(now_ms)
        })
        .ok_or_else(HttpError::unauthorized)?;
    let integration = repository
        .get(&credential.integration_id)
        .await?
        .filter(|integration| {
            integration.transport == IntegrationTransport::Http
                && integration.inbound_enabled
                && integration.enabled
                && integration
                    .expires_at_ms
                    .is_none_or(|expires_at| expires_at > now_ms)
        })
        .ok_or_else(HttpError::unauthorized)?;
    let scope = open_business_scope(request.method(), request.uri().path())
        .ok_or_else(|| HttpError::forbidden("business API capability is not open"))?;
    if !integration.scopes.iter().any(|candidate| candidate == "*")
        && !integration.permits(scope, now_ms)
    {
        return Err(HttpError::forbidden(format!(
            "integration scope {scope} is required"
        )));
    }
    let cipher = state
        .integration_secrets
        .as_ref()
        .ok_or_else(|| HttpError::internal("integration secret cipher is not configured"))?;
    let secret = cipher.decrypt(&credential.secret_ciphertext)?;
    let method = request.method().as_str().to_string();
    let signed_uri = request
        .extensions()
        .get::<OriginalUri>()
        .map(|value| &value.0)
        .unwrap_or_else(|| request.uri());
    let path = signed_uri.path().to_string();
    let query = signed_uri.query().unwrap_or_default().to_string();
    let (mut parts, body) = request.into_parts();
    let body = to_bytes(body, MAX_OPEN_API_BODY_BYTES)
        .await
        .map_err(|_| HttpError::bad_request("request body exceeds the configured limit"))?;
    if content_sha256 != body_sha256(&body) {
        return Err(HttpError::unauthorized());
    }
    verify_request(
        secret.as_bytes(),
        &SignedRequest {
            access_key: &access_key,
            timestamp_ms: timestamp,
            nonce: &nonce,
            method: &method,
            path: &path,
            query: &query,
            body: &body,
        },
        &signature,
    )
    .map_err(HttpError::from_auth)?;
    state
        .integration_nonces
        .claim_rate_slot(&access_key, now_ms)
        .map_err(HttpError::from_auth)?;
    state
        .integration_nonces
        .claim(&access_key, &nonce, now_ms)
        .map_err(HttpError::from_auth)?;
    let identity = format!("integration:{}", integration.integration_id);
    let (token, session) =
        state
            .auth
            .issue_service_session(&identity, Role::Admin, Duration::from_secs(300))?;
    let cookie = format!("{SESSION_COOKIE}={token}");
    parts.headers.insert(
        COOKIE,
        HeaderValue::from_str(&cookie)
            .map_err(|_| HttpError::internal("invalid session cookie"))?,
    );
    parts.headers.insert(
        HeaderName::from_static(CSRF_HEADER),
        HeaderValue::from_str(&session.csrf_token)
            .map_err(|_| HttpError::internal("invalid CSRF token"))?,
    );
    if request_method_requires_csrf(&method) {
        let origin = state
            .auth
            .allowed_origins()
            .first()
            .ok_or_else(|| HttpError::internal("UI origin policy is empty"))?;
        parts.headers.insert(
            ORIGIN,
            HeaderValue::from_str(origin)
                .map_err(|_| HttpError::internal("invalid configured origin"))?,
        );
    }
    Ok((Request::from_parts(parts, Body::from(body)), token))
}

fn required_header(headers: &HeaderMap, name: &'static str) -> Result<String, HttpError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(HttpError::unauthorized)
}

fn request_method_requires_csrf(method: &str) -> bool {
    method != Method::GET.as_str()
}

fn open_business_scope(method: &Method, path: &str) -> Option<&'static str> {
    if path.contains("/images") {
        return Some(if *method == Method::GET || path.ends_with("/access") {
            "images:read"
        } else {
            "devices:control"
        });
    }
    if path.contains("/ptz") {
        return Some("devices:control");
    }
    if path.contains("/talk") || path.contains("/broadcast") {
        return Some("audio:control");
    }
    if path.contains("/preview") {
        return Some("streams:preview");
    }
    if path.contains("/playback") || path.contains("/playbacks") {
        return Some("streams:playback");
    }
    let operation = if *method == Method::GET {
        "read"
    } else {
        "write"
    };
    let capability = if path.contains("/events") {
        "events"
    } else if path.contains("cloud-recordings")
        || path.contains("/records")
        || path.contains("/images")
    {
        "recordings"
    } else if path.contains("/ai/") {
        "ai"
    } else if path.contains("/streams")
        || path.contains("/playbacks")
        || path.contains("/media/")
        || path.contains("/preview")
        || path.contains("/playback")
        || path.contains("/download")
        || path.contains("/talk")
        || path.contains("/broadcast")
    {
        "streams"
    } else if path.contains("/devices") || path.contains("/ptz") || path.contains("/gb28181/") {
        "devices"
    } else if path.contains("/nodes") {
        "nodes"
    } else if path.contains("/leases") {
        "leases"
    } else if path.contains("/dashboard") || path.contains("/runtime/") {
        "runtime"
    } else {
        return None;
    };
    match (capability, operation) {
        ("events", "read") => Some("events:read"),
        ("events", "write") => Some("events:write"),
        ("recordings", "read") => Some("recordings:read"),
        ("recordings", "write") => Some("recordings:write"),
        ("ai", "read") => Some("ai:read"),
        ("ai", "write") => Some("ai:write"),
        ("streams", "read") => Some("streams:read"),
        ("streams", "write") => Some("streams:write"),
        ("devices", "read") => Some("devices:read"),
        ("devices", "write") => Some("devices:write"),
        ("nodes", "read") => Some("nodes:read"),
        ("nodes", "write") => Some("nodes:write"),
        ("leases", "read") => Some("leases:read"),
        ("leases", "write") => Some("leases:write"),
        ("runtime", "read") => Some("runtime:read"),
        ("runtime", "write") => Some("runtime:write"),
        _ => None,
    }
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct LoginRequest {
    username: String,
    password: String,
}

struct OptionalPeerIp(Option<IpAddr>);

impl<S> FromRequestParts<S> for OptionalPeerIp
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    fn from_request_parts(
        parts: &mut Parts,
        _state: &S,
    ) -> impl std::future::Future<Output = Result<Self, Self::Rejection>> + Send {
        let remote_ip = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(address)| address.ip());
        async move { Ok(Self(remote_ip)) }
    }
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct HealthResponse {
    status: &'static str,
}

async fn debug_http_request(request: Request<Body>, next: Next) -> Response {
    let method = request.method().clone();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or("unmatched")
        .to_string();
    let started = Instant::now();
    debug!("guard http inbound: method={method}, route={route}");
    let response = next.run(request).await;
    debug!(
        "guard http outbound: method={method}, route={route}, status={}, elapsed_ms={}",
        response.status().as_u16(),
        started.elapsed().as_millis()
    );
    response
}

async fn renew_ui_session(
    State(auth): State<AuthState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    let token = request
        .headers()
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookie| cookie_value(cookie, SESSION_COOKIE));
    let may_renew = request.method() != Method::OPTIONS;
    let mut response = next.run(request).await;
    if may_renew
        && response.status().is_success()
        && !response.headers().contains_key(SET_COOKIE)
        && let Some(token) = token
        && auth.renew_session(&token).is_ok()
        && let Ok(cookie) = HeaderValue::from_str(&auth.session_cookie(&token))
    {
        response.headers_mut().insert(SET_COOKIE, cookie);
    }
    response
}

fn redacted(value: &str) -> &'static str {
    if value.is_empty() {
        "<empty>"
    } else {
        "<redacted>"
    }
}

fn redacted_option(value: Option<&String>) -> &'static str {
    value.map(|value| redacted(value)).unwrap_or("<none>")
}

async fn health_live() -> Json<HealthResponse> {
    Json(HealthResponse { status: "live" })
}

async fn health_ready(State(state): State<HttpState>) -> Result<Json<HealthResponse>, HttpError> {
    state.outbox.list(1).await?;
    Ok(Json(HealthResponse { status: "ready" }))
}

async fn metrics(State(state): State<HttpState>) -> Result<Response, HttpError> {
    let nodes = state.api.list_nodes();
    let outbox = state.outbox.list(10_000).await?;
    let events = state.api.poll_events(EventQuery::default())?;
    let mut body = String::new();
    body.push_str("# TYPE gmv_guard_nodes gauge\n");
    body.push_str(&format!("gmv_guard_nodes {}\n", nodes.len()));
    body.push_str("# TYPE gmv_guard_nodes_by_health gauge\n");
    for health in [
        HealthState::Starting,
        HealthState::Ready,
        HealthState::Degraded,
        HealthState::Draining,
        HealthState::Offline,
    ] {
        let count = nodes.iter().filter(|node| node.health == health).count();
        body.push_str(&format!(
            "gmv_guard_nodes_by_health{{health=\"{}\"}} {}\n",
            health_label(health),
            count
        ));
    }
    body.push_str("# TYPE gmv_guard_events gauge\n");
    body.push_str(&format!("gmv_guard_events {}\n", events.items.len()));
    body.push_str("# TYPE gmv_guard_outbox_backlog gauge\n");
    let backlog = outbox
        .iter()
        .filter(|record| !record.state.is_terminal())
        .count();
    body.push_str(&format!("gmv_guard_outbox_backlog {}\n", backlog));
    body.push_str("# TYPE gmv_guard_outbox_dead gauge\n");
    let dead = outbox
        .iter()
        .filter(|record| record.state == OutboxState::Dead)
        .count();
    body.push_str(&format!("gmv_guard_outbox_dead {}\n", dead));

    let mut response = body.into_response();
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );
    Ok(response)
}

fn health_label(health: HealthState) -> &'static str {
    match health {
        HealthState::Starting => "starting",
        HealthState::Ready => "ready",
        HealthState::Degraded => "degraded",
        HealthState::Draining => "draining",
        HealthState::Offline => "offline",
    }
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct SessionResponse {
    username: String,
    role: &'static str,
    nickname: String,
    csrf_token: String,
    expires_at_ms: u64,
}

async fn login(
    State(state): State<HttpState>,
    OptionalPeerIp(remote_ip): OptionalPeerIp,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> Result<Response, HttpError> {
    debug!(
        "/api/v2/auth/login, req: username={}, password={}",
        request.username,
        redacted(&request.password)
    );
    verify_origin(&state.auth, &headers)?;
    if !state
        .auth
        .local_admin_login_allowed(&request.username, remote_ip)
    {
        return Err(HttpError::forbidden(
            "bootstrap admin can only login from loopback",
        ));
    }
    let (token, session) = state
        .auth
        .authenticate(&request.username, &request.password)
        .map_err(HttpError::from_auth)?;
    let mut response = Json(session_response(session)).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&state.auth.session_cookie(&token))
            .map_err(|_| HttpError::internal("invalid session cookie"))?,
    );
    Ok(response)
}

async fn current_session(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Response, HttpError> {
    debug!("/api/v2/auth/session, req:<empty>");
    let (token, _) = authenticated_with_token(&state.auth, &headers)?;
    let session = state
        .auth
        .renew_session(&token)
        .map_err(HttpError::from_auth)?;
    let mut response = Json(session_response(session)).into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&state.auth.session_cookie(&token))
            .map_err(|_| HttpError::internal("invalid session cookie"))?,
    );
    Ok(response)
}

async fn logout(State(state): State<HttpState>, headers: HeaderMap) -> Result<Response, HttpError> {
    debug!("/api/v2/auth/logout, req:<empty>");
    verify_origin(&state.auth, &headers)?;
    let (token, session) = authenticated_with_token(&state.auth, &headers)?;
    verify_csrf(&state.auth, &session, &headers)?;
    state.auth.logout(&token);
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&state.auth.clear_cookie())
            .map_err(|_| HttpError::internal("invalid clear cookie"))?,
    );
    Ok(response)
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct DashboardResponse {
    node_count: usize,
    event_count: usize,
    next_after_id: Option<String>,
}

async fn dashboard(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<DashboardResponse>, HttpError> {
    debug!("/api/v2/dashboard, req:<empty>");
    let session = authenticated(&state.auth, &headers)?;
    state
        .auth
        .require_role(&session, Role::Viewer)
        .map_err(|_| HttpError::forbidden("UI role is not allowed"))?;
    let events = state.api.poll_events(EventQuery::default())?;
    Ok(Json(DashboardResponse {
        node_count: state.api.list_nodes().len(),
        event_count: events.items.len(),
        next_after_id: events.next_after_id,
    }))
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct NodeResponse {
    node_id: String,
    instance_id: String,
    kind: String,
    service: String,
    protocol: Option<String>,
    display_name: String,
    connection: String,
    connection_label: String,
    health: String,
    health_label: String,
    scheduling: String,
    scheduling_label: String,
    capabilities: Vec<String>,
    pending_leases: u32,
    host_metrics: HostMetricsResponse,
    business_metrics: std::collections::HashMap<String, String>,
    config: std::collections::HashMap<String, String>,
    zone: Option<String>,
    last_seen_at_ms: i64,
    generation: u64,
    sequence: u64,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct HostMetricsResponse {
    cpu_usage_percent: f64,
    load_average_1m: f64,
    load_average_5m: f64,
    load_average_15m: f64,
    memory_total_bytes: u64,
    memory_used_bytes: u64,
    swap_total_bytes: u64,
    swap_used_bytes: u64,
    disk_read_bytes_per_sec: u64,
    disk_write_bytes_per_sec: u64,
    network_receive_bytes_per_sec: u64,
    network_transmit_bytes_per_sec: u64,
    process_resident_memory_bytes: u64,
    process_threads: u32,
}

impl From<crate::store::model::HostMetricsRecord> for HostMetricsResponse {
    fn from(value: crate::store::model::HostMetricsRecord) -> Self {
        Self {
            cpu_usage_percent: value.cpu_usage_percent,
            load_average_1m: value.load_average_1m,
            load_average_5m: value.load_average_5m,
            load_average_15m: value.load_average_15m,
            memory_total_bytes: value.memory_total_bytes,
            memory_used_bytes: value.memory_used_bytes,
            swap_total_bytes: value.swap_total_bytes,
            swap_used_bytes: value.swap_used_bytes,
            disk_read_bytes_per_sec: value.disk_read_bytes_per_sec,
            disk_write_bytes_per_sec: value.disk_write_bytes_per_sec,
            network_receive_bytes_per_sec: value.network_receive_bytes_per_sec,
            network_transmit_bytes_per_sec: value.network_transmit_bytes_per_sec,
            process_resident_memory_bytes: value.process_resident_memory_bytes,
            process_threads: value.process_threads,
        }
    }
}

impl From<NodeRecord> for NodeResponse {
    fn from(node: NodeRecord) -> Self {
        let service = node_service(&node);
        let connection_label = node_connection_label(node.connection).to_string();
        let health_label = node_health_label(node.health).to_string();
        let scheduling_label = node_scheduling_label(node.scheduling).to_string();
        Self {
            node_id: node.identity.node_id.clone(),
            instance_id: node.identity.instance_id.clone(),
            kind: service.clone(),
            service,
            protocol: node_protocol(&node),
            display_name: node_display_name(&node),
            connection: format!("{:?}", node.connection).to_uppercase(),
            connection_label,
            health: format!("{:?}", node.health).to_uppercase(),
            health_label,
            scheduling: format!("{:?}", node.scheduling).to_uppercase(),
            scheduling_label,
            capabilities: node.capabilities,
            pending_leases: node.pending_leases,
            host_metrics: node.host_metrics.into(),
            business_metrics: node.business_metrics,
            config: node.config,
            zone: node.zone,
            last_seen_at_ms: node.last_seen_at_ms,
            generation: node.generation,
            sequence: node.sequence,
        }
    }
}

fn node_connection_label(state: ConnectionState) -> &'static str {
    match state {
        ConnectionState::Connected => "已连接",
        ConnectionState::Disconnected => "已断开",
        ConnectionState::Superseded => "已替代",
    }
}

fn node_health_label(state: HealthState) -> &'static str {
    match state {
        HealthState::Starting => "启动中",
        HealthState::Ready => "就绪",
        HealthState::Degraded => "降级",
        HealthState::Draining => "排空中",
        HealthState::Offline => "离线",
    }
}

fn node_scheduling_label(state: SchedulingState) -> &'static str {
    match state {
        SchedulingState::Enabled => "可调度",
        SchedulingState::Disabled => "不可调度",
        SchedulingState::TimeUnsynced => "时间未同步",
    }
}

fn node_service(node: &NodeRecord) -> String {
    node.config
        .get("service")
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| format!("{:?}", node.identity.kind).to_lowercase())
}

fn node_protocol(node: &NodeRecord) -> Option<String> {
    node.config
        .get("protocol")
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .or_else(|| {
            node.capabilities.iter().find_map(|capability| {
                capability
                    .strip_prefix("protocol.")
                    .filter(|value| !value.trim().is_empty())
                    .map(ToOwned::to_owned)
            })
        })
}

fn node_display_name(node: &NodeRecord) -> String {
    format!("{}:{}", node_service(node), node.identity.node_id)
}

async fn nodes(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<NodeResponse>>, HttpError> {
    debug!("/api/v2/nodes, req:<empty>");
    require_role(&state.auth, &headers, Role::Viewer)?;
    Ok(Json(
        state.api.list_nodes().into_iter().map(Into::into).collect(),
    ))
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct LeaseResponse {
    lease_id: String,
    route_id: String,
    resource_id: String,
    node_id: String,
    instance_id: String,
    state: &'static str,
    expires_at_ms: i64,
}

impl From<LeaseRecord> for LeaseResponse {
    fn from(lease: LeaseRecord) -> Self {
        Self {
            lease_id: lease.lease_id,
            route_id: lease.route_id,
            resource_id: lease.resource_id,
            node_id: lease.node_id,
            instance_id: lease.instance_id,
            state: lease_state(lease.state),
            expires_at_ms: lease.expires_at_ms,
        }
    }
}

async fn leases(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<LeaseResponse>>, HttpError> {
    debug!("/api/v2/leases, req:<empty>");
    require_role(&state.auth, &headers, Role::Viewer)?;
    Ok(Json(
        state
            .api
            .list_leases()
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct EventHttpQuery {
    after_id: Option<String>,
    limit: Option<usize>,
    topic_prefix: Option<String>,
    min_priority: Option<u8>,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct EventResponse {
    event_id: String,
    topic: String,
    priority: u8,
    payload: String,
}

impl From<EventRecord> for EventResponse {
    fn from(event: EventRecord) -> Self {
        Self {
            event_id: event.event_id,
            topic: event.topic,
            priority: event.priority,
            payload: String::from_utf8_lossy(&event.payload).into_owned(),
        }
    }
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct EventPageResponse {
    items: Vec<EventResponse>,
    next_after_id: Option<String>,
}

async fn events(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<EventHttpQuery>,
) -> Result<Json<EventPageResponse>, HttpError> {
    debug!("/api/v2/events, req:{query:?}");
    let session = authenticated(&state.auth, &headers)?;
    state
        .auth
        .require_role(&session, Role::Viewer)
        .map_err(|_| HttpError::forbidden("UI role is not allowed"))?;
    let page = state.api.poll_events(EventQuery {
        cursor: CursorQuery {
            after_id: query.after_id,
            limit: query
                .limit
                .unwrap_or(crate::api::v2::page::DEFAULT_PAGE_SIZE),
        },
        topic_prefix: query.topic_prefix,
        min_priority: query.min_priority,
    })?;
    Ok(Json(EventPageResponse {
        items: page.items.into_iter().map(Into::into).collect(),
        next_after_id: page.next_after_id,
    }))
}

fn authenticated(auth: &AuthState, headers: &HeaderMap) -> Result<UiSession, HttpError> {
    authenticated_with_token(auth, headers).map(|(_, session)| session)
}

fn authenticated_with_token(
    auth: &AuthState,
    headers: &HeaderMap,
) -> Result<(String, UiSession), HttpError> {
    let cookie = headers
        .get(COOKIE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(HttpError::unauthorized)?;
    let token = cookie_value(cookie, SESSION_COOKIE).ok_or_else(HttpError::unauthorized)?;
    let session = auth.session(&token).map_err(HttpError::from_auth)?;
    Ok((token, session))
}

fn verify_origin(auth: &AuthState, headers: &HeaderMap) -> Result<(), HttpError> {
    auth.verify_origin(headers.get(ORIGIN).and_then(|value| value.to_str().ok()))
        .map_err(|_| HttpError::forbidden("request origin is not allowed"))
}

fn verify_csrf(
    auth: &AuthState,
    session: &UiSession,
    headers: &HeaderMap,
) -> Result<(), HttpError> {
    auth.verify_csrf(
        session,
        headers
            .get(CSRF_HEADER)
            .and_then(|value| value.to_str().ok()),
    )
    .map_err(|_| HttpError::forbidden("invalid CSRF token"))
}

fn session_response(session: UiSession) -> SessionResponse {
    SessionResponse {
        username: session.username,
        role: role_name(session.role),
        nickname: session.nickname,
        csrf_token: session.csrf_token,
        expires_at_ms: session.expires_at_ms,
    }
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::Viewer => "viewer",
        Role::Operator => "operator",
        Role::Admin => "admin",
    }
}

fn lease_state(state: crate::core::LeaseState) -> &'static str {
    match state {
        crate::core::LeaseState::Allocated => "allocated",
        crate::core::LeaseState::Confirmed => "confirmed",
        crate::core::LeaseState::Failed => "failed",
        crate::core::LeaseState::Released => "released",
        crate::core::LeaseState::Expired => "expired",
    }
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct OutboxQuery {
    limit: Option<usize>,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct UserResponse {
    username: String,
    role: &'static str,
    nickname: String,
    enabled: bool,
    expires_at_ms: Option<i64>,
    created_at_ms: i64,
    updated_at_ms: i64,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct CreateUserRequest {
    username: String,
    role: String,
    password: String,
    #[serde(default)]
    nickname: String,
    #[serde(default = "default_enabled")]
    enabled: bool,
    #[serde(default)]
    expires_at_ms: Option<i64>,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct UpdateUserRequest {
    role: String,
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    nickname: Option<String>,
    enabled: bool,
    #[serde(default, deserialize_with = "deserialize_user_expiration_update")]
    expires_at_ms: UserExpirationUpdate,
}

#[derive(Debug, Clone, Copy, Default)]
enum UserExpirationUpdate {
    #[default]
    Unchanged,
    Set(Option<i64>),
}

fn deserialize_user_expiration_update<'de, D>(
    deserializer: D,
) -> Result<UserExpirationUpdate, D::Error>
where
    D: base::serde::Deserializer<'de>,
{
    <Option<i64> as base::serde::Deserialize>::deserialize(deserializer)
        .map(UserExpirationUpdate::Set)
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct UpdateProfileRequest {
    #[serde(default)]
    nickname: Option<String>,
    #[serde(default)]
    password: Option<String>,
}

async fn current_profile(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<UserResponse>, HttpError> {
    debug!("/api/v2/me, req:<empty>");
    let session = require_role(&state.auth, &headers, Role::Viewer)?;
    let profile = require_user_repository(&state)?
        .list_profiles()
        .await?
        .into_iter()
        .find(|profile| profile.username == session.username)
        .ok_or_else(|| GuardError::NotFound(format!("user {}", session.username)))?;
    Ok(Json(user_response(profile)))
}

async fn update_profile(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<UpdateProfileRequest>,
) -> Result<Json<UserResponse>, HttpError> {
    debug!(
        "/api/v2/me, req: nickname={:?}, password={}",
        request.nickname,
        redacted_option(request.password.as_ref())
    );
    let session = require_write(&state.auth, &headers, Role::Viewer)?;
    let users = require_user_repository(&state)?;
    let current = users
        .list_profiles()
        .await?
        .into_iter()
        .find(|profile| profile.username == session.username)
        .ok_or_else(|| GuardError::NotFound(format!("user {}", session.username)))?;
    let hash = request.password.as_deref().map(password_hash).transpose()?;
    users
        .upsert_user(
            &session.username,
            current.role,
            hash.as_deref(),
            request.nickname.as_deref(),
            UserAccess {
                enabled: current.enabled,
                expires_at_ms: current.expires_at_ms,
            },
            http_now_ms()?,
        )
        .await?;
    let user = users
        .load_user(&session.username)
        .await?
        .ok_or_else(|| GuardError::NotFound(format!("user {}", session.username)))?;
    state.auth.upsert_user(user.clone());
    state
        .auth
        .refresh_user_sessions(&session.username, user.role, &user.nickname);
    let profile = users
        .list_profiles()
        .await?
        .into_iter()
        .find(|profile| profile.username == session.username)
        .ok_or_else(|| GuardError::NotFound(format!("user {}", session.username)))?;
    Ok(Json(user_response(profile)))
}

async fn list_users(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<UserResponse>>, HttpError> {
    debug!("/api/v2/users, req:<empty>");
    require_role(&state.auth, &headers, Role::Admin)?;
    let users = require_user_repository(&state)?.list_profiles().await?;
    Ok(Json(users.into_iter().map(user_response).collect()))
}

async fn create_user(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<UserResponse>), HttpError> {
    debug!(
        "/api/v2/users, req: username={}, role={}, password={}, nickname={}, enabled={}, expires_at_ms={:?}",
        request.username,
        request.role,
        redacted(&request.password),
        request.nickname,
        request.enabled,
        request.expires_at_ms
    );
    require_write(&state.auth, &headers, Role::Admin)?;
    let username = request.username.trim().to_string();
    let role = Role::parse(&request.role)?;
    let hash = password_hash(&request.password)?;
    let now_ms = http_now_ms()?;
    validate_user_expiration(request.expires_at_ms, now_ms)?;
    let users = require_user_repository(&state)?;
    users
        .upsert_user(
            &username,
            role,
            Some(&hash),
            Some(&request.nickname),
            UserAccess {
                enabled: request.enabled,
                expires_at_ms: request.expires_at_ms,
            },
            now_ms,
        )
        .await?;
    refresh_auth_user(&state.auth, users, &username).await?;
    let profile = users
        .list_profiles()
        .await?
        .into_iter()
        .find(|profile| profile.username == username)
        .ok_or_else(|| GuardError::NotFound(format!("user {username}")))?;
    Ok((StatusCode::CREATED, Json(user_response(profile))))
}

async fn update_user(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(username): Path<String>,
    Json(request): Json<UpdateUserRequest>,
) -> Result<Json<UserResponse>, HttpError> {
    debug!(
        "/api/v2/users/{{username}}, req: username={}, role={}, password={}, nickname={:?}, enabled={}, expires_at_ms={:?}",
        username,
        request.role,
        redacted_option(request.password.as_ref()),
        request.nickname,
        request.enabled,
        request.expires_at_ms
    );
    require_write(&state.auth, &headers, Role::Admin)?;
    let username = username.trim().to_string();
    let role = Role::parse(&request.role)?;
    let hash = request.password.as_deref().map(password_hash).transpose()?;
    let now_ms = http_now_ms()?;
    let users = require_user_repository(&state)?;
    let expires_at_ms = match request.expires_at_ms {
        UserExpirationUpdate::Set(expires_at_ms) => expires_at_ms,
        UserExpirationUpdate::Unchanged => users
            .list_profiles()
            .await?
            .into_iter()
            .find(|profile| profile.username == username)
            .map(|profile| profile.expires_at_ms)
            .ok_or_else(|| GuardError::NotFound(format!("user {username}")))?,
    };
    validate_user_expiration(expires_at_ms, now_ms)?;
    users
        .upsert_user(
            &username,
            role,
            hash.as_deref(),
            request.nickname.as_deref(),
            UserAccess {
                enabled: request.enabled,
                expires_at_ms,
            },
            now_ms,
        )
        .await?;
    refresh_auth_user(&state.auth, users, &username).await?;
    let profile = users
        .list_profiles()
        .await?
        .into_iter()
        .find(|profile| profile.username == username)
        .ok_or_else(|| GuardError::NotFound(format!("user {username}")))?;
    Ok(Json(user_response(profile)))
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct CreateIntegrationRequest {
    name: String,
    transport: String,
    inbound_enabled: bool,
    outbound_enabled: bool,
    #[serde(default = "default_enabled")]
    enabled: bool,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(default)]
    expires_at_ms: Option<i64>,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct UpdateIntegrationRequest {
    name: String,
    inbound_enabled: bool,
    outbound_enabled: bool,
    enabled: bool,
    scopes: Vec<String>,
    #[serde(default)]
    expires_at_ms: Option<i64>,
    expected_config_version: i64,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct CreateIntegrationCredentialRequest {
    purpose: CredentialPurpose,
    #[serde(default)]
    expires_at_ms: Option<i64>,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct CreatedIntegrationCredentialResponse {
    credential: IntegrationCredentialSummary,
    secret: String,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct SaveHttpIntegrationRequest {
    #[serde(default)]
    callback_url: Option<String>,
    callback_timeout_ms: i64,
    private_network_policy: String,
    #[serde(default)]
    private_network_allowlist: Vec<String>,
    max_attempts: i64,
    event_ttl_ms: i64,
    max_response_bytes: i64,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct SaveMqttIntegrationRequest {
    protocol_version: String,
    #[serde(default)]
    allowed_actions: Vec<String>,
    command_topic: String,
    result_topic: String,
    event_topic_prefix: String,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct SaveIntegrationMappingRequest {
    #[serde(default)]
    mapping_id: Option<String>,
    direction: String,
    source_type: String,
    schema_version: String,
    destination_kind: String,
    destination: String,
    payload_profile: String,
    #[serde(default = "default_enabled")]
    enabled: bool,
}

async fn list_integrations(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Integration>>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    Ok(Json(require_integration_repository(&state)?.list().await?))
}

async fn get_integration(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(integration_id): Path<String>,
) -> Result<Json<Integration>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    let value = require_integration_repository(&state)?
        .get(&integration_id)
        .await?
        .ok_or_else(|| GuardError::NotFound(format!("integration {integration_id}")))?;
    Ok(Json(value))
}

async fn create_integration(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateIntegrationRequest>,
) -> Result<(StatusCode, Json<Integration>), HttpError> {
    let session = require_write(&state.auth, &headers, Role::Admin)?;
    let now_ms = http_now_ms()?;
    let transport = IntegrationTransport::parse(&request.transport.to_ascii_uppercase())?;
    let integration_id = format!("int_{}", Uuid::new_v4().simple());
    let value = Integration {
        integration_id: integration_id.clone(),
        name: request.name.trim().to_string(),
        transport,
        inbound_enabled: request.inbound_enabled,
        outbound_enabled: request.outbound_enabled,
        enabled: request.enabled,
        scopes: request.scopes,
        expires_at_ms: request.expires_at_ms,
        config_version: 1,
        created_by: session.username.clone(),
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    };
    value.validate(now_ms)?;
    require_http_integration_master_key(&state, &value)?;
    if value.transport == IntegrationTransport::Mqtt && value.enabled && !state.mqtt_runtime_enabled
    {
        return Err(GuardError::InvalidConfig(
            "MQTT runtime must be enabled before enabling the integration".to_string(),
        )
        .into());
    }
    let repository = require_integration_repository(&state)?;
    repository.upsert(&value).await?;
    match transport {
        IntegrationTransport::Http => {
            repository
                .upsert_http_config(&IntegrationHttpConfig {
                    integration_id: integration_id.clone(),
                    callback_url: None,
                    callback_timeout_ms: 5_000,
                    private_network_policy: "deny".to_string(),
                    private_network_allowlist: Vec::new(),
                    max_attempts: 5,
                    event_ttl_ms: 259_200_000,
                    max_response_bytes: 65_536,
                    updated_at_ms: now_ms,
                })
                .await?;
        }
        IntegrationTransport::Mqtt => {
            repository
                .upsert_mqtt_config(&IntegrationMqttConfig {
                    integration_id: integration_id.clone(),
                    protocol_version: state.mqtt_runtime_protocol_version.clone(),
                    allowed_actions: Vec::new(),
                    command_topic: format!("gmv/commands/{integration_id}"),
                    result_topic: format!("gmv/command-results/{integration_id}"),
                    event_topic_prefix: format!("gmv/events/{integration_id}"),
                    updated_at_ms: now_ms,
                })
                .await?;
        }
    }
    append_integration_audit(
        repository,
        Some(&integration_id),
        &session.username,
        "integration.create",
        &integration_id,
        "created",
        now_ms,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(value)))
}

async fn update_integration(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(integration_id): Path<String>,
    Json(request): Json<UpdateIntegrationRequest>,
) -> Result<Json<Integration>, HttpError> {
    let session = require_write(&state.auth, &headers, Role::Admin)?;
    let repository = require_integration_repository(&state)?;
    let mut value = repository
        .get(&integration_id)
        .await?
        .ok_or_else(|| GuardError::NotFound(format!("integration {integration_id}")))?;
    if value.config_version != request.expected_config_version {
        return Err(GuardError::Conflict(format!(
            "integration config version changed: expected {}, actual {}",
            request.expected_config_version, value.config_version
        ))
        .into());
    }
    let now_ms = http_now_ms()?;
    value.name = request.name.trim().to_string();
    value.inbound_enabled = request.inbound_enabled;
    value.outbound_enabled = request.outbound_enabled;
    value.enabled = request.enabled;
    value.scopes = request.scopes;
    value.expires_at_ms = request.expires_at_ms;
    value.config_version += 1;
    value.updated_at_ms = now_ms;
    value.validate(now_ms)?;
    require_http_integration_master_key(&state, &value)?;
    if value.transport == IntegrationTransport::Mqtt && value.enabled {
        if !state.mqtt_runtime_enabled {
            return Err(GuardError::InvalidConfig(
                "MQTT runtime must be enabled before enabling the integration".to_string(),
            )
            .into());
        }
        let mqtt = repository
            .mqtt_config(&integration_id)
            .await?
            .ok_or_else(|| GuardError::NotFound(format!("MQTT integration {integration_id}")))?;
        if mqtt.protocol_version != state.mqtt_runtime_protocol_version {
            return Err(GuardError::InvalidConfig(format!(
                "MQTT integration protocol_version {} must match running broker connection {}",
                mqtt.protocol_version, state.mqtt_runtime_protocol_version
            ))
            .into());
        }
    }
    repository.upsert(&value).await?;
    append_integration_audit(
        repository,
        Some(&integration_id),
        &session.username,
        "integration.update",
        &integration_id,
        "updated",
        now_ms,
    )
    .await?;
    Ok(Json(value))
}

async fn list_integration_credentials(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(integration_id): Path<String>,
) -> Result<Json<Vec<IntegrationCredentialSummary>>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    let values = require_integration_repository(&state)?
        .list_credentials(&integration_id)
        .await?
        .into_iter()
        .map(Into::into)
        .collect();
    Ok(Json(values))
}

async fn create_integration_credential(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(integration_id): Path<String>,
    Json(request): Json<CreateIntegrationCredentialRequest>,
) -> Result<(StatusCode, Json<CreatedIntegrationCredentialResponse>), HttpError> {
    let session = require_write(&state.auth, &headers, Role::Admin)?;
    let repository = require_integration_repository(&state)?;
    let integration = repository
        .get(&integration_id)
        .await?
        .ok_or_else(|| GuardError::NotFound(format!("integration {integration_id}")))?;
    if integration.transport != IntegrationTransport::Http {
        return Err(GuardError::InvalidConfig(
            "HMAC credentials are only valid for HTTP integrations".to_string(),
        )
        .into());
    }
    let cipher = state.integration_secrets.as_ref().ok_or_else(|| {
        GuardError::InvalidConfig(format!(
            "{} is required to create integration credentials",
            crate::integration::secret::INTEGRATION_MASTER_KEY_CONFIG
        ))
    })?;
    let now_ms = http_now_ms()?;
    if request
        .expires_at_ms
        .is_some_and(|expires_at| expires_at <= now_ms)
    {
        return Err(GuardError::InvalidConfig(
            "credential expiry must be in the future".to_string(),
        )
        .into());
    }
    let secret = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let credential = IntegrationCredential {
        credential_id: format!("cred_{}", Uuid::new_v4().simple()),
        access_key: format!("ak_{}", Uuid::new_v4().simple()),
        integration_id: integration_id.clone(),
        purpose: request.purpose,
        secret_ciphertext: cipher.encrypt(&secret)?,
        key_version: 1,
        status: CredentialStatus::Active,
        not_before_ms: now_ms,
        expires_at_ms: request.expires_at_ms,
        revoked_at_ms: None,
        created_by: session.username.clone(),
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    };
    repository.insert_credential(&credential).await?;
    append_integration_audit(
        repository,
        Some(&integration_id),
        &session.username,
        "credential.create",
        &credential.credential_id,
        "created",
        now_ms,
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedIntegrationCredentialResponse {
            credential: credential.into(),
            secret,
        }),
    ))
}

async fn revoke_integration_credential(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((integration_id, credential_id)): Path<(String, String)>,
) -> Result<StatusCode, HttpError> {
    let session = require_write(&state.auth, &headers, Role::Admin)?;
    let repository = require_integration_repository(&state)?;
    let now_ms = http_now_ms()?;
    let belongs = repository
        .list_credentials(&integration_id)
        .await?
        .iter()
        .any(|credential| credential.credential_id == credential_id);
    if !belongs {
        return Err(GuardError::NotFound(format!("credential {credential_id}")).into());
    }
    repository.revoke_credential(&credential_id, now_ms).await?;
    append_integration_audit(
        repository,
        Some(&integration_id),
        &session.username,
        "credential.revoke",
        &credential_id,
        "revoked",
        now_ms,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_integration_http(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(integration_id): Path<String>,
) -> Result<Json<IntegrationHttpConfig>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    let value = require_integration_repository(&state)?
        .http_config(&integration_id)
        .await?
        .ok_or_else(|| GuardError::NotFound(format!("HTTP integration {integration_id}")))?;
    Ok(Json(value))
}

async fn update_integration_http(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(integration_id): Path<String>,
    Json(request): Json<SaveHttpIntegrationRequest>,
) -> Result<Json<IntegrationHttpConfig>, HttpError> {
    let session = require_write(&state.auth, &headers, Role::Admin)?;
    require_integration_transport(&state, &integration_id, IntegrationTransport::Http).await?;
    let now_ms = http_now_ms()?;
    let value = IntegrationHttpConfig {
        integration_id: integration_id.clone(),
        callback_url: request.callback_url,
        callback_timeout_ms: request.callback_timeout_ms,
        private_network_policy: request.private_network_policy,
        private_network_allowlist: request.private_network_allowlist,
        max_attempts: request.max_attempts,
        event_ttl_ms: request.event_ttl_ms,
        max_response_bytes: request.max_response_bytes,
        updated_at_ms: now_ms,
    };
    value.validate()?;
    let repository = require_integration_repository(&state)?;
    repository.upsert_http_config(&value).await?;
    append_integration_audit(
        repository,
        Some(&integration_id),
        &session.username,
        "http.update",
        &integration_id,
        "updated",
        now_ms,
    )
    .await?;
    Ok(Json(value))
}

async fn get_integration_mqtt(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(integration_id): Path<String>,
) -> Result<Json<IntegrationMqttConfig>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    let value = require_integration_repository(&state)?
        .mqtt_config(&integration_id)
        .await?
        .ok_or_else(|| GuardError::NotFound(format!("MQTT integration {integration_id}")))?;
    Ok(Json(value))
}

async fn integration_mqtt_runtime(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<base::serde_json::Value>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    Ok(Json(base::serde_json::json!({
        "enabled": state.mqtt_runtime_enabled,
        "protocol_version": state.mqtt_runtime_protocol_version,
        "connection_scope": "deployment",
        "qos": 1,
        "retain": false
    })))
}

async fn update_integration_mqtt(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(integration_id): Path<String>,
    Json(request): Json<SaveMqttIntegrationRequest>,
) -> Result<Json<IntegrationMqttConfig>, HttpError> {
    let session = require_write(&state.auth, &headers, Role::Admin)?;
    require_integration_transport(&state, &integration_id, IntegrationTransport::Mqtt).await?;
    let now_ms = http_now_ms()?;
    let value = IntegrationMqttConfig {
        integration_id: integration_id.clone(),
        protocol_version: request.protocol_version.to_ascii_lowercase(),
        allowed_actions: request.allowed_actions,
        command_topic: request.command_topic,
        result_topic: request.result_topic,
        event_topic_prefix: request.event_topic_prefix,
        updated_at_ms: now_ms,
    };
    value.validate()?;
    if value.protocol_version != state.mqtt_runtime_protocol_version {
        return Err(GuardError::InvalidConfig(format!(
            "MQTT protocol_version must match running broker connection {}",
            state.mqtt_runtime_protocol_version
        ))
        .into());
    }
    let repository = require_integration_repository(&state)?;
    repository.upsert_mqtt_config(&value).await?;
    append_integration_audit(
        repository,
        Some(&integration_id),
        &session.username,
        "mqtt.update",
        &integration_id,
        &format!("protocol_version={}", value.protocol_version),
        now_ms,
    )
    .await?;
    Ok(Json(value))
}

async fn list_integration_mappings(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(integration_id): Path<String>,
) -> Result<Json<Vec<IntegrationMapping>>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    Ok(Json(
        require_integration_repository(&state)?
            .list_mappings(&integration_id)
            .await?,
    ))
}

async fn upsert_integration_mapping(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(integration_id): Path<String>,
    Json(request): Json<SaveIntegrationMappingRequest>,
) -> Result<Json<IntegrationMapping>, HttpError> {
    let session = require_write(&state.auth, &headers, Role::Admin)?;
    if request.direction != "OUTBOUND"
        || !matches!(request.destination_kind.as_str(), "HTTP" | "MQTT")
        || request.source_type.trim().is_empty()
        || request.source_type.len() > 255
        || !request.source_type.bytes().all(|value| {
            value.is_ascii_alphanumeric() || matches!(value, b'.' | b'_' | b'-' | b'*')
        })
        || request.destination.trim().is_empty()
        || request.destination.len() > 512
        || request.schema_version != "v1"
        || request.payload_profile != "event-envelope-v1"
    {
        return Err(GuardError::InvalidConfig("invalid integration mapping".to_string()).into());
    }
    let repository = require_integration_repository(&state)?;
    let integration = repository
        .get(&integration_id)
        .await?
        .ok_or_else(|| GuardError::NotFound(format!("integration {integration_id}")))?;
    match integration.transport {
        IntegrationTransport::Http => {
            let config = repository
                .http_config(&integration_id)
                .await?
                .ok_or_else(|| {
                    GuardError::InvalidConfig("HTTP integration config missing".to_string())
                })?;
            if request.destination_kind != "HTTP"
                || config.callback_url.as_deref() != Some(request.destination.as_str())
            {
                return Err(GuardError::InvalidConfig(
                    "HTTP mapping destination must match the configured callback URL".to_string(),
                )
                .into());
            }
        }
        IntegrationTransport::Mqtt => {
            let config = repository
                .mqtt_config(&integration_id)
                .await?
                .ok_or_else(|| {
                    GuardError::InvalidConfig("MQTT integration config missing".to_string())
                })?;
            let prefix = format!("{}/", config.event_topic_prefix);
            if request.destination_kind != "MQTT" || !request.destination.starts_with(&prefix) {
                return Err(GuardError::InvalidConfig(
                    "MQTT mapping destination must use the integration event topic prefix"
                        .to_string(),
                )
                .into());
            }
        }
    }
    let now_ms = http_now_ms()?;
    let value = IntegrationMapping {
        mapping_id: request
            .mapping_id
            .unwrap_or_else(|| format!("map_{}", Uuid::new_v4().simple())),
        integration_id: integration_id.clone(),
        direction: request.direction,
        source_type: request.source_type,
        schema_version: request.schema_version,
        destination_kind: request.destination_kind,
        destination: request.destination,
        payload_profile: request.payload_profile,
        enabled: request.enabled,
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    };
    repository.upsert_mapping(&value).await?;
    append_integration_audit(
        repository,
        Some(&integration_id),
        &session.username,
        "mapping.upsert",
        &value.mapping_id,
        "updated",
        now_ms,
    )
    .await?;
    Ok(Json(value))
}

async fn list_integration_audits(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<OutboxQuery>,
) -> Result<Json<Vec<IntegrationAudit>>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    Ok(Json(
        require_integration_repository(&state)?
            .list_audits(query.limit.unwrap_or(100).min(1_000))
            .await?,
    ))
}

fn require_integration_repository(state: &HttpState) -> Result<&IntegrationRepository, HttpError> {
    state.integrations.as_ref().ok_or_else(|| {
        GuardError::InvalidConfig("integration persistent store is disabled".to_string()).into()
    })
}

fn require_http_integration_master_key(
    state: &HttpState,
    integration: &Integration,
) -> Result<(), HttpError> {
    if integration.transport == IntegrationTransport::Http
        && integration.enabled
        && state.integration_secrets.is_none()
    {
        return Err(HttpError {
            status: StatusCode::BAD_REQUEST,
            code: "integration_master_key_missing".to_string(),
            message: format!(
                "{} is required before enabling an HTTP integration",
                crate::integration::secret::INTEGRATION_MASTER_KEY_CONFIG
            ),
            user_message: Some("部署未配置 HTTP 集成主密钥，暂不能启用 HTTP 接入".to_string()),
            retryable: Some(false),
            details: BTreeMap::new(),
        });
    }
    Ok(())
}

async fn require_integration_transport(
    state: &HttpState,
    integration_id: &str,
    expected: IntegrationTransport,
) -> Result<(), HttpError> {
    let value = require_integration_repository(state)?
        .get(integration_id)
        .await?
        .ok_or_else(|| GuardError::NotFound(format!("integration {integration_id}")))?;
    if value.transport != expected {
        return Err(GuardError::InvalidConfig(format!(
            "integration {integration_id} transport does not match"
        ))
        .into());
    }
    Ok(())
}

async fn append_integration_audit(
    repository: &IntegrationRepository,
    integration_id: Option<&str>,
    actor: &str,
    action: &str,
    target_id: &str,
    detail_summary: &str,
    now_ms: i64,
) -> GuardResult<()> {
    repository
        .append_audit(&IntegrationAudit {
            audit_id: format!("audit_{}", Uuid::new_v4().simple()),
            integration_id: integration_id.map(str::to_string),
            actor: actor.to_string(),
            action: action.to_string(),
            target_id: target_id.to_string(),
            outcome: "SUCCESS".to_string(),
            detail_summary: detail_summary.to_string(),
            created_at_ms: now_ms,
        })
        .await
}

fn user_response(profile: UserProfile) -> UserResponse {
    UserResponse {
        username: profile.username,
        role: profile.role.as_str(),
        nickname: profile.nickname,
        enabled: profile.enabled,
        expires_at_ms: profile.expires_at_ms,
        created_at_ms: profile.created_at_ms,
        updated_at_ms: profile.updated_at_ms,
    }
}

fn validate_user_expiration(expires_at_ms: Option<i64>, now_ms: i64) -> Result<(), HttpError> {
    if expires_at_ms.is_some_and(|expires_at_ms| expires_at_ms <= now_ms) {
        return Err(HttpError {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_user_expiration".to_string(),
            message: "expires_at_ms must be in the future or null".to_string(),
            user_message: Some("用户有效期必须晚于当前时间，或选择永久".to_string()),
            retryable: Some(false),
            details: BTreeMap::new(),
        });
    }
    Ok(())
}

fn require_user_repository(state: &HttpState) -> Result<&UserRepository, HttpError> {
    state.users.as_ref().ok_or_else(|| HttpError {
        status: StatusCode::NOT_IMPLEMENTED,
        code: "user_store_disabled".to_string(),
        message: "persistent user store is disabled".to_string(),
        user_message: Some("用户管理未启用持久化存储".to_string()),
        retryable: Some(false),
        details: BTreeMap::new(),
    })
}

async fn refresh_auth_user(
    auth: &AuthState,
    users: &UserRepository,
    username: &str,
) -> Result<(), HttpError> {
    auth.revoke_user_sessions(username);
    match users.load_user(username).await? {
        Some(user) => auth.upsert_user(user),
        None => auth.remove_user(username),
    }
    Ok(())
}

fn password_hash(password: &str) -> Result<String, HttpError> {
    if password.is_empty() {
        return Err(HttpError {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_user".to_string(),
            message: "password is required".to_string(),
            user_message: Some("请输入密码".to_string()),
            retryable: Some(false),
            details: BTreeMap::new(),
        });
    }
    hash_ui_password(password).map_err(|_| HttpError::internal("password hash failed"))
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct OutboxResponse {
    outbox_id: String,
    event_id: String,
    integration_id: String,
    mapping_id: String,
    destination_kind: &'static str,
    destination: String,
    state: &'static str,
    attempts: u32,
    next_attempt_at_ms: i64,
    last_error: Option<String>,
    created_at_ms: i64,
    updated_at_ms: i64,
    expires_at_ms: Option<i64>,
}

impl From<OutboxRecord> for OutboxResponse {
    fn from(record: OutboxRecord) -> Self {
        Self {
            outbox_id: record.outbox_id,
            event_id: record.event_id,
            integration_id: record.integration_id,
            mapping_id: record.mapping_id,
            destination_kind: match record.destination_kind {
                OutboxDestinationKind::Mqtt => "mqtt",
                OutboxDestinationKind::Webhook => "webhook",
            },
            destination: record.destination,
            state: match record.state {
                OutboxState::Pending => "pending",
                OutboxState::Sending => "sending",
                OutboxState::Delivered => "delivered",
                OutboxState::RetryWait => "retry_wait",
                OutboxState::Dead => "dead",
            },
            attempts: record.attempts,
            next_attempt_at_ms: record.next_attempt_at_ms,
            last_error: record.last_error,
            created_at_ms: record.created_at_ms,
            updated_at_ms: record.updated_at_ms,
            expires_at_ms: record.expires_at_ms,
        }
    }
}

async fn outbox_records(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<OutboxQuery>,
) -> Result<Json<Vec<OutboxResponse>>, HttpError> {
    debug!("/api/v2/integrations/outbox, req:{query:?}");
    let session = authenticated(&state.auth, &headers)?;
    state
        .auth
        .require_role(&session, Role::Viewer)
        .map_err(|_| HttpError::forbidden("UI role is not allowed"))?;
    let limit = query.limit.unwrap_or(100).clamp(1, 500);
    Ok(Json(
        state
            .outbox
            .list(limit)
            .await?
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}
async fn retry_outbox(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(outbox_id): Path<String>,
) -> Result<Json<OutboxResponse>, HttpError> {
    debug!("/api/v2/integrations/outbox/{{outbox_id}}/retry, req: outbox_id={outbox_id}");
    verify_origin(&state.auth, &headers)?;
    let session = authenticated(&state.auth, &headers)?;
    state
        .auth
        .require_role(&session, Role::Operator)
        .map_err(|_| HttpError::forbidden("UI role is not allowed"))?;
    verify_csrf(&state.auth, &session, &headers)?;
    Ok(Json(
        state
            .outbox
            .retry_dead(&outbox_id, http_now_ms()?)
            .await?
            .into(),
    ))
}

fn operation_request(
    operation_id: String,
    kind: &str,
    session: &UiSession,
    required_role: Role,
) -> OperationRequest {
    OperationRequest {
        operation_id,
        kind: kind.to_string(),
        requested_by: session.username.clone(),
        caller_role: session.role,
        required_role,
        dangerous: false,
        confirmation: None,
    }
}

fn http_now_ms() -> Result<i64, HttpError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .map_err(|error| HttpError::internal(format!("system clock before epoch: {error}")))
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct PreviewRequest {
    request_id: String,
    channel_id: String,
    #[serde(default)]
    session_node_id: String,
    #[serde(default)]
    token: String,
    #[serde(default)]
    start_time_sec: u32,
    #[serde(default)]
    end_time_sec: u32,
    #[serde(default)]
    trans_mode: String,
    #[serde(default)]
    output_type: String,
    #[serde(default)]
    audio_codec: String,
    #[serde(default)]
    startup_timeout_ms: Option<u64>,
    #[serde(default)]
    talk_codec: String,
    #[serde(default)]
    talk_sample_rate: u32,
    #[serde(default)]
    talk_channel_count: u32,
    #[serde(default)]
    talk_frame_duration_ms: u32,
    #[serde(default)]
    playback_id: String,
}

fn device_stream_options(request: &PreviewRequest) -> DeviceStreamOptions {
    DeviceStreamOptions {
        session_node_id: request.session_node_id.clone(),
        token: request.token.clone(),
        start_time_sec: request.start_time_sec,
        end_time_sec: request.end_time_sec,
        trans_mode: request.trans_mode.clone(),
        output_type: request.output_type.clone(),
        audio_codec: request.audio_codec.clone(),
        talk_codec: request.talk_codec.clone(),
        talk_sample_rate: request.talk_sample_rate,
        talk_channel_count: request.talk_channel_count,
        talk_frame_duration_ms: request.talk_frame_duration_ms,
        playback_id: request.playback_id.clone(),
    }
}

fn issue_playback_ticket(
    state: &HttpState,
    mut stream: StreamSummary,
    ui_session_token: &str,
    session: &UiSession,
    required_role: Role,
) -> Result<StreamSummary, HttpError> {
    if stream.endpoint.is_empty() || stream.state != StreamSummaryState::Running {
        return Ok(stream);
    }
    let token = Uuid::new_v4().to_string();
    let now_ms = http_now_ms()?;
    let ticket = PlaybackTicketRecord {
        token: token.clone(),
        stream_id: stream.stream_id.clone(),
        playback_id: stream.playback_id.clone(),
        playback_start_time_sec: stream.playback_start_time_sec,
        playback_end_time_sec: stream.playback_end_time_sec,
        output_id: String::new(),
        subscription_id: stream.subscription_id.clone(),
        lease_id: stream.lease_id.clone(),
        route_id: stream.route_id.clone(),
        username: session.username.clone(),
        ui_session_token: ui_session_token.to_string(),
        required_role,
        issued_at_ms: now_ms,
        expires_at_ms: now_ms + playback_ticket_ttl_ms(&session.username),
        absolute_expires_at_ms: playback_ticket_absolute_expiry(&session.username, now_ms),
        renewal_count: 0,
    };
    state.api.store().upsert_playback_ticket(ticket);
    stream.endpoint = endpoint_with_playback_token(&stream.endpoint, &token);
    Ok(stream)
}

fn playback_ticket_ttl_ms(username: &str) -> i64 {
    if username.starts_with("integration:") {
        INTEGRATION_PLAYBACK_TOKEN_TTL_MS
    } else {
        PLAYBACK_TOKEN_TTL_MS
    }
}

fn playback_ticket_absolute_expiry(username: &str, now_ms: i64) -> i64 {
    if username.starts_with("integration:") {
        now_ms.saturating_add(INTEGRATION_PLAYBACK_MAX_LIFETIME_MS)
    } else {
        now_ms.saturating_add(PLAYBACK_TOKEN_TTL_MS)
    }
}

fn issue_stream_output_ticket(
    state: &HttpState,
    mut output: StreamOutputSummary,
    subscription_id: &str,
    ui_session_token: &str,
    session: &UiSession,
) -> Result<StreamOutputSummary, HttpError> {
    if output.endpoint.is_empty() {
        return Ok(output);
    }
    let route = state
        .api
        .store()
        .routes()
        .into_iter()
        .find(|route| route.resource_id == output.stream_id);
    let lease = state
        .api
        .store()
        .leases()
        .into_iter()
        .find(|lease| lease.resource_id == output.stream_id);
    let token = Uuid::new_v4().to_string();
    let now_ms = http_now_ms()?;
    let ticket = PlaybackTicketRecord {
        token: token.clone(),
        stream_id: output.stream_id.clone(),
        playback_id: String::new(),
        playback_start_time_sec: 0,
        playback_end_time_sec: 0,
        output_id: output.output_id.clone(),
        subscription_id: subscription_id.to_string(),
        lease_id: lease.map(|lease| lease.lease_id).unwrap_or_default(),
        route_id: route.map(|route| route.route_id).unwrap_or_default(),
        username: session.username.clone(),
        ui_session_token: ui_session_token.to_string(),
        required_role: Role::Viewer,
        issued_at_ms: now_ms,
        expires_at_ms: now_ms + playback_ticket_ttl_ms(&session.username),
        absolute_expires_at_ms: playback_ticket_absolute_expiry(&session.username, now_ms),
        renewal_count: 0,
    };
    state.api.store().upsert_playback_ticket(ticket);
    output.endpoint = endpoint_with_playback_token(&output.endpoint, &token);
    Ok(output)
}

fn endpoint_with_playback_token(endpoint: &str, token: &str) -> String {
    let (base, query) = endpoint.split_once('?').unwrap_or((endpoint, ""));
    let mut parameters = query
        .split('&')
        .filter(|part| !part.is_empty() && !part.starts_with("gmv-token="))
        .map(str::to_string)
        .collect::<Vec<_>>();
    parameters.push(format!("gmv-token={token}"));
    format!("{base}?{}", parameters.join("&"))
}

fn playback_token_from_endpoint(endpoint: &str) -> Option<&str> {
    endpoint
        .split_once('?')
        .map(|(_, query)| query)
        .into_iter()
        .flat_map(|query| query.split('&'))
        .find_map(|part| part.strip_prefix("gmv-token="))
        .filter(|token| !token.is_empty())
}

async fn compensate_stream_start(state: &HttpState, operation_id: &str, stream: &StreamSummary) {
    if let Some(token) = playback_token_from_endpoint(&stream.endpoint) {
        state.api.store().revoke_playback_token(token);
    }
    if stream.subscription_id.is_empty() {
        return;
    }
    let _ = BusinessControl::new(state.api.store())
        .release_stream(
            &format!("compensate-{operation_id}"),
            &stream.stream_id,
            &stream.subscription_id,
        )
        .await;
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct PtzControlRequest {
    #[serde(rename = "leftRight")]
    left_right: u32,
    #[serde(rename = "upDown")]
    up_down: u32,
    #[serde(rename = "inOut")]
    in_out: u32,
    #[serde(rename = "horizonSpeed")]
    horizon_speed: u32,
    #[serde(rename = "verticalSpeed")]
    vertical_speed: u32,
    #[serde(rename = "zoomSpeed")]
    zoom_speed: u32,
}

impl PtzControlRequest {
    fn command(&self) -> Result<&'static str, HttpError> {
        match (self.left_right, self.up_down, self.in_out) {
            (0, 0, 0) => Ok("stop"),
            (1, 1, 0) => Ok("left_up"),
            (2, 1, 0) => Ok("right_up"),
            (1, 2, 0) => Ok("left_down"),
            (2, 2, 0) => Ok("right_down"),
            (1, 0, 0) => Ok("left"),
            (2, 0, 0) => Ok("right"),
            (0, 1, 0) => Ok("up"),
            (0, 2, 0) => Ok("down"),
            (0, 0, 1) => Ok("zoom_out"),
            (0, 0, 2) => Ok("zoom_in"),
            _ => Err(HttpError::bad_request("invalid ptz control values")),
        }
    }

    fn speed(&self, command: &str) -> Result<u32, HttpError> {
        if command == "stop" {
            return Ok(1);
        }
        if self.in_out > 0 && self.zoom_speed > 0 {
            return Ok(self.zoom_speed);
        }
        let mut speed = 0;
        if self.left_right > 0 {
            speed = speed.max(self.horizon_speed);
        }
        if self.up_down > 0 {
            speed = speed.max(self.vertical_speed);
        }
        if speed == 0 {
            return Err(HttpError::bad_request("ptz speed is required"));
        }
        Ok(speed)
    }
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct PtzRequest {
    channel_id: String,
    #[serde(flatten)]
    control: PtzControlRequest,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct GbPtzRequest {
    #[serde(rename = "deviceId")]
    device_id: String,
    #[serde(rename = "channelId")]
    channel_id: String,
    #[serde(flatten)]
    control: PtzControlRequest,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct StartAiRequest {
    request_id: String,
    stream_id: String,
    model: String,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct GbDeviceRequest {
    #[serde(default)]
    device_id: String,
    #[serde(default)]
    session_node_id: String,
    #[serde(default)]
    domain_id: String,
    #[serde(default)]
    domain: String,
    #[serde(default)]
    longitude: String,
    #[serde(default)]
    latitude: String,
    #[serde(default)]
    address: String,
    #[serde(default)]
    pwd: String,
    #[serde(default)]
    pwd_check: i64,
    #[serde(default)]
    alias: String,
    #[serde(default = "default_enabled_i64")]
    status: i64,
    #[serde(default = "default_heartbeat_sec_i64")]
    heartbeat_sec: i64,
    #[serde(default)]
    snapshot_to_mode: i64,
    #[serde(default)]
    tenant_id: String,
    #[serde(default)]
    sys_org_code: String,
    #[serde(default)]
    create_by: String,
    #[serde(default)]
    update_by: String,
}

fn default_enabled_i64() -> i64 {
    1
}
fn default_heartbeat_sec_i64() -> i64 {
    60
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct GbDeviceListQuery {
    page: Option<u32>,
    page_size: Option<u32>,
    session_node_id: Option<String>,
    domain_id: Option<String>,
    device_id: Option<String>,
    device_name: Option<String>,
    registered_only: Option<bool>,
}

#[derive(Debug, Default, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct GbSessionNodeQuery {
    #[serde(default)]
    session_node_id: String,
}

#[derive(Debug, Default, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct GbRecordListQuery {
    #[serde(default)]
    session_node_id: String,
    start_time_sec: Option<i64>,
    end_time_sec: Option<i64>,
    page: Option<u32>,
    page_size: Option<u32>,
}

#[derive(Debug, Default, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct GbActiveStreamQuery {
    #[serde(default)]
    session_node_id: String,
    #[serde(default)]
    page: u32,
    #[serde(default)]
    page_size: u32,
    #[serde(default)]
    stream_id: String,
    #[serde(default)]
    stream_node_id: String,
    #[serde(default)]
    device_id: String,
    #[serde(default)]
    channel_id: String,
    #[serde(default)]
    ssrc: String,
    #[serde(default)]
    dialog_state: String,
}

#[derive(Debug, Default, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct GbStreamManagementQuery {
    #[serde(default)]
    session_node_id: String,
}

#[derive(Debug, Default, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct GbStreamHistoryQuery {
    #[serde(default)]
    session_node_id: String,
    #[serde(default)]
    page: u32,
    #[serde(default)]
    page_size: u32,
    #[serde(default)]
    stream_id: String,
    #[serde(default)]
    stream_node_id: String,
    #[serde(default)]
    device_id: String,
    #[serde(default)]
    channel_id: String,
    #[serde(default)]
    ssrc: String,
    #[serde(default)]
    state: String,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct GbMonitoredStreamStopRequest {
    session_node_id: String,
    request_id: String,
    stop_reason: String,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct GbDeviceDeleteRequest {
    session_node_id: String,
    domain_id: String,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct GbDeviceResponse {
    device_id: String,
    session_node_id: String,
    domain_id: String,
    domain: String,
    longitude: Option<String>,
    latitude: Option<String>,
    address: Option<String>,
    pwd: Option<String>,
    pwd_check: i64,
    alias: Option<String>,
    status: i64,
    heartbeat_sec: i64,
    snapshot_to_mode: i64,
    del: i64,
    create_time: Option<String>,
    tenant_id: Option<String>,
    sys_org_code: Option<String>,
    create_by: Option<String>,
    update_by: Option<String>,
    update_time: Option<String>,
    monitor_status: i64,
    device_type: Option<String>,
    manufacturer: Option<String>,
    model: Option<String>,
    firmware: Option<String>,
    gb_version: Option<String>,
    max_camera: i64,
    camera_in_count: i64,
    camera_off_count: i64,
    register_time: Option<String>,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct GbDevicePageResponse {
    items: Vec<GbDeviceResponse>,
    total: u64,
    page: u32,
    page_size: u32,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct GbChannelResponse {
    device_id: String,
    channel_id: String,
    name: String,
    manufacturer: String,
    model: String,
    owner: String,
    status: String,
    civil_code: String,
    address: String,
    parent_id: String,
    ip_address: String,
    port: i64,
    longitude: String,
    latitude: String,
    ptz_type: String,
    alias_name: String,
    pic_url: String,
    snapshot: i64,
    over_pic_id: String,
    ptz_enable: i64,
    talk_enable: i64,
    audio_enable: i64,
    record_enable: i64,
    playback_enable: i64,
    alarm_enable: i64,
    biz_enable: i64,
    sort_no: i64,
    created_at_ms: i64,
    updated_at_ms: i64,
    cover_image_id: String,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct GbChannelRequest {
    #[serde(default)]
    alias_name: String,
    #[serde(default)]
    snapshot: i64,
    #[serde(default)]
    over_pic_id: String,
    #[serde(default)]
    ptz_enable: i64,
    #[serde(default)]
    talk_enable: i64,
    #[serde(default)]
    audio_enable: i64,
    #[serde(default)]
    record_enable: i64,
    #[serde(default)]
    playback_enable: i64,
    #[serde(default)]
    alarm_enable: i64,
    #[serde(default = "default_biz_enable")]
    biz_enable: i64,
    #[serde(default)]
    sort_no: i64,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct GbChannelImageResponse {
    image_id: String,
    device_id: String,
    channel_id: String,
    image_url: String,
    created_at_ms: i64,
    file_name: String,
    content_type: String,
    file_size: u64,
    can_preview: bool,
    session_node_id: String,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct GbChannelImagePageResponse {
    items: Vec<GbChannelImageResponse>,
    total: u64,
    page: u32,
    page_size: u32,
}

#[derive(Debug, Default, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct GbChannelImageListQuery {
    #[serde(default)]
    session_node_id: String,
    #[serde(default)]
    start_time_ms: i64,
    #[serde(default)]
    end_time_ms: i64,
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_image_page_size")]
    page_size: u32,
}

fn default_image_page_size() -> u32 {
    12
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct GbChannelImageAccessRequest {
    session_node_id: String,
    #[serde(default)]
    mode: String,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct GbChannelCoverRequest {
    session_node_id: String,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct GbChannelImageAccessResponse {
    url: String,
    expires_at_ms: i64,
    content_type: String,
    file_name: String,
    file_size: u64,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct GbResourceConfirmationResponse {
    status: i64,
    resource_kind: String,
    owner_scope: String,
    owner_id: String,
    suggested_enum_id: String,
    source_parent_id: String,
    confirmed_by: String,
    confirmed_at_ms: i64,
    remark: String,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct GbResourceResponse {
    device_id: String,
    resource_id: String,
    name: String,
    status: String,
    parent_id: String,
    type_code: String,
    enum_id: String,
    enum_name: String,
    suggested_kind: String,
    classification_mode: String,
    effective_kind: String,
    effective_owner_scope: String,
    effective_owner_id: String,
    warning: String,
    biz_enable: i64,
    owner_biz_enable: i64,
    supported: bool,
    available: bool,
    unavailable_reason: String,
    confirmation: Option<GbResourceConfirmationResponse>,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct SaveGbResourceConfirmationBody {
    request_id: String,
    resource_kind: String,
    owner_scope: String,
    owner_id: String,
    #[serde(default)]
    remark: String,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct ResetGbResourceConfirmationBody {
    request_id: String,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct GbSessionConfigResponse {
    domain: String,
    domain_id: String,
    wan_ip: String,
    wan_port: u32,
}
#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct GbStreamRequest {
    request_id: String,
    #[serde(default)]
    session_node_id: String,
    #[serde(default)]
    token: String,
    #[serde(default)]
    start_time_sec: u32,
    #[serde(default)]
    end_time_sec: u32,
    #[serde(default)]
    trans_mode: String,
    #[serde(default)]
    output_type: String,
    #[serde(default)]
    audio_codec: String,
    #[serde(default)]
    startup_timeout_ms: Option<u64>,
    #[serde(default)]
    playback_id: String,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct CloudRecordingCreateRequest {
    request_id: String,
    session_node_id: String,
    start_time_sec: i64,
    end_time_sec: i64,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct CloudRecordingListQuery {
    session_node_id: String,
    #[serde(default = "default_page")]
    page: u32,
    #[serde(default = "default_cloud_recording_page_size")]
    page_size: u32,
}

fn default_page() -> u32 {
    1
}

fn default_cloud_recording_page_size() -> u32 {
    50
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct CloudRecordingActionRequest {
    request_id: String,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct CloudRecordingAccessRequest {
    #[serde(default)]
    mode: String,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct CloudRecordingSummary {
    task_id: String,
    request_id: String,
    session_node_id: String,
    device_id: String,
    channel_id: String,
    start_time_sec: i64,
    end_time_sec: i64,
    requested_duration_sec: u64,
    status: String,
    file_state: String,
    progress_percent: u32,
    recorded_duration_ms: u64,
    progress_stale: bool,
    current_size_bytes: u64,
    final_size_bytes: u64,
    file_format: String,
    requested_by: String,
    created_at_ms: i64,
    started_at_ms: i64,
    finished_at_ms: i64,
    updated_at_ms: i64,
    error_code: String,
    error_message: String,
    can_stop: bool,
    can_play: bool,
    can_download: bool,
    can_delete: bool,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct CloudRecordingListResponse {
    items: Vec<CloudRecordingSummary>,
    total: u64,
    page: u32,
    page_size: u32,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct CloudRecordingAccessResponse {
    url: String,
    expires_at_ms: i64,
    content_type: String,
    file_name: String,
    file_size: u64,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct PlaybackSpeedRequest {
    speed_rate: f32,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct ReleaseStreamRequest {
    request_id: String,
    subscription_id: String,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct PlaybackSpeedResponse {
    accepted: bool,
    speed_rate: f32,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct PlaybackSeekRequest {
    request_id: String,
    stream_id: String,
    position_sec: u32,
    expected_generation: u64,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct VersionedPlaybackSpeedRequest {
    request_id: String,
    stream_id: String,
    speed_rate: f32,
    expected_generation: u64,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct PlaybackStateRequest {
    request_id: String,
    stream_id: String,
    paused: bool,
    expected_generation: u64,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct PlaybackControlHttpResponse {
    accepted: bool,
    generation: u64,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct IntegrationPlaybackRenewRequest {
    renew: bool,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct IntegrationPlaybackRenewResponse {
    renewed: bool,
    revoked: bool,
    expires_at_ms: Option<i64>,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct PlaybackPresenceHeartbeatRequest {
    items: Vec<PlaybackPresenceHeartbeatItem>,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct PlaybackPresenceHeartbeatItem {
    playback_id: String,
    stream_id: String,
    subscription_id: String,
    generation: u64,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct PlaybackPresenceHeartbeatResponse {
    server_time_ms: i64,
    items: Vec<PlaybackPresenceHeartbeatResult>,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct PlaybackPresenceHeartbeatResult {
    playback_id: String,
    stream_id: String,
    accepted: bool,
    terminal: bool,
    generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    presence_deadline_ms: Option<i64>,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct CreateStreamOutputRequest {
    request_id: String,
    output_type: String,
    #[serde(default)]
    subscription_id: String,
    #[serde(default = "default_output_audio_codec")]
    audio_codec: String,
    #[serde(default)]
    startup_timeout_ms: Option<u64>,
}

fn default_output_audio_codec() -> String {
    "aac".to_string()
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct CloseStreamOutputResponse {
    closed: bool,
    output_id: String,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct GbSnapshotRequest {
    request_id: String,
    #[serde(default)]
    count: Option<u8>,
    #[serde(default)]
    interval: Option<u8>,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct GbSnapshotResponse {
    session_id: String,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct GbRecordQueryRequest {
    request_id: String,
    session_node_id: String,
    start_time_sec: i64,
    end_time_sec: i64,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct GbRecordQueryBatchResponse {
    batch_id: String,
    status: String,
    start_time_sec: i64,
    end_time_sec: i64,
    created_at_ms: i64,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct GbRecordSegmentResponse {
    segment_id: i64,
    batch_id: String,
    device_id: String,
    channel_id: String,
    remote_device_id: String,
    name: String,
    file_path: String,
    address: String,
    start_time_sec: i64,
    end_time_sec: i64,
    secrecy: i64,
    record_type: String,
    recorder_id: String,
    file_size: i64,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct GbChannelRecordsResponse {
    current_batch: Option<GbRecordQueryBatchResponse>,
    attempt_batch: Option<GbRecordQueryBatchResponse>,
    segments: Vec<GbRecordSegmentResponse>,
    next_query_at_ms: i64,
    server_time_ms: i64,
    total: i64,
    page: u32,
    page_size: u32,
}

fn default_biz_enable() -> i64 {
    1
}

fn empty_to_none(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
}

fn gb_device_request(request: GbDeviceRequest) -> RpcGbDevice {
    RpcGbDevice {
        device_id: request.device_id,
        session_node_id: request.session_node_id,
        domain_id: request.domain_id,
        domain: request.domain,
        longitude: request.longitude,
        latitude: request.latitude,
        address: request.address,
        pwd: request.pwd,
        pwd_check: request.pwd_check,
        alias: request.alias,
        status: request.status,
        heartbeat_sec: request.heartbeat_sec,
        snapshot_to_mode: request.snapshot_to_mode,
        del: 0,
        create_time: String::new(),
        tenant_id: request.tenant_id,
        sys_org_code: request.sys_org_code,
        create_by: request.create_by,
        update_by: request.update_by,
        update_time: String::new(),
        monitor_status: 0,
        device_type: String::new(),
        manufacturer: String::new(),
        model: String::new(),
        firmware: String::new(),
        gb_version: String::new(),
        max_camera: 0,
        camera_in_count: 0,
        camera_off_count: 0,
        register_time: String::new(),
    }
}

fn gb_device_response(record: RpcGbDevice) -> GbDeviceResponse {
    GbDeviceResponse {
        device_id: record.device_id,
        session_node_id: record.session_node_id,
        domain_id: record.domain_id,
        domain: record.domain,
        longitude: empty_to_none(record.longitude),
        latitude: empty_to_none(record.latitude),
        address: empty_to_none(record.address),
        pwd: empty_to_none(record.pwd),
        pwd_check: record.pwd_check,
        alias: empty_to_none(record.alias),
        status: record.status,
        heartbeat_sec: record.heartbeat_sec,
        snapshot_to_mode: record.snapshot_to_mode,
        del: record.del,
        create_time: empty_to_none(record.create_time),
        tenant_id: empty_to_none(record.tenant_id),
        sys_org_code: empty_to_none(record.sys_org_code),
        create_by: empty_to_none(record.create_by),
        update_by: empty_to_none(record.update_by),
        update_time: empty_to_none(record.update_time),
        monitor_status: record.monitor_status,
        device_type: empty_to_none(record.device_type),
        manufacturer: empty_to_none(record.manufacturer),
        model: empty_to_none(record.model),
        firmware: empty_to_none(record.firmware),
        gb_version: empty_to_none(record.gb_version),
        max_camera: record.max_camera,
        camera_in_count: record.camera_in_count,
        camera_off_count: record.camera_off_count,
        register_time: empty_to_none(record.register_time),
    }
}

fn gb_device_page_response(page: GbDevicePage) -> GbDevicePageResponse {
    GbDevicePageResponse {
        items: page.devices.into_iter().map(gb_device_response).collect(),
        total: page.total,
        page: page.page,
        page_size: page.page_size,
    }
}

fn gb_channel_response(record: RpcGbChannel) -> GbChannelResponse {
    GbChannelResponse {
        device_id: record.device_id,
        channel_id: record.channel_id,
        name: record.name,
        manufacturer: record.manufacturer,
        model: record.model,
        owner: record.owner,
        status: record.status,
        civil_code: record.civil_code,
        address: record.address,
        parent_id: record.parent_id,
        ip_address: record.ip_address,
        port: record.port,
        longitude: record.longitude,
        latitude: record.latitude,
        ptz_type: record.ptz_type,
        alias_name: record.alias_name,
        pic_url: record.pic_url,
        snapshot: record.snapshot,
        over_pic_id: record.over_pic_id,
        ptz_enable: record.ptz_enable,
        talk_enable: record.talk_enable,
        audio_enable: record.audio_enable,
        record_enable: record.record_enable,
        playback_enable: record.playback_enable,
        alarm_enable: record.alarm_enable,
        biz_enable: record.biz_enable,
        sort_no: record.sort_no,
        created_at_ms: record.created_at_ms,
        updated_at_ms: record.updated_at_ms,
        cover_image_id: record.cover_image_id,
    }
}

fn gb_channel_records_response(record: RpcGbChannelRecordsResponse) -> GbChannelRecordsResponse {
    GbChannelRecordsResponse {
        current_batch: record.current_batch.map(gb_record_batch_response),
        attempt_batch: record.attempt_batch.map(gb_record_batch_response),
        segments: record
            .segments
            .into_iter()
            .map(gb_record_segment_response)
            .collect(),
        next_query_at_ms: record.next_query_at_ms,
        server_time_ms: record.server_time_ms,
        total: record.total,
        page: record.page,
        page_size: record.page_size,
    }
}

fn gb_record_batch_response(record: RpcGbRecordQueryBatch) -> GbRecordQueryBatchResponse {
    GbRecordQueryBatchResponse {
        batch_id: record.batch_id,
        status: record.status,
        start_time_sec: record.start_time_sec,
        end_time_sec: record.end_time_sec,
        created_at_ms: record.created_at_ms,
    }
}

fn gb_record_segment_response(record: RpcGbRecordSegment) -> GbRecordSegmentResponse {
    GbRecordSegmentResponse {
        segment_id: record.segment_id,
        batch_id: record.batch_id,
        device_id: record.device_id,
        channel_id: record.channel_id,
        remote_device_id: record.remote_device_id,
        name: record.name,
        file_path: record.file_path,
        address: record.address,
        start_time_sec: record.start_time_sec,
        end_time_sec: record.end_time_sec,
        secrecy: record.secrecy,
        record_type: record.record_type,
        recorder_id: record.recorder_id,
        file_size: record.file_size,
    }
}

fn gb_resource_response(record: RpcGbResource) -> GbResourceResponse {
    GbResourceResponse {
        device_id: record.device_id,
        resource_id: record.resource_id,
        name: record.name,
        status: record.status,
        parent_id: record.parent_id,
        type_code: record.type_code,
        enum_id: record.enum_id,
        enum_name: record.enum_name,
        suggested_kind: record.suggested_kind,
        classification_mode: record.classification_mode,
        effective_kind: record.effective_kind,
        effective_owner_scope: record.effective_owner_scope,
        effective_owner_id: record.effective_owner_id,
        warning: record.warning,
        biz_enable: record.biz_enable,
        owner_biz_enable: record.owner_biz_enable,
        supported: record.supported,
        available: record.available,
        unavailable_reason: record.unavailable_reason,
        confirmation: record
            .confirmation
            .map(|confirmation| GbResourceConfirmationResponse {
                status: confirmation.status,
                resource_kind: confirmation.resource_kind,
                owner_scope: confirmation.owner_scope,
                owner_id: confirmation.owner_id,
                suggested_enum_id: confirmation.suggested_enum_id,
                source_parent_id: confirmation.source_parent_id,
                confirmed_by: confirmation.confirmed_by,
                confirmed_at_ms: confirmation.confirmed_at_ms,
                remark: confirmation.remark,
            }),
    }
}

fn gb_channel_request(
    device_id: String,
    channel_id: String,
    request: GbChannelRequest,
) -> RpcGbChannel {
    RpcGbChannel {
        device_id,
        channel_id,
        alias_name: request.alias_name,
        snapshot: request.snapshot,
        over_pic_id: request.over_pic_id,
        ptz_enable: request.ptz_enable,
        talk_enable: request.talk_enable,
        audio_enable: request.audio_enable,
        record_enable: request.record_enable,
        playback_enable: request.playback_enable,
        alarm_enable: request.alarm_enable,
        biz_enable: request.biz_enable,
        sort_no: request.sort_no,
        ..Default::default()
    }
}

fn log_gb_device_request(path: &str, request: &GbDeviceRequest) {
    debug!(
        "{path}, req: device_id={}, session_node_id={}, domain_id={}, domain={}, longitude={}, latitude={}, address={}, pwd={}, pwd_check={}, alias={}, status={}, heartbeat_sec={}, tenant_id={}, sys_org_code={}, create_by={}, update_by={}",
        request.device_id,
        request.session_node_id,
        request.domain_id,
        request.domain,
        request.longitude,
        request.latitude,
        request.address,
        redacted(&request.pwd),
        request.pwd_check,
        request.alias,
        request.status,
        request.heartbeat_sec,
        request.tenant_id,
        request.sys_org_code,
        request.create_by,
        request.update_by
    );
}

fn log_preview_request(path: &str, device_id: &str, request: &PreviewRequest) {
    debug!(
        "{path}, req: device_id={}, request_id={}, channel_id={}, session_node_id={}, token={}, start_time_sec={}, end_time_sec={}, trans_mode={}, output_type={}, talk_codec={}, talk_sample_rate={}, talk_channel_count={}, talk_frame_duration_ms={}",
        device_id,
        request.request_id,
        request.channel_id,
        request.session_node_id,
        redacted(&request.token),
        request.start_time_sec,
        request.end_time_sec,
        request.trans_mode,
        request.output_type,
        request.talk_codec,
        request.talk_sample_rate,
        request.talk_channel_count,
        request.talk_frame_duration_ms
    );
}

fn gb_session_config_response(record: GbSessionConfigSummary) -> GbSessionConfigResponse {
    GbSessionConfigResponse {
        domain: record.domain,
        domain_id: record.domain_id,
        wan_ip: record.wan_ip,
        wan_port: record.wan_port,
    }
}
fn gb_channel_image_response(record: RpcGbChannelImage) -> GbChannelImageResponse {
    GbChannelImageResponse {
        image_id: record.image_id,
        device_id: record.device_id,
        channel_id: record.channel_id,
        image_url: record.image_url,
        created_at_ms: record.created_at_ms,
        file_name: record.file_name,
        content_type: record.content_type,
        file_size: record.file_size,
        can_preview: record.can_preview,
        session_node_id: record.session_node_id,
    }
}

fn gb_preview_request(channel_id: String, request: GbStreamRequest) -> PreviewRequest {
    PreviewRequest {
        request_id: request.request_id,
        channel_id,
        session_node_id: request.session_node_id,
        token: request.token,
        start_time_sec: request.start_time_sec,
        end_time_sec: request.end_time_sec,
        trans_mode: request.trans_mode,
        output_type: request.output_type,
        audio_codec: request.audio_codec,
        startup_timeout_ms: request.startup_timeout_ms,
        talk_codec: String::new(),
        talk_sample_rate: 0,
        talk_channel_count: 0,
        talk_frame_duration_ms: 0,
        playback_id: request.playback_id,
    }
}

async fn gb_session_node_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(node_id): Path<String>,
) -> Result<Json<GbSessionConfigResponse>, HttpError> {
    debug!("/api/v2/gb28181/session-nodes/{{node_id}}/config, req: node_id={node_id}");
    require_role(&state.auth, &headers, Role::Viewer)?;
    let config = BusinessControl::new(state.api.store())
        .gb_session_config(&node_id)
        .await?;
    Ok(Json(gb_session_config_response(config)))
}
async fn gb_devices(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<GbDeviceListQuery>,
) -> Result<Json<GbDevicePageResponse>, HttpError> {
    let page = query.page.unwrap_or(1).max(1);
    let page_size = query
        .page_size
        .unwrap_or(DEFAULT_GB_DEVICE_PAGE_SIZE)
        .clamp(1, MAX_GB_DEVICE_PAGE_SIZE);
    let query_session_node_id = query
        .session_node_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let query_domain_id = query
        .domain_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let device_id = query
        .device_id
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    let device_name = query
        .device_name
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    let registered_only = query.registered_only.unwrap_or(false);
    require_role(&state.auth, &headers, Role::Viewer)?;
    let control = BusinessControl::new(state.api.store());
    let (session_node_id, domain_id) = match (query_session_node_id, query_domain_id) {
        (Some(session_node_id), Some(domain_id)) => {
            (session_node_id.to_string(), domain_id.to_string())
        }
        (None, None) => control.first_gb_session_node_by_domain().await?,
        _ => {
            return Err(HttpError::bad_request(
                "session_node_id and domain_id must be provided together",
            ));
        }
    };
    debug!(
        "/api/v2/gb28181/devices, req: session_node_id={session_node_id}, domain_id={domain_id}, device_id={device_id}, device_name={device_name}, registered_only={registered_only}, page={page}, page_size={page_size}"
    );
    let devices = control
        .list_gb_device_page(
            &session_node_id,
            &domain_id,
            device_id,
            device_name,
            registered_only,
            page,
            page_size,
        )
        .await?;
    Ok(Json(gb_device_page_response(devices)))
}

async fn create_gb_device(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<GbDeviceRequest>,
) -> Result<(StatusCode, Json<GbDeviceResponse>), HttpError> {
    log_gb_device_request("/api/v2/gb28181/devices", &request);
    require_write(&state.auth, &headers, Role::Operator)?;
    let device = BusinessControl::new(state.api.store())
        .create_gb_device(gb_device_request(request))
        .await?;
    Ok((StatusCode::CREATED, Json(gb_device_response(device))))
}

async fn gb_device(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
) -> Result<Json<GbDeviceResponse>, HttpError> {
    debug!("/api/v2/gb28181/devices/{{device_id}}, req: device_id={device_id}");
    require_role(&state.auth, &headers, Role::Viewer)?;
    let device = BusinessControl::new(state.api.store())
        .get_gb_device(&device_id)
        .await?
        .ok_or_else(|| GuardError::NotFound(format!("GB28181 device {device_id}")))?;
    Ok(Json(gb_device_response(device)))
}

async fn update_gb_device(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Json(mut request): Json<GbDeviceRequest>,
) -> Result<Json<GbDeviceResponse>, HttpError> {
    request.device_id = device_id.clone();
    log_gb_device_request("/api/v2/gb28181/devices/{device_id}", &request);
    require_write(&state.auth, &headers, Role::Operator)?;
    let device = BusinessControl::new(state.api.store())
        .update_gb_device(gb_device_request(request))
        .await?;
    Ok(Json(gb_device_response(device)))
}

async fn delete_gb_device(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Json(request): Json<GbDeviceDeleteRequest>,
) -> Result<StatusCode, HttpError> {
    debug!(
        "/api/v2/gb28181/devices/{{device_id}}/delete, req: device_id={device_id}, session_node_id={}, domain_id={}",
        request.session_node_id, request.domain_id
    );
    require_write(&state.auth, &headers, Role::Operator)?;
    if request.session_node_id.trim().is_empty() || request.domain_id.trim().is_empty() {
        return Err(HttpError::bad_request(
            "session_node_id and domain_id are required",
        ));
    }
    BusinessControl::new(state.api.store())
        .delete_gb_device(&request.session_node_id, &device_id, &request.domain_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn gb_channels(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Query(query): Query<GbSessionNodeQuery>,
) -> Result<Json<Vec<GbChannelResponse>>, HttpError> {
    debug!(
        "/api/v2/gb28181/devices/{{device_id}}/channels, req: device_id={device_id}, session_node_id={}",
        query.session_node_id
    );
    require_role(&state.auth, &headers, Role::Viewer)?;
    let channels = BusinessControl::new(state.api.store())
        .list_gb_channels_for_session(&query.session_node_id, &device_id)
        .await?;
    Ok(Json(
        channels.into_iter().map(gb_channel_response).collect(),
    ))
}

async fn gb_resources(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Query(query): Query<GbSessionNodeQuery>,
) -> Result<Json<Vec<GbResourceResponse>>, HttpError> {
    debug!(
        "/api/v2/gb28181/devices/{{device_id}}/resources, req: device_id={device_id}, session_node_id={}",
        query.session_node_id
    );
    require_role(&state.auth, &headers, Role::Viewer)?;
    let resources = BusinessControl::new(state.api.store())
        .list_gb_resources_for_session(&query.session_node_id, &device_id)
        .await?;
    Ok(Json(
        resources.into_iter().map(gb_resource_response).collect(),
    ))
}

async fn save_gb_resource_confirmation(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((device_id, resource_id)): Path<(String, String)>,
    Json(request): Json<SaveGbResourceConfirmationBody>,
) -> Result<Json<GbResourceResponse>, HttpError> {
    let session = require_write(&state.auth, &headers, Role::Admin)?;
    if request.request_id.trim().is_empty() {
        return Err(HttpError::bad_request("request_id is required"));
    }
    let control = BusinessControl::new(state.api.store());
    let current = control
        .list_gb_resources(&device_id)
        .await?
        .into_iter()
        .find(|resource| resource.resource_id == resource_id)
        .ok_or_else(|| {
            GuardError::NotFound(format!("GB28181 resource {device_id}/{resource_id}"))
        })?;
    debug!(
        "/api/v2/gb28181/devices/{{device_id}}/resources/{{resource_id}}/confirmation, req: device_id={}, resource_id={}, resource_kind={}, owner_scope={}, owner_id={}, request_id={}, confirmed_by={}",
        device_id,
        resource_id,
        request.resource_kind,
        request.owner_scope,
        request.owner_id,
        request.request_id,
        session.username,
    );
    let operation_id = request.request_id.clone();
    state.api.start_operation(operation_request(
        operation_id.clone(),
        "gb28181.resource_confirmation.save",
        &session,
        Role::Admin,
    ))?;
    let result = control
        .save_gb_resource_confirmation(SaveGbResourceConfirmationRequest {
            device_id,
            resource_id,
            resource_kind: request.resource_kind,
            owner_scope: request.owner_scope,
            owner_id: request.owner_id,
            suggested_enum_id: current.enum_id,
            source_parent_id: current.parent_id,
            confirmed_by: session.username,
            remark: request.remark,
            request_id: request.request_id,
        })
        .await;
    match result {
        Ok(resource) => {
            state
                .api
                .succeed_operation(&operation_id, "resource confirmation saved")?;
            Ok(Json(gb_resource_response(resource)))
        }
        Err(error) => {
            let _ = state.api.fail_operation(&operation_id, error.clone());
            Err(HttpError::from_operation(error, &operation_id))
        }
    }
}

async fn reset_gb_resource_confirmation(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((device_id, resource_id)): Path<(String, String)>,
    Json(request): Json<ResetGbResourceConfirmationBody>,
) -> Result<Json<GbResourceResponse>, HttpError> {
    let session = require_write(&state.auth, &headers, Role::Admin)?;
    if request.request_id.trim().is_empty() {
        return Err(HttpError::bad_request("request_id is required"));
    }
    debug!(
        "/api/v2/gb28181/devices/{{device_id}}/resources/{{resource_id}}/confirmation/reset, req: device_id={}, resource_id={}, request_id={}, confirmed_by={}",
        device_id, resource_id, request.request_id, session.username,
    );
    let operation_id = request.request_id.clone();
    state.api.start_operation(operation_request(
        operation_id.clone(),
        "gb28181.resource_confirmation.reset",
        &session,
        Role::Admin,
    ))?;
    let result = BusinessControl::new(state.api.store())
        .reset_gb_resource_confirmation(ResetGbResourceConfirmationRequest {
            device_id,
            resource_id,
            confirmed_by: session.username,
            request_id: request.request_id,
        })
        .await;
    match result {
        Ok(resource) => {
            state
                .api
                .succeed_operation(&operation_id, "resource confirmation reset")?;
            Ok(Json(gb_resource_response(resource)))
        }
        Err(error) => {
            let _ = state.api.fail_operation(&operation_id, error.clone());
            Err(HttpError::from_operation(error, &operation_id))
        }
    }
}

async fn gb_channel(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((device_id, channel_id)): Path<(String, String)>,
) -> Result<Json<GbChannelResponse>, HttpError> {
    debug!(
        "/api/v2/gb28181/devices/{{device_id}}/channels/{{channel_id}}, req: device_id={device_id}, channel_id={channel_id}"
    );
    require_role(&state.auth, &headers, Role::Viewer)?;
    let channel = BusinessControl::new(state.api.store())
        .get_gb_channel(&device_id, &channel_id)
        .await?
        .ok_or_else(|| GuardError::NotFound(format!("GB28181 channel {device_id}/{channel_id}")))?;
    Ok(Json(gb_channel_response(channel)))
}

async fn update_gb_channel(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((device_id, channel_id)): Path<(String, String)>,
    Json(request): Json<GbChannelRequest>,
) -> Result<Json<GbChannelResponse>, HttpError> {
    debug!(
        "/api/v2/gb28181/devices/{{device_id}}/channels/{{channel_id}}, req: device_id={device_id}, channel_id={channel_id}, body={request:?}"
    );
    require_write(&state.auth, &headers, Role::Operator)?;
    let channel = BusinessControl::new(state.api.store())
        .update_gb_channel(gb_channel_request(device_id, channel_id, request))
        .await?;
    Ok(Json(gb_channel_response(channel)))
}

async fn gb_channel_images(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((device_id, channel_id)): Path<(String, String)>,
    Query(query): Query<GbChannelImageListQuery>,
) -> Result<Json<GbChannelImagePageResponse>, HttpError> {
    debug!(
        "/api/v2/gb28181/devices/{{device_id}}/channels/{{channel_id}}/images, req: device_id={device_id}, channel_id={channel_id}"
    );
    require_role(&state.auth, &headers, Role::Viewer)?;
    let control = BusinessControl::new(state.api.store());
    let session_node_id = if query.session_node_id.trim().is_empty() {
        control
            .get_gb_device(&device_id)
            .await?
            .map(|device| device.session_node_id)
            .filter(|node_id| !node_id.is_empty())
            .ok_or_else(|| GuardError::NotFound(format!("GB28181 device {device_id}")))?
    } else {
        query.session_node_id
    };
    let response = control
        .list_gb_channel_images(
            &session_node_id,
            &device_id,
            &channel_id,
            query.start_time_ms,
            query.end_time_ms,
            query.page,
            query.page_size,
        )
        .await?;
    Ok(Json(GbChannelImagePageResponse {
        items: response
            .images
            .into_iter()
            .map(gb_channel_image_response)
            .collect(),
        total: response.total,
        page: response.page,
        page_size: response.page_size,
    }))
}

async fn issue_gb_channel_image_access(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((device_id, channel_id, image_id)): Path<(String, String, String)>,
    Json(request): Json<GbChannelImageAccessRequest>,
) -> Result<Json<GbChannelImageAccessResponse>, HttpError> {
    require_write(&state.auth, &headers, Role::Viewer)?;
    if request.session_node_id.trim().is_empty() {
        return Err(HttpError::bad_request("session_node_id is required"));
    }
    let operation_id = format!("image-access-{}", Uuid::new_v4());
    let access = BusinessControl::new(state.api.store())
        .issue_gb_channel_image_access(
            &request.session_node_id,
            gmv_protocol::session::v1::IssueGbChannelImageAccessRequest {
                operation: Some(gmv_protocol::common::v1::OperationRef {
                    operation_id,
                    idempotency_key: String::new(),
                }),
                image_id,
                device_id,
                channel_id,
                mode: request.mode,
            },
        )
        .await?;
    Ok(Json(GbChannelImageAccessResponse {
        url: access.url,
        expires_at_ms: access.expires_at_ms,
        content_type: access.content_type,
        file_name: access.file_name,
        file_size: access.file_size,
    }))
}

async fn set_gb_channel_cover(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((device_id, channel_id, image_id)): Path<(String, String, String)>,
    Json(request): Json<GbChannelCoverRequest>,
) -> Result<Json<GbChannelResponse>, HttpError> {
    require_write(&state.auth, &headers, Role::Operator)?;
    if request.session_node_id.trim().is_empty() {
        return Err(HttpError::bad_request("session_node_id is required"));
    }
    let response = BusinessControl::new(state.api.store())
        .set_gb_channel_cover(
            &request.session_node_id,
            gmv_protocol::session::v1::SetGbChannelCoverRequest {
                operation: Some(gmv_protocol::common::v1::OperationRef {
                    operation_id: format!("image-cover-{}", Uuid::new_v4()),
                    idempotency_key: String::new(),
                }),
                device_id,
                channel_id,
                image_id,
            },
        )
        .await?;
    let channel = response.channel.ok_or_else(|| {
        GuardError::Conflict("session returned empty GB28181 channel".to_string())
    })?;
    Ok(Json(gb_channel_response(channel)))
}

async fn gb_channel_records(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((device_id, channel_id)): Path<(String, String)>,
    Query(query): Query<GbRecordListQuery>,
) -> Result<Json<GbChannelRecordsResponse>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    if query.session_node_id.trim().is_empty() {
        return Err(HttpError::bad_request("session_node_id is required"));
    }
    if query.start_time_sec.is_some_and(|value| value <= 0)
        || query.end_time_sec.is_some_and(|value| value <= 0)
    {
        return Err(HttpError::bad_request(
            "record time filters must be positive",
        ));
    }
    if query
        .start_time_sec
        .zip(query.end_time_sec)
        .is_some_and(|(start, end)| start > end)
    {
        return Err(HttpError::bad_request(
            "start_time_sec must not be later than end_time_sec",
        ));
    }
    let page = query.page.unwrap_or(1);
    let page_size = query.page_size.unwrap_or(10);
    if page == 0 || page_size == 0 || page_size > 100 {
        return Err(HttpError::bad_request(
            "page must be positive and page_size must be between 1 and 100",
        ));
    }
    let records = BusinessControl::new(state.api.store())
        .get_gb_channel_records(
            &query.session_node_id,
            &device_id,
            &channel_id,
            query.start_time_sec.unwrap_or_default(),
            query.end_time_sec.unwrap_or_default(),
            page,
            page_size,
        )
        .await?;
    Ok(Json(gb_channel_records_response(records)))
}

async fn query_gb_channel_records(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((device_id, channel_id)): Path<(String, String)>,
    Json(request): Json<GbRecordQueryRequest>,
) -> Result<(StatusCode, Json<GbChannelRecordsResponse>), HttpError> {
    require_write(&state.auth, &headers, Role::Operator)?;
    if request.request_id.trim().is_empty() {
        return Err(HttpError::bad_request("request_id is required"));
    }
    if request.session_node_id.trim().is_empty() {
        return Err(HttpError::bad_request("session_node_id is required"));
    }
    if request.start_time_sec <= 0 || request.end_time_sec <= request.start_time_sec {
        return Err(HttpError::bad_request(
            "record_query_range_required: valid start_time_sec and end_time_sec are required",
        ));
    }
    if request.end_time_sec.saturating_sub(request.start_time_sec) > 366 * 24 * 60 * 60 {
        return Err(HttpError::bad_request(
            "record_query_range_too_large: maximum range is 366 days",
        ));
    }
    let records = BusinessControl::new(state.api.store())
        .query_gb_channel_records(
            &request.session_node_id,
            &request.request_id,
            &device_id,
            &channel_id,
            request.start_time_sec,
            request.end_time_sec,
        )
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(gb_channel_records_response(records)),
    ))
}

async fn gb_snapshot_image(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((device_id, channel_id)): Path<(String, String)>,
    Json(request): Json<GbSnapshotRequest>,
) -> Result<(StatusCode, Json<GbSnapshotResponse>), HttpError> {
    debug!(
        "/api/v2/gb28181/devices/{{device_id}}/channels/{{channel_id}}/images, req: device_id={device_id}, channel_id={channel_id}, body={request:?}"
    );
    let session = require_write(&state.auth, &headers, Role::Operator)?;
    let operation_id = request.request_id.clone();
    state.api.start_operation(operation_request(
        operation_id.clone(),
        "snapshot.image",
        &session,
        Role::Operator,
    ))?;
    let result = BusinessControl::new(state.api.store())
        .snapshot_image(
            &operation_id,
            &device_id,
            &channel_id,
            request.count.map(u32::from).unwrap_or_default(),
            request.interval.map(u32::from).unwrap_or_default(),
        )
        .await;
    match result {
        Ok(session_id) => {
            state
                .api
                .succeed_operation(&operation_id, "snapshot accepted")?;
            Ok((
                StatusCode::ACCEPTED,
                Json(GbSnapshotResponse { session_id }),
            ))
        }
        Err(error) => {
            let _ = state.api.fail_operation(&operation_id, error.clone());
            Err(HttpError::from_operation(error, &operation_id))
        }
    }
}

async fn create_cloud_recording(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((device_id, channel_id)): Path<(String, String)>,
    Json(request): Json<CloudRecordingCreateRequest>,
) -> Result<(StatusCode, Json<CloudRecordingSummary>), HttpError> {
    let session = require_write(&state.auth, &headers, Role::Viewer)?;
    if request.request_id.trim().is_empty() || request.session_node_id.trim().is_empty() {
        return Err(HttpError::bad_request(
            "request_id and session_node_id are required",
        ));
    }
    let recording = BusinessControl::new(state.api.store())
        .create_cloud_recording(CreateCloudRecordingRequest {
            operation: Some(gmv_protocol::common::v1::OperationRef {
                operation_id: request.request_id.clone(),
                idempotency_key: request.request_id.clone(),
            }),
            request_id: request.request_id,
            session_node_id: request.session_node_id,
            device_id,
            channel_id,
            start_time_sec: request.start_time_sec,
            end_time_sec: request.end_time_sec,
            requested_by: session.username,
        })
        .await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(cloud_recording_summary(recording)),
    ))
}

async fn list_cloud_recordings(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((device_id, channel_id)): Path<(String, String)>,
    Query(query): Query<CloudRecordingListQuery>,
) -> Result<Json<CloudRecordingListResponse>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    let (items, total, page, page_size) = BusinessControl::new(state.api.store())
        .list_cloud_recordings(
            &query.session_node_id,
            ListCloudRecordingsRequest {
                device_id,
                channel_id,
                page: query.page,
                page_size: query.page_size,
                include_deleted: false,
            },
        )
        .await?;
    Ok(Json(CloudRecordingListResponse {
        items: items.into_iter().map(cloud_recording_summary).collect(),
        total,
        page,
        page_size,
    }))
}

async fn get_cloud_recording(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> Result<Json<CloudRecordingSummary>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    let recording = BusinessControl::new(state.api.store())
        .get_cloud_recording(&task_id)
        .await?;
    Ok(Json(cloud_recording_summary(recording)))
}

async fn stop_cloud_recording(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
    Json(request): Json<CloudRecordingActionRequest>,
) -> Result<Json<CloudRecordingSummary>, HttpError> {
    require_write(&state.auth, &headers, Role::Operator)?;
    let recording = BusinessControl::new(state.api.store())
        .stop_cloud_recording(&task_id, &request.request_id)
        .await?;
    Ok(Json(cloud_recording_summary(recording)))
}

async fn delete_cloud_recording(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
    Json(request): Json<CloudRecordingActionRequest>,
) -> Result<Json<CloudRecordingSummary>, HttpError> {
    require_write(&state.auth, &headers, Role::Operator)?;
    let recording = BusinessControl::new(state.api.store())
        .delete_cloud_recording(&task_id, &request.request_id)
        .await?;
    Ok(Json(cloud_recording_summary(recording)))
}

async fn issue_cloud_recording_access(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
    Json(request): Json<CloudRecordingAccessRequest>,
) -> Result<Json<CloudRecordingAccessResponse>, HttpError> {
    require_write(&state.auth, &headers, Role::Viewer)?;
    let operation_id = format!("cloud-access-{}", Uuid::new_v4());
    let access = BusinessControl::new(state.api.store())
        .issue_cloud_recording_access(&task_id, &operation_id, &request.mode)
        .await?;
    Ok(Json(CloudRecordingAccessResponse {
        url: access.url,
        expires_at_ms: access.expires_at_ms,
        content_type: access.content_type,
        file_name: access.file_name,
        file_size: access.file_size,
    }))
}

fn cloud_recording_summary(recording: RpcCloudRecordingSummary) -> CloudRecordingSummary {
    let status = CloudRecordingStatus::try_from(recording.status)
        .map(|status| match status {
            CloudRecordingStatus::Starting => "STARTING",
            CloudRecordingStatus::Running => "RUNNING",
            CloudRecordingStatus::Stopping => "STOPPING",
            CloudRecordingStatus::Completed => "COMPLETED",
            CloudRecordingStatus::Stopped => "STOPPED",
            CloudRecordingStatus::Partial => "PARTIAL",
            CloudRecordingStatus::Failed => "FAILED",
            CloudRecordingStatus::Deleting => "DELETING",
            CloudRecordingStatus::Deleted => "DELETED",
            CloudRecordingStatus::Unspecified => "UNSPECIFIED",
        })
        .unwrap_or("UNSPECIFIED")
        .to_string();
    let file_state = CloudRecordingFileState::try_from(recording.file_state)
        .map(|state| match state {
            CloudRecordingFileState::None => "NONE",
            CloudRecordingFileState::Writing => "WRITING",
            CloudRecordingFileState::Ready => "READY",
            CloudRecordingFileState::Missing => "MISSING",
            CloudRecordingFileState::Deleted => "DELETED",
            CloudRecordingFileState::Unspecified => "UNSPECIFIED",
        })
        .unwrap_or("UNSPECIFIED")
        .to_string();
    CloudRecordingSummary {
        task_id: recording.task_id,
        request_id: recording.request_id,
        session_node_id: recording.session_node_id,
        device_id: recording.device_id,
        channel_id: recording.channel_id,
        start_time_sec: recording.start_time_sec,
        end_time_sec: recording.end_time_sec,
        requested_duration_sec: recording.requested_duration_sec,
        status,
        file_state,
        progress_percent: recording.progress_percent,
        recorded_duration_ms: recording.recorded_duration_ms,
        progress_stale: recording.progress_stale,
        current_size_bytes: recording.current_size_bytes,
        final_size_bytes: recording.final_size_bytes,
        file_format: recording.file_format,
        requested_by: recording.requested_by,
        created_at_ms: recording.created_at_ms,
        started_at_ms: recording.started_at_ms,
        finished_at_ms: recording.finished_at_ms,
        updated_at_ms: recording.updated_at_ms,
        error_code: recording.error_code,
        error_message: recording.error_message,
        can_stop: recording.can_stop,
        can_play: recording.can_play,
        can_download: recording.can_download,
        can_delete: recording.can_delete,
    }
}

async fn gb_preview(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((device_id, channel_id)): Path<(String, String)>,
    Json(request): Json<GbStreamRequest>,
) -> Result<(StatusCode, Json<MediaOperationSummary>), HttpError> {
    start_media_operation_http(
        state,
        headers,
        device_id,
        gb_preview_request(channel_id, request),
        DeviceStreamHttpPolicy::output("stream.start", "stream started"),
        |control, operation_id, device_id, channel_id, options| async move {
            control
                .start_live_with_options(&operation_id, &device_id, &channel_id, options)
                .await
        },
    )
    .await
}

async fn gb_playback(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((device_id, channel_id)): Path<(String, String)>,
    Json(mut request): Json<GbStreamRequest>,
) -> Result<(StatusCode, Json<MediaOperationSummary>), HttpError> {
    if request.start_time_sec == 0
        || request.end_time_sec == 0
        || request.start_time_sec >= request.end_time_sec
    {
        return Err(HttpError::bad_request(
            "a valid playback time range is required",
        ));
    }
    if request.playback_id.trim().is_empty() {
        request.playback_id = request.request_id.clone();
    }
    start_media_operation_http(
        state,
        headers,
        device_id,
        gb_preview_request(channel_id, request),
        DeviceStreamHttpPolicy::output("stream.playback", "playback started"),
        |control, operation_id, device_id, channel_id, options| async move {
            control
                .start_playback_with_options(&operation_id, &device_id, &channel_id, options)
                .await
        },
    )
    .await
}

async fn gb_ptz(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((device_id, channel_id)): Path<(String, String)>,
    Json(request): Json<GbPtzRequest>,
) -> Result<Json<base::serde_json::Value>, HttpError> {
    if request.device_id != device_id || request.channel_id != channel_id {
        return Err(HttpError::bad_request("ptz body ids must match path ids"));
    }
    let command = request.control.command()?;
    let speed = request.control.speed(command)?;
    debug!(
        "/api/v2/gb28181/devices/{{device_id}}/channels/{{channel_id}}/ptz, req: device_id={device_id}, channel_id={channel_id}, command={command}, speed={speed}"
    );
    let session = require_write(&state.auth, &headers, Role::Viewer)?;
    let operation_id = format!("ptz-{}", http_now_ms()?);
    state.api.start_operation(operation_request(
        operation_id.clone(),
        "device.ptz",
        &session,
        Role::Viewer,
    ))?;
    let ptz_result = BusinessControl::new(state.api.store())
        .ptz(&operation_id, &device_id, &channel_id, command, speed)
        .await;
    match ptz_result {
        Ok(count) => {
            state.api.succeed_operation(&operation_id, "ptz accepted")?;
            Ok(Json(
                base::serde_json::json!({ "accepted": true, "count": count }),
            ))
        }
        Err(error) => {
            let _ = state.api.fail_operation(&operation_id, error.clone());
            Err(HttpError::from_operation(error, &operation_id))
        }
    }
}

async fn devices(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<DeviceSummary>>, HttpError> {
    debug!("/api/v2/devices, req:<empty>");
    require_role(&state.auth, &headers, Role::Viewer)?;
    let control = BusinessControl::new(state.api.store());
    let devices = control.list_gb_devices().await?;
    let mut summaries = Vec::with_capacity(devices.len());
    for device in devices {
        let channels = control
            .list_gb_channels(&device.device_id)
            .await?
            .into_iter()
            .map(|channel| channel.channel_id)
            .collect();
        summaries.push(DeviceSummary {
            device_id: device.device_id,
            name: if device.alias.is_empty() {
                device.domain
            } else {
                device.alias
            },
            session_node_id: device.session_node_id,
            channels,
            online: device.status != 0 && device.del == 0,
        });
    }
    Ok(Json(summaries))
}

async fn preview(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Json(request): Json<PreviewRequest>,
) -> Result<(StatusCode, Json<MediaOperationSummary>), HttpError> {
    start_media_operation_http(
        state,
        headers,
        device_id,
        request,
        DeviceStreamHttpPolicy::output("stream.start", "stream started"),
        |control, operation_id, device_id, channel_id, options| async move {
            control
                .start_live_with_options(&operation_id, &device_id, &channel_id, options)
                .await
        },
    )
    .await
}

async fn playback(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Json(request): Json<PreviewRequest>,
) -> Result<(StatusCode, Json<StreamSummary>), HttpError> {
    start_device_stream_http(
        state,
        headers,
        device_id,
        request,
        DeviceStreamHttpPolicy::output("stream.playback", "playback started"),
        |control, operation_id, device_id, channel_id, options| async move {
            control
                .start_playback_with_options(&operation_id, &device_id, &channel_id, options)
                .await
        },
    )
    .await
}

async fn download(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Json(request): Json<PreviewRequest>,
) -> Result<(StatusCode, Json<StreamSummary>), HttpError> {
    start_device_stream_http(
        state,
        headers,
        device_id,
        request,
        DeviceStreamHttpPolicy::output("stream.download", "download started"),
        |control, operation_id, device_id, channel_id, options| async move {
            control
                .start_download_with_options(&operation_id, &device_id, &channel_id, options)
                .await
        },
    )
    .await
}

async fn talk(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Json(request): Json<PreviewRequest>,
) -> Result<(StatusCode, Json<StreamSummary>), HttpError> {
    start_device_stream_http(
        state,
        headers,
        device_id,
        request,
        DeviceStreamHttpPolicy::input("device.talk", "talk started"),
        |control, operation_id, device_id, channel_id, options| async move {
            control
                .start_talk_with_options(&operation_id, &device_id, &channel_id, options)
                .await
        },
    )
    .await
}

async fn gb_broadcast(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Json(request): Json<PreviewRequest>,
) -> Result<(StatusCode, Json<StreamSummary>), HttpError> {
    start_device_stream_http(
        state,
        headers,
        device_id,
        request,
        DeviceStreamHttpPolicy::input("device.broadcast", "broadcast started"),
        |control, operation_id, device_id, channel_id, options| async move {
            control
                .start_talk_with_options(&operation_id, &device_id, &channel_id, options)
                .await
        },
    )
    .await
}

struct DeviceStreamHttpPolicy<'a> {
    operation_kind: &'a str,
    success_message: &'a str,
    issue_ticket: bool,
    required_role: Role,
}

impl<'a> DeviceStreamHttpPolicy<'a> {
    fn output(operation_kind: &'a str, success_message: &'a str) -> Self {
        Self {
            operation_kind,
            success_message,
            issue_ticket: true,
            required_role: Role::Viewer,
        }
    }

    fn input(operation_kind: &'a str, success_message: &'a str) -> Self {
        Self {
            operation_kind,
            success_message,
            issue_ticket: false,
            required_role: Role::Operator,
        }
    }
}

async fn start_media_operation_http<F, Fut>(
    state: HttpState,
    headers: HeaderMap,
    device_id: String,
    request: PreviewRequest,
    policy: DeviceStreamHttpPolicy<'_>,
    rpc_start: F,
) -> Result<(StatusCode, Json<MediaOperationSummary>), HttpError>
where
    F: FnOnce(BusinessControl, String, String, String, DeviceStreamOptions) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<StreamSummary, GuardError>> + Send + 'static,
{
    log_preview_request(policy.operation_kind, &device_id, &request);
    let (ui_session_token, session) =
        require_write_with_token(&state.auth, &headers, policy.required_role)?;
    let operation_id = request.request_id.clone();
    if let Ok(existing) = state.api.get_operation(&operation_id) {
        require_operation_owner(&existing, &session)?;
        require_operation_kind(&existing, policy.operation_kind)?;
        let summary = media_operation_summary(existing);
        let status = if summary.state == MediaOperationState::Ready {
            StatusCode::OK
        } else {
            StatusCode::ACCEPTED
        };
        return Ok((status, Json(summary)));
    }
    BusinessControl::new(state.api.store()).validate_live_start()?;
    let hard_timeout_ms =
        media_startup_timeout_ms(request.startup_timeout_ms, FIRST_PREVIEW_HARD_TIMEOUT_MS)?;
    let (existing, created) = state.api.start_operation_once(operation_request(
        operation_id.clone(),
        policy.operation_kind,
        &session,
        policy.required_role,
    ))?;
    if !created {
        let summary = media_operation_summary(existing);
        let status = if summary.state == MediaOperationState::Ready {
            StatusCode::OK
        } else {
            StatusCode::ACCEPTED
        };
        return Ok((status, Json(summary)));
    }
    state.api.configure_media_operation(
        &operation_id,
        "waiting_device_response",
        MEDIA_CHECKPOINT_MS,
        hard_timeout_ms,
    )?;

    let task_state = state.clone();
    let task_operation_id = operation_id.clone();
    let success_message = policy.success_message.to_string();
    let issue_ticket = policy.issue_ticket;
    let channel_id = request.channel_id.clone();
    let options = device_stream_options(&request);
    base::tokio::spawn(async move {
        let control = BusinessControl::new(task_state.api.store());
        let start_result = base::tokio::time::timeout(
            Duration::from_millis(hard_timeout_ms),
            rpc_start(
                control,
                task_operation_id.clone(),
                device_id,
                channel_id,
                options,
            ),
        )
        .await;
        let stream = match start_result {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => {
                let _ = task_state.api.fail_operation(&task_operation_id, error);
                return;
            }
            Err(_) => {
                let error = GuardError::user_visible(
                    "media_startup_timeout",
                    "device media did not become ready before the absolute deadline",
                    "视频仍未启动，请检查设备和网络后重试",
                    true,
                    BTreeMap::new(),
                );
                let _ = task_state.api.fail_operation(&task_operation_id, error);
                return;
            }
        };
        let started_stream = stream.clone();

        if task_state
            .api
            .get_operation(&task_operation_id)
            .is_ok_and(|record| record.status == OperationStatus::Cancelled)
        {
            compensate_stream_start(&task_state, &task_operation_id, &started_stream).await;
            return;
        }

        let stream = if issue_ticket {
            match issue_playback_ticket(
                &task_state,
                stream,
                &ui_session_token,
                &session,
                Role::Viewer,
            ) {
                Ok(stream) => stream,
                Err(error) => {
                    compensate_stream_start(&task_state, &task_operation_id, &started_stream).await;
                    let _ = task_state.api.fail_operation(
                        &task_operation_id,
                        GuardError::Conflict(format!(
                            "playback ticket creation failed: {}",
                            error.message
                        )),
                    );
                    return;
                }
            }
        } else {
            stream
        };
        let published_stream = stream.clone();
        let result = match base::serde_json::to_value(stream) {
            Ok(result) => result,
            Err(error) => {
                compensate_stream_start(&task_state, &task_operation_id, &published_stream).await;
                let _ = task_state.api.fail_operation(
                    &task_operation_id,
                    GuardError::Conflict(format!("stream result serialization failed: {error}")),
                );
                return;
            }
        };
        if task_state
            .api
            .succeed_operation_with_result(&task_operation_id, success_message, result)
            .is_err()
        {
            compensate_stream_start(&task_state, &task_operation_id, &published_stream).await;
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(media_operation_summary(
            state.api.get_operation(&operation_id)?,
        )),
    ))
}

async fn start_device_stream_http<F, Fut>(
    state: HttpState,
    headers: HeaderMap,
    device_id: String,
    request: PreviewRequest,
    policy: DeviceStreamHttpPolicy<'_>,
    rpc_start: F,
) -> Result<(StatusCode, Json<StreamSummary>), HttpError>
where
    F: FnOnce(BusinessControl, String, String, String, DeviceStreamOptions) -> Fut,
    Fut: std::future::Future<Output = Result<StreamSummary, GuardError>>,
{
    log_preview_request(policy.operation_kind, &device_id, &request);
    let (ui_session_token, session) =
        require_write_with_token(&state.auth, &headers, policy.required_role)?;
    let operation_id = request.request_id.clone();
    let (existing, created) = state.api.start_operation_once(operation_request(
        operation_id.clone(),
        policy.operation_kind,
        &session,
        policy.required_role,
    ))?;
    if !created {
        return if existing.status == OperationStatus::Succeeded {
            existing
                .result
                .and_then(|result| base::serde_json::from_value(result).ok())
                .map(|stream| (StatusCode::OK, Json(stream)))
                .ok_or_else(|| {
                    HttpError::from_operation(
                        GuardError::Conflict("stored stream result is unavailable".to_string()),
                        &operation_id,
                    )
                })
        } else {
            Err(HttpError::from_operation(
                existing.error.unwrap_or_else(|| {
                    GuardError::Conflict("stream start is already in progress".to_string())
                }),
                &operation_id,
            ))
        };
    }
    let start_result = rpc_start(
        BusinessControl::new(state.api.store()),
        request.request_id.clone(),
        device_id,
        request.channel_id.clone(),
        device_stream_options(&request),
    )
    .await;
    match start_result {
        Ok(stream) => {
            let started_stream = stream.clone();
            let stream = if policy.issue_ticket {
                match issue_playback_ticket(
                    &state,
                    stream,
                    &ui_session_token,
                    &session,
                    Role::Viewer,
                ) {
                    Ok(stream) => stream,
                    Err(error) => {
                        compensate_stream_start(&state, &operation_id, &started_stream).await;
                        let guard_error = GuardError::Conflict(format!(
                            "playback ticket creation failed: {}",
                            error.message
                        ));
                        let _ = state.api.fail_operation(&operation_id, guard_error.clone());
                        return Err(HttpError::from_operation(guard_error, &operation_id));
                    }
                }
            } else {
                stream
            };
            let stored = match base::serde_json::to_value(&stream) {
                Ok(stored) => stored,
                Err(error) => {
                    compensate_stream_start(&state, &operation_id, &stream).await;
                    return Err(HttpError::internal(format!(
                        "serialize stream result: {error}"
                    )));
                }
            };
            if let Err(error) = state.api.succeed_operation_with_result(
                &operation_id,
                policy.success_message,
                stored,
            ) {
                compensate_stream_start(&state, &operation_id, &stream).await;
                return Err(HttpError::from_operation(error, &operation_id));
            }
            Ok((StatusCode::ACCEPTED, Json(stream)))
        }
        Err(error) => {
            let _ = state.api.fail_operation(&operation_id, error.clone());
            Err(HttpError::from_operation(error, &operation_id))
        }
    }
}

async fn ptz(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Json(request): Json<PtzRequest>,
) -> Result<Json<base::serde_json::Value>, HttpError> {
    let command = request.control.command()?;
    let speed = request.control.speed(command)?;
    debug!(
        "/api/v2/devices/{{device_id}}/ptz, req: device_id={device_id}, channel_id={}, command={command}, speed={speed}",
        request.channel_id
    );
    let session = require_write(&state.auth, &headers, Role::Viewer)?;
    let operation_id = format!("ptz-{}", http_now_ms()?);
    state.api.start_operation(operation_request(
        operation_id.clone(),
        "device.ptz",
        &session,
        Role::Viewer,
    ))?;
    let ptz_result = BusinessControl::new(state.api.store())
        .ptz(
            &operation_id,
            &device_id,
            &request.channel_id,
            command,
            speed,
        )
        .await;
    match ptz_result {
        Ok(count) => {
            state.api.succeed_operation(&operation_id, "ptz accepted")?;
            Ok(Json(
                base::serde_json::json!({ "accepted": true, "count": count }),
            ))
        }
        Err(error) => {
            let _ = state.api.fail_operation(&operation_id, error.clone());
            Err(HttpError::from_operation(error, &operation_id))
        }
    }
}

async fn streams(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<StreamSummary>>, HttpError> {
    debug!("/api/v2/streams, req:<empty>");
    require_role(&state.auth, &headers, Role::Viewer)?;
    Ok(Json(real_streams(&state)))
}

async fn gb_active_streams(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<GbActiveStreamQuery>,
) -> Result<Json<ActiveStreamDialogPage>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    let session_node_id = query.session_node_id.trim();
    if session_node_id.is_empty() {
        return Err(GuardError::InvalidConfig("session_node_id is required".to_string()).into());
    }
    let response = BusinessControl::new(state.api.store())
        .list_active_stream_dialogs(
            session_node_id,
            ListActiveStreamDialogsRequest {
                page: query.page,
                page_size: query.page_size,
                stream_id: query.stream_id,
                stream_node_id: query.stream_node_id,
                device_id: query.device_id,
                channel_id: query.channel_id,
                ssrc: query.ssrc,
                dialog_state: query.dialog_state,
                expected_session: None,
            },
        )
        .await?;
    Ok(Json(ActiveStreamDialogPage {
        items: response
            .items
            .into_iter()
            .map(|item| ActiveStreamDialogItem {
                stream_id: item.stream_id,
                session_node_id: item.session_node_id,
                session_instance_id: item.session_instance_id,
                stream_node_id: item.stream_node_id,
                device_id: item.device_id,
                channel_id: item.channel_id,
                ssrc: item.ssrc,
                dialog_state: item.dialog_state,
                created_at_ms: item.created_at_ms,
                established_at_ms: item.established_at_ms,
                started_at_ms: item.started_at_ms,
                session_type: item.session_type,
            })
            .collect(),
        total: response.total,
        page: response.page,
        page_size: response.page_size,
        server_time_ms: response.server_time_ms,
    }))
}

async fn gb_active_stream_management(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(stream_id): Path<String>,
    Query(query): Query<GbStreamManagementQuery>,
) -> Result<Json<ActiveStreamManagementInfo>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    let session_node_id = query.session_node_id.trim();
    if session_node_id.is_empty() {
        return Err(GuardError::InvalidConfig("session_node_id is required".to_string()).into());
    }
    let response = BusinessControl::new(state.api.store())
        .get_active_stream_management(session_node_id, &stream_id)
        .await?;
    match ActiveStreamManagementState::try_from(response.state) {
        Ok(ActiveStreamManagementState::Active) => {
            let item = response.active.ok_or_else(|| {
                GuardError::Conflict("session omitted active stream management item".to_string())
            })?;
            Ok(Json(ActiveStreamManagementInfo {
                state: "active".to_string(),
                active: Some(ActiveStreamMonitorItem {
                    stream_id: item.stream_id,
                    session_node_id: item.session_node_id,
                    session_instance_id: item.session_instance_id,
                    stream_node_id: item.stream_node_id,
                    device_id: item.device_id,
                    channel_id: item.channel_id,
                    ssrc: item.ssrc,
                    state: item.state,
                    dialog_state: item.dialog_state,
                    media_state: item.media_state,
                    media_ready: item.media_ready,
                    created_at_ms: item.created_at_ms,
                    established_at_ms: item.established_at_ms,
                    started_at_ms: item.started_at_ms,
                    diagnostic_reason: item.diagnostic_reason,
                    session_type: item.session_type,
                    viewer_count: item.viewer_count,
                    viewer_formats: item
                        .viewer_formats
                        .into_iter()
                        .map(|format| ActiveStreamViewerFormat {
                            media_format: format.media_format,
                            viewer_count: format.viewer_count,
                        })
                        .collect(),
                    supported_formats: item.supported_formats,
                    output_format: item.output_format,
                }),
                ended: None,
            }))
        }
        Ok(ActiveStreamManagementState::Ended) => {
            let item = response.ended.ok_or_else(|| {
                GuardError::Conflict("session omitted ended stream management item".to_string())
            })?;
            Ok(Json(ActiveStreamManagementInfo {
                state: "ended".to_string(),
                active: None,
                ended: Some(stream_history_monitor_item(item)),
            }))
        }
        _ => Err(GuardError::Conflict(
            "session returned invalid stream management state".to_string(),
        )
        .into()),
    }
}

async fn gb_stream_history(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<GbStreamHistoryQuery>,
) -> Result<Json<StreamHistoryMonitorPage>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    let session_node_id = query.session_node_id.trim();
    if session_node_id.is_empty() {
        return Err(GuardError::InvalidConfig("session_node_id is required".to_string()).into());
    }
    let response = BusinessControl::new(state.api.store())
        .list_stream_history(
            session_node_id,
            ListStreamHistoryRequest {
                page: query.page,
                page_size: query.page_size,
                stream_id: query.stream_id,
                stream_node_id: query.stream_node_id,
                device_id: query.device_id,
                channel_id: query.channel_id,
                ssrc: query.ssrc,
                state: query.state,
                expected_session: None,
            },
        )
        .await?;
    Ok(Json(StreamHistoryMonitorPage {
        items: response
            .items
            .into_iter()
            .map(stream_history_monitor_item)
            .collect(),
        total: response.total,
        page: response.page,
        page_size: response.page_size,
        server_time_ms: response.server_time_ms,
    }))
}

fn stream_history_monitor_item(
    item: gmv_protocol::session::v1::StreamHistoryItem,
) -> StreamHistoryMonitorItem {
    StreamHistoryMonitorItem {
        stream_id: item.stream_id,
        session_node_id: item.session_node_id,
        stream_node_id: item.stream_node_id,
        device_id: item.device_id,
        channel_id: item.channel_id,
        ssrc: item.ssrc,
        session_type: item.session_type,
        state: item.state,
        created_at_ms: item.created_at_ms,
        established_at_ms: item.established_at_ms,
        terminated_at_ms: item.terminated_at_ms,
        duration_ms: item.duration_ms,
        terminal_reason: item.terminal_reason,
        terminal_reason_label: item.terminal_reason_label,
        error_code: item.error_code,
        legacy_terminal_time: item.legacy_terminal_time,
        stop_reason: item.stop_reason,
    }
}

async fn stop_gb_monitored_stream(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(stream_id): Path<String>,
    Json(request): Json<GbMonitoredStreamStopRequest>,
) -> Result<Json<MonitoredStreamStopResponse>, HttpError> {
    require_write(&state.auth, &headers, Role::Operator)?;
    let stop_reason = request.stop_reason.trim();
    if request.session_node_id.trim().is_empty()
        || request.request_id.trim().is_empty()
        || stop_reason.is_empty()
        || stop_reason.chars().count() > 255
        || stop_reason.contains('\0')
    {
        return Err(GuardError::InvalidConfig(
            "session_node_id, request_id and stop_reason (1..=255 characters) are required"
                .to_string(),
        )
        .into());
    }
    let response = BusinessControl::new(state.api.store())
        .stop_monitored_stream(
            request.session_node_id.trim(),
            request.request_id.trim(),
            &stream_id,
            stop_reason,
        )
        .await?;
    let state = match gmv_protocol::session::v1::DeviceStreamState::try_from(response.state) {
        Ok(gmv_protocol::session::v1::DeviceStreamState::Stopping) => "stopping",
        Ok(gmv_protocol::session::v1::DeviceStreamState::Stopped) => "stopped",
        _ => "unknown",
    };
    Ok(Json(MonitoredStreamStopResponse {
        stream_id: response.stream_id,
        state: state.to_string(),
        session_node_id: response.session_node_id,
        session_instance_id: response.session_instance_id,
    }))
}

async fn list_stream_outputs(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(stream_id): Path<String>,
) -> Result<Json<Vec<StreamOutputSummary>>, HttpError> {
    debug!("/api/v2/streams/{{stream_id}}/outputs, req: stream_id={stream_id}");
    require_role(&state.auth, &headers, Role::Viewer)?;
    let outputs = BusinessControl::new(state.api.store())
        .list_stream_outputs(&stream_id)
        .await?;
    Ok(Json(outputs))
}

async fn create_stream_output(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(stream_id): Path<String>,
    Json(request): Json<CreateStreamOutputRequest>,
) -> Result<(StatusCode, Json<MediaOperationSummary>), HttpError> {
    debug!(
        "/api/v2/streams/{{stream_id}}/outputs, req: stream_id={stream_id}, output_type={}, audio_codec={}",
        request.output_type, request.audio_codec
    );
    let (ui_session_token, session) =
        require_write_with_token(&state.auth, &headers, Role::Operator)?;
    let operation_id = request.request_id;
    if let Ok(existing) = state.api.get_operation(&operation_id) {
        require_operation_owner(&existing, &session)?;
        require_operation_kind(&existing, "stream.output.create")?;
        let summary = media_operation_summary(existing);
        let status = if summary.state == MediaOperationState::Ready {
            StatusCode::OK
        } else {
            StatusCode::ACCEPTED
        };
        return Ok((status, Json(summary)));
    }
    BusinessControl::new(state.api.store())
        .validate_stream_output_target(&stream_id, &request.output_type)?;
    let hard_timeout_ms = media_startup_timeout_ms(
        request.startup_timeout_ms,
        output_startup_timeout_ms(&request.output_type),
    )?;
    let (existing, created) = state.api.start_operation_once(operation_request(
        operation_id.clone(),
        "stream.output.create",
        &session,
        Role::Operator,
    ))?;
    if !created {
        let summary = media_operation_summary(existing);
        let status = if summary.state == MediaOperationState::Ready {
            StatusCode::OK
        } else {
            StatusCode::ACCEPTED
        };
        return Ok((status, Json(summary)));
    }
    state.api.configure_media_operation(
        &operation_id,
        "building_output",
        MEDIA_CHECKPOINT_MS,
        hard_timeout_ms,
    )?;
    let task_state = state.clone();
    let task_operation_id = operation_id.clone();
    base::tokio::spawn(async move {
        let result = base::tokio::time::timeout(
            Duration::from_millis(hard_timeout_ms),
            BusinessControl::new(task_state.api.store()).create_stream_output(
                &task_operation_id,
                &stream_id,
                &request.output_type,
                &request.audio_codec,
                &request.subscription_id,
            ),
        )
        .await;
        let output = match result {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                let _ = task_state.api.fail_operation(&task_operation_id, error);
                return;
            }
            Err(_) => {
                let output_type = request.output_type.trim().to_ascii_lowercase();
                if matches!(output_type.as_str(), "flv" | "fmp4" | "hls" | "ll_hls") {
                    let _ = BusinessControl::new(task_state.api.store())
                        .close_stream_output(
                            &format!("timeout-{task_operation_id}"),
                            &stream_id,
                            &format!("out-{output_type}-{task_operation_id}"),
                        )
                        .await;
                }
                let _ = task_state.api.fail_operation(
                    &task_operation_id,
                    GuardError::user_visible(
                        "media_startup_timeout",
                        "stream output creation exceeded the protocol deadline",
                        "播放方式准备超时，请保持当前方式或稍后重试",
                        true,
                        BTreeMap::new(),
                    ),
                );
                return;
            }
        };
        if task_state
            .api
            .get_operation(&task_operation_id)
            .is_ok_and(|record| record.status == OperationStatus::Cancelled)
        {
            let _ = BusinessControl::new(task_state.api.store())
                .close_stream_output(
                    &format!("cancelled-{task_operation_id}"),
                    &output.stream_id,
                    &output.output_id,
                )
                .await;
            return;
        }
        let output_id = output.output_id.clone();
        let output_stream_id = output.stream_id.clone();
        let output = match issue_stream_output_ticket(
            &task_state,
            output,
            &request.subscription_id,
            &ui_session_token,
            &session,
        ) {
            Ok(output) => output,
            Err(error) => {
                let _ = BusinessControl::new(task_state.api.store())
                    .close_stream_output(
                        &format!("compensate-{task_operation_id}"),
                        &output_stream_id,
                        &output_id,
                    )
                    .await;
                let _ = task_state.api.fail_operation(
                    &task_operation_id,
                    GuardError::Conflict(format!(
                        "stream output ticket creation failed: {}",
                        error.message
                    )),
                );
                return;
            }
        };
        let result = match base::serde_json::to_value(output) {
            Ok(result) => result,
            Err(error) => {
                task_state
                    .api
                    .store()
                    .revoke_playback_tickets_for_output(&output_stream_id, &output_id);
                let _ = BusinessControl::new(task_state.api.store())
                    .close_stream_output(
                        &format!("compensate-{task_operation_id}"),
                        &output_stream_id,
                        &output_id,
                    )
                    .await;
                let _ = task_state.api.fail_operation(
                    &task_operation_id,
                    GuardError::Conflict(format!("output result serialization failed: {error}")),
                );
                return;
            }
        };
        if task_state
            .api
            .succeed_operation_with_result(&task_operation_id, "stream output ready", result)
            .is_err()
            && task_state
                .api
                .get_operation(&task_operation_id)
                .is_ok_and(|record| record.status == OperationStatus::Cancelled)
        {
            let _ = BusinessControl::new(task_state.api.store())
                .close_stream_output(
                    &format!("cancelled-{task_operation_id}"),
                    &output_stream_id,
                    &output_id,
                )
                .await;
        }
    });
    Ok((
        StatusCode::ACCEPTED,
        Json(media_operation_summary(
            state.api.get_operation(&operation_id)?,
        )),
    ))
}

fn output_startup_timeout_ms(output_type: &str) -> u64 {
    if output_type.eq_ignore_ascii_case("hls") || output_type.eq_ignore_ascii_case("ll_hls") {
        12_000
    } else {
        10_000
    }
}

fn media_startup_timeout_ms(
    requested: Option<u64>,
    protocol_default_ms: u64,
) -> Result<u64, HttpError> {
    let Some(requested) = requested else {
        return Ok(protocol_default_ms);
    };
    if !(protocol_default_ms..=30_000).contains(&requested) {
        return Err(HttpError::bad_request(format!(
            "startup_timeout_ms must be between {protocol_default_ms} and 30000"
        )));
    }
    Ok(requested)
}

async fn close_stream_output(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((stream_id, output_id)): Path<(String, String)>,
) -> Result<Json<CloseStreamOutputResponse>, HttpError> {
    debug!(
        "/api/v2/streams/{{stream_id}}/outputs/{{output_id}}/close, req: stream_id={stream_id}, output_id={output_id}"
    );
    let session = require_write(&state.auth, &headers, Role::Operator)?;
    let operation_id = format!("close-output-{output_id}-{}", session.username);
    let (existing, created) = state.api.start_operation_once(operation_request(
        operation_id.clone(),
        "stream.output.close",
        &session,
        Role::Operator,
    ))?;
    if !created {
        return if existing.status == OperationStatus::Succeeded {
            Ok(Json(CloseStreamOutputResponse {
                closed: true,
                output_id,
            }))
        } else {
            Err(HttpError::from_operation(
                existing.error.unwrap_or_else(|| {
                    GuardError::Conflict("stream output close is already in progress".to_string())
                }),
                &operation_id,
            ))
        };
    }
    let result = BusinessControl::new(state.api.store())
        .close_stream_output(&operation_id, &stream_id, &output_id)
        .await;
    match result {
        Ok(closed) => {
            state
                .api
                .store()
                .revoke_playback_tickets_for_output(&stream_id, &output_id);
            state
                .api
                .succeed_operation(&operation_id, "stream output closed")?;
            Ok(Json(CloseStreamOutputResponse { closed, output_id }))
        }
        Err(error) => {
            let _ = state.api.fail_operation(&operation_id, error.clone());
            Err(HttpError::from_operation(error, &operation_id))
        }
    }
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct MediaOperationQuery {
    ids: Option<String>,
}

async fn media_operations(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<MediaOperationQuery>,
) -> Result<Json<Vec<MediaOperationSummary>>, HttpError> {
    let session = require_role(&state.auth, &headers, Role::Viewer)?;
    let requested_ids = query.ids.map(|ids| {
        ids.split(',')
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .take(100)
            .map(str::to_string)
            .collect::<std::collections::HashSet<_>>()
    });
    let operations = state
        .api
        .list_operations()
        .into_iter()
        .filter(|record| record.checkpoint_ms > 0)
        .filter(|record| operation_visible_to(record, &session))
        .filter(|record| {
            requested_ids
                .as_ref()
                .is_none_or(|ids| ids.contains(&record.operation_id))
        })
        .map(media_operation_summary)
        .collect();
    Ok(Json(operations))
}

async fn media_operation(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
) -> Result<Json<MediaOperationSummary>, HttpError> {
    let session = require_role(&state.auth, &headers, Role::Viewer)?;
    let record = state.api.get_operation(&operation_id)?;
    require_operation_owner(&record, &session)?;
    Ok(Json(media_operation_summary(record)))
}

async fn continue_media_operation(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
) -> Result<Json<MediaOperationSummary>, HttpError> {
    let session = require_write(&state.auth, &headers, Role::Viewer)?;
    let record = state.api.get_operation(&operation_id)?;
    require_operation_owner(&record, &session)?;
    Ok(Json(media_operation_summary(record)))
}

async fn cancel_media_operation(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
) -> Result<Json<MediaOperationSummary>, HttpError> {
    let session = require_write(&state.auth, &headers, Role::Viewer)?;
    let record = state.api.get_operation(&operation_id)?;
    require_operation_owner(&record, &session)?;
    Ok(Json(media_operation_summary(
        state.api.cancel_operation(&operation_id)?,
    )))
}

fn operation_visible_to(record: &OperationRecord, session: &UiSession) -> bool {
    session.role == Role::Admin || record.requested_by == session.username
}

fn require_operation_owner(record: &OperationRecord, session: &UiSession) -> Result<(), HttpError> {
    if operation_visible_to(record, session) {
        Ok(())
    } else {
        Err(HttpError::forbidden(
            "media operation belongs to another user",
        ))
    }
}

fn require_operation_kind(record: &OperationRecord, expected: &str) -> Result<(), HttpError> {
    if record.kind == expected {
        Ok(())
    } else {
        Err(GuardError::Conflict(format!(
            "operation {} already belongs to {}",
            record.operation_id, record.kind
        ))
        .into())
    }
}

fn media_operation_summary(record: OperationRecord) -> MediaOperationSummary {
    let terminal = record.status.is_terminal();
    let state = match record.status {
        OperationStatus::Accepted | OperationStatus::Running => MediaOperationState::Preparing,
        OperationStatus::Succeeded => MediaOperationState::Ready,
        OperationStatus::Failed => MediaOperationState::Failed,
        OperationStatus::Cancelled => MediaOperationState::Cancelled,
    };
    let elapsed_ms = if terminal {
        record.updated_at_ms
    } else {
        http_now_ms().unwrap_or(record.updated_at_ms)
    }
    .saturating_sub(record.started_at_ms)
    .max(0) as u64;
    let can_continue = matches!(state, MediaOperationState::Preparing)
        && (record.hard_timeout_ms == 0 || elapsed_ms < record.hard_timeout_ms);
    let error = record.error.as_ref().map(media_operation_error);
    MediaOperationSummary {
        operation_id: record.operation_id,
        state,
        stage: record.stage,
        elapsed_ms,
        last_progress_at_ms: record.updated_at_ms,
        checkpoint_ms: record.checkpoint_ms,
        hard_timeout_ms: record.hard_timeout_ms,
        can_continue,
        result: record.result,
        error,
    }
}

fn media_operation_error(error: &GuardError) -> MediaOperationError {
    match error {
        GuardError::UserVisible {
            code,
            message,
            user_message,
            retryable,
            ..
        } => MediaOperationError {
            code: code.clone(),
            message: message.clone(),
            user_message: user_message.clone(),
            retryable: *retryable,
        },
        _ => MediaOperationError {
            code: "media_operation_failed".to_string(),
            message: error.to_string(),
            user_message: "媒体操作失败，请稍后重试".to_string(),
            retryable: true,
        },
    }
}

async fn media_transport(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<MediaTransportCapability>, HttpError> {
    debug!("/api/v2/media/transport, req:<empty>");
    require_role(&state.auth, &headers, Role::Viewer)?;
    Ok(Json(MediaTransportCapability::from_https_http2_verified(
        state.media_https_http2_verified,
    )))
}

async fn stop_stream(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(stream_id): Path<String>,
) -> Result<Json<StreamSummary>, HttpError> {
    debug!("/api/v2/streams/{{stream_id}}/stop, req: stream_id={stream_id}");
    let session = require_write(&state.auth, &headers, Role::Operator)?;
    let operation_id = format!("stop-{stream_id}-{}", session.username);
    let (existing, created) = state.api.start_operation_once(operation_request(
        operation_id.clone(),
        "stream.stop",
        &session,
        Role::Operator,
    ))?;
    if !created {
        return if existing.status == OperationStatus::Succeeded {
            existing
                .result
                .and_then(|result| base::serde_json::from_value(result).ok())
                .map(Json)
                .ok_or_else(|| {
                    HttpError::from_operation(
                        GuardError::Conflict("stored stop result is unavailable".to_string()),
                        &operation_id,
                    )
                })
        } else {
            Err(HttpError::from_operation(
                existing.error.unwrap_or_else(|| {
                    GuardError::Conflict("stream stop is already in progress".to_string())
                }),
                &operation_id,
            ))
        };
    }
    let stop_result = BusinessControl::new(state.api.store())
        .stop_stream(&operation_id, &stream_id)
        .await;
    match stop_result {
        Ok(stream) => {
            state
                .api
                .store()
                .revoke_playback_tickets_for_stream(&stream_id);
            let result = base::serde_json::to_value(&stream)
                .map_err(|error| HttpError::internal(format!("serialize stop result: {error}")))?;
            state.api.succeed_operation_with_result(
                &operation_id,
                if stream.state == StreamSummaryState::Stopped {
                    "stream stopped"
                } else {
                    "stream stop accepted"
                },
                result,
            )?;
            Ok(Json(stream))
        }
        Err(error) => {
            let _ = state.api.fail_operation(&operation_id, error.clone());
            Err(HttpError::from_operation(error, &operation_id))
        }
    }
}

async fn release_stream(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(stream_id): Path<String>,
    Json(request): Json<ReleaseStreamRequest>,
) -> Result<Json<StreamSummary>, HttpError> {
    debug!(
        "/api/v2/streams/{{stream_id}}/release, req: stream_id={stream_id}, subscription_id={}",
        if request.subscription_id.is_empty() {
            "<empty>"
        } else {
            "<redacted>"
        }
    );
    let (ui_session_token, session) =
        require_write_with_token(&state.auth, &headers, Role::Viewer)?;
    if session.role == Role::Viewer
        && !state
            .api
            .store()
            .playback_tickets_for_subscription(&stream_id, &request.subscription_id)
            .iter()
            .any(|ticket| {
                playback_control_owner_matches(ticket, &session.username, &ui_session_token)
            })
    {
        return Err(HttpError::forbidden(
            "stream subscription belongs to another UI session",
        ));
    }
    let operation_id = request.request_id;
    let (existing, created) = state.api.start_operation_once(operation_request(
        operation_id.clone(),
        "stream.release",
        &session,
        Role::Viewer,
    ))?;
    if !created {
        return if existing.status == OperationStatus::Succeeded {
            existing
                .result
                .and_then(|result| base::serde_json::from_value(result).ok())
                .map(Json)
                .ok_or_else(|| {
                    HttpError::from_operation(
                        GuardError::Conflict("stored release result is unavailable".to_string()),
                        &operation_id,
                    )
                })
        } else {
            Err(HttpError::from_operation(
                existing.error.unwrap_or_else(|| {
                    GuardError::Conflict("stream release is already in progress".to_string())
                }),
                &operation_id,
            ))
        };
    }
    let result = BusinessControl::new(state.api.store())
        .release_stream(&operation_id, &stream_id, &request.subscription_id)
        .await;
    match result {
        Ok(stream) => {
            state
                .api
                .store()
                .revoke_playback_tickets_for_subscription(&stream_id, &request.subscription_id);
            if stream.state == StreamSummaryState::Stopped {
                state
                    .api
                    .store()
                    .revoke_playback_tickets_for_stream(&stream_id);
            }
            let stored = base::serde_json::to_value(&stream).map_err(|error| {
                HttpError::internal(format!("serialize release result: {error}"))
            })?;
            state.api.succeed_operation_with_result(
                &operation_id,
                "stream subscription released",
                stored,
            )?;
            Ok(Json(stream))
        }
        Err(error) => {
            let _ = state.api.fail_operation(&operation_id, error.clone());
            Err(HttpError::from_operation(error, &operation_id))
        }
    }
}

fn require_playback_control(
    state: &HttpState,
    headers: &HeaderMap,
    playback_id: &str,
    stream_id: &str,
) -> Result<PlaybackTicketRecord, HttpError> {
    let (ui_session_token, session) = require_write_with_token(&state.auth, headers, Role::Viewer)?;
    let ticket = state
        .api
        .store()
        .find_playback_control_ticket(playback_id, stream_id)
        .ok_or_else(|| HttpError::forbidden("playback control owner not found"))?;
    if !playback_control_owner_matches(&ticket, &session.username, &ui_session_token) {
        return Err(HttpError::forbidden(
            "playback control belongs to another UI session",
        ));
    }
    Ok(ticket)
}

fn playback_control_owner_matches(
    ticket: &PlaybackTicketRecord,
    username: &str,
    ui_session_token: &str,
) -> bool {
    ticket.username == username
        && (username.starts_with("integration:") || ticket.ui_session_token == ui_session_token)
}

async fn seek_playback(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(playback_id): Path<String>,
    Json(request): Json<PlaybackSeekRequest>,
) -> Result<Json<PlaybackControlHttpResponse>, HttpError> {
    let ticket = require_playback_control(&state, &headers, &playback_id, &request.stream_id)?;
    if request.position_sec < ticket.playback_start_time_sec
        || request.position_sec > ticket.playback_end_time_sec
    {
        return Err(HttpError::bad_request(
            "playback seek position is outside the selected range",
        ));
    }
    let generation = BusinessControl::new(state.api.store())
        .seek_playback(
            &request.request_id,
            &playback_id,
            &request.stream_id,
            request.position_sec,
            request.expected_generation,
        )
        .await?;
    Ok(Json(PlaybackControlHttpResponse {
        accepted: true,
        generation,
    }))
}

async fn set_versioned_playback_speed(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(playback_id): Path<String>,
    Json(request): Json<VersionedPlaybackSpeedRequest>,
) -> Result<Json<PlaybackControlHttpResponse>, HttpError> {
    require_playback_control(&state, &headers, &playback_id, &request.stream_id)?;
    let generation = BusinessControl::new(state.api.store())
        .set_playback_speed_versioned(
            &request.request_id,
            &playback_id,
            &request.stream_id,
            request.speed_rate,
            request.expected_generation,
        )
        .await?;
    Ok(Json(PlaybackControlHttpResponse {
        accepted: true,
        generation,
    }))
}

async fn set_playback_state(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(playback_id): Path<String>,
    Json(request): Json<PlaybackStateRequest>,
) -> Result<Json<PlaybackControlHttpResponse>, HttpError> {
    let ticket = require_playback_control(&state, &headers, &playback_id, &request.stream_id)?;
    let generation = BusinessControl::new(state.api.store())
        .set_playback_state(
            &request.request_id,
            &playback_id,
            &request.stream_id,
            request.paused,
            request.expected_generation,
            &ticket.subscription_id,
        )
        .await?;
    Ok(Json(PlaybackControlHttpResponse {
        accepted: true,
        generation,
    }))
}

async fn heartbeat_playback_presence(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<PlaybackPresenceHeartbeatRequest>,
) -> Result<Json<PlaybackPresenceHeartbeatResponse>, HttpError> {
    const MAX_ITEMS: usize = 64;
    if request.items.is_empty() || request.items.len() > MAX_ITEMS {
        return Err(HttpError::bad_request(format!(
            "playback presence heartbeat items must contain 1..={MAX_ITEMS} entries"
        )));
    }
    let (ui_session_token, session) =
        require_write_with_token(&state.auth, &headers, Role::Operator)?;
    let store = state.api.store();
    let mut rpc_items = Vec::with_capacity(request.items.len());
    for item in request.items {
        let ticket = store
            .find_playback_control_ticket(&item.playback_id, &item.stream_id)
            .filter(|ticket| {
                playback_control_owner_matches(ticket, &session.username, &ui_session_token)
                    && ticket.subscription_id == item.subscription_id
            });
        if ticket.is_some() {
            rpc_items.push(PlaybackPresenceHeartbeat {
                playback_id: item.playback_id,
                stream_id: item.stream_id,
                subscription_id: item.subscription_id,
                generation: item.generation,
            });
        } else {
            return Err(HttpError::forbidden("playback presence owner not found"));
        }
    }
    let (server_time_ms, rpc_results) = BusinessControl::new(store)
        .refresh_playback_presences(rpc_items)
        .await?;
    let items = rpc_results
        .into_iter()
        .map(|item| PlaybackPresenceHeartbeatResult {
            playback_id: item.playback_id,
            stream_id: item.stream_id,
            accepted: item.accepted,
            terminal: item.terminal,
            generation: item.generation,
            presence_deadline_ms: item.presence_deadline_ms,
        })
        .collect();
    Ok(Json(PlaybackPresenceHeartbeatResponse {
        server_time_ms,
        items,
    }))
}

async fn set_playback_speed(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(stream_id): Path<String>,
    Json(request): Json<PlaybackSpeedRequest>,
) -> Result<Json<PlaybackSpeedResponse>, HttpError> {
    debug!(
        "/api/v2/streams/{{stream_id}}/speed, req: stream_id={stream_id}, speed_rate={}",
        request.speed_rate
    );
    let session = require_write(&state.auth, &headers, Role::Operator)?;
    let operation_id = format!("speed-{stream_id}-{}", request.speed_rate);
    state.api.start_operation(operation_request(
        operation_id.clone(),
        "stream.playback_speed",
        &session,
        Role::Operator,
    ))?;
    BusinessControl::new(state.api.store())
        .set_playback_speed(&operation_id, &stream_id, request.speed_rate)
        .await?;
    Ok(Json(PlaybackSpeedResponse {
        accepted: true,
        speed_rate: request.speed_rate,
    }))
}

async fn renew_integration_playback_ticket(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(token): Path<String>,
    Json(request): Json<IntegrationPlaybackRenewRequest>,
) -> Result<Json<IntegrationPlaybackRenewResponse>, HttpError> {
    let (_, session) = require_write_with_token(&state.auth, &headers, Role::Viewer)?;
    if !session.username.starts_with("integration:") {
        return Err(HttpError::forbidden(
            "playback ticket renewal requires integration identity",
        ));
    }
    let store = state.api.store();
    let mut ticket = store
        .get_playback_ticket(&token)
        .ok_or_else(|| GuardError::NotFound("playback ticket".to_string()))?;
    if ticket.username != session.username {
        return Err(HttpError::forbidden("playback ticket owner mismatch"));
    }
    if !request.renew {
        store.revoke_playback_token(&token);
        return Ok(Json(IntegrationPlaybackRenewResponse {
            renewed: false,
            revoked: true,
            expires_at_ms: None,
        }));
    }
    let now_ms = http_now_ms()?;
    if ticket.expires_at_ms <= now_ms {
        store.revoke_playback_token(&token);
        return Err(GuardError::InvalidIdentity("playback ticket expired".to_string()).into());
    }
    let ttl_ms = playback_ticket_ttl_ms(&ticket.username);
    if ticket.renewal_count >= INTEGRATION_PLAYBACK_MAX_RENEWALS
        || now_ms.saturating_add(ttl_ms) > ticket.absolute_expires_at_ms
    {
        store.revoke_playback_token(&token);
        return Err(HttpError::forbidden(
            "playback ticket renewal limit reached",
        ));
    }
    ticket.expires_at_ms = now_ms.saturating_add(ttl_ms);
    ticket.renewal_count = ticket.renewal_count.saturating_add(1);
    state.auth.extend_service_session(
        &ticket.ui_session_token,
        Duration::from_millis(ttl_ms as u64),
    )?;
    let expires_at_ms = ticket.expires_at_ms;
    store.upsert_playback_ticket(ticket);
    Ok(Json(IntegrationPlaybackRenewResponse {
        renewed: true,
        revoked: false,
        expires_at_ms: Some(expires_at_ms),
    }))
}

async fn ai_tasks(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<AiTaskSummary>>, HttpError> {
    debug!("/api/v2/ai/tasks, req:<empty>");
    require_role(&state.auth, &headers, Role::Viewer)?;
    Ok(Json(real_ai_tasks(&state)))
}

async fn start_ai_task(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<StartAiRequest>,
) -> Result<(StatusCode, Json<AiTaskSummary>), HttpError> {
    debug!("/api/v2/ai/tasks, req:{request:?}");
    let session = require_write(&state.auth, &headers, Role::Operator)?;
    let operation_id = request.request_id.clone();
    state.api.start_operation(operation_request(
        operation_id.clone(),
        "ai.start",
        &session,
        Role::Operator,
    ))?;
    let start_result = BusinessControl::new(state.api.store())
        .start_ai(&request.request_id, &request.stream_id, &request.model)
        .await;
    match start_result {
        Ok(task) => {
            state
                .api
                .succeed_operation(&operation_id, "ai task started")?;
            Ok((StatusCode::ACCEPTED, Json(task)))
        }
        Err(error) => {
            let _ = state.api.fail_operation(&operation_id, error.clone());
            Err(HttpError::from_operation(error, &operation_id))
        }
    }
}

async fn cancel_ai_task(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> Result<Json<AiTaskSummary>, HttpError> {
    debug!("/api/v2/ai/tasks/{{task_id}}/cancel, req: task_id={task_id}");
    let session = require_write(&state.auth, &headers, Role::Operator)?;
    let operation_id = format!("cancel-{task_id}");
    state.api.start_operation(operation_request(
        operation_id.clone(),
        "ai.cancel",
        &session,
        Role::Operator,
    ))?;
    let cancel_result = BusinessControl::new(state.api.store())
        .cancel_ai(&operation_id, &task_id)
        .await;
    match cancel_result {
        Ok(task) => {
            state
                .api
                .succeed_operation(&operation_id, "ai task cancelled")?;
            Ok(Json(task))
        }
        Err(error) => {
            let _ = state.api.fail_operation(&operation_id, error.clone());
            Err(HttpError::from_operation(error, &operation_id))
        }
    }
}

async fn runtime_status(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<RuntimeStatus>, HttpError> {
    debug!("/api/v2/runtime/status, req:<empty>");
    require_role(&state.auth, &headers, Role::Viewer)?;
    Ok(Json(real_status(&state)))
}

fn real_streams(state: &HttpState) -> Vec<StreamSummary> {
    let store = state.api.store();
    let leases = store.leases();
    store
        .routes()
        .into_iter()
        .filter(|route| !route.resource_id.starts_with("ai-"))
        .map(|route| {
            let owner = store.get_stream_session_owner(&route.resource_id);
            let lease = leases
                .iter()
                .find(|lease| lease.resource_id == route.resource_id);
            StreamSummary {
                stream_id: route.resource_id,
                device_id: String::new(),
                channel_id: String::new(),
                node_id: route.node_id,
                instance_id: route.instance_id,
                lease_id: lease
                    .map(|lease| lease.lease_id.clone())
                    .unwrap_or_default(),
                route_id: route.route_id,
                endpoint: String::new(),
                video_codec: String::new(),
                audio_codec: String::new(),
                subscription_id: String::new(),
                session_node_id: owner
                    .as_ref()
                    .map(|owner| owner.node_id.clone())
                    .unwrap_or_default(),
                session_instance_id: owner.map(|owner| owner.instance_id).unwrap_or_default(),
                playback_id: String::new(),
                playback_generation: 0,
                playback_start_time_sec: 0,
                playback_end_time_sec: 0,
                state: if route.state == RouteState::Closed {
                    StreamSummaryState::Stopped
                } else if lease
                    .map(|lease| {
                        lease.state == LeaseState::Failed || lease.state == LeaseState::Expired
                    })
                    .unwrap_or(false)
                {
                    StreamSummaryState::Failed
                } else {
                    StreamSummaryState::Running
                },
            }
        })
        .collect()
}

fn real_ai_tasks(state: &HttpState) -> Vec<AiTaskSummary> {
    let store = state.api.store();
    let leases = store.leases();
    store
        .routes()
        .into_iter()
        .filter(|route| route.resource_id.starts_with("ai-"))
        .map(|route| {
            let lease = leases
                .iter()
                .find(|lease| lease.resource_id == route.resource_id);
            AiTaskSummary {
                task_id: route.resource_id,
                model: String::new(),
                stream_id: String::new(),
                node_id: route.node_id,
                instance_id: route.instance_id,
                lease_id: lease
                    .map(|lease| lease.lease_id.clone())
                    .unwrap_or_default(),
                route_id: route.route_id,
                state: if lease
                    .map(|lease| {
                        lease.state == LeaseState::Failed || lease.state == LeaseState::Expired
                    })
                    .unwrap_or(false)
                {
                    AiTaskSummaryState::Failed
                } else if route.state == RouteState::Closed {
                    AiTaskSummaryState::Cancelled
                } else {
                    AiTaskSummaryState::Running
                },
            }
        })
        .collect()
}

fn real_status(state: &HttpState) -> RuntimeStatus {
    let streams = real_streams(state);
    let ai_tasks = real_ai_tasks(state);
    RuntimeStatus {
        guard_available: true,
        streams: streams.len(),
        running_streams: streams
            .iter()
            .filter(|stream| stream.state == StreamSummaryState::Running)
            .count(),
        ai_tasks: ai_tasks.len(),
        running_ai_tasks: ai_tasks
            .iter()
            .filter(|task| task.state == AiTaskSummaryState::Running)
            .count(),
        ptz_commands: 0,
    }
}

fn require_role(auth: &AuthState, headers: &HeaderMap, role: Role) -> Result<UiSession, HttpError> {
    let session = authenticated(auth, headers)?;
    auth.require_role(&session, role)
        .map_err(|_| HttpError::forbidden("UI role is not allowed"))?;
    Ok(session)
}

fn require_write(
    auth: &AuthState,
    headers: &HeaderMap,
    role: Role,
) -> Result<UiSession, HttpError> {
    require_write_with_token(auth, headers, role).map(|(_, session)| session)
}

fn require_write_with_token(
    auth: &AuthState,
    headers: &HeaderMap,
    role: Role,
) -> Result<(String, UiSession), HttpError> {
    verify_origin(auth, headers)?;
    let (token, session) = authenticated_with_token(auth, headers)?;
    auth.require_role(&session, role)
        .map_err(|_| HttpError::forbidden("UI role is not allowed"))?;
    verify_csrf(auth, &session, headers)?;
    Ok((token, session))
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct ErrorResponse {
    code: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retryable: Option<bool>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    details: BTreeMap<String, String>,
}

struct HttpError {
    status: StatusCode,
    code: String,
    message: String,
    user_message: Option<String>,
    retryable: Option<bool>,
    details: BTreeMap<String, String>,
}

impl HttpError {
    fn from_operation(error: GuardError, operation_id: &str) -> Self {
        let mut error = Self::from(error);
        error
            .details
            .insert("operation_id".to_string(), operation_id.to_string());
        error
    }

    fn unauthorized() -> Self {
        let code = GmvGuardErrorCode::Unauthorized;
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: code.api_code().to_string(),
            message: "authentication required".to_string(),
            user_message: Some(code.out_msg().to_string()),
            retryable: Some(code.retryable()),
            details: BTreeMap::new(),
        }
    }

    fn forbidden(message: impl Into<String>) -> Self {
        let message = message.into();
        let code = if message == "invalid CSRF token" {
            GmvGuardErrorCode::CsrfInvalid
        } else {
            GmvGuardErrorCode::Forbidden
        };
        Self {
            status: StatusCode::FORBIDDEN,
            code: code.api_code().to_string(),
            message,
            user_message: Some(code.out_msg().to_string()),
            retryable: Some(code.retryable()),
            details: BTreeMap::new(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        let code = GmvGuardErrorCode::Internal;
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: code.api_code().to_string(),
            message: message.into(),
            user_message: Some(code.out_msg().to_string()),
            retryable: Some(code.retryable()),
            details: BTreeMap::new(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        let code = GmvGuardErrorCode::BadRequest;
        Self {
            status: StatusCode::BAD_REQUEST,
            code: code.api_code().to_string(),
            message: message.into(),
            user_message: Some(code.out_msg().to_string()),
            retryable: Some(code.retryable()),
            details: BTreeMap::new(),
        }
    }

    fn from_auth(error: GuardError) -> Self {
        match error {
            GuardError::Capacity(message) => Self {
                status: StatusCode::TOO_MANY_REQUESTS,
                code: GmvGuardErrorCode::RateLimited.api_code().to_string(),
                message,
                user_message: Some(GmvGuardErrorCode::RateLimited.out_msg().to_string()),
                retryable: Some(GmvGuardErrorCode::RateLimited.retryable()),
                details: BTreeMap::new(),
            },
            GuardError::InvalidIdentity(_) => Self::unauthorized(),
            other => other.into(),
        }
    }
}

impl From<GuardError> for HttpError {
    fn from(error: GuardError) -> Self {
        if let GuardError::UserVisible {
            code,
            message,
            user_message,
            retryable,
            details,
        } = error
        {
            return Self {
                status: status_for_error_code(&code),
                code,
                message,
                user_message: Some(user_message),
                retryable: Some(retryable),
                details,
            };
        }

        let status = match &error {
            GuardError::InvalidConfig(_) | GuardError::InvalidIdentity(_) => {
                StatusCode::BAD_REQUEST
            }
            GuardError::Conflict(_)
            | GuardError::DuplicateEvent(_)
            | GuardError::StaleInstance(_) => StatusCode::CONFLICT,
            GuardError::NotFound(_) => StatusCode::NOT_FOUND,
            GuardError::Capacity(_) => StatusCode::TOO_MANY_REQUESTS,
            GuardError::TimeUnsynced(_) => StatusCode::SERVICE_UNAVAILABLE,
            GuardError::UserVisible { .. } => unreachable!(),
        };
        let code = code_for_guard_error(&error);
        let user_message = user_message_for_guard_error(&error);
        let retryable = retryable_for_guard_error(&error);
        Self {
            status,
            code,
            message: error.to_string(),
            user_message: Some(user_message),
            retryable: Some(retryable),
            details: BTreeMap::new(),
        }
    }
}

impl From<GlobalError> for HttpError {
    fn from(error: GlobalError) -> Self {
        let output = err::global_error_output(&error);
        let guard_code = GmvGuardErrorCode::from_code(output.code);
        let status = guard_code
            .map(status_for_guard_error_code)
            .unwrap_or_else(|| status_for_global_code(output.code));
        let code = guard_code
            .map(|code| code.api_code().to_string())
            .unwrap_or_else(|| output.code_name.to_string());
        Self {
            status,
            code,
            message: error.to_string(),
            user_message: Some(output.user_message.into_owned()),
            retryable: Some(output.retryable),
            details: BTreeMap::new(),
        }
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                code: self.code,
                message: self.message,
                user_message: self.user_message,
                operation_id: self.details.get("operation_id").cloned(),
                trace_id: self.details.get("trace_id").cloned(),
                retryable: self.retryable,
                details: self.details,
            }),
        )
            .into_response()
    }
}

fn status_for_error_code(code: &str) -> StatusCode {
    GmvGuardErrorCode::from_api_code(code)
        .map(status_for_guard_error_code)
        .unwrap_or(StatusCode::CONFLICT)
}

fn status_for_guard_error_code(code: GmvGuardErrorCode) -> StatusCode {
    match code {
        GmvGuardErrorCode::NodeRpcTimeout | GmvGuardErrorCode::StreamInputTimeout => {
            StatusCode::GATEWAY_TIMEOUT
        }
        GmvGuardErrorCode::NodeRpcConnectFailed
        | GmvGuardErrorCode::NodeRpcTlsFailed
        | GmvGuardErrorCode::NodeRpcUnavailable
        | GmvGuardErrorCode::NodeUnavailable
        | GmvGuardErrorCode::TimeUnsynced => StatusCode::SERVICE_UNAVAILABLE,
        GmvGuardErrorCode::NodeNotFound
        | GmvGuardErrorCode::NodeEndpointMissing
        | GmvGuardErrorCode::NotFound => StatusCode::NOT_FOUND,
        GmvGuardErrorCode::CapacityExceeded | GmvGuardErrorCode::RateLimited => {
            StatusCode::TOO_MANY_REQUESTS
        }
        GmvGuardErrorCode::BadRequest | GmvGuardErrorCode::InvalidArgument => {
            StatusCode::BAD_REQUEST
        }
        GmvGuardErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
        GmvGuardErrorCode::Forbidden | GmvGuardErrorCode::CsrfInvalid => StatusCode::FORBIDDEN,
        GmvGuardErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        GmvGuardErrorCode::Conflict
        | GmvGuardErrorCode::StaleInstance
        | GmvGuardErrorCode::DuplicateEvent
        | GmvGuardErrorCode::PtzRejected
        | GmvGuardErrorCode::SnapshotRejected => StatusCode::CONFLICT,
    }
}

fn status_for_global_code(code: u16) -> StatusCode {
    match code {
        1210 => StatusCode::GATEWAY_TIMEOUT,
        1220 | 1230 => StatusCode::SERVICE_UNAVAILABLE,
        1140 => StatusCode::NOT_FOUND,
        1150 | 1190 => StatusCode::BAD_REQUEST,
        1170 => StatusCode::UNAUTHORIZED,
        1180 => StatusCode::FORBIDDEN,
        1240 => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::CONFLICT,
    }
}

fn code_for_guard_error(error: &GuardError) -> String {
    match error {
        GuardError::InvalidConfig(_) => "bad_request",
        GuardError::InvalidIdentity(_) => "bad_request",
        GuardError::Conflict(_) => "conflict",
        GuardError::NotFound(message) if message.contains("node") => "node_not_found",
        GuardError::NotFound(_) => "not_found",
        GuardError::StaleInstance(_) => "stale_instance",
        GuardError::Capacity(_) => "capacity_exceeded",
        GuardError::TimeUnsynced(_) => "time_unsynced",
        GuardError::DuplicateEvent(_) => "duplicate_event",
        GuardError::UserVisible { code, .. } => code.as_str(),
    }
    .to_string()
}

fn user_message_for_guard_error(error: &GuardError) -> String {
    let code = match error {
        GuardError::InvalidConfig(_) | GuardError::InvalidIdentity(_) => {
            GmvGuardErrorCode::BadRequest
        }
        GuardError::Conflict(message) if message.contains("offline") => {
            GmvGuardErrorCode::NodeUnavailable
        }
        GuardError::Conflict(_) | GuardError::DuplicateEvent(_) | GuardError::StaleInstance(_) => {
            GmvGuardErrorCode::Conflict
        }
        GuardError::NotFound(message) if message.contains("node") => {
            GmvGuardErrorCode::NodeNotFound
        }
        GuardError::NotFound(_) => GmvGuardErrorCode::NotFound,
        GuardError::Capacity(_) => GmvGuardErrorCode::CapacityExceeded,
        GuardError::TimeUnsynced(_) => GmvGuardErrorCode::TimeUnsynced,
        GuardError::UserVisible { user_message, .. } => return user_message.clone(),
    };
    code.out_msg().to_string()
}

fn retryable_for_guard_error(error: &GuardError) -> bool {
    match error {
        GuardError::UserVisible { retryable, .. } => *retryable,
        _ => GmvGuardErrorCode::from_api_code(&code_for_guard_error(error))
            .map(|code| code.retryable())
            .unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GbStreamRequest, GuardError, HttpError, OPEN_BUSINESS_OPERATIONS, api_docs_contract_page,
        endpoint_with_playback_token, gb_preview_request, media_startup_timeout_ms,
        node_connection_label, node_health_label, node_scheduling_label, open_business_scope,
        openapi_operation_parameters, openapi_operation_summary, openapi_request_body,
        openapi_responses, playback_control_owner_matches, playback_token_from_endpoint,
    };
    use crate::auth::Role;
    use crate::core::{ConnectionState, HealthState, SchedulingState};
    use crate::store::model::PlaybackTicketRecord;
    use axum::http::Method;

    #[test]
    fn playback_control_ownership_does_not_expire_with_media_url_ticket() {
        let ticket = PlaybackTicketRecord {
            token: "media-token".to_string(),
            stream_id: "stream-1".to_string(),
            playback_id: "playback-1".to_string(),
            playback_start_time_sec: 100,
            playback_end_time_sec: 200,
            output_id: String::new(),
            subscription_id: "subscription-1".to_string(),
            lease_id: "lease-1".to_string(),
            route_id: "route-1".to_string(),
            username: "operator".to_string(),
            ui_session_token: "ui-session".to_string(),
            required_role: Role::Operator,
            issued_at_ms: 0,
            expires_at_ms: 0,
            absolute_expires_at_ms: i64::MAX,
            renewal_count: 0,
        };

        assert!(playback_control_owner_matches(
            &ticket,
            "operator",
            "ui-session"
        ));
        assert!(!playback_control_owner_matches(
            &ticket,
            "operator",
            "another-session"
        ));
    }

    #[test]
    fn node_state_labels_cover_all_api_states() {
        assert_eq!(node_connection_label(ConnectionState::Connected), "已连接");
        assert_eq!(
            node_connection_label(ConnectionState::Disconnected),
            "已断开"
        );
        assert_eq!(node_connection_label(ConnectionState::Superseded), "已替代");

        assert_eq!(node_health_label(HealthState::Starting), "启动中");
        assert_eq!(node_health_label(HealthState::Ready), "就绪");
        assert_eq!(node_health_label(HealthState::Degraded), "降级");
        assert_eq!(node_health_label(HealthState::Draining), "排空中");
        assert_eq!(node_health_label(HealthState::Offline), "离线");

        assert_eq!(node_scheduling_label(SchedulingState::Enabled), "可调度");
        assert_eq!(node_scheduling_label(SchedulingState::Disabled), "不可调度");
        assert_eq!(
            node_scheduling_label(SchedulingState::TimeUnsynced),
            "时间未同步"
        );
    }

    #[test]
    fn playback_ticket_replaces_internal_subscription_token() {
        let endpoint = endpoint_with_playback_token(
            "https://media.example/stream.flv?quality=main&gmv-token=subscription",
            "ticket",
        );

        assert_eq!(
            endpoint,
            "https://media.example/stream.flv?quality=main&gmv-token=ticket"
        );
        assert_eq!(playback_token_from_endpoint(&endpoint), Some("ticket"));
    }

    #[test]
    fn operation_error_keeps_operation_id_for_http_response() {
        let error = HttpError::from_operation(
            GuardError::Conflict("remote rejection".to_string()),
            "op-123",
        );
        assert_eq!(
            error.details.get("operation_id").map(String::as_str),
            Some("op-123")
        );
    }

    #[test]
    fn startup_timeout_override_cannot_shorten_protocol_default_or_exceed_cap() {
        assert!(matches!(media_startup_timeout_ms(None, 12_000), Ok(12_000)));
        assert!(matches!(
            media_startup_timeout_ms(Some(20_000), 12_000),
            Ok(20_000)
        ));
        assert!(media_startup_timeout_ms(Some(11_999), 12_000).is_err());
        assert!(media_startup_timeout_ms(Some(30_001), 12_000).is_err());
    }

    #[test]
    fn gb_stream_request_preserves_explicit_session_node() {
        let request = gb_preview_request(
            "channel-1".to_string(),
            GbStreamRequest {
                request_id: "request-1".to_string(),
                session_node_id: "session-b".to_string(),
                token: String::new(),
                start_time_sec: 0,
                end_time_sec: 0,
                trans_mode: String::new(),
                output_type: "flv".to_string(),
                audio_codec: "aac".to_string(),
                startup_timeout_ms: None,
                playback_id: String::new(),
            },
        );

        assert_eq!(request.session_node_id, "session-b");
    }

    #[test]
    fn open_business_contract_registry_is_scoped_and_excludes_management_paths() {
        let mut operations = std::collections::HashSet::new();
        for (path, methods) in OPEN_BUSINESS_OPERATIONS {
            assert!(!path.contains("/users"));
            assert!(!path.contains("/integrations"));
            assert!(!path.contains("/system"));
            for method in *methods {
                let method = match *method {
                    "get" => Method::GET,
                    "post" => Method::POST,
                    other => panic!("unexpected public HTTP method: {other}"),
                };
                assert!(open_business_scope(&method, path).is_some());
                assert!(
                    operations.insert((method, *path)),
                    "duplicate public operation"
                );
            }
        }
        assert!(operations.contains(&(Method::POST, "/playback-tickets/{token}/renew")));
    }

    #[test]
    fn public_http_contract_uses_chinese_descriptions() {
        fn contains_chinese(value: &str) -> bool {
            value
                .chars()
                .any(|character| ('\u{4e00}'..='\u{9fff}').contains(&character))
        }

        for (path, methods) in OPEN_BUSINESS_OPERATIONS {
            for method in *methods {
                let summary = openapi_operation_summary(method, path);
                assert!(
                    contains_chinese(summary),
                    "operation summary must be Chinese: {method} {path}"
                );
                assert_ne!(
                    summary, "查询 Guard 业务数据",
                    "missing GET summary: {path}"
                );
                assert_ne!(
                    summary, "提交 Guard 业务操作",
                    "missing POST summary: {path}"
                );
                for parameter in openapi_operation_parameters(method, path) {
                    assert!(
                        parameter["description"]
                            .as_str()
                            .is_some_and(contains_chinese),
                        "parameter description must be Chinese: {method} {path}"
                    );
                    assert_ne!(
                        parameter["description"].as_str(),
                        Some("业务字段。"),
                        "parameter description must be specific: {method} {path}"
                    );
                }
                if let Some(request_body) = openapi_request_body(method, path) {
                    for property in
                        request_body["content"]["application/json"]["schema"]["properties"]
                            .as_object()
                            .into_iter()
                            .flatten()
                            .map(|(_, property)| property)
                    {
                        assert!(
                            property["description"]
                                .as_str()
                                .is_some_and(contains_chinese),
                            "request field description must be Chinese: {method} {path}"
                        );
                        assert_ne!(
                            property["description"].as_str(),
                            Some("业务字段。"),
                            "request field description must be specific: {method} {path}"
                        );
                    }
                }
            }
        }
        assert!(
            openapi_responses("查询业务数据")["200"]["description"]
                .as_str()
                .is_some_and(contains_chinese)
        );
    }

    #[test]
    fn online_contract_pages_keep_raw_json_entries() {
        let http = api_docs_contract_page(
            "http",
            "HTTP 三方接入文档",
            "HTTP 契约",
            "/api-docs/openapi.json",
        );
        let mqtt = api_docs_contract_page(
            "mqtt",
            "MQTT 三方接入文档",
            "MQTT 契约",
            "/api-docs/asyncapi.json",
        );

        assert!(http.contains("data-mode=\"http\""));
        assert!(http.contains("/api-docs/openapi.json"));
        assert!(mqtt.contains("data-mode=\"mqtt\""));
        assert!(mqtt.contains("/api-docs/asyncapi.json"));
        assert!(include_str!("api_docs.js").contains("查看原始 JSON 定义"));
    }
}
