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
    SaveGbResourceConfirmationRequest, StreamProfileVerification, VideoStreamProfile,
};
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::set_header::SetResponseHeaderLayer;
use uuid::Uuid;

use crate::api::v2::control::{
    BroadcastOperationOptions, BroadcastTargetOptions, BusinessControl, DeviceStreamOptions,
    GbDevicePage, GbSessionConfigSummary,
};
use crate::api::v2::model::{
    ActiveStreamDialogItem, ActiveStreamDialogPage, ActiveStreamManagementInfo,
    ActiveStreamMonitorItem, ActiveStreamViewerFormat, AiTaskSummary, AiTaskSummaryState,
    BroadcastOperationSummary, DeviceSummary, MediaOperationError, MediaOperationState,
    MediaOperationSummary, MediaTransportCapability, MonitoredStreamStopResponse, RuntimeStatus,
    StreamHistoryMonitorItem, StreamHistoryMonitorPage, StreamOutputSummary, StreamSummary,
    StreamSummaryState,
};
use crate::api::v2::paths;
use crate::api::v2::{ApiV2, CursorQuery, EventQuery};
use crate::auth::session::{SESSION_COOKIE, cookie_value};
use crate::auth::{
    AuthState, Role, UiSession, UserAccess, UserProfile, hash_password as hash_ui_password,
};
use crate::bus::router::topic_matches;
use crate::core::{
    ConnectionState, GmvGuardErrorCode, GuardError, GuardResult, HealthState, LeaseState,
    RouteState, SchedulingState,
};
use crate::integration::hmac::{HmacNonceCache, SignedRequest, body_sha256, verify_request};
use crate::integration::model::{
    CredentialPurpose, CredentialStatus, Integration, IntegrationAudit, IntegrationCredential,
    IntegrationCredentialSummary, IntegrationHttpConfig, IntegrationMapping, IntegrationMqttConfig,
    IntegrationTransport, MqttRuntimeApplyState,
};
use crate::integration::secret::{IntegrationSecretCipher, IntegrationSecretManager};
use crate::integration::{IntegrationPrincipal, principal as integration_principal};
use crate::operation::OperationRequest;
use crate::operation::{OperationRecord, OperationStatus};
use crate::outbox::OutboxRepository;
use crate::runtime::event_forwarder::EventForwarder;
use crate::store::InMemoryGuardStore;
use crate::store::command::{HttpCommandClaim, http_command_id, validate_request_id};
use crate::store::model::{
    EventRecord, INTEGRATION_PLAYBACK_MAX_LIFETIME_MS, INTEGRATION_PLAYBACK_MAX_RENEWALS,
    INTEGRATION_PLAYBACK_TOKEN_TTL_MS, LeaseRecord, NodeRecord, OutboxDestinationKind,
    OutboxRecord, OutboxState, PLAYBACK_TOKEN_TTL_MS, PlaybackTicketRecord,
};
use crate::store::persistent::{CommandRepository, IntegrationRepository, UserRepository};

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
const REQUEST_ID_HEADER: &str = "x-gmv-request-id";
const MAX_OPEN_API_BODY_BYTES: usize = 1024 * 1024;
const MAX_OPEN_API_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const HTTP_IDEMPOTENCY_TTL_MS: i64 = 86_400_000;

#[derive(Debug, Clone)]
pub struct HttpState {
    pub api: ApiV2,
    pub auth: AuthState,
    pub outbox: OutboxRepository,
    pub users: Option<UserRepository>,
    pub integrations: Option<IntegrationRepository>,
    pub commands: Option<CommandRepository>,
    pub integration_secrets: Option<IntegrationSecretManager>,
    pub integration_nonces: HmacNonceCache,
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
            "/integrations/business",
            get(get_business_integration).post(save_business_integration),
        )
        .route("/integrations/master-key", get(integration_master_key))
        .route(
            "/integrations/master-key/rotate",
            post(rotate_integration_master_key),
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
            "/integrations/{integration_id}/credentials/{credential_id}/reveal",
            post(reveal_integration_credential),
        )
        .route(
            "/integrations/{integration_id}/http",
            get(get_integration_http).post(update_integration_http),
        )
        .route(
            "/integrations/{integration_id}/mqtt",
            get(get_integration_mqtt),
        )
        .route(
            "/integrations/business/mqtt/runtime",
            get(integration_mqtt_runtime).post(update_integration_mqtt_runtime),
        )
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
        .route("/gb28181/broadcasts/start", post(start_broadcast_operation))
        .route(
            "/gb28181/broadcasts/{broadcast_id}",
            get(get_broadcast_operation),
        )
        .route(
            "/gb28181/broadcasts/{broadcast_id}/targets/{leg_id}/stop",
            post(stop_broadcast_target),
        )
        .route(
            "/gb28181/broadcasts/{broadcast_id}/stop-all",
            post(stop_broadcast_operation),
        )
        .route("/devices", get(devices))
        .route("/devices/{device_id}/preview", post(preview))
        .route("/devices/{device_id}/playback", post(playback))
        .route("/devices/{device_id}/download", post(download))
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
    ("/gb28181/broadcasts/start", &["post"]),
    ("/gb28181/broadcasts/{broadcast_id}", &["get"]),
    (
        "/gb28181/broadcasts/{broadcast_id}/targets/{leg_id}/stop",
        &["post"],
    ),
    ("/gb28181/broadcasts/{broadcast_id}/stop-all", &["post"]),
    ("/devices", &["get"]),
    ("/devices/{device_id}/preview", &["post"]),
    ("/devices/{device_id}/playback", &["post"]),
    ("/devices/{device_id}/download", &["post"]),
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
    Ok(Json(openapi_contract()))
}

pub fn openapi_contract() -> base::serde_json::Value {
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
                "description": format!("{summary}。请求须完成 GMV-HMAC-SHA256-V1 签名校验，并具备所列权限；POST 还必须提供已纳入签名的 X-GMV-Request-ID。"),
                "x-gmv-required-scope": required_scope,
                "parameters": openapi_operation_parameters(method, path),
                "security": [{"GmvAccessKey": [], "GmvTimestamp": [], "GmvNonce": [], "GmvContentSha256": [], "GmvSignature": []}],
                "responses": openapi_responses(method, path, summary)
            });
            let operation_object = operation
                .as_object_mut()
                .expect("OpenAPI operation must be an object");
            operation_object.insert(
                "x-gmv-request-example".to_string(),
                openapi_request_example(method, path),
            );
            if let Some(action) = crate::integration::model::mqtt_action_for_http(method, path) {
                operation_object.insert(
                    "x-gmv-mqtt-action".to_string(),
                    base::serde_json::json!(action),
                );
            }
            if let Some(special) = crate::integration::model::mqtt_special_for_http(method, path) {
                operation_object.insert(
                    "x-gmv-mqtt-special".to_string(),
                    base::serde_json::json!(special),
                );
            }
            if let Some(request_body) = openapi_request_body(method, path) {
                operation_object.insert("requestBody".to_string(), request_body);
            }
            operations.insert((*method).to_string(), operation);
        }
        paths.insert(format!("/openapi/v1{path}"), operations.into());
    }
    base::serde_json::json!({
        "openapi": "3.1.0",
        "info": {
            "title": "GMV Guard 三方 HTTP 开放接口",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "面向第三方业务系统的 Guard Server HTTP 接口契约。所有请求与响应正文均使用 JSON，写请求仅使用 POST；HTTP 与 MQTT 是并列协议适配器，均进入 Guard 业务控制面。"
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
        "webhooks": openapi_webhooks_contract(),
        "components": {
            "securitySchemes": {
                "GmvAccessKey": {"type": "apiKey", "in": "header", "name": HMAC_ACCESS_KEY_HEADER, "description": "第三方应用的 Access Key。"},
                "GmvTimestamp": {"type": "apiKey", "in": "header", "name": HMAC_TIMESTAMP_HEADER, "description": "请求发起时间，单位为毫秒。"},
                "GmvNonce": {"type": "apiKey", "in": "header", "name": HMAC_NONCE_HEADER, "description": "一次性随机值，用于防止重放。"},
                "GmvContentSha256": {"type": "apiKey", "in": "header", "name": HMAC_CONTENT_SHA256_HEADER, "description": "JSON 请求正文的 SHA-256 十六进制摘要。"},
                "GmvSignature": {"type": "apiKey", "in": "header", "name": HMAC_SIGNATURE_HEADER, "description": "canonical_request 的 HMAC-SHA256 十六进制签名。"}
            },
            "schemas": {
                "IntegrationEventEnvelope": {
                    "type": "object",
                    "description": "Guard 回调第三方时发送的统一事件信封。payload 的字段由 event_type 对应业务事件决定。",
                    "properties": {
                        "event_id": {"type": "string", "description": "全局事件标识，可用于第三方幂等去重。"},
                        "event_type": {
                            "type": "string",
                            "enum": crate::integration::model::INTEGRATION_CALLBACK_EVENTS.iter().map(|event| event.event_type).collect::<Vec<_>>(),
                            "description": "回调事件类型。"
                        },
                        "schema_version": {"type": "string", "const": "v1", "description": "事件信封版本。"},
                        "occurred_at_ms": {"type": "integer", "format": "int64", "description": "事件发生时间，Unix 毫秒。"},
                        "payload": {
                            "description": "事件业务数据；必须与 event_type 对应。",
                            "oneOf": crate::integration::model::INTEGRATION_CALLBACK_EVENTS
                                .iter()
                                .map(|event| integration_callback_payload_schema(event.payload_kind))
                                .collect::<Vec<_>>()
                        }
                    },
                    "required": ["event_id", "event_type", "schema_version", "occurred_at_ms", "payload"]
                },
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
            "canonical_fields": ["version", "access_key", "timestamp_ms", "nonce", "method", "path", "canonical_query", "request_id", "body_sha256"],
            "request_id_header": REQUEST_ID_HEADER,
            "request_id_required_for": ["POST"],
            "idempotency_window_ms": HTTP_IDEMPOTENCY_TTL_MS
        },
        "x-gmv-http-mqtt-capabilities": integration_capabilities_contract()
    })
}

fn integration_callback_events_contract() -> base::serde_json::Value {
    crate::integration::model::INTEGRATION_CALLBACK_EVENTS
        .iter()
        .map(integration_callback_event_contract)
        .collect::<Vec<_>>()
        .into()
}

fn integration_callback_event_contract(
    event: &crate::integration::model::IntegrationCallbackEventContract,
) -> base::serde_json::Value {
    let path = event.event_type.replace('.', "/");
    base::serde_json::json!({
        "event_type": event.event_type,
        "summary": event.summary,
        "description": event.description,
        "method": "POST",
        "payload_profile": "event-envelope-v1",
        "http_path_suffix": format!("/{path}"),
        "mqtt_topic_suffix": path,
        "payload_schema": integration_callback_payload_schema(event.payload_kind),
        "payload_example": integration_callback_payload_example(event.payload_kind),
        "envelope_example": integration_callback_event_example(event)
    })
}

fn integration_callback_payload_schema(
    kind: crate::integration::model::IntegrationCallbackPayloadKind,
) -> base::serde_json::Value {
    use crate::integration::model::IntegrationCallbackPayloadKind;

    match kind {
        IntegrationCallbackPayloadKind::SessionAlarm => base::serde_json::json!({
            "type": "object",
            "title": "GB28181 设备报警 Payload",
            "additionalProperties": false,
            "required": ["priority", "method", "alarmType", "timeStr", "deviceId", "channelId"],
            "properties": {
                "priority": {"type": "integer", "minimum": 0, "maximum": 255, "description": "GB28181 报警级别。"},
                "method": {"type": "integer", "minimum": 0, "maximum": 255, "description": "GB28181 报警方式。"},
                "alarmType": {"type": "integer", "minimum": 0, "maximum": 255, "description": "GB28181 报警类型。"},
                "timeStr": {"type": "string", "description": "设备上报的报警时间原文。"},
                "deviceId": {"type": "string", "description": "产生报警的 GB28181 设备标识。"},
                "channelId": {"type": "string", "description": "产生报警的 GB28181 通道标识。"}
            }
        }),
        IntegrationCallbackPayloadKind::SessionPlaybackPresenceTerminal => {
            base::serde_json::json!({
                "type": "object",
                "title": "回放在线状态终止 Payload",
                "additionalProperties": false,
                "required": ["playback_id", "stream_id", "subscription_id", "generation", "stream_stopped", "reason"],
                "properties": {
                    "playback_id": {"type": "string", "description": "回放会话标识。"},
                    "stream_id": {"type": "string", "description": "关联媒体流标识。"},
                    "subscription_id": {"type": "string", "description": "本次回放订阅标识。"},
                    "generation": {"type": "integer", "format": "uint64", "minimum": 0, "description": "回放在线状态代次，用于识别过期终态。"},
                    "stream_stopped": {"type": "boolean", "description": "该订阅结束后媒体流是否已经停止。"},
                    "reason": {"type": "string", "description": "终止原因，例如 heartbeat_timeout。"}
                }
            })
        }
        IntegrationCallbackPayloadKind::AvaiTaskResult => base::serde_json::json!({
            "type": "object",
            "title": "智能分析任务结果 Payload",
            "additionalProperties": false,
            "required": ["task_id", "task_type", "route_id", "state", "result"],
            "properties": {
                "task_id": {"type": "string", "description": "智能分析任务标识。"},
                "task_type": {"type": "string", "description": "分析能力类型，例如 ai.vehicle。"},
                "route_id": {"type": "string", "description": "任务执行时绑定的 Guard 路由标识。"},
                "state": {"type": "string", "const": "succeeded", "description": "当前公开结果事件只在任务成功时发布。"},
                "result": {"description": "分析执行器返回的 JSON 结果；业务字段由 task_type 对应能力定义。"}
            }
        }),
        IntegrationCallbackPayloadKind::PlaybackTicketRenewRequested => base::serde_json::json!({
            "type": "object",
            "title": "回放票据续期请求 Payload",
            "additionalProperties": false,
            "required": ["token", "playback_id", "stream_id", "output_id", "subscription_id", "expires_at_ms", "response_action"],
            "properties": {
                "token": {"type": "string", "description": "即将过期、需要续期的回放访问票据。"},
                "playback_id": {"type": "string", "description": "回放会话标识。"},
                "stream_id": {"type": "string", "description": "关联媒体流标识。"},
                "output_id": {"type": "string", "description": "关联媒体输出标识。"},
                "subscription_id": {"type": "string", "description": "关联订阅标识。"},
                "expires_at_ms": {"type": "integer", "format": "int64", "description": "当前票据过期时间，Unix 毫秒。"},
                "response_action": {"type": "string", "const": "playback.ticket.renew", "description": "第三方提交续期响应时调用的 MQTT action；HTTP 使用对应开放接口。"}
            }
        }),
    }
}

fn integration_callback_payload_example(
    kind: crate::integration::model::IntegrationCallbackPayloadKind,
) -> base::serde_json::Value {
    use crate::integration::model::IntegrationCallbackPayloadKind;

    match kind {
        IntegrationCallbackPayloadKind::SessionAlarm => base::serde_json::json!({
            "priority": 1,
            "method": 1,
            "alarmType": 1,
            "timeStr": "2026-08-09T15:30:00+08:00",
            "deviceId": "34020000001320000001",
            "channelId": "34020000001320000001"
        }),
        IntegrationCallbackPayloadKind::SessionPlaybackPresenceTerminal => {
            base::serde_json::json!({
                "playback_id": "playback-001",
                "stream_id": "stream-001",
                "subscription_id": "subscription-001",
                "generation": 3,
                "stream_stopped": true,
                "reason": "heartbeat_timeout"
            })
        }
        IntegrationCallbackPayloadKind::AvaiTaskResult => base::serde_json::json!({
            "task_id": "task-001",
            "task_type": "ai.vehicle",
            "route_id": "route-001",
            "state": "succeeded",
            "result": {"detections": [{"label": "car", "score": 0.98}]}
        }),
        IntegrationCallbackPayloadKind::PlaybackTicketRenewRequested => base::serde_json::json!({
            "token": "ticket-REDACTED",
            "playback_id": "playback-001",
            "stream_id": "stream-001",
            "output_id": "output-001",
            "subscription_id": "subscription-001",
            "expires_at_ms": 1700000300000_i64,
            "response_action": "playback.ticket.renew"
        }),
    }
}

fn integration_callback_event_schema(
    event: &crate::integration::model::IntegrationCallbackEventContract,
) -> base::serde_json::Value {
    base::serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["event_id", "event_type", "schema_version", "occurred_at_ms", "payload"],
        "properties": {
            "event_id": {"type": "string", "description": "全局事件标识，可用于第三方幂等去重。"},
            "event_type": {"type": "string", "const": event.event_type, "description": "点分格式的事件类型。"},
            "schema_version": {"type": "string", "const": "v1", "description": "事件信封版本。"},
            "occurred_at_ms": {"type": "integer", "format": "int64", "description": "事件发生时间，Unix 毫秒。"},
            "payload": integration_callback_payload_schema(event.payload_kind)
        }
    })
}

fn integration_callback_event_example(
    event: &crate::integration::model::IntegrationCallbackEventContract,
) -> base::serde_json::Value {
    base::serde_json::json!({
        "event_id": format!("event-{}", event.event_type.replace('.', "-")),
        "event_type": event.event_type,
        "schema_version": "v1",
        "occurred_at_ms": 1700000000000_i64,
        "payload": integration_callback_payload_example(event.payload_kind)
    })
}

fn openapi_webhooks_contract() -> base::serde_json::Value {
    let mut webhooks = base::serde_json::Map::new();
    for event in crate::integration::model::INTEGRATION_CALLBACK_EVENTS {
        let path = event.event_type.replace('.', "/");
        webhooks.insert(event.event_type.replace('.', "_"), base::serde_json::json!({
            "post": {
                "tags": ["事件回调"],
                "summary": event.summary,
                "description": format!("{} Guard 将事件写入持久化 outbox，并向 callback_url 追加 /{} 后发起签名 POST。第三方返回任意 2xx 即视为接收成功；其他结果按 TTL 重试。", event.description, path),
                "x-gmv-callback-url-source": "HTTP 接入配置 callback_url（基础地址）",
                "x-gmv-callback-path": format!("{{callback_url}}/{path}"),
                "x-gmv-event-types": [integration_callback_event_contract(event)],
                "parameters": [
                    {"name": HMAC_ACCESS_KEY_HEADER, "in": "header", "required": true, "description": "回调签名凭证的 Access Key。", "schema": {"type": "string"}},
                    {"name": HMAC_TIMESTAMP_HEADER, "in": "header", "required": true, "description": "回调发起时间，Unix 毫秒。", "schema": {"type": "integer", "format": "int64"}},
                    {"name": HMAC_NONCE_HEADER, "in": "header", "required": true, "description": "一次性随机值，用于防止重放。", "schema": {"type": "string"}},
                    {"name": HMAC_CONTENT_SHA256_HEADER, "in": "header", "required": true, "description": "JSON 请求正文的 SHA-256 十六进制摘要。", "schema": {"type": "string"}},
                    {"name": HMAC_SIGNATURE_HEADER, "in": "header", "required": true, "description": "canonical_request 的 HMAC-SHA256 十六进制签名；回调签名的 request_id 字段为空字符串。", "schema": {"type": "string"}}
                ],
                "requestBody": {
                    "required": true,
                    "content": {
                        "application/json": {
                            "schema": integration_callback_event_schema(event),
                            "example": integration_callback_event_example(event)
                        }
                    }
                },
                "responses": {
                    "204": {"description": "第三方已接收回调；任意 2xx 响应均具有相同语义。"},
                    "400": {"description": "第三方拒绝请求，Guard 将按投递策略重试。"},
                    "500": {"description": "第三方处理失败，Guard 将按投递策略重试。"}
                }
            }
        }));
    }
    base::serde_json::Value::Object(webhooks)
}

fn integration_capabilities_contract() -> base::serde_json::Value {
    let items =
        OPEN_BUSINESS_OPERATIONS
            .iter()
            .flat_map(|(path, methods)| {
                methods.iter().map(move |method| {
                let http_method = if *method == "get" { &Method::GET } else { &Method::POST };
                base::serde_json::json!({
                    "http_method": method.to_uppercase(),
                    "http_path": format!("/openapi/v1{path}"),
                    "mqtt_action": crate::integration::model::mqtt_action_for_http(method, path),
                    "mqtt_special": crate::integration::model::mqtt_special_for_http(method, path),
                    "required_scope": open_business_scope(http_method, path)
                })
            })
            })
            .collect::<Vec<_>>();
    base::serde_json::json!(items)
}

fn openapi_request_example(method: &str, path: &str) -> base::serde_json::Value {
    let rendered_path = path
        .replace("{device_id}", "device-001")
        .replace("{channel_id}", "channel-001")
        .replace("{resource_id}", "resource-001")
        .replace("{image_id}", "image-001")
        .replace("{task_id}", "task-001")
        .replace("{broadcast_id}", "broadcast-001")
        .replace("{leg_id}", "leg-001")
        .replace("{stream_id}", "stream-001")
        .replace("{playback_id}", "playback-001")
        .replace("{operation_id}", "operation-001")
        .replace("{output_id}", "output-001")
        .replace("{node_id}", "session-001")
        .replace("{token}", "PLAYBACK_TOKEN");
    let query = if method == "get" {
        openapi_query_fields(path)
            .iter()
            .map(|(name, _)| {
                (
                    (*name).to_string(),
                    schema_example(&openapi_request_field_schema(path, name)),
                )
            })
            .collect::<base::serde_json::Map<_, _>>()
    } else {
        base::serde_json::Map::new()
    };
    let mut body = openapi_request_body(method, path)
        .map(|request| schema_example(&request["content"]["application/json"]["schema"]));
    if let Some(object) = body
        .as_mut()
        .and_then(base::serde_json::Value::as_object_mut)
    {
        for (field, value) in [
            ("request_id", "req-001"),
            ("device_id", "device-001"),
            ("channel_id", "channel-001"),
            ("resource_id", "resource-001"),
            ("image_id", "image-001"),
            ("task_id", "task-001"),
            ("broadcast_id", "broadcast-001"),
            ("leg_id", "leg-001"),
            ("stream_id", "stream-001"),
            ("playback_id", "playback-001"),
            ("operation_id", "operation-001"),
            ("output_id", "output-001"),
            ("node_id", "session-001"),
            ("session_node_id", "session-001"),
        ] {
            if object.contains_key(field) {
                object.insert(field.to_string(), base::serde_json::json!(value));
            }
        }
    }
    base::serde_json::json!({
        "method": method.to_uppercase(),
        "path": format!("/openapi/v1{rendered_path}"),
        "headers": if method == "post" {
            base::serde_json::json!({
                "x-gmv-access-key": "ACCESS_KEY",
                "x-gmv-timestamp": 1700000000000_i64,
                "x-gmv-nonce": "UNIQUE_NONCE",
                "x-gmv-request-id": "req-001",
                "x-gmv-content-sha256": "BODY_SHA256_HEX",
                "x-gmv-signature": "HMAC_SHA256_HEX"
            })
        } else {
            base::serde_json::json!({
                "x-gmv-access-key": "ACCESS_KEY",
                "x-gmv-timestamp": 1700000000000_i64,
                "x-gmv-nonce": "UNIQUE_NONCE",
                "x-gmv-content-sha256": "EMPTY_BODY_SHA256_HEX",
                "x-gmv-signature": "HMAC_SHA256_HEX"
            })
        },
        "query": query,
        "body": body
    })
}

fn schema_example(schema: &base::serde_json::Value) -> base::serde_json::Value {
    if let Some(value) = schema.get("example") {
        return value.clone();
    }
    if let Some(value) = schema.get("default") {
        return value.clone();
    }
    if let Some(value) = schema.get("const") {
        return value.clone();
    }
    if let Some(value) = schema
        .get("enum")
        .and_then(base::serde_json::Value::as_array)
        .and_then(|values| values.first())
    {
        return value.clone();
    }
    if let Some(properties) = schema
        .get("properties")
        .and_then(base::serde_json::Value::as_object)
    {
        return properties
            .iter()
            .map(|(name, property)| (name.clone(), schema_example(property)))
            .collect::<base::serde_json::Map<_, _>>()
            .into();
    }
    if schema.get("type") == Some(&base::serde_json::json!("array")) {
        return base::serde_json::json!([schema_example(&schema["items"])]);
    }
    match schema.get("type").and_then(base::serde_json::Value::as_str) {
        Some("boolean") => base::serde_json::json!(true),
        Some("integer") => base::serde_json::json!(1),
        Some("number") => base::serde_json::json!(1.0),
        Some("object") => base::serde_json::json!({}),
        _ => base::serde_json::json!("example"),
    }
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
                "schema": openapi_request_field_schema(path, name)
            })
        }));
    } else if method == "post" {
        parameters.push(base::serde_json::json!({
            "name": REQUEST_ID_HEADER,
            "in": "header",
            "required": true,
            "description": "调用方生成的幂等请求标识，必须包含在 HMAC canonical_request 中；同一应用重复提交相同请求时返回原结果。",
            "schema": {"type": "string", "minLength": 1, "maxLength": 128}
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
        ("post", "/gb28181/broadcasts/start") => "创建单目标或多目标语音广播任务",
        ("get", "/gb28181/broadcasts/{broadcast_id}") => "查询语音广播任务及目标状态",
        ("post", "/gb28181/broadcasts/{broadcast_id}/targets/{leg_id}/stop") => {
            "停止语音广播中的指定目标"
        }
        ("post", "/gb28181/broadcasts/{broadcast_id}/stop-all") => "停止语音广播全部目标",
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
        ("get", "/devices") => "查询可用设备",
        ("post", "/devices/{device_id}/preview") => "创建设备实时预览",
        ("post", "/devices/{device_id}/playback") => "创建设备录像回放",
        ("post", "/devices/{device_id}/download") => "创建设备录像下载",
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
        "/gb28181/devices/{device_id}/channels/{channel_id}/images" => &[
            ("session_node_id", true),
            ("start_time_ms", false),
            ("end_time_ms", false),
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
            ("snapshot_to_mode", false),
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
            ("broadcast_enable", false),
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
            ("stream_profile", false),
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
        "/gb28181/broadcasts/start" => &[
            ("request_id", true),
            ("token", false),
            ("default_trans_mode", false),
            ("codec", false),
            ("sample_rate", false),
            ("channel_count", false),
            ("frame_duration_ms", false),
            ("targets", true),
        ],
        "/gb28181/broadcasts/{broadcast_id}/targets/{leg_id}/stop"
        | "/gb28181/broadcasts/{broadcast_id}/stop-all" => &[("request_id", false)],
        "/devices/{device_id}/preview"
        | "/devices/{device_id}/playback"
        | "/devices/{device_id}/download" => &[
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
            ("broadcast_codec", false),
            ("broadcast_sample_rate", false),
            ("broadcast_channel_count", false),
            ("broadcast_frame_duration_ms", false),
            ("playback_id", false),
            ("stream_profile", false),
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
        .map(|(name, _)| {
            (
                (*name).to_string(),
                openapi_request_field_schema(path, name),
            )
        })
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
    if name == "targets" {
        return base::serde_json::json!({
            "type": "array",
            "minItems": 1,
            "description": openapi_field_description(name),
            "items": {
                "type": "object",
                "required": ["device_id", "channel_id", "session_node_id"],
                "properties": {
                    "device_id": openapi_field_schema("device_id"),
                    "channel_id": openapi_field_schema("channel_id"),
                    "session_node_id": openapi_field_schema("session_node_id"),
                    "trans_mode": openapi_field_schema("trans_mode")
                }
            }
        });
    }
    if name == "items" {
        return base::serde_json::json!({
            "type": "array",
            "minItems": 1,
            "maxItems": 64,
            "description": openapi_field_description(name),
            "items": {
                "type": "object",
                "required": ["playback_id", "stream_id", "subscription_id", "generation"],
                "properties": {
                    "playback_id": openapi_field_schema("playback_id"),
                    "stream_id": openapi_field_schema("stream_id"),
                    "subscription_id": openapi_field_schema("subscription_id"),
                    "generation": {"type": "integer", "minimum": 0, "description": "回放控制版本号。"}
                }
            }
        });
    }
    match name {
        "trans_mode" | "default_trans_mode" => {
            return base::serde_json::json!({
                "type": "string",
                "enum": ["udp", "tcp_active", "tcp_passive"],
                "default": "udp",
                "example": "udp",
                "description": openapi_field_description(name)
            });
        }
        "leftRight" | "upDown" | "inOut" => {
            return base::serde_json::json!({
                "type": "integer",
                "enum": [0, 1, 2],
                "description": openapi_field_description(name)
            });
        }
        "horizonSpeed" | "verticalSpeed" => {
            return base::serde_json::json!({
                "type": "integer",
                "minimum": 0,
                "maximum": 255,
                "example": 64,
                "description": openapi_field_description(name)
            });
        }
        "zoomSpeed" => {
            return base::serde_json::json!({
                "type": "integer",
                "minimum": 0,
                "maximum": 15,
                "example": 8,
                "description": openapi_field_description(name)
            });
        }
        "speed_rate" => {
            return base::serde_json::json!({
                "type": "number",
                "enum": [0.5, 1.0, 2.0, 4.0],
                "example": 1.0,
                "description": openapi_field_description(name)
            });
        }
        "snapshot_to_mode" => {
            return base::serde_json::json!({
                "type": "integer",
                "enum": [0, 1],
                "default": 0,
                "description": openapi_field_description(name)
            });
        }
        "mode" => {
            return base::serde_json::json!({
                "type": "string",
                "enum": ["inline", "attachment"],
                "default": "inline",
                "description": openapi_field_description(name)
            });
        }
        "page" => {
            return base::serde_json::json!({
                "type": "integer", "minimum": 1, "default": 1,
                "description": openapi_field_description(name)
            });
        }
        "page_size" => {
            return base::serde_json::json!({
                "type": "integer", "minimum": 1, "maximum": 500,
                "description": openapi_field_description(name)
            });
        }
        "limit" => {
            return base::serde_json::json!({
                "type": "integer", "minimum": 1, "maximum": 500, "default": 100,
                "description": openapi_field_description(name)
            });
        }
        "min_priority" => {
            return base::serde_json::json!({
                "type": "integer", "minimum": 0, "maximum": 255,
                "description": openapi_field_description(name)
            });
        }
        "count" | "interval" => {
            return base::serde_json::json!({
                "type": "integer", "minimum": 1, "maximum": 255,
                "description": openapi_field_description(name)
            });
        }
        "expected_generation" | "position_sec" => {
            return base::serde_json::json!({
                "type": "integer", "minimum": 0,
                "description": openapi_field_description(name)
            });
        }
        "start_time_sec" | "end_time_sec" => {
            return base::serde_json::json!({
                "type": "integer", "format": "int64", "minimum": 0,
                "example": 1700000000,
                "description": openapi_field_description(name)
            });
        }
        "start_time_ms" | "end_time_ms" => {
            return base::serde_json::json!({
                "type": "integer", "format": "int64", "minimum": 0,
                "example": 1700000000000_i64,
                "description": openapi_field_description(name)
            });
        }
        "request_id" => {
            return base::serde_json::json!({
                "type": "string", "minLength": 1, "maxLength": 128,
                "pattern": "^[^\\s]+$", "example": "req-20260808-0001",
                "description": openapi_field_description(name)
            });
        }
        _ => {}
    }
    base::serde_json::json!({
        "type": openapi_field_type(name),
        "description": openapi_field_description(name)
    })
}

fn openapi_request_field_schema(path: &str, name: &str) -> base::serde_json::Value {
    let mut schema = openapi_field_schema(name);
    let object = schema
        .as_object_mut()
        .expect("OpenAPI field schema must be an object");
    match name {
        "stream_profile" => {
            let live = path.ends_with("/preview");
            object.insert(
                "enum".to_string(),
                if live {
                    base::serde_json::json!(["main", "sub"])
                } else {
                    base::serde_json::json!(["main"])
                },
            );
            object.insert("default".to_string(), base::serde_json::json!("main"));
        }
        "output_type" => {
            let values = if path.ends_with("/preview") {
                base::serde_json::json!(["flv", "fmp4", "hls", "ll_hls"])
            } else if path.ends_with("/download") {
                base::serde_json::json!(["flv", "fmp4", "hls", "mp4"])
            } else if path.ends_with("/outputs") {
                base::serde_json::json!(["flv", "fmp4", "hls", "ll_hls"])
            } else {
                base::serde_json::json!(["flv", "fmp4", "hls"])
            };
            object.insert("enum".to_string(), values);
        }
        "audio_codec" => {
            object.insert("enum".to_string(), base::serde_json::json!(["aac"]));
        }
        "startup_timeout_ms" => {
            let minimum = if path.ends_with("/outputs") {
                10_000
            } else {
                FIRST_PREVIEW_HARD_TIMEOUT_MS
            };
            object.insert("minimum".to_string(), base::serde_json::json!(minimum));
            object.insert("maximum".to_string(), base::serde_json::json!(30_000));
            if !path.ends_with("/outputs") {
                object.insert(
                    "default".to_string(),
                    base::serde_json::json!(FIRST_PREVIEW_HARD_TIMEOUT_MS),
                );
            }
        }
        "page_size" if path.contains("/records") || path.contains("/images") => {
            object.insert("maximum".to_string(), base::serde_json::json!(100));
        }
        "broadcast_codec" | "codec" => {
            object.insert("enum".to_string(), base::serde_json::json!(["PCMA"]));
            object.insert("default".to_string(), base::serde_json::json!("PCMA"));
        }
        "broadcast_sample_rate" | "sample_rate" => {
            object.insert("const".to_string(), base::serde_json::json!(8_000));
            object.insert("default".to_string(), base::serde_json::json!(8_000));
        }
        "broadcast_channel_count" | "channel_count" => {
            object.insert("const".to_string(), base::serde_json::json!(1));
            object.insert("default".to_string(), base::serde_json::json!(1));
        }
        "broadcast_frame_duration_ms" | "frame_duration_ms" => {
            object.insert("minimum".to_string(), base::serde_json::json!(10));
            object.insert("maximum".to_string(), base::serde_json::json!(60));
            object.insert("default".to_string(), base::serde_json::json!(20));
        }
        _ => {}
    }
    schema
}

fn openapi_field_type(name: &str) -> &'static str {
    match name {
        "registered_only" | "paused" | "renew" | "snapshot" | "ptz_enable" | "broadcast_enable"
        | "audio_enable" | "record_enable" | "playback_enable" | "alarm_enable" | "biz_enable" => {
            "boolean"
        }
        "speed_rate" | "longitude" | "latitude" => "number",
        "items" | "targets" => "array",
        "limit"
        | "min_priority"
        | "page"
        | "page_size"
        | "pwd_check"
        | "status"
        | "heartbeat_sec"
        | "sort_no"
        | "start_time_sec"
        | "end_time_sec"
        | "start_time_ms"
        | "end_time_ms"
        | "startup_timeout_ms"
        | "broadcast_sample_rate"
        | "broadcast_channel_count"
        | "broadcast_frame_duration_ms"
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
        "broadcast_id" => "语音广播父任务唯一标识。",
        "leg_id" => "语音广播目标媒体 leg 唯一标识。",
        "targets" => "语音广播目标列表，每项包含设备、通道、Session 与可选传输模式。",
        "default_trans_mode" => "广播任务默认媒体传输模式。",
        "codec" => "浏览器上传的广播音频编码，当前固定为 PCMA。",
        "sample_rate" => "广播输入采样率，当前固定为 8000 Hz。",
        "channel_count" => "广播输入声道数，当前固定为单声道。",
        "frame_duration_ms" => "广播输入音频帧时长，当前固定为 20 毫秒。",
        "operation_id" => "媒体或业务操作的唯一标识。",
        "node_id" => "Guard 注册节点的唯一标识。",
        "device_id" | "deviceId" => "设备唯一标识。",
        "channel_id" | "channelId" => "设备通道唯一标识。",
        "resource_id" => "设备资源唯一标识。",
        "image_id" => "通道截图资源唯一标识。",
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
        "snapshot_to_mode" => {
            "设备截图投递目标：0=signaling_peer，截图回传到发起信令的对端；1=business_target，截图投递到业务配置目标。默认 0。"
        }
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
        "broadcast_enable" => "是否启用语音广播。",
        "audio_enable" => "是否启用音频。",
        "record_enable" => "是否启用录像。",
        "playback_enable" => "是否启用回放。",
        "alarm_enable" => "是否启用告警。",
        "biz_enable" => "是否启用业务处理。",
        "sort_no" => "通道显示排序号。",
        "start_time_sec" => {
            "Unix 秒时间戳。回放、下载和录像查询中必须小于 end_time_sec；实时预览不使用时传 0 或省略。"
        }
        "end_time_sec" => {
            "Unix 秒时间戳。回放、下载和录像查询中必须大于 start_time_sec；实时预览不使用时传 0 或省略。"
        }
        "trans_mode" => {
            "GB28181 媒体传输模式：udp=UDP；tcp_active=Session 主动连接设备；tcp_passive=Session 监听并等待设备连接。空值按 udp。"
        }
        "output_type" => {
            "播放输出封装：flv、fmp4、hls；ll_hls 仅实时预览；mp4 仅有限时长下载。省略时使用 Session 默认输出。"
        }
        "audio_codec" => "输出音频目标编码；当前仅支持 aac。省略表示保持源音频或使用默认策略。",
        "stream_profile" => {
            "视频码流档位：main=主码流，sub=辅码流。实时预览支持二者，默认 main；回放和下载仅支持 main。"
        }
        "startup_timeout_ms" => {
            "等待媒体准备完成的超时，最大 30000 毫秒；预览/回放/下载最小及默认值为 15000，单独创建输出时 flv/fmp4 默认 10000、hls/ll_hls 默认 12000。"
        }
        "broadcast_codec" => "语音广播编码；当前仅支持 PCMA（G.711 A-law）。",
        "broadcast_sample_rate" => "语音广播采样率；当前固定 8000 Hz。",
        "broadcast_channel_count" => "语音广播声道数；当前固定 1（单声道）。",
        "broadcast_frame_duration_ms" => {
            "语音广播帧时长，范围 10～60 毫秒，默认 20；8000×帧时长必须能被 1000 整除。"
        }
        "leftRight" => "水平控制值：0 停止、1 向左、2 向右。",
        "upDown" => "垂直控制值：0 停止、1 向上、2 向下。",
        "inOut" => "变倍控制值：0 停止、1 缩小、2 放大。",
        "horizonSpeed" => {
            "水平转动速度。leftRight 非 0 时取 1～255；0 表示该轴不参与，超过 255 的值不会被调用方契约接受。"
        }
        "verticalSpeed" => {
            "垂直转动速度。upDown 非 0 时取 1～255；0 表示该轴不参与。斜向转动采用水平、垂直速度中的较大值。"
        }
        "zoomSpeed" => "变倍速度。inOut 非 0 时取 1～15；0 表示不变倍。设备侧有效速度上限为 15。",
        "count" => "本次截图数量，范围 1～255；省略时使用 Session 截图配置默认值。",
        "interval" => "连续截图间隔，范围 1～255，单位为秒；省略时使用 Session 截图配置默认值。",
        "mode" => {
            "资源响应方式：inline=浏览器内联预览，attachment=作为附件下载；其他值按 inline，调用方应只发送枚举值。"
        }
        "start_time_ms" => "查询开始时间，Unix 毫秒时间戳。",
        "end_time_ms" => "查询结束时间，Unix 毫秒时间戳。",
        "stream_node_id" => "负责媒体流的流节点标识。",
        "ssrc" => "RTP 同步源标识。",
        "dialog_state" => "GB28181 SIP Dialog 状态筛选值。",
        "state" => "业务状态筛选值。",
        "stop_reason" => "停止媒体流的原因。",
        "subscription_id" => "媒体订阅唯一标识。",
        "speed_rate" => "回放倍速，仅支持 0.5、1.0、2.0、4.0。",
        "position_sec" => {
            "目标回放位置，Unix 秒时间戳，必须位于创建回放时的 start_time_sec～end_time_sec 区间内。"
        }
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
        "page_size" => "每页记录数；设备列表最多 500，录像和截图列表最多 100。",
        "device_name" => "设备名称模糊筛选值。",
        "registered_only" => "是否只返回已注册设备。",
        _ => "业务字段。",
    }
}

fn openapi_responses(method: &str, path: &str, summary: &str) -> base::serde_json::Value {
    let success_schema = openapi_success_schema(method, path, summary);
    let success_example = schema_example(&success_schema);
    let error_example = base::serde_json::json!({
        "code": "invalid_request",
        "message": "request validation failed",
        "user_message": "请求参数不正确，请检查字段后重试",
        "operation_id": null,
        "trace_id": "trace-001",
        "retryable": false,
        "details": {}
    });
    let error_content = base::serde_json::json!({
        "application/json": {"schema": {"$ref": "#/components/schemas/ErrorResponse"}, "example": error_example}
    });
    let (success_status, success_description, has_body) =
        openapi_success_response_kind(method, path);
    let mut responses = base::serde_json::Map::new();
    responses.insert(
        success_status.to_string(),
        if has_body {
            base::serde_json::json!({
                "description": success_description,
                "content": {"application/json": {"schema": success_schema, "example": success_example}}
            })
        } else {
            base::serde_json::json!({"description": success_description})
        },
    );
    for (status, description) in [
        ("400", "请求参数、字段格式或业务前置条件不正确。"),
        (
            "401",
            "Access Key、时间戳、nonce、正文摘要或 HMAC 签名无效。",
        ),
        (
            "403",
            "第三方应用未获得该接口所需权限，或无权访问目标资源。",
        ),
        ("404", "目标设备、通道、流、任务或操作不存在。"),
        ("409", "当前资源状态与请求冲突。"),
        (
            "429",
            "超过调用频率或系统容量限制，可按返回信息决定是否重试。",
        ),
        ("500", "Guard 内部处理失败。"),
    ] {
        responses.insert(
            status.to_string(),
            base::serde_json::json!({"description": description, "content": error_content.clone()}),
        );
    }
    responses.into()
}

fn openapi_success_response_kind(method: &str, path: &str) -> (&'static str, &'static str, bool) {
    if method == "post" && path == "/gb28181/devices/{device_id}/delete" {
        return ("204", "设备删除成功，无响应正文。", false);
    }
    if method == "post" && matches!(path, "/gb28181/devices" | "/gb28181/broadcasts/start") {
        return ("201", "资源创建成功，返回新建资源 JSON。", true);
    }
    if method == "post"
        && (path.ends_with("/preview")
            || path.ends_with("/playback")
            || path.ends_with("/download")
            || path.ends_with("/records/query")
            || path.ends_with("/images")
            || path.ends_with("/cloud-recordings")
            || path == "/ai/tasks"
            || path == "/streams/{stream_id}/outputs")
    {
        return ("202", "异步操作已受理，返回操作状态或任务摘要 JSON。", true);
    }
    ("200", "请求成功，返回 JSON 业务数据。", true)
}

fn openapi_success_schema(method: &str, path: &str, summary: &str) -> base::serde_json::Value {
    let fields: &[&str] = match path {
        "/dashboard" => &["node_count", "event_count", "next_after_id"],
        "/media/transport" => &["scheme", "http_version", "multi_view_limit"],
        path if path.starts_with("/media/operations") => &[
            "operation_id",
            "state",
            "stage",
            "elapsed_ms",
            "last_progress_at_ms",
            "checkpoint_ms",
            "hard_timeout_ms",
            "can_continue",
            "result",
            "error",
        ],
        "/nodes" => &[
            "node_id",
            "instance_id",
            "kind",
            "service",
            "protocol",
            "display_name",
            "connection",
            "connection_label",
            "health",
            "health_label",
            "scheduling",
            "scheduling_label",
            "capabilities",
            "pending_leases",
            "host_metrics",
            "business_metrics",
            "config",
            "zone",
            "last_seen_at_ms",
            "generation",
            "sequence",
        ],
        "/leases" => &[
            "lease_id",
            "route_id",
            "resource_id",
            "node_id",
            "instance_id",
            "state",
            "expires_at_ms",
        ],
        "/events" => &["items", "next_after_id"],
        "/gb28181/session-nodes/{node_id}/config" => &["domain", "domain_id", "wan_ip", "wan_port"],
        "/gb28181/devices" if method == "get" => &["items", "total", "page", "page_size"],
        "/gb28181/devices"
        | "/gb28181/devices/{device_id}"
        | "/gb28181/devices/{device_id}/delete" => gb_device_response_fields(),
        "/gb28181/devices/{device_id}/channels"
        | "/gb28181/devices/{device_id}/channels/{channel_id}" => gb_channel_response_fields(),
        "/gb28181/devices/{device_id}/resources"
        | "/gb28181/devices/{device_id}/resources/{resource_id}/confirmation"
        | "/gb28181/devices/{device_id}/resources/{resource_id}/confirmation/reset" => {
            gb_resource_response_fields()
        }
        "/gb28181/devices/{device_id}/channels/{channel_id}/ptz" | "/devices/{device_id}/ptz" => {
            &["accepted", "count"]
        }
        "/gb28181/devices/{device_id}/channels/{channel_id}/images" if method == "get" => {
            &["items", "total", "page", "page_size"]
        }
        "/gb28181/devices/{device_id}/channels/{channel_id}/images" => &["session_id"],
        "/gb28181/devices/{device_id}/channels/{channel_id}/images/{image_id}/access" => &[
            "url",
            "expires_at_ms",
            "content_type",
            "file_name",
            "file_size",
        ],
        "/gb28181/devices/{device_id}/channels/{channel_id}/images/{image_id}/cover" => {
            gb_channel_response_fields()
        }
        "/gb28181/devices/{device_id}/channels/{channel_id}/records"
        | "/gb28181/devices/{device_id}/channels/{channel_id}/records/query" => &[
            "current_batch",
            "attempt_batch",
            "segments",
            "next_query_at_ms",
            "server_time_ms",
            "total",
            "page",
            "page_size",
        ],
        "/gb28181/devices/{device_id}/channels/{channel_id}/cloud-recordings"
            if method == "get" =>
        {
            &["items", "total", "page", "page_size"]
        }
        path if path.contains("cloud-recordings") && path.ends_with("/access") => &[
            "url",
            "expires_at_ms",
            "content_type",
            "file_name",
            "file_size",
        ],
        path if path.contains("cloud-recordings") => cloud_recording_response_fields(),
        "/gb28181/broadcasts/start" | "/gb28181/broadcasts/{broadcast_id}" => &[
            "broadcast_id",
            "stream_node_id",
            "input_url",
            "state",
            "target_summaries",
        ],
        "/gb28181/broadcasts/{broadcast_id}/targets/{leg_id}/stop"
        | "/gb28181/broadcasts/{broadcast_id}/stop-all" => &[
            "broadcast_id",
            "stream_node_id",
            "input_url",
            "state",
            "target_summaries",
        ],
        "/gb28181/devices/{device_id}/channels/{channel_id}/preview"
        | "/gb28181/devices/{device_id}/channels/{channel_id}/playback" => &[
            "operation_id",
            "state",
            "stage",
            "elapsed_ms",
            "last_progress_at_ms",
            "checkpoint_ms",
            "hard_timeout_ms",
            "can_continue",
            "result",
            "error",
        ],
        "/devices" => &["device_id", "name", "session_node_id", "channels", "online"],
        "/streams"
        | "/devices/{device_id}/preview"
        | "/devices/{device_id}/playback"
        | "/devices/{device_id}/download"
        | "/streams/{stream_id}/stop"
        | "/streams/{stream_id}/release" => &[
            "stream_id",
            "device_id",
            "channel_id",
            "node_id",
            "instance_id",
            "lease_id",
            "route_id",
            "endpoint",
            "video_codec",
            "audio_codec",
            "mime_codec",
            "broadcast_profile",
            "requested_stream_profile",
            "effective_stream_profile",
            "stream_profile_verification",
            "subscription_id",
            "session_node_id",
            "session_instance_id",
            "playback_id",
            "playback_generation",
            "playback_start_time_sec",
            "playback_end_time_sec",
            "state",
        ],
        "/streams/{stream_id}/outputs" if method == "get" => &[
            "output_id",
            "stream_id",
            "output_type",
            "endpoint",
            "state",
            "video_codec",
            "audio_codec",
            "mime_codec",
        ],
        "/streams/{stream_id}/outputs" => &[
            "operation_id",
            "state",
            "stage",
            "elapsed_ms",
            "last_progress_at_ms",
            "checkpoint_ms",
            "hard_timeout_ms",
            "can_continue",
            "result",
            "error",
        ],
        "/streams/{stream_id}/outputs/{output_id}/close" => &["closed", "output_id"],
        "/streams/{stream_id}/speed" => &["accepted", "speed_rate"],
        "/playbacks/{playback_id}/seek"
        | "/playbacks/{playback_id}/speed"
        | "/playbacks/{playback_id}/state" => &["accepted", "generation"],
        "/playbacks/presence/heartbeat" => &["server_time_ms", "items"],
        "/playback-tickets/{token}/renew" => &["renewed", "revoked", "expires_at_ms"],
        "/ai/tasks" | "/ai/tasks/{task_id}/cancel" => &[
            "task_id",
            "model",
            "stream_id",
            "node_id",
            "instance_id",
            "lease_id",
            "route_id",
            "state",
        ],
        "/runtime/status" => &[
            "guard_available",
            "streams",
            "running_streams",
            "ai_tasks",
            "running_ai_tasks",
            "ptz_commands",
        ],
        "/gb28181/streams" => &["items", "total", "page", "page_size", "server_time_ms"],
        "/gb28181/streams/{stream_id}/management" => &["state", "active", "ended"],
        "/gb28181/stream-history" => &["items", "total", "page", "page_size", "server_time_ms"],
        "/gb28181/streams/{stream_id}/stop" => &[
            "stream_id",
            "state",
            "session_node_id",
            "session_instance_id",
        ],
        path if path.contains("/broadcasts/") || path == "/gb28181/broadcasts/start" => &[
            "broadcast_id",
            "stream_node_id",
            "input_url",
            "state",
            "target_summaries",
        ],
        path if path.contains("/images") => &[
            "image_id",
            "device_id",
            "channel_id",
            "session_node_id",
            "url",
            "created_at_ms",
            "items",
            "page",
            "page_size",
            "total",
        ],
        path if path.contains("cloud-recordings") => &[
            "task_id",
            "device_id",
            "channel_id",
            "session_node_id",
            "state",
            "start_time_sec",
            "end_time_sec",
            "url",
            "items",
            "page",
            "page_size",
            "total",
        ],
        path if path.contains("/records") => &[
            "query_id",
            "device_id",
            "channel_id",
            "start_time_sec",
            "end_time_sec",
            "items",
            "page",
            "page_size",
            "total",
        ],
        path if path.contains("/gb28181/devices") => &[
            "device_id",
            "device_name",
            "channel_id",
            "channel_name",
            "session_node_id",
            "domain_id",
            "registered",
            "online",
            "status",
            "items",
            "page",
            "page_size",
            "total",
        ],
        path if path.contains("/gb28181/streams") || path.contains("stream-history") => &[
            "stream_id",
            "session_node_id",
            "stream_node_id",
            "device_id",
            "channel_id",
            "ssrc",
            "state",
            "dialog_state",
            "media_state",
            "items",
            "page",
            "page_size",
            "total",
            "server_time_ms",
        ],
        _ => &["accepted", "operation_id", "state"],
    };
    let item_fields = fields
        .iter()
        .copied()
        .filter(|field| !matches!(*field, "items" | "page" | "page_size" | "total"))
        .collect::<Vec<_>>();
    let is_array = method == "get"
        && matches!(
            path,
            "/media/operations"
                | "/nodes"
                | "/leases"
                | "/gb28181/devices/{device_id}/channels"
                | "/gb28181/devices/{device_id}/resources"
                | "/devices"
                | "/streams"
                | "/streams/{stream_id}/outputs"
                | "/ai/tasks"
        );
    if is_array {
        return base::serde_json::json!({
            "type": "array",
            "description": format!("{summary}成功时返回的数组。"),
            "items": openapi_response_object_schema(path, summary, &item_fields)
        });
    }
    openapi_response_object_schema(path, summary, fields)
}

fn openapi_response_object_schema(
    path: &str,
    summary: &str,
    fields: &[&str],
) -> base::serde_json::Value {
    let properties = fields
        .iter()
        .map(|field| {
            (
                (*field).to_string(),
                openapi_response_field_schema(path, field),
            )
        })
        .collect::<base::serde_json::Map<_, _>>();
    base::serde_json::json!({
        "type": "object",
        "description": format!("{summary}成功时返回的 JSON 对象。"),
        "properties": properties,
        "required": fields,
        "additionalProperties": false
    })
}

fn openapi_response_field_schema(path: &str, field: &str) -> base::serde_json::Value {
    let description = openapi_response_field_description(field);
    if matches!(
        field,
        "channels" | "endpoints" | "capabilities" | "supported_formats"
    ) {
        return base::serde_json::json!({
            "type": "array",
            "description": description,
            "items": {"type": "string", "description": "数组字符串条目。"}
        });
    }
    if matches!(field, "items" | "segments" | "target_summaries") {
        let item_fields = openapi_nested_item_fields(path, field);
        return base::serde_json::json!({
            "type": "array",
            "description": "业务条目数组，数组元素字段如下展开。",
            "items": openapi_response_object_schema(path, "数组元素", item_fields)
        });
    }
    if field == "viewer_formats" {
        return base::serde_json::json!({
            "type": "array",
            "description": "按输出格式统计的观看者数组。",
            "items": openapi_response_object_schema(path, "观看格式", &["media_format", "viewer_count"])
        });
    }
    if field == "host_metrics" {
        return base::serde_json::json!({
            "type": "object",
            "description": "节点最近一次上报的主机资源指标。",
            "properties": openapi_response_properties(path, &[
                "cpu_usage_percent", "load_average_1m", "load_average_5m", "load_average_15m",
                "memory_total_bytes", "memory_used_bytes", "swap_total_bytes", "swap_used_bytes",
                "disk_read_bytes_per_sec", "disk_write_bytes_per_sec",
                "network_receive_bytes_per_sec", "network_transmit_bytes_per_sec",
                "process_resident_memory_bytes", "process_threads"
            ]),
            "additionalProperties": false
        });
    }
    if matches!(field, "business_metrics" | "config") {
        return base::serde_json::json!({
            "type": "object",
            "description": description,
            "additionalProperties": {"type": "string", "description": "指标或配置值。"}
        });
    }
    if field == "confirmation" {
        return base::serde_json::json!({
            "type": ["object", "null"],
            "description": description,
            "properties": openapi_response_properties(path, &[
                "status", "resource_kind", "owner_scope", "owner_id", "suggested_enum_id",
                "source_parent_id", "confirmed_by", "confirmed_at_ms", "remark"
            ]),
            "additionalProperties": false
        });
    }
    if field == "error" {
        return base::serde_json::json!({
            "type": ["object", "null"],
            "description": description,
            "properties": openapi_response_properties(path, &["code", "message", "user_message", "retryable"]),
            "additionalProperties": false
        });
    }
    if matches!(field, "active" | "ended") {
        let item_fields = if field == "active" {
            active_stream_response_fields()
        } else {
            stream_history_response_fields()
        };
        return base::serde_json::json!({
            "type": ["object", "null"],
            "description": if field == "active" { "活动流详情；流已结束时为 null。" } else { "已结束流详情；流仍活动时为 null。" },
            "properties": openapi_response_properties(path, item_fields),
            "additionalProperties": false
        });
    }
    if matches!(field, "current_batch" | "attempt_batch") {
        let fields = [
            "batch_id",
            "status",
            "start_time_sec",
            "end_time_sec",
            "created_at_ms",
        ];
        return base::serde_json::json!({
            "type": ["object", "null"],
            "description": "录像检索批次；没有对应批次时为 null。",
            "properties": openapi_response_properties(path, &fields),
            "additionalProperties": false
        });
    }
    if matches!(
        field,
        "online"
            | "registered"
            | "accepted"
            | "guard_available"
            | "can_continue"
            | "closed"
            | "renewed"
            | "revoked"
            | "supported"
            | "available"
            | "can_stop"
            | "can_play"
            | "can_download"
            | "can_delete"
            | "progress_stale"
            | "can_preview"
            | "media_ready"
            | "legacy_terminal_time"
    ) {
        return base::serde_json::json!({"type": "boolean", "description": description});
    }
    if field.ends_with("_ms")
        || field.ends_with("_sec")
        || field.ends_with("_bytes")
        || field.ends_with("_bytes_per_sec")
        || field.ends_with("_count")
        || matches!(
            field,
            "node_count"
                | "event_count"
                | "page"
                | "page_size"
                | "total"
                | "streams"
                | "running_streams"
                | "ai_tasks"
                | "running_ai_tasks"
                | "ptz_commands"
                | "playback_generation"
                | "multi_view_limit"
                | "generation"
                | "count"
                | "wan_port"
                | "progress_percent"
                | "pending_leases"
                | "sequence"
                | "priority"
                | "process_threads"
                | "file_size"
                | "segment_id"
                | "secrecy"
                | "port"
                | "pwd_check"
                | "snapshot"
                | "snapshot_to_mode"
                | "del"
                | "monitor_status"
                | "max_camera"
                | "ptz_enable"
                | "broadcast_enable"
                | "audio_enable"
                | "record_enable"
                | "playback_enable"
                | "alarm_enable"
                | "biz_enable"
                | "owner_biz_enable"
                | "sort_no"
        )
        || (field == "status"
            && matches!(
                path,
                "/gb28181/devices"
                    | "/gb28181/devices/{device_id}"
                    | "/gb28181/devices/{device_id}/delete"
            ))
    {
        return base::serde_json::json!({"type": "integer", "description": description});
    }
    if field == "result" {
        return base::serde_json::json!({"type": ["object", "null"], "description": description});
    }
    if field == "speed_rate" {
        return base::serde_json::json!({"type": "number", "description": "实际生效的播放速度倍率。"});
    }
    if field.ends_with("_percent") || field.starts_with("load_average_") {
        return base::serde_json::json!({"type": "number", "description": description});
    }
    base::serde_json::json!({"type": ["string", "null"], "description": description})
}

fn openapi_response_field_description(field: &str) -> &'static str {
    let request_description = openapi_field_description(field);
    if request_description != "业务字段。" {
        return request_description;
    }
    match field {
        "node_count" => "当前已登记节点数量。",
        "event_count" => "本页返回的事件数量。",
        "next_after_id" => "下一次增量查询应传入的事件或流游标。",
        "scheme" => "媒体访问 URL 使用的协议方案，例如 http 或 https。",
        "http_version" => "媒体访问链路已验证的 HTTP 协议版本。",
        "multi_view_limit" => "当前传输能力建议的单客户端多画面上限。",
        "instance_id" | "session_instance_id" => "节点当前进程实例标识，用于隔离重启前后的状态。",
        "kind" => "节点类型。",
        "service" => "节点提供的服务名称。",
        "protocol" => "节点使用的业务协议。",
        "display_name" | "name" => "面向调用方展示的名称。",
        "connection" => "节点连接状态机器值。",
        "connection_label" => "节点连接状态中文名称。",
        "health" => "节点健康状态机器值。",
        "health_label" => "节点健康状态中文名称。",
        "scheduling" => "节点是否可参与新任务调度的机器值。",
        "scheduling_label" => "节点调度状态中文名称。",
        "endpoints" => "节点或租约公开的访问端点数组。",
        "capabilities" => "节点声明的能力标识数组。",
        "pending_leases" => "节点当前等待完成的租约数量。",
        "host_metrics" => "节点最近一次上报的主机资源指标对象。",
        "business_metrics" => "节点上报的业务指标键值对象。",
        "config" => "节点公开的非敏感运行配置键值对象。",
        "channels" => "设备包含的通道标识数组。",
        "supported_formats" => "当前媒体流支持创建的输出格式数组。",
        "zone" => "节点所属部署区域。",
        "last_seen_at_ms" => "Guard 最后收到节点心跳的 Unix 毫秒时间戳。",
        "lease_id" => "资源租约唯一标识。",
        "route_id" => "资源路由唯一标识。",
        "stream_type" | "session_type" => "媒体会话类型。",
        "expires_at_ms" => "资源、票据或签名地址的 Unix 毫秒过期时间；null 表示无该值。",
        "operation_id" => "异步媒体或业务操作唯一标识，可用于继续查询。",
        "stage" => "操作当前执行阶段。",
        "elapsed_ms" => "操作从创建到当前的累计耗时，单位毫秒。",
        "last_progress_at_ms" => "操作最后产生进展的 Unix 毫秒时间戳。",
        "checkpoint_ms" => "操作进入可由调用方继续决策阶段的毫秒阈值。",
        "hard_timeout_ms" => "操作强制结束前的最大毫秒时限。",
        "can_continue" => "当前操作是否允许调用继续接口。",
        "result" => "操作成功后的业务结果对象；未完成或失败时为 null。",
        "error" => "操作失败详情对象；未失败时为 null。",
        "online" => "设备当前是否在线。",
        "endpoint" | "input_url" | "url" | "image_url" => {
            "受控访问地址；可能包含短期令牌，调用方不得记录或越权转发。"
        }
        "video_codec" => "媒体视频编码名称。",
        "audio_codec" => "媒体音频编码名称。",
        "mime_codec" => "Stream 根据实际输出生成的完整 MIME codec 字符串。",
        "requested_stream_profile" => "调用方请求的码流档位。",
        "effective_stream_profile" => "设备或会话实际生效的码流档位。",
        "stream_profile_verification" => "实际码流档位的验证状态。",
        "playback_generation" | "generation" => "回放控制版本号，用于并发控制。",
        "sequence" => "节点状态上报序号。",
        "priority" => "事件优先级数值。",
        "payload" => "事件业务 JSON 的 UTF-8 字符串；调用方按 topic 再解析。",
        "topic" => "事件主题名称。",
        "event_id" => "事件唯一标识，也是 MQTT 事件消费幂等键。",
        "guard_available" => "Guard 控制面当前是否可提供服务。",
        "streams" | "ai_tasks" | "ptz_commands" => "当前登记的对应业务对象总数。",
        "running_streams" | "running_ai_tasks" => "当前处于运行状态的对应业务对象数量。",
        "target_summaries" => "广播各目标的独立执行结果数组。",
        "target_key" => "广播目标的稳定关联键。",
        "transport" => "目标实际使用的媒体传输模式。",
        "profile" => "目标实际使用的音频传输规格。",
        "reason" | "diagnostic_reason" => "当前状态的原因或诊断说明。",
        "file_name" => "建议调用方展示或保存的文件名。",
        "content_type" => "资源的 MIME Content-Type。",
        "file_size" | "current_size_bytes" | "final_size_bytes" => "文件大小，单位字节。",
        "total" => "满足当前筛选条件的总记录数。",
        "deleted" => "目标资源是否已删除。",
        "closed" => "媒体输出是否已关闭。",
        "renewed" => "播放票据本次是否成功续期。",
        "revoked" => "播放票据是否已被撤销。",
        "accepted" => "业务命令是否已被目标状态机接受。",
        "server_time_ms" => "服务端生成响应时的 Unix 毫秒时间戳。",
        "presence_deadline_ms" => "下一次观看心跳最迟到达时间；终态时可能为 null。",
        "created_at_ms" => "资源创建的 Unix 毫秒时间戳。",
        "updated_at_ms" => "资源最后更新的 Unix 毫秒时间戳。",
        "started_at_ms" => "任务或媒体开始执行的 Unix 毫秒时间戳。",
        "finished_at_ms" | "terminated_at_ms" => "任务或媒体结束的 Unix 毫秒时间戳。",
        "duration_ms" | "recorded_duration_ms" => "持续时长，单位毫秒。",
        "progress_percent" => "任务完成进度百分比，范围 0～100。",
        "progress_stale" => "任务进度是否长时间未更新。",
        "file_state" => "录像文件当前可用状态。",
        "file_format" => "录像文件封装格式。",
        "requested_by" => "创建该任务的身份标识。",
        "error_code" => "稳定的业务错误代码；无错误时为空字符串或 null。",
        "error_message" => "面向开发者的错误说明；无错误时为空字符串。",
        "can_stop" | "can_play" | "can_download" | "can_delete" => {
            "当前任务状态是否允许执行对应后续操作。"
        }
        "supported" => "资源类型是否被 Guard 当前版本支持。",
        "available" => "资源当前是否可用于业务操作。",
        "unavailable_reason" | "warning" => "资源不可用原因或兼容性提示。",
        "confirmation" => "人工资源分类确认信息；尚未确认时为 null。",
        _ => "响应中的业务数据字段；具体值与当前资源状态一致。",
    }
}

fn openapi_response_properties(
    path: &str,
    fields: &[&str],
) -> base::serde_json::Map<String, base::serde_json::Value> {
    fields
        .iter()
        .map(|field| {
            (
                (*field).to_string(),
                openapi_response_field_schema(path, field),
            )
        })
        .collect()
}

fn openapi_nested_item_fields(path: &str, field: &str) -> &'static [&'static str] {
    if field == "target_summaries" {
        return &[
            "target_key",
            "device_id",
            "channel_id",
            "session_node_id",
            "leg_id",
            "transport",
            "profile",
            "state",
            "reason",
        ];
    }
    if field == "segments" {
        return &[
            "segment_id",
            "batch_id",
            "device_id",
            "channel_id",
            "remote_device_id",
            "name",
            "file_path",
            "address",
            "start_time_sec",
            "end_time_sec",
            "secrecy",
            "record_type",
            "recorder_id",
            "file_size",
        ];
    }
    match path {
        "/events" => &["event_id", "topic", "priority", "payload"],
        "/gb28181/devices" => gb_device_response_fields(),
        path if path.contains("/images") => gb_image_response_fields(),
        path if path.contains("cloud-recordings") => cloud_recording_response_fields(),
        "/playbacks/presence/heartbeat" => &[
            "playback_id",
            "stream_id",
            "accepted",
            "terminal",
            "generation",
            "presence_deadline_ms",
        ],
        "/gb28181/stream-history" => stream_history_response_fields(),
        "/gb28181/streams" => active_stream_dialog_response_fields(),
        _ => &["id", "state"],
    }
}

fn gb_device_response_fields() -> &'static [&'static str] {
    &[
        "device_id",
        "session_node_id",
        "domain_id",
        "domain",
        "longitude",
        "latitude",
        "address",
        "pwd",
        "pwd_check",
        "alias",
        "status",
        "heartbeat_sec",
        "snapshot_to_mode",
        "del",
        "create_time",
        "tenant_id",
        "sys_org_code",
        "create_by",
        "update_by",
        "update_time",
        "monitor_status",
        "device_type",
        "manufacturer",
        "model",
        "firmware",
        "gb_version",
        "max_camera",
        "camera_in_count",
        "camera_off_count",
        "register_time",
    ]
}

fn gb_channel_response_fields() -> &'static [&'static str] {
    &[
        "device_id",
        "channel_id",
        "name",
        "manufacturer",
        "model",
        "owner",
        "status",
        "civil_code",
        "address",
        "parent_id",
        "ip_address",
        "port",
        "longitude",
        "latitude",
        "ptz_type",
        "alias_name",
        "pic_url",
        "snapshot",
        "over_pic_id",
        "ptz_enable",
        "broadcast_enable",
        "audio_enable",
        "record_enable",
        "playback_enable",
        "alarm_enable",
        "biz_enable",
        "sort_no",
        "created_at_ms",
        "updated_at_ms",
        "cover_image_id",
    ]
}

fn gb_image_response_fields() -> &'static [&'static str] {
    &[
        "image_id",
        "device_id",
        "channel_id",
        "image_url",
        "created_at_ms",
        "file_name",
        "content_type",
        "file_size",
        "can_preview",
        "session_node_id",
    ]
}

fn gb_resource_response_fields() -> &'static [&'static str] {
    &[
        "device_id",
        "resource_id",
        "name",
        "status",
        "parent_id",
        "type_code",
        "enum_id",
        "enum_name",
        "suggested_kind",
        "classification_mode",
        "effective_kind",
        "effective_owner_scope",
        "effective_owner_id",
        "warning",
        "biz_enable",
        "owner_biz_enable",
        "supported",
        "available",
        "unavailable_reason",
        "confirmation",
    ]
}

fn cloud_recording_response_fields() -> &'static [&'static str] {
    &[
        "task_id",
        "request_id",
        "session_node_id",
        "device_id",
        "channel_id",
        "start_time_sec",
        "end_time_sec",
        "requested_duration_sec",
        "status",
        "file_state",
        "progress_percent",
        "recorded_duration_ms",
        "progress_stale",
        "current_size_bytes",
        "final_size_bytes",
        "file_format",
        "requested_by",
        "created_at_ms",
        "started_at_ms",
        "finished_at_ms",
        "updated_at_ms",
        "error_code",
        "error_message",
        "can_stop",
        "can_play",
        "can_download",
        "can_delete",
    ]
}

fn active_stream_response_fields() -> &'static [&'static str] {
    &[
        "stream_id",
        "session_node_id",
        "session_instance_id",
        "stream_node_id",
        "device_id",
        "channel_id",
        "ssrc",
        "state",
        "dialog_state",
        "media_state",
        "media_ready",
        "created_at_ms",
        "established_at_ms",
        "started_at_ms",
        "diagnostic_reason",
        "session_type",
        "viewer_count",
        "viewer_formats",
        "supported_formats",
        "output_format",
        "requested_stream_profile",
        "effective_stream_profile",
        "stream_profile_verification",
    ]
}

fn active_stream_dialog_response_fields() -> &'static [&'static str] {
    &[
        "stream_id",
        "session_node_id",
        "session_instance_id",
        "stream_node_id",
        "device_id",
        "channel_id",
        "ssrc",
        "dialog_state",
        "created_at_ms",
        "established_at_ms",
        "started_at_ms",
        "session_type",
    ]
}

fn stream_history_response_fields() -> &'static [&'static str] {
    &[
        "stream_id",
        "session_node_id",
        "stream_node_id",
        "device_id",
        "channel_id",
        "ssrc",
        "session_type",
        "state",
        "created_at_ms",
        "established_at_ms",
        "terminated_at_ms",
        "duration_ms",
        "terminal_reason",
        "terminal_reason_label",
        "error_code",
        "legacy_terminal_time",
        "stop_reason",
    ]
}

fn mqtt_action_payload_schemas() -> base::serde_json::Value {
    let mut schemas = base::serde_json::json!({
        "StreamStartPayload": {
            "type": "object",
            "description": "实时点播参数。command.target 填设备 ID，payload.channel_id 填通道 ID；成功结果的 result.endpoint 是播放地址。",
            "required": ["channel_id"],
            "properties": {
                "device_id": {"type": "string", "description": "可选设备 ID；省略时使用 command.target，若同时提供应与 target 表示同一设备。"},
                "channel_id": {"type": "string", "minLength": 1, "description": "要点播的设备通道 ID。"},
                "session_node_id": {"type": "string", "description": "可选 GB28181 Session 节点；省略时由 Guard 调度。"},
                "token": {"type": "string", "description": "可选媒体订阅 token；省略时 Guard 根据 command_id 生成。不要复用到其他点播。"},
                "trans_mode": {"type": "string", "enum": ["udp", "tcp_active", "tcp_passive"], "default": "udp", "description": "媒体传输：udp；tcp_active=Session 主动连接设备；tcp_passive=Session 等待设备连接。"},
                "output_type": {"type": "string", "enum": ["flv", "fmp4", "hls", "ll_hls"], "description": "播放输出封装；省略时使用 Session 默认输出。ll_hls 仅实时点播。"},
                "audio_codec": {"type": "string", "enum": ["aac"], "description": "可选音频转码目标；当前仅支持 aac。"},
                "stream_profile": {"type": "string", "enum": ["main", "sub"], "default": "main", "description": "main=主码流，sub=辅码流；默认 main。"}
            }
        },
        "StreamStopPayload": {
            "type": "object",
            "description": "停止流。command.target 必须填启动成功结果中的 result.stream_id；payload 当前为空对象。",
            "properties": {}
        },
        "StreamPlaybackPayload": {
            "type": "object",
            "description": "按设备录像时间范围创建回放；command.target 填设备 ID，成功结果的 result.endpoint 是播放地址。",
            "required": ["channel_id", "start_time_sec", "end_time_sec"],
            "properties": {
                "device_id": {"type": "string", "description": "可选设备 ID；省略时使用 command.target。"},
                "channel_id": {"type": "string", "minLength": 1, "description": "录像所属通道 ID。"},
                "start_time_sec": {"type": "integer", "format": "int64", "minimum": 1, "description": "回放开始 Unix 秒时间戳，必须小于 end_time_sec。"},
                "end_time_sec": {"type": "integer", "format": "int64", "minimum": 1, "description": "回放结束 Unix 秒时间戳，必须大于 start_time_sec。"},
                "session_node_id": {"type": "string", "description": "可选 GB28181 Session 节点；省略时由 Guard 调度。"},
                "token": {"type": "string", "description": "可选媒体订阅 token；省略时 Guard 生成。"},
                "trans_mode": {"type": "string", "enum": ["udp", "tcp_active", "tcp_passive"], "default": "udp", "description": "GB28181 媒体传输模式。"},
                "output_type": {"type": "string", "enum": ["flv", "fmp4", "hls"], "description": "回放输出封装；ll_hls 不支持回放。"},
                "audio_codec": {"type": "string", "enum": ["aac"], "description": "可选音频转码目标。"},
                "playback_id": {"type": "string", "description": "可选调用方回放会话 ID；省略时由服务生成。"},
                "stream_profile": {"type": "string", "enum": ["main"], "default": "main", "description": "回放仅支持主码流 main。"}
            }
        },
        "StreamDownloadPayload": {
            "type": "object",
            "description": "按设备录像时间范围创建有限时长下载流；command.target 填设备 ID。",
            "required": ["channel_id", "start_time_sec", "end_time_sec"],
            "properties": {
                "device_id": {"type": "string", "description": "可选设备 ID；省略时使用 command.target。"},
                "channel_id": {"type": "string", "minLength": 1, "description": "录像所属通道 ID。"},
                "start_time_sec": {"type": "integer", "format": "int64", "minimum": 1, "description": "下载开始 Unix 秒时间戳，必须小于 end_time_sec。"},
                "end_time_sec": {"type": "integer", "format": "int64", "minimum": 1, "description": "下载结束 Unix 秒时间戳，必须大于 start_time_sec。"},
                "session_node_id": {"type": "string", "description": "可选 GB28181 Session 节点；省略时由 Guard 调度。"},
                "token": {"type": "string", "description": "可选媒体订阅 token；省略时 Guard 生成。"},
                "trans_mode": {"type": "string", "enum": ["udp", "tcp_active", "tcp_passive"], "default": "udp", "description": "GB28181 媒体传输模式。"},
                "output_type": {"type": "string", "enum": ["flv", "fmp4", "hls", "mp4"], "description": "下载输出封装；mp4 仅适用于有限时长下载。省略时使用 Session 默认输出；需要生成 MP4 文件时应显式传 mp4。"},
                "audio_codec": {"type": "string", "enum": ["aac"], "description": "可选音频转码目标。"},
                "stream_profile": {"type": "string", "enum": ["main"], "default": "main", "description": "下载仅支持主码流 main。"}
            }
        },
        "DeviceBroadcastPayload": {
            "type": "object",
            "description": "启动单设备语音广播。成功结果的 result.endpoint 为音频输入地址，调用方按 PCMA/8000Hz/单声道上传 RTP 音频。",
            "required": ["channel_id"],
            "properties": {
                "device_id": {"type": "string", "description": "可选设备 ID；省略时使用 command.target。"},
                "channel_id": {"type": "string", "minLength": 1, "description": "接收广播的通道 ID。"},
                "session_node_id": {"type": "string", "description": "可选 GB28181 Session 节点；省略时由 Guard 调度。"},
                "trans_mode": {"type": "string", "enum": ["udp", "tcp_active", "tcp_passive"], "default": "udp", "description": "广播 RTP 传输模式。"},
                "broadcast_codec": {"type": "string", "enum": ["PCMA"], "default": "PCMA", "description": "当前固定 PCMA（G.711 A-law）。"},
                "broadcast_sample_rate": {"type": "integer", "const": 8000, "default": 8000, "description": "当前固定 8000 Hz。"},
                "broadcast_channel_count": {"type": "integer", "const": 1, "default": 1, "description": "当前固定单声道。"},
                "broadcast_frame_duration_ms": {"type": "integer", "minimum": 10, "maximum": 60, "default": 20, "description": "音频帧时长，10～60 ms；8000×该值必须能被 1000 整除。"}
            }
        },
        "DevicePtzPayload": {
            "type": "object",
            "description": "云台方向三元组只允许：停止(0,0,0)、八方向组合、变倍(0,0,1|2)。转动与变倍不能同时发送。停止时速度字段忽略。",
            "required": ["channel_id", "leftRight", "upDown", "inOut", "horizonSpeed", "verticalSpeed", "zoomSpeed"],
            "properties": {
                "channel_id": {"type": "string", "minLength": 1, "description": "设备通道 ID。"},
                "leftRight": {"type": "integer", "enum": [0, 1, 2], "description": "0=不水平转动，1=左，2=右。"},
                "upDown": {"type": "integer", "enum": [0, 1, 2], "description": "0=不垂直转动，1=上，2=下。"},
                "inOut": {"type": "integer", "enum": [0, 1, 2], "description": "0=不变倍，1=缩小(zoom out)，2=放大(zoom in)。"},
                "horizonSpeed": {"type": "integer", "minimum": 0, "maximum": 255, "description": "leftRight 非 0 时为 1～255；该轴不参与时传 0。"},
                "verticalSpeed": {"type": "integer", "minimum": 0, "maximum": 255, "description": "upDown 非 0 时为 1～255；该轴不参与时传 0。斜向取两轴较大速度。"},
                "zoomSpeed": {"type": "integer", "minimum": 0, "maximum": 15, "description": "inOut 非 0 时为 1～15；不变倍时传 0。"}
            }
        },
        "AiStartPayload": {
            "type": "object",
            "required": ["model"],
            "properties": {
                "stream_id": {"type": "string", "description": "可选媒体流 ID；省略时使用 command.target。流必须处于可分析状态。"},
                "model": {"type": "string", "minLength": 1, "description": "部署环境已注册且目标 AI 节点声明支持的模型标识。"}
            }
        },
        "AiCancelPayload": {"type": "object", "description": "取消 AI 任务；command.target 填 ai.start 成功结果中的 result.task_id。", "properties": {}},
        "PlaybackTicketRenewPayload": {
            "type": "object",
            "required": ["renew"],
            "description": "响应 Guard 定向发送的播放票据续期事件；command.target 填事件中的 token。",
            "properties": {"renew": {"type": "boolean", "description": "true=续期 5 分钟，false=立即撤销；省略无效。"}}
        },
        "StreamCommandResult": {
            "type": "object",
            "description": "媒体命令结果。start/playback/download/broadcast 成功后使用 endpoint；stop 成功后 state 为 stopping 或 stopped。",
            "required": ["stream_id", "state"],
            "properties": {
                "stream_id": {"type": "string", "description": "后续 stream.stop 的 command.target。"},
                "device_id": {"type": "string"},
                "channel_id": {"type": "string"},
                "endpoint": {"type": "string", "description": "可直接交给播放器或广播上传端的地址；调用方不得记录其中的访问 token。"},
                "subscription_id": {"type": "string", "description": "媒体订阅标识，用于关联播放生命周期。"},
                "playback_id": {"type": "string", "description": "回放会话标识；实时点播和下载可为空。"},
                "requested_stream_profile": {"type": "string", "enum": ["", "main", "sub"]},
                "effective_stream_profile": {"type": "string", "enum": ["", "main", "sub"]},
                "stream_profile_verification": {"type": "string", "description": "码流选择确认状态。"},
                "video_codec": {"type": "string"},
                "audio_codec": {"type": "string"},
                "mime_codec": {"type": "string"},
                "state": {"type": "string", "enum": ["running", "stopping", "stopped", "failed"]}
            }
        },
        "PtzCommandResult": {
            "type": "object",
            "required": ["accepted", "command", "speed", "sequence", "count"],
            "properties": {
                "accepted": {"type": "boolean", "const": true},
                "command": {"type": "string", "enum": ["stop", "left_up", "right_up", "left_down", "right_down", "left", "right", "up", "down", "zoom_out", "zoom_in"]},
                "speed": {"type": "integer", "minimum": 1, "maximum": 255, "description": "Guard 实际提交给 Session 的速度；变倍在设备侧最大按 15 生效。"},
                "sequence": {"type": "integer", "minimum": 0, "description": "Guard 接受的 PTZ 命令序号。"},
                "count": {"type": "integer", "minimum": 0, "description": "与 HTTP 响应 count 一致的命令序号兼容字段。"}
            }
        },
        "AiCommandResult": {
            "type": "object",
            "required": ["task_id", "model", "stream_id", "state"],
            "properties": {
                "task_id": {"type": "string", "description": "后续 ai.cancel 的 command.target。"},
                "model": {"type": "string"},
                "stream_id": {"type": "string"},
                "state": {"type": "string", "enum": ["running", "cancelled", "failed"]}
            }
        },
        "PlaybackTicketRenewResult": {
            "type": "object",
            "required": ["renewed", "revoked", "expires_at_ms"],
            "properties": {
                "renewed": {"type": "boolean"},
                "revoked": {"type": "boolean"},
                "expires_at_ms": {"type": ["integer", "null"], "format": "int64", "description": "续期后的 Unix 毫秒过期时间；撤销时为 null。"}
            }
        }
    });
    let object = schemas
        .as_object_mut()
        .expect("MQTT component schemas must be an object");
    for action in crate::integration::model::MQTT_COMMAND_ACTIONS {
        let payload_name = mqtt_payload_schema_name(action);
        let result_name = mqtt_result_schema_name(action);
        if !object.contains_key(&payload_name) {
            object.insert(payload_name, mqtt_payload_schema_from_http(action));
        }
        if !object.contains_key(&result_name) {
            object.insert(result_name, mqtt_result_schema_from_http(action));
        }
    }
    schemas
}

fn mqtt_payload_schema_name(action: &str) -> String {
    match action {
        "stream.start" => "StreamStartPayload".to_string(),
        "stream.stop" => "StreamStopPayload".to_string(),
        "stream.playback" => "StreamPlaybackPayload".to_string(),
        "stream.download" => "StreamDownloadPayload".to_string(),
        "device.broadcast" => "DeviceBroadcastPayload".to_string(),
        "device.ptz" => "DevicePtzPayload".to_string(),
        "ai.start" => "AiStartPayload".to_string(),
        "ai.cancel" => "AiCancelPayload".to_string(),
        "playback.ticket.renew" => "PlaybackTicketRenewPayload".to_string(),
        _ => format!("MqttPayload_{}", action.replace('.', "_")),
    }
}

fn mqtt_result_schema_name(action: &str) -> String {
    match action {
        "stream.start" | "stream.stop" | "stream.playback" | "stream.download"
        | "device.broadcast" => "StreamCommandResult".to_string(),
        "device.ptz" => "PtzCommandResult".to_string(),
        "ai.start" | "ai.cancel" => "AiCommandResult".to_string(),
        "playback.ticket.renew" => "PlaybackTicketRenewResult".to_string(),
        _ => format!("MqttResult_{}", action.replace('.', "_")),
    }
}

fn http_contract_for_mqtt_action(action: &str) -> Option<(&'static str, &'static str)> {
    OPEN_BUSINESS_OPERATIONS.iter().find_map(|(path, methods)| {
        methods.iter().find_map(|method| {
            (crate::integration::model::mqtt_action_for_http(method, path) == Some(action))
                .then_some((*method, *path))
        })
    })
}

fn mqtt_payload_schema_from_http(action: &str) -> base::serde_json::Value {
    let Some((method, path)) = http_contract_for_mqtt_action(action) else {
        return base::serde_json::json!({
            "type": "object",
            "description": format!("{action} 请求参数。"),
            "properties": {}
        });
    };
    let mut properties = base::serde_json::Map::new();
    let mut required = Vec::<String>::new();
    for name in path
        .split('{')
        .skip(1)
        .filter_map(|part| part.split_once('}').map(|(name, _)| name))
    {
        properties.insert(name.to_string(), openapi_request_field_schema(path, name));
        if name != mqtt_target_field(action) {
            required.push(name.to_string());
        }
    }
    if method == "get" {
        for (name, is_required) in openapi_query_fields(path) {
            properties.insert(
                (*name).to_string(),
                openapi_request_field_schema(path, name),
            );
            if *is_required {
                required.push((*name).to_string());
            }
        }
    } else if let Some(body) = openapi_request_body(method, path) {
        let schema = &body["content"]["application/json"]["schema"];
        if let Some(body_properties) = schema
            .get("properties")
            .and_then(base::serde_json::Value::as_object)
        {
            properties.extend(
                body_properties
                    .iter()
                    .map(|(name, property)| (name.clone(), property.clone())),
            );
        }
        if let Some(body_required) = schema
            .get("required")
            .and_then(base::serde_json::Value::as_array)
        {
            required.extend(
                body_required
                    .iter()
                    .filter_map(base::serde_json::Value::as_str)
                    .map(str::to_string),
            );
        }
    }
    required.sort();
    required.dedup();
    base::serde_json::json!({
        "type": "object",
        "description": format!("{action} 参数；等价 HTTP：{} /openapi/v1{path}。路径字段在 MQTT 中放入 payload，主资源也可使用 command.target。", method.to_uppercase()),
        "required": required,
        "properties": properties,
        "additionalProperties": false
    })
}

fn mqtt_result_schema_from_http(action: &str) -> base::serde_json::Value {
    if action == "gb.device.delete" {
        return openapi_response_object_schema(
            "/gb28181/devices/{device_id}/delete",
            "删除 GB28181 设备",
            &["deleted", "device_id"],
        );
    }
    if action == "stream.output.create" {
        return openapi_response_object_schema(
            "/streams/{stream_id}/outputs",
            "创建媒体输出的 MQTT 业务终态",
            &["output_id", "stream_id", "output_type", "endpoint", "state"],
        );
    }
    let Some((method, path)) = http_contract_for_mqtt_action(action) else {
        return base::serde_json::json!({"type": "object", "properties": {}});
    };
    openapi_success_schema(method, path, openapi_operation_summary(method, path))
}

fn mqtt_payload_refs() -> Vec<base::serde_json::Value> {
    crate::integration::model::MQTT_COMMAND_ACTIONS
        .iter()
        .map(|action| {
            base::serde_json::json!({
                "$ref": format!("#/components/schemas/{}", mqtt_payload_schema_name(action))
            })
        })
        .collect()
}

fn mqtt_result_refs() -> Vec<base::serde_json::Value> {
    let mut names = crate::integration::model::MQTT_COMMAND_ACTIONS
        .iter()
        .map(|action| mqtt_result_schema_name(action))
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    let mut refs = names
        .into_iter()
        .map(|name| base::serde_json::json!({"$ref": format!("#/components/schemas/{name}")}))
        .collect::<Vec<_>>();
    refs.push(base::serde_json::json!({"type": "null"}));
    refs
}

fn mqtt_command_examples() -> base::serde_json::Value {
    let mut examples = base::serde_json::json!([
        {"name": "stream.start", "payload": {"integration_id": "partner-a", "command_id": "cmd-live-001", "issued_at_ms": 1700000000000_i64, "expires_at_ms": 1700000060000_i64, "action": "stream.start", "target": "device-001", "payload": {"channel_id": "channel-001", "trans_mode": "udp", "output_type": "flv", "audio_codec": "aac", "stream_profile": "main"}}},
        {"name": "stream.stop", "payload": {"integration_id": "partner-a", "command_id": "cmd-stop-001", "issued_at_ms": 1700000000000_i64, "expires_at_ms": 1700000060000_i64, "action": "stream.stop", "target": "stream-001", "payload": {}}},
        {"name": "stream.playback", "payload": {"integration_id": "partner-a", "command_id": "cmd-playback-001", "issued_at_ms": 1700000000000_i64, "expires_at_ms": 1700000060000_i64, "action": "stream.playback", "target": "device-001", "payload": {"channel_id": "channel-001", "start_time_sec": 1699996400, "end_time_sec": 1700000000, "trans_mode": "tcp_active", "output_type": "hls", "stream_profile": "main"}}},
        {"name": "stream.download", "payload": {"integration_id": "partner-a", "command_id": "cmd-download-001", "issued_at_ms": 1700000000000_i64, "expires_at_ms": 1700000060000_i64, "action": "stream.download", "target": "device-001", "payload": {"channel_id": "channel-001", "start_time_sec": 1699996400, "end_time_sec": 1700000000, "output_type": "mp4", "stream_profile": "main"}}},
        {"name": "device.broadcast", "payload": {"integration_id": "partner-a", "command_id": "cmd-broadcast-001", "issued_at_ms": 1700000000000_i64, "expires_at_ms": 1700000060000_i64, "action": "device.broadcast", "target": "device-001", "payload": {"channel_id": "channel-001", "broadcast_codec": "PCMA", "broadcast_sample_rate": 8000, "broadcast_channel_count": 1, "broadcast_frame_duration_ms": 20}}},
        {"name": "device.ptz", "payload": {"integration_id": "partner-a", "command_id": "cmd-ptz-001", "issued_at_ms": 1700000000000_i64, "expires_at_ms": 1700000060000_i64, "action": "device.ptz", "target": "device-001", "payload": {"channel_id": "channel-001", "leftRight": 1, "upDown": 0, "inOut": 0, "horizonSpeed": 128, "verticalSpeed": 0, "zoomSpeed": 0}}},
        {"name": "ai.start", "payload": {"integration_id": "partner-a", "command_id": "cmd-ai-start-001", "issued_at_ms": 1700000000000_i64, "expires_at_ms": 1700000060000_i64, "action": "ai.start", "target": "stream-001", "payload": {"model": "vehicle"}}},
        {"name": "ai.cancel", "payload": {"integration_id": "partner-a", "command_id": "cmd-ai-cancel-001", "issued_at_ms": 1700000000000_i64, "expires_at_ms": 1700000060000_i64, "action": "ai.cancel", "target": "ai-task-001", "payload": {}}},
        {"name": "playback.ticket.renew", "payload": {"integration_id": "partner-a", "command_id": "cmd-renew-001", "issued_at_ms": 1700000000000_i64, "expires_at_ms": 1700000060000_i64, "action": "playback.ticket.renew", "target": "ticket-token", "payload": {"renew": true}}}
    ]);
    let schemas = mqtt_action_payload_schemas();
    let items = examples
        .as_array_mut()
        .expect("MQTT command examples must be an array");
    for action in crate::integration::model::MQTT_COMMAND_ACTIONS {
        if items.iter().any(|item| item["name"] == **action) {
            continue;
        }
        let payload_schema = &schemas[&mqtt_payload_schema_name(action)];
        items.push(base::serde_json::json!({
            "name": action,
            "payload": mqtt_command_example(action, schema_example(payload_schema))
        }));
    }
    examples
}

fn mqtt_command_example(action: &str, payload: base::serde_json::Value) -> base::serde_json::Value {
    base::serde_json::json!({
        "integration_id": "partner-a",
        "command_id": format!("cmd-{}-001", action.replace('.', "-")),
        "issued_at_ms": 1700000000000_i64,
        "expires_at_ms": 1700000060000_i64,
        "action": action,
        "target": mqtt_target_example(action),
        "payload": payload
    })
}

fn mqtt_target_example(action: &str) -> &'static str {
    match mqtt_target_field(action) {
        "operation_id" => "operation-001",
        "output_id" => "output-001",
        "stream_id" => "stream-001",
        "playback_id" => "playback-001",
        "token" => "ticket-token",
        "task_id" => "task-001",
        "broadcast_id" => "broadcast-001",
        "leg_id" => "broadcast-leg-001",
        "image_id" => "image-001",
        "channel_id" => "channel-001",
        "resource_id" => "resource-001",
        "node_id" => "session-001",
        "device_id" => "device-001",
        _ => "query",
    }
}

fn mqtt_target_field(action: &str) -> &'static str {
    match action {
        "media.operation.get" | "media.operation.continue" | "media.operation.cancel" => {
            "operation_id"
        }
        "stream.output.close" => "output_id",
        "stream.stop"
        | "stream.release"
        | "stream.speed.set"
        | "stream.output.list"
        | "stream.output.create"
        | "gb.stream.management"
        | "gb.stream.stop"
        | "ai.start" => "stream_id",
        "playback.seek" | "playback.speed.set" | "playback.state.set" => "playback_id",
        "playback.ticket.renew" => "token",
        "cloud_recording.get"
        | "cloud_recording.stop"
        | "cloud_recording.delete"
        | "cloud_recording.access"
        | "ai.cancel" => "task_id",
        "broadcast.get" | "broadcast.stop_all" => "broadcast_id",
        "broadcast.stop_target" => "leg_id",
        "gb.image.access" | "gb.image.cover" => "image_id",
        "gb.channel.get"
        | "gb.channel.update"
        | "gb.image.list"
        | "gb.image.snapshot"
        | "gb.record.list"
        | "gb.record.query"
        | "cloud_recording.list"
        | "cloud_recording.create" => "channel_id",
        "gb.resource.confirm" | "gb.resource.reset" => "resource_id",
        "gb.session_config.get" => "node_id",
        "gb.device.create" | "gb.device.get" | "gb.device.update" | "gb.device.delete"
        | "gb.channel.list" | "gb.resource.list" | "stream.start" | "stream.playback"
        | "stream.download" | "device.broadcast" | "device.ptz" => "device_id",
        _ => "query（固定占位值）",
    }
}

fn mqtt_action_usage_contract() -> base::serde_json::Value {
    let mut usage = base::serde_json::Map::new();
    for action in crate::integration::model::MQTT_COMMAND_ACTIONS {
        let equivalents = OPEN_BUSINESS_OPERATIONS
            .iter()
            .flat_map(|(path, methods)| methods.iter().map(move |method| (*method, *path)))
            .filter(|(method, path)| {
                crate::integration::model::mqtt_action_for_http(method, path) == Some(*action)
            })
            .collect::<Vec<_>>();
        let summary = equivalents
            .first()
            .map(|(method, path)| openapi_operation_summary(method, path))
            .unwrap_or("MQTT 命令");
        let http = equivalents
            .iter()
            .map(|(method, path)| format!("{} /openapi/v1{path}", method.to_uppercase()))
            .collect::<Vec<_>>();
        let http_operations = equivalents
            .iter()
            .map(|(method, path)| {
                base::serde_json::json!({
                    "method": method.to_uppercase(),
                    "path": format!("/openapi/v1{path}"),
                    "summary": openapi_operation_summary(method, path)
                })
            })
            .collect::<Vec<_>>();
        usage.insert(
            (*action).to_string(),
            base::serde_json::json!({
                "summary": summary,
                "target": mqtt_target_field(action),
                "required_scope": crate::integration::model::mqtt_action_scope(action),
                "payload_schema": mqtt_payload_schema_name(action),
                "result_schema": mqtt_result_schema_name(action),
                "http_equivalents": http,
                "http_equivalent_operations": http_operations
            }),
        );
    }
    usage.into()
}

fn mqtt_action_examples_contract() -> base::serde_json::Value {
    let schemas = mqtt_action_payload_schemas();
    let mut examples = base::serde_json::Map::new();
    for action in crate::integration::model::MQTT_COMMAND_ACTIONS {
        let payload = schema_example(&schemas[&mqtt_payload_schema_name(action)]);
        let result = schema_example(&schemas[&mqtt_result_schema_name(action)]);
        let command = mqtt_command_example(action, payload);
        let command_id = command["command_id"].clone();
        examples.insert(
            (*action).to_string(),
            base::serde_json::json!({
                "request": command,
                "success": {
                    "schema_version": "v1",
                    "integration_id": "partner-a",
                    "command_id": command_id,
                    "operation_id": command_id,
                    "action": action,
                    "state": "succeeded",
                    "error_code": null,
                    "result": result,
                    "occurred_at_ms": 1700000001000_i64
                },
                "failure": {
                    "schema_version": "v1",
                    "integration_id": "partner-a",
                    "command_id": command_id,
                    "operation_id": command_id,
                    "action": action,
                    "state": "failed",
                    "error_code": "invalid_command",
                    "result": null,
                    "occurred_at_ms": 1700000001000_i64
                }
            }),
        );
    }
    examples.into()
}

async fn asyncapi_document(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<base::serde_json::Value>, HttpError> {
    require_role(&state.auth, &headers, Role::Admin)?;
    Ok(Json(asyncapi_contract()))
}

pub fn asyncapi_contract() -> base::serde_json::Value {
    base::serde_json::json!({
        "asyncapi": "3.0.0",
        "info": {
            "title": "GMV Guard 三方 MQTT 接入",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "面向第三方业务系统的 MQTT 消息契约。以 Guard 为观察方描述订阅和发布方向，Payload 统一使用 UTF-8 JSON。"
        },
        "servers": {
            "mqttV3": {"host": "{broker}", "protocol": "mqtt", "protocolVersion": "3.1.1", "description": "MQTT Runtime 配置 protocol_version=v3 时使用 MQTT V3.1.1。"},
            "mqttV5": {"host": "{broker}", "protocol": "mqtt", "protocolVersion": "5.0", "description": "MQTT Runtime 配置 protocol_version=v5 时使用 MQTT V5.0。"}
        },
        "operations": {
            "receiveIntegrationCommand": {
                "action": "receive",
                "channel": {"$ref": "#/channels/commands"},
                "summary": "Guard 接收第三方命令",
                "description": "第三方以 QoS 1、retain=false 发布。先订阅结果 Topic，再发布命令；PUBACK 仅表示 Broker 收到，不表示业务成功。"
            },
            "sendCommandResult": {
                "action": "send",
                "channel": {"$ref": "#/channels/commandResults"},
                "summary": "Guard 发布命令终态",
                "description": "调用方按 command_id 关联结果。state=succeeded 时读取 result；state=failed 时读取 error_code。"
            },
            "sendIntegrationEvent": {
                "action": "send",
                "channel": {"$ref": "#/channels/events"},
                "summary": "Guard 发布业务事件",
                "description": "仅发布应用已配置 mapping 的事件；消费方按 event_id 幂等。"
            }
        },
        "channels": {
            "commands": {
                "address": "gmv/commands/{integration_id}",
                "description": "第三方发布命令，Guard 订阅并执行。必须使用应用配置中的精确 command_topic；推荐 QoS 1、retain=false。",
                "parameters": {"integration_id": {"$ref": "#/components/parameters/integrationId"}},
                "messages": {"command": {"$ref": "#/components/messages/IntegrationCommand"}}
            },
            "commandResults": {
                "address": "gmv/command-results/{integration_id}",
                "description": "Guard 发布命令执行终态，第三方必须在发命令前订阅应用配置中的精确 result_topic。媒体成功结果含播放/上传 endpoint。",
                "parameters": {"integration_id": {"$ref": "#/components/parameters/integrationId"}},
                "messages": {"result": {"$ref": "#/components/messages/CommandResult"}}
            },
            "events": {
                "address": "gmv/events/{integration_id}/{event_type}",
                "description": "Guard 以 QoS 1、retain=false 发布已配置映射的业务事件，第三方按 event_id 幂等消费。event_type 中的点号在 Topic 中展开为斜杠，例如 session.alarm 发布到 gmv/events/{integration_id}/session/alarm。",
                "x-gmv-event-types": integration_callback_events_contract(),
                "parameters": {
                    "integration_id": {"$ref": "#/components/parameters/integrationId"},
                    "event_type": {"$ref": "#/components/parameters/eventType"}
                },
                "messages": {
                    "event": {"$ref": "#/components/messages/EventEnvelope"}
                }
            }
        },
        "components": {
            "parameters": {
                "integrationId": {"description": "第三方应用唯一标识。"},
                "eventType": {
                    "description": "事件类型的 Topic 后缀；契约事件名中的点号按斜杠展开。",
                    "enum": crate::integration::model::INTEGRATION_CALLBACK_EVENTS.iter().map(|event| event.event_type.replace('.', "/")).collect::<Vec<_>>()
                }
            },
            "schemas": mqtt_action_payload_schemas(),
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
                            "command_id": {"type": "string", "minLength": 1, "maxLength": 128, "pattern": "^[^\\s]+$", "description": "第三方生成的全局唯一幂等命令标识。同一业务重试必须复用；Guard 在持久化窗口内不会重复执行。"},
                            "issued_at_ms": {"type": "integer", "format": "int64", "description": "命令签发时间，Unix 毫秒时间戳。应使用调用方当前时间。"},
                            "expires_at_ms": {"type": "integer", "format": "int64", "description": "命令过期时间，必须不早于 issued_at_ms，且二者差值不超过 300000 ms；Guard 收到时已过期则拒绝。"},
                            "action": {"type": "string", "enum": crate::integration::model::MQTT_COMMAND_ACTIONS, "description": "应用已获授权的命令动作。"},
                            "target": {"type": "string", "minLength": 1, "description": "action 的主目标：媒体启动/PTZ 通常为 device_id；stream.stop 为 stream_id；ai.cancel 为 task_id；票据续期为 token。"},
                            "payload": {
                                "description": "必须与 action 对应；字段名、枚举、范围、默认值和联动规则见对应 Payload schema。",
                                "oneOf": mqtt_payload_refs()
                            }
                        },
                        "examples": mqtt_command_examples()
                    }
                },
                "CommandResult": {
                    "name": "CommandResult",
                    "title": "命令结果",
                    "summary": "Guard 对第三方命令的受理或执行状态。",
                    "contentType": "application/json",
                    "payload": {
                        "type": "object",
                        "required": ["schema_version", "integration_id", "command_id", "operation_id", "action", "state", "result", "occurred_at_ms"],
                        "properties": {
                            "schema_version": {"type": "string", "const": "v1", "description": "命令结果 schema 版本。"},
                            "integration_id": {"type": "string", "description": "第三方应用标识。"},
                            "command_id": {"type": "string", "description": "对应第三方命令的幂等标识。"},
                            "operation_id": {"type": "string", "description": "Guard 内部业务操作标识。"},
                            "action": {"type": "string", "enum": crate::integration::model::MQTT_COMMAND_ACTIONS, "description": "该结果对应的命令动作；调用方仍应以 command_id 为主键关联。"},
                            "state": {"type": "string", "enum": ["succeeded", "failed"], "description": "命令执行终态。"},
                            "error_code": {"type": ["string", "null"], "description": "失败时的稳定错误代码；成功时为 null。"},
                            "result": {
                                "description": "成功时为 action 对应的业务结果；失败时为 null。媒体启动结果至少包含后续停止所需的 stream_id，并在可播放/上传时包含 endpoint。",
                                "oneOf": mqtt_result_refs()
                            },
                            "occurred_at_ms": {"type": "integer", "description": "结果产生的 Unix 毫秒时间戳。"}
                        },
                        "examples": [
                            {"name": "stream.start succeeded", "payload": {"schema_version": "v1", "integration_id": "partner-a", "command_id": "cmd-001", "operation_id": "cmd-001", "action": "stream.start", "state": "succeeded", "error_code": null, "result": {"stream_id": "stream-001", "device_id": "device-001", "channel_id": "channel-001", "endpoint": "https://media.example/live/stream-001.flv?token=REDACTED", "subscription_id": "subscription-001", "state": "running"}, "occurred_at_ms": 1700000001000_i64}},
                            {"name": "command failed", "payload": {"schema_version": "v1", "integration_id": "partner-a", "command_id": "cmd-002", "operation_id": "cmd-002", "action": "stream.playback", "state": "failed", "error_code": "invalid_command", "result": null, "occurred_at_ms": 1700000001000_i64}}
                        ]
                    }
                },
                "EventEnvelope": {
                    "name": "EventEnvelope",
                    "title": "业务事件",
                    "summary": "Guard 向第三方发布的 JSON 事件信封；payload 由 event_type 决定。",
                    "contentType": "application/json",
                    "payload": {
                        "oneOf": crate::integration::model::INTEGRATION_CALLBACK_EVENTS
                            .iter()
                            .map(integration_callback_event_schema)
                            .collect::<Vec<_>>(),
                        "examples": crate::integration::model::INTEGRATION_CALLBACK_EVENTS
                            .iter()
                            .map(|event| base::serde_json::json!({
                                "name": event.event_type,
                                "payload": integration_callback_event_example(event)
                            }))
                            .collect::<Vec<_>>()
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
            "description": "协议版本由 Guard 部署级 MQTT Runtime 统一选择；两个版本使用相同 JSON Payload。"
        },
        "x-gmv-mqtt-workflow": {
            "qos": 1,
            "retain": false,
            "steps": [
                "使用交付的 Broker TLS/账号连接，并按 Guard MQTT Runtime 当前 active revision 的协议版本接入",
                "先订阅应用配置中的精确 result_topic；如需事件，再订阅获授权的 event topic",
                "生成全局唯一 command_id，issued_at_ms 使用当前 Unix 毫秒，expires_at_ms 与其差值不得超过 300000",
                "向精确 command_topic 发布 UTF-8 JSON；PUBACK 只表示 Broker 收到",
                "按 command_id 等待 result_topic 终态；succeeded 读取 result，failed 读取 error_code",
                "媒体命令使用 result.endpoint，保存 result.stream_id；业务结束后用 stream.stop 主动释放"
            ],
            "retry": "网络超时或结果暂未到达时，重发同一业务命令必须复用原 command_id；不得用新 command_id 盲目重试。",
            "security": "endpoint 可能包含访问 token，应作为凭据处理，不写日志、不拼入错误信息、不转发给未授权终端。"
        },
        "x-gmv-action-usage": mqtt_action_usage_contract(),
        "x-gmv-action-examples": mqtt_action_examples_contract(),
        "x-gmv-event-types": integration_callback_events_contract(),
        "x-gmv-http-mqtt-capabilities": integration_capabilities_contract()
    })
}

async fn api_manifest(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<base::serde_json::Value>, HttpError> {
    require_role(&state.auth, &headers, Role::Admin)?;
    Ok(Json(api_manifest_contract()))
}

pub fn api_manifest_contract() -> base::serde_json::Value {
    base::serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "http": {"base_path": "/openapi/v1", "methods": ["GET", "POST"], "auth": "GMV-HMAC-SHA256-V1", "request_id_header": REQUEST_ID_HEADER, "idempotency_window_ms": HTTP_IDEMPOTENCY_TTL_MS},
        "mqtt": {"protocol_versions": ["v3", "v5"], "default": "v3", "qos": 1, "command_actions": crate::integration::model::MQTT_COMMAND_ACTIONS},
        "scopes": ["*", "devices:read", "devices:write", "devices:control", "images:read", "audio:control", "streams:read", "streams:write", "streams:preview", "streams:playback", "recordings:read", "recordings:write", "events:read", "ai:read", "ai:write", "nodes:read", "leases:read", "runtime:read"],
        "capabilities": integration_capabilities_contract()
    })
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
        .route("/gb28181/broadcasts/start", post(start_broadcast_operation))
        .route(
            "/gb28181/broadcasts/{broadcast_id}",
            get(get_broadcast_operation),
        )
        .route(
            "/gb28181/broadcasts/{broadcast_id}/targets/{leg_id}/stop",
            post(stop_broadcast_target),
        )
        .route(
            "/gb28181/broadcasts/{broadcast_id}/stop-all",
            post(stop_broadcast_operation),
        )
        .route("/devices", get(devices))
        .route("/devices/{device_id}/preview", post(preview))
        .route("/devices/{device_id}/playback", post(playback))
        .route("/devices/{device_id}/download", post(download))
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
        Ok(OpenBusinessAuthentication::Execute {
            request,
            principal,
            command_id,
        }) => {
            let response = integration_principal::scope(principal, next.run(request)).await;
            if let Some(command_id) = command_id {
                persist_open_business_response(&state, &command_id, response).await
            } else {
                response
            }
        }
        Ok(OpenBusinessAuthentication::Replay(response)) => response,
        Err(error) => error.into_response(),
    }
}

enum OpenBusinessAuthentication {
    Execute {
        request: Request<Body>,
        principal: IntegrationPrincipal,
        command_id: Option<String>,
    },
    Replay(Response),
}

async fn authenticate_open_business_request_inner(
    state: &HttpState,
    request: Request<Body>,
) -> Result<OpenBusinessAuthentication, HttpError> {
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
    let secret = cipher.decrypt(&credential.secret_ciphertext).await?;
    let method = request.method().as_str().to_string();
    let request_id = if request.method() == Method::POST {
        let request_id = request
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| HttpError::bad_request("X-GMV-Request-ID is required for POST"))?;
        validate_request_id(&request_id)?;
        Some(request_id)
    } else {
        None
    };
    let signed_uri = request
        .extensions()
        .get::<OriginalUri>()
        .map(|value| &value.0)
        .unwrap_or_else(|| request.uri());
    let path = signed_uri.path().to_string();
    let query = signed_uri.query().unwrap_or_default().to_string();
    let (parts, body) = request.into_parts();
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
            request_id: request_id.as_deref().unwrap_or_default(),
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
    let principal = IntegrationPrincipal {
        integration_id: integration.integration_id,
        scope: scope.to_string(),
    };
    let command_id = if let Some(request_id) = request_id {
        let repository = state
            .commands
            .as_ref()
            .ok_or_else(|| HttpError::internal("command repository is not configured"))?;
        let command_id = http_command_id(&principal.integration_id, &request_id);
        let action = format!("{} {}", method.to_ascii_uppercase(), path);
        let request_hash =
            body_sha256(format!("{}\n{}\n{}\n{}", method, path, query, content_sha256).as_bytes());
        match repository
            .claim_http(
                &command_id,
                &principal.integration_id,
                &request_id,
                &action,
                &request_hash,
                now_ms.saturating_add(HTTP_IDEMPOTENCY_TTL_MS),
                now_ms,
            )
            .await?
        {
            HttpCommandClaim::Claimed { command_id, .. } => Some(command_id),
            HttpCommandClaim::Pending { operation_id } => {
                return Err(HttpError::from_operation(
                    GuardError::Conflict("request is already in progress".to_string()),
                    &operation_id,
                ));
            }
            HttpCommandClaim::Completed {
                operation_id: _,
                status,
                response_body,
            } => {
                let status = StatusCode::from_u16(status)
                    .map_err(|_| HttpError::internal("stored HTTP status is invalid"))?;
                let mut response = Response::new(Body::from(response_body));
                *response.status_mut() = status;
                response.headers_mut().insert(
                    CONTENT_TYPE,
                    HeaderValue::from_static("application/json; charset=utf-8"),
                );
                return Ok(OpenBusinessAuthentication::Replay(response));
            }
        }
    } else {
        None
    };
    Ok(OpenBusinessAuthentication::Execute {
        request: Request::from_parts(parts, Body::from(body)),
        principal,
        command_id,
    })
}

async fn persist_open_business_response(
    state: &HttpState,
    command_id: &str,
    response: Response,
) -> Response {
    let status = response.status();
    let (parts, body) = response.into_parts();
    let body = match to_bytes(body, MAX_OPEN_API_RESPONSE_BYTES).await {
        Ok(body) => body,
        Err(_) => return HttpError::internal("open API response exceeds 16 MiB").into_response(),
    };
    let Some(repository) = &state.commands else {
        return HttpError::internal("command repository is not configured").into_response();
    };
    let now_ms = match http_now_ms() {
        Ok(now_ms) => now_ms,
        Err(error) => return error.into_response(),
    };
    if let Err(error) = repository
        .complete_http(command_id, status.as_u16(), &body, now_ms)
        .await
    {
        return HttpError::from(error).into_response();
    }
    Response::from_parts(parts, Body::from(body))
}

fn required_header(headers: &HeaderMap, name: &'static str) -> Result<String, HttpError> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(HttpError::unauthorized)
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
    if path.contains("/broadcast") {
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
    session.require_role(Role::Viewer)?;
    let events = state.api.poll_events(EventQuery::default())?;
    Ok(Json(DashboardResponse {
        node_count: state.api.list_nodes().len(),
        event_count: events.items.len(),
        next_after_id: events.next_after_id,
    }))
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
pub(crate) struct NodeResponse {
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
pub(crate) struct LeaseResponse {
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
    session.require_role(Role::Viewer)?;
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

#[derive(Debug, Clone)]
struct RequestPrincipal {
    username: String,
    role: Role,
    ui_session: Option<UiSession>,
}

impl RequestPrincipal {
    fn from_ui(session: UiSession) -> Self {
        Self {
            username: session.username.clone(),
            role: session.role,
            ui_session: Some(session),
        }
    }

    fn from_integration(principal: IntegrationPrincipal) -> Self {
        Self {
            username: principal.identity(),
            role: Role::Operator,
            ui_session: None,
        }
    }

    fn require_role(&self, required: Role) -> Result<(), HttpError> {
        if self.role.allows(required) {
            Ok(())
        } else {
            Err(HttpError::forbidden("caller role is not allowed"))
        }
    }
}

fn authenticated(auth: &AuthState, headers: &HeaderMap) -> Result<RequestPrincipal, HttpError> {
    if let Some(principal) = integration_principal::current() {
        return Ok(RequestPrincipal::from_integration(principal));
    }
    authenticated_with_token(auth, headers).map(|(_, session)| RequestPrincipal::from_ui(session))
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

#[derive(base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct CreatedIntegrationCredentialResponse {
    credential: IntegrationCredentialSummary,
    secret: String,
}

#[derive(base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct RevealIntegrationCredentialRequest {
    password: String,
}

#[derive(base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct RevealedIntegrationCredentialResponse {
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
struct SaveMqttRuntimeRequest {
    protocol_version: String,
    broker: String,
    port: u16,
    client_id: String,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
    tls: bool,
    publish_event_ttl_sec: i64,
    expected_config_version: i64,
    request_id: String,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct SaveBusinessIntegrationRequest {
    request_id: String,
    name: String,
    transport: String,
    inbound_enabled: bool,
    outbound_enabled: bool,
    enabled: bool,
    scopes: Vec<String>,
    #[serde(default)]
    expires_at_ms: Option<i64>,
    #[serde(default)]
    expected_config_version: i64,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct BusinessIntegrationResponse {
    state: &'static str,
    integration: Option<Integration>,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct IntegrationMasterKeyResponse {
    configured: bool,
    key_version: i64,
    created_at_ms: i64,
    updated_by: String,
    updated_at_ms: i64,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct RotateIntegrationMasterKeyRequest {
    request_id: String,
    expected_key_version: i64,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct MqttRuntimeResponse {
    configured: bool,
    broker_connected: bool,
    config: Option<crate::integration::model::MqttRuntimeConfig>,
    connection_scope: &'static str,
    qos: u8,
    retain: bool,
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

async fn integration_master_key(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<IntegrationMasterKeyResponse>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    let value = require_integration_repository(&state)?
        .master_key()
        .await?
        .ok_or_else(|| GuardError::Conflict("integration master key is missing".to_string()))?;
    Ok(Json(integration_master_key_response(value)))
}

async fn rotate_integration_master_key(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<RotateIntegrationMasterKeyRequest>,
) -> Result<Json<IntegrationMasterKeyResponse>, HttpError> {
    let session = require_write(&state.auth, &headers, Role::Admin)?;
    validate_request_id(&request.request_id)?;
    let secrets = state
        .integration_secrets
        .as_ref()
        .ok_or_else(|| GuardError::Conflict("integration master key is unavailable".to_string()))?;
    let repository = require_integration_repository(&state)?;
    let new_key_material = IntegrationSecretCipher::random_key_material();
    let new_cipher = IntegrationSecretCipher::from_base64_key_no_pad(&new_key_material)?;
    let now_ms = http_now_ms()?;
    let mut current_cipher = secrets.write().await;
    let value = repository
        .rotate_master_key(
            &current_cipher,
            &new_cipher,
            &new_key_material,
            request.expected_key_version,
            &session.username,
            &format!("audit_{}", Uuid::new_v4().simple()),
            now_ms,
        )
        .await?;
    *current_cipher = new_cipher;
    Ok(Json(integration_master_key_response(value)))
}

fn integration_master_key_response(
    value: crate::integration::model::IntegrationMasterKey,
) -> IntegrationMasterKeyResponse {
    IntegrationMasterKeyResponse {
        configured: true,
        key_version: value.key_version,
        created_at_ms: value.created_at_ms,
        updated_by: value.updated_by,
        updated_at_ms: value.updated_at_ms,
    }
}

async fn list_integrations(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Integration>>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    let repository = require_integration_repository(&state)?;
    Ok(Json(
        repository
            .business_integration()
            .await?
            .into_iter()
            .collect(),
    ))
}

async fn get_business_integration(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<BusinessIntegrationResponse>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    let repository = require_integration_repository(&state)?;
    if let Some(integration) = repository.business_integration().await? {
        return Ok(Json(BusinessIntegrationResponse {
            state: "ready",
            integration: Some(integration),
        }));
    }
    Ok(Json(BusinessIntegrationResponse {
        state: "unconfigured",
        integration: None,
    }))
}

async fn save_business_integration(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<SaveBusinessIntegrationRequest>,
) -> Result<Json<Integration>, HttpError> {
    let session = require_write(&state.auth, &headers, Role::Admin)?;
    if request.request_id.trim().is_empty() {
        return Err(GuardError::InvalidConfig("request_id is required".to_string()).into());
    }
    let repository = require_integration_repository(&state)?;
    let now_ms = http_now_ms()?;
    let transport = IntegrationTransport::parse(&request.transport.to_ascii_uppercase())?;
    let mut switched_from = None;
    let mut value = if let Some(mut value) = repository.business_integration().await? {
        if value.config_version != request.expected_config_version {
            return Err(GuardError::Conflict(format!(
                "integration config version changed: expected {}, actual {}",
                request.expected_config_version, value.config_version
            ))
            .into());
        }
        if value.transport != transport {
            if value.enabled {
                return Err(GuardError::Conflict(
                    "disable the business integration before switching transport".to_string(),
                )
                .into());
            }
            let (commands, outbox) = repository
                .transport_switch_blockers(&value.integration_id)
                .await?;
            let active_operations = state
                .api
                .list_operations()
                .into_iter()
                .filter(|operation| {
                    operation.requested_by == format!("integration:{}", value.integration_id)
                        && !operation.status.is_terminal()
                })
                .count();
            if commands > 0 || outbox > 0 || active_operations > 0 {
                return Err(GuardError::Conflict(format!(
                    "transport switch blocked: commands={commands}, outbox={outbox}, operations={active_operations}"
                ))
                .into());
            }
            switched_from = Some(value.transport);
            value.transport = transport;
        }
        value.config_version = value.config_version.saturating_add(1);
        value
    } else {
        Integration {
            integration_id: format!("int_{}", Uuid::new_v4().simple()),
            name: String::new(),
            transport,
            inbound_enabled: false,
            outbound_enabled: false,
            enabled: false,
            scopes: Vec::new(),
            expires_at_ms: None,
            config_version: 1,
            created_by: session.username.clone(),
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        }
    };
    value.name = request.name.trim().to_string();
    value.inbound_enabled = request.inbound_enabled;
    value.outbound_enabled = request.outbound_enabled;
    value.enabled = request.enabled;
    value.scopes = request.scopes;
    value.expires_at_ms = request.expires_at_ms;
    value.updated_at_ms = now_ms;
    value.validate(now_ms)?;
    let is_new = repository.get(&value.integration_id).await?.is_none();
    repository.upsert(&value).await?;
    if let Some(previous) = switched_from {
        repository
            .deactivate_transport(&value.integration_id, previous, now_ms)
            .await?;
    }
    if value.transport == IntegrationTransport::Http
        && repository
            .http_config(&value.integration_id)
            .await?
            .is_none()
    {
        repository
            .upsert_http_config(&default_http_config(&value.integration_id, now_ms))
            .await?;
    }
    append_integration_audit(
        repository,
        Some(&value.integration_id),
        &session.username,
        if is_new {
            "integration.create"
        } else {
            "integration.update"
        },
        &value.integration_id,
        "updated",
        now_ms,
    )
    .await?;
    Ok(Json(value))
}

async fn get_integration(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(integration_id): Path<String>,
) -> Result<Json<Integration>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    let repository = require_integration_repository(&state)?;
    require_business_integration(repository, &integration_id).await?;
    let value = repository
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
    let repository = require_integration_repository(&state)?;
    if repository.business_integration_id().await?.is_some() || !repository.list().await?.is_empty()
    {
        return Err(GuardError::Conflict(
            "the business integration already exists; edit it instead".to_string(),
        )
        .into());
    }
    repository.upsert(&value).await?;
    if transport == IntegrationTransport::Http {
        repository
            .upsert_http_config(&default_http_config(&integration_id, now_ms))
            .await?;
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
    require_business_integration(repository, &integration_id).await?;
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
    let repository = require_integration_repository(&state)?;
    require_business_integration(repository, &integration_id).await?;
    let values = repository
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
    require_business_integration(repository, &integration_id).await?;
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
    let cipher = state
        .integration_secrets
        .as_ref()
        .ok_or_else(|| GuardError::Conflict("integration master key is unavailable".to_string()))?;
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
        secret_ciphertext: cipher.encrypt(&secret).await?,
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
    require_business_integration(repository, &integration_id).await?;
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

async fn reveal_integration_credential(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((integration_id, credential_id)): Path<(String, String)>,
    Json(request): Json<RevealIntegrationCredentialRequest>,
) -> Result<Json<RevealedIntegrationCredentialResponse>, HttpError> {
    let session = require_write(&state.auth, &headers, Role::Admin)?;
    if request.password.is_empty()
        || !state
            .auth
            .verify_current_password(&session.username, &request.password)
            .map_err(HttpError::from_auth)?
    {
        return Err(HttpError::secondary_auth_failed());
    }
    let repository = require_integration_repository(&state)?;
    require_business_integration(repository, &integration_id).await?;
    let now_ms = http_now_ms()?;
    let credential = repository
        .list_credentials(&integration_id)
        .await?
        .into_iter()
        .find(|credential| credential.credential_id == credential_id)
        .ok_or_else(|| GuardError::NotFound(format!("credential {credential_id}")))?;
    if !credential.is_active_at(now_ms) {
        return Err(GuardError::Conflict(
            "only active integration credentials can be revealed".to_string(),
        )
        .into());
    }
    let cipher = state
        .integration_secrets
        .as_ref()
        .ok_or_else(|| GuardError::Conflict("integration master key is unavailable".to_string()))?;
    let secret = cipher.decrypt(&credential.secret_ciphertext).await?;
    append_integration_audit(
        repository,
        Some(&integration_id),
        &session.username,
        "credential.reveal",
        &credential_id,
        "revealed after secondary authentication",
        now_ms,
    )
    .await?;
    Ok(Json(RevealedIntegrationCredentialResponse { secret }))
}

async fn get_integration_http(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(integration_id): Path<String>,
) -> Result<Json<IntegrationHttpConfig>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    require_integration_transport(&state, &integration_id, IntegrationTransport::Http).await?;
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
    require_integration_transport(&state, &integration_id, IntegrationTransport::Mqtt).await?;
    let value = require_integration_repository(&state)?
        .mqtt_config(&integration_id)
        .await?
        .ok_or_else(|| GuardError::NotFound(format!("MQTT integration {integration_id}")))?;
    Ok(Json(value))
}

async fn integration_mqtt_runtime(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<MqttRuntimeResponse>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    let config = require_integration_repository(&state)?
        .mqtt_runtime_config()
        .await?;
    let broker_connected = mqtt_broker_connected(config.as_ref());
    Ok(Json(MqttRuntimeResponse {
        configured: config.is_some(),
        broker_connected,
        config,
        connection_scope: "deployment",
        qos: 1,
        retain: false,
    }))
}

fn mqtt_broker_connected(config: Option<&crate::integration::model::MqttRuntimeConfig>) -> bool {
    config.is_some_and(|value| {
        value.apply_state == MqttRuntimeApplyState::Connected
            && value.active_revision == Some(value.desired_revision)
    })
}

async fn update_integration_mqtt_runtime(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<SaveMqttRuntimeRequest>,
) -> Result<Json<crate::integration::model::MqttRuntimeConfig>, HttpError> {
    let session = require_write(&state.auth, &headers, Role::Admin)?;
    if request.request_id.trim().is_empty() {
        return Err(GuardError::InvalidConfig("request_id is required".to_string()).into());
    }
    let repository = require_integration_repository(&state)?;
    let business = repository.business_integration().await?.ok_or_else(|| {
        GuardError::Conflict(
            "enable the MQTT business integration before configuring MQTT runtime".to_string(),
        )
    })?;
    if !business.enabled || business.transport != IntegrationTransport::Mqtt {
        return Err(GuardError::Conflict(
            "enable the MQTT business integration before configuring MQTT runtime".to_string(),
        )
        .into());
    }
    let current = repository.mqtt_runtime_config().await?;
    let current_revision = if let Some(current) = &current {
        repository
            .mqtt_runtime_revision(current.desired_revision)
            .await?
    } else {
        None
    };
    let username = request.username.map(|value| value.trim().to_string());
    let password_ciphertext = match (username.as_ref(), request.password.as_deref()) {
        (None, None) => None,
        (None, Some(_)) => {
            return Err(GuardError::InvalidConfig(
                "MQTT username is required when password is supplied".to_string(),
            )
            .into());
        }
        (Some(_), Some(password)) if !password.is_empty() => Some(
            state
                .integration_secrets
                .as_ref()
                .ok_or_else(|| {
                    GuardError::Conflict("integration master key is unavailable".to_string())
                })?
                .encrypt(password)
                .await?,
        ),
        (Some(username), None) => {
            let ciphertext = current_revision
                .as_ref()
                .filter(|current| current.username.as_ref() == Some(username))
                .and_then(|current| current.password_ciphertext.clone());
            if let Some(ciphertext) = ciphertext.as_deref()
                && state
                    .integration_secrets
                    .as_ref()
                    .ok_or_else(|| {
                        GuardError::Conflict("integration master key is unavailable".to_string())
                    })?
                    .decrypt(ciphertext)
                    .await
                    .is_err()
            {
                return Err(GuardError::user_visible(
                    "mqtt_password_reentry_required",
                    "stored MQTT password cannot be decrypted with the current integration master key",
                    "已存 MQTT 密码无法使用当前主密钥解密，请重新输入密码后保存",
                    false,
                    BTreeMap::new(),
                )
                .into());
            }
            ciphertext
        }
        (Some(_), Some(_)) => None,
    };
    let now_ms = http_now_ms()?;
    let value = crate::integration::model::MqttRuntimeRevision {
        revision: 0,
        protocol_version: request.protocol_version.to_ascii_lowercase(),
        broker: request.broker.trim().to_string(),
        port: request.port,
        client_id: request.client_id.trim().to_string(),
        username,
        password_ciphertext,
        tls: request.tls,
        publish_event_ttl_sec: request.publish_event_ttl_sec,
        created_by: session.username.clone(),
        created_at_ms: now_ms,
    };
    value.validate()?;
    let saved = repository
        .save_mqtt_runtime_config(&value, request.expected_config_version)
        .await?;
    append_integration_audit(
        repository,
        repository.business_integration_id().await?.as_deref(),
        &session.username,
        "mqtt.runtime.update",
        "business",
        "pending",
        now_ms,
    )
    .await?;
    Ok(Json(saved))
}

async fn list_integration_mappings(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(integration_id): Path<String>,
) -> Result<Json<Vec<IntegrationMapping>>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    require_business_integration(require_integration_repository(&state)?, &integration_id).await?;
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
    require_business_integration(require_integration_repository(&state)?, &integration_id).await?;
    if request.direction != "OUTBOUND"
        || !matches!(request.destination_kind.as_str(), "HTTP" | "MQTT")
        || !valid_event_mapping_source(&request.source_type)
        || !mapping_source_matches_callback_contract(&request.source_type)
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
    let existing_mapping = if let Some(mapping_id) = request.mapping_id.as_deref() {
        Some(
            repository
                .list_mappings(&integration_id)
                .await?
                .into_iter()
                .find(|mapping| mapping.mapping_id == mapping_id)
                .ok_or_else(|| GuardError::NotFound(format!("mapping {mapping_id}")))?,
        )
    } else {
        None
    };
    match integration.transport {
        IntegrationTransport::Http => {
            let config = repository
                .http_config(&integration_id)
                .await?
                .ok_or_else(|| {
                    GuardError::InvalidConfig("HTTP integration config missing".to_string())
                })?;
            if request.destination_kind != "HTTP"
                || (request.enabled
                    && config.callback_url.as_deref() != Some(request.destination.as_str()))
                || (!request.enabled
                    && existing_mapping
                        .as_ref()
                        .is_none_or(|mapping| mapping.destination_kind != "HTTP"))
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

fn valid_event_mapping_source(source_type: &str) -> bool {
    !source_type.is_empty()
        && source_type.len() <= 255
        && source_type.split('.').all(|segment| {
            matches!(segment, "*" | "**")
                || (!segment.is_empty()
                    && segment
                        .bytes()
                        .all(|value| value.is_ascii_alphanumeric() || matches!(value, b'_' | b'-')))
        })
}

fn mapping_source_matches_callback_contract(source_type: &str) -> bool {
    crate::integration::model::INTEGRATION_CALLBACK_EVENTS
        .iter()
        .any(|event| topic_matches(source_type, event.event_type))
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

async fn require_integration_transport(
    state: &HttpState,
    integration_id: &str,
    expected: IntegrationTransport,
) -> Result<(), HttpError> {
    let repository = require_integration_repository(state)?;
    require_business_integration(repository, integration_id).await?;
    let value = repository
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

async fn require_business_integration(
    repository: &IntegrationRepository,
    integration_id: &str,
) -> Result<(), HttpError> {
    if repository.business_integration_id().await?.as_deref() != Some(integration_id) {
        return Err(GuardError::InvalidIdentity(
            "integration is not the active business integration".to_string(),
        )
        .into());
    }
    Ok(())
}

fn default_http_config(integration_id: &str, now_ms: i64) -> IntegrationHttpConfig {
    IntegrationHttpConfig {
        integration_id: integration_id.to_string(),
        callback_url: None,
        callback_timeout_ms: 5_000,
        private_network_policy: "deny".to_string(),
        private_network_allowlist: Vec::new(),
        max_attempts: 5,
        event_ttl_ms: 259_200_000,
        max_response_bytes: 65_536,
        updated_at_ms: now_ms,
    }
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
    session.require_role(Role::Viewer)?;
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
    session.require_role(Role::Operator)?;
    let ui_session = session
        .ui_session
        .as_ref()
        .ok_or_else(HttpError::unauthorized)?;
    verify_csrf(&state.auth, ui_session, &headers)?;
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
    session: &RequestPrincipal,
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

fn http_stream_profile_name(value: i32) -> String {
    match VideoStreamProfile::try_from(value).unwrap_or(VideoStreamProfile::Unspecified) {
        VideoStreamProfile::Sub => "sub",
        VideoStreamProfile::Main => "main",
        VideoStreamProfile::Unspecified => "",
    }
    .to_string()
}

fn http_profile_verification_name(value: i32) -> String {
    match StreamProfileVerification::try_from(value)
        .unwrap_or(StreamProfileVerification::Unspecified)
    {
        StreamProfileVerification::Confirmed => "confirmed",
        StreamProfileVerification::Unverified => "unverified",
        StreamProfileVerification::Unspecified => "",
    }
    .to_string()
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
    broadcast_codec: String,
    #[serde(default)]
    broadcast_sample_rate: u32,
    #[serde(default)]
    broadcast_channel_count: u32,
    #[serde(default)]
    broadcast_frame_duration_ms: u32,
    #[serde(default)]
    playback_id: String,
    #[serde(default)]
    stream_profile: String,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct BroadcastTargetRequest {
    device_id: String,
    channel_id: String,
    #[serde(default)]
    session_node_id: String,
    #[serde(default)]
    trans_mode: String,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct BroadcastOperationRequest {
    request_id: String,
    #[serde(default)]
    token: String,
    #[serde(default)]
    default_trans_mode: String,
    #[serde(default = "default_broadcast_codec")]
    codec: String,
    #[serde(default = "default_broadcast_sample_rate")]
    sample_rate: u32,
    #[serde(default = "default_broadcast_channel_count")]
    channel_count: u32,
    #[serde(default = "default_broadcast_frame_duration_ms")]
    frame_duration_ms: u32,
    targets: Vec<BroadcastTargetRequest>,
}

#[derive(Debug, Default, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct BroadcastStopRequest {
    #[serde(default)]
    request_id: String,
}

fn default_broadcast_codec() -> String {
    "PCMA".to_string()
}

fn default_broadcast_sample_rate() -> u32 {
    8_000
}

fn default_broadcast_channel_count() -> u32 {
    1
}

fn default_broadcast_frame_duration_ms() -> u32 {
    20
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
        broadcast_codec: request.broadcast_codec.clone(),
        broadcast_sample_rate: request.broadcast_sample_rate,
        broadcast_channel_count: request.broadcast_channel_count,
        broadcast_frame_duration_ms: request.broadcast_frame_duration_ms,
        playback_id: request.playback_id.clone(),
        broadcast_id: String::new(),
        broadcast_leg_id: String::new(),
        expected_stream_node_id: String::new(),
        stream_profile: request.stream_profile.clone(),
    }
}

fn issue_playback_ticket(
    state: &HttpState,
    mut stream: StreamSummary,
    ui_session_token: &str,
    session: &RequestPrincipal,
    required_role: Role,
) -> Result<StreamSummary, HttpError> {
    if stream.endpoint.is_empty() || stream.state != StreamSummaryState::Running {
        return Ok(stream);
    }
    let token = Uuid::new_v4().to_string();
    let now_ms = http_now_ms()?;
    let ui_session_token = playback_session_token(state, ui_session_token, session)?;
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
        ui_session_token,
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
    session: &RequestPrincipal,
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
    let ui_session_token = playback_session_token(state, ui_session_token, session)?;
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
        ui_session_token,
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

fn playback_session_token(
    state: &HttpState,
    candidate: &str,
    session: &RequestPrincipal,
) -> Result<String, HttpError> {
    if !candidate.is_empty() || !session.username.starts_with("integration:") {
        return Ok(candidate.to_string());
    }
    state
        .auth
        .issue_service_session(
            &session.username,
            Role::Viewer,
            Duration::from_millis(INTEGRATION_PLAYBACK_TOKEN_TTL_MS as u64),
        )
        .map(|(token, _)| token)
        .map_err(Into::into)
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
pub(crate) struct GbDeviceRequest {
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
pub(crate) struct GbDeviceResponse {
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
pub(crate) struct GbDevicePageResponse {
    items: Vec<GbDeviceResponse>,
    total: u64,
    page: u32,
    page_size: u32,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
pub(crate) struct GbChannelResponse {
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
    broadcast_enable: i64,
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
pub(crate) struct GbChannelRequest {
    #[serde(default)]
    alias_name: String,
    #[serde(default)]
    snapshot: i64,
    #[serde(default)]
    over_pic_id: String,
    #[serde(default)]
    ptz_enable: i64,
    #[serde(default)]
    broadcast_enable: i64,
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
pub(crate) struct GbChannelImageResponse {
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
pub(crate) struct GbChannelImagePageResponse {
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
pub(crate) struct GbChannelImageAccessResponse {
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
pub(crate) struct GbResourceResponse {
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
pub(crate) struct GbSessionConfigResponse {
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
    #[serde(default)]
    stream_profile: String,
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
pub(crate) struct CloudRecordingSummary {
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
pub(crate) struct CloudRecordingListResponse {
    items: Vec<CloudRecordingSummary>,
    total: u64,
    page: u32,
    page_size: u32,
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
pub(crate) struct CloudRecordingAccessResponse {
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
pub(crate) struct GbSnapshotResponse {
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
pub(crate) struct GbChannelRecordsResponse {
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

pub(crate) fn gb_device_request(request: GbDeviceRequest) -> RpcGbDevice {
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

pub(crate) fn gb_device_response(record: RpcGbDevice) -> GbDeviceResponse {
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

pub(crate) fn gb_device_page_response(page: GbDevicePage) -> GbDevicePageResponse {
    GbDevicePageResponse {
        items: page.devices.into_iter().map(gb_device_response).collect(),
        total: page.total,
        page: page.page,
        page_size: page.page_size,
    }
}

pub(crate) fn gb_channel_response(record: RpcGbChannel) -> GbChannelResponse {
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
        broadcast_enable: record.broadcast_enable,
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

pub(crate) fn gb_channel_records_response(
    record: RpcGbChannelRecordsResponse,
) -> GbChannelRecordsResponse {
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

pub(crate) fn gb_resource_response(record: RpcGbResource) -> GbResourceResponse {
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

pub(crate) fn gb_channel_request(
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
        broadcast_enable: request.broadcast_enable,
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
        "{path}, req: device_id={}, request_id={}, channel_id={}, session_node_id={}, token={}, start_time_sec={}, end_time_sec={}, trans_mode={}, output_type={}, broadcast_codec={}, broadcast_sample_rate={}, broadcast_channel_count={}, broadcast_frame_duration_ms={}",
        device_id,
        request.request_id,
        request.channel_id,
        request.session_node_id,
        redacted(&request.token),
        request.start_time_sec,
        request.end_time_sec,
        request.trans_mode,
        request.output_type,
        request.broadcast_codec,
        request.broadcast_sample_rate,
        request.broadcast_channel_count,
        request.broadcast_frame_duration_ms
    );
}

pub(crate) fn gb_session_config_response(
    record: GbSessionConfigSummary,
) -> GbSessionConfigResponse {
    GbSessionConfigResponse {
        domain: record.domain,
        domain_id: record.domain_id,
        wan_ip: record.wan_ip,
        wan_port: record.wan_port,
    }
}
pub(crate) fn gb_channel_image_response(record: RpcGbChannelImage) -> GbChannelImageResponse {
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
        broadcast_codec: String::new(),
        broadcast_sample_rate: 0,
        broadcast_channel_count: 0,
        broadcast_frame_duration_ms: 0,
        playback_id: request.playback_id,
        stream_profile: request.stream_profile,
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

pub(crate) fn cloud_recording_summary(
    recording: RpcCloudRecordingSummary,
) -> CloudRecordingSummary {
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

async fn start_broadcast_operation(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<BroadcastOperationRequest>,
) -> Result<(StatusCode, Json<BroadcastOperationSummary>), HttpError> {
    require_write(&state.auth, &headers, Role::Operator)?;
    if request.request_id.trim().is_empty() {
        return Err(HttpError::bad_request("request_id is required"));
    }
    if request.codec != "PCMA"
        || request.sample_rate != 8_000
        || request.channel_count != 1
        || request.frame_duration_ms != 20
    {
        return Err(HttpError::bad_request(
            "broadcast_profile_unsupported: expected PCMA/8000/mono/20ms",
        ));
    }
    let options = BroadcastOperationOptions {
        token: request.token,
        default_trans_mode: request.default_trans_mode,
        codec: request.codec,
        sample_rate: request.sample_rate,
        channel_count: request.channel_count,
        frame_duration_ms: request.frame_duration_ms,
        targets: request
            .targets
            .into_iter()
            .map(|target| BroadcastTargetOptions {
                device_id: target.device_id,
                channel_id: target.channel_id,
                session_node_id: target.session_node_id,
                trans_mode: target.trans_mode,
            })
            .collect(),
    };
    let summary = BusinessControl::new(state.api.store())
        .start_broadcast_operation(&request.request_id, options)
        .await?;
    Ok((StatusCode::CREATED, Json(summary)))
}

async fn get_broadcast_operation(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(broadcast_id): Path<String>,
) -> Result<Json<BroadcastOperationSummary>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    Ok(Json(
        BusinessControl::new(state.api.store()).get_broadcast_operation(&broadcast_id)?,
    ))
}

async fn stop_broadcast_target(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((broadcast_id, leg_id)): Path<(String, String)>,
    Json(request): Json<BroadcastStopRequest>,
) -> Result<Json<BroadcastOperationSummary>, HttpError> {
    require_write(&state.auth, &headers, Role::Operator)?;
    let operation_id = if request.request_id.trim().is_empty() {
        Uuid::now_v7().to_string()
    } else {
        request.request_id
    };
    Ok(Json(
        BusinessControl::new(state.api.store())
            .stop_broadcast_target(&operation_id, &broadcast_id, &leg_id)
            .await?,
    ))
}

async fn stop_broadcast_operation(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(broadcast_id): Path<String>,
    Json(request): Json<BroadcastStopRequest>,
) -> Result<Json<BroadcastOperationSummary>, HttpError> {
    require_write(&state.auth, &headers, Role::Operator)?;
    let operation_id = if request.request_id.trim().is_empty() {
        Uuid::now_v7().to_string()
    } else {
        request.request_id
    };
    Ok(Json(
        BusinessControl::new(state.api.store())
            .stop_broadcast_operation(&operation_id, &broadcast_id)
            .await?,
    ))
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
}

async fn start_media_operation_http<F, Fut>(
    state: HttpState,
    headers: HeaderMap,
    device_id: String,
    mut request: PreviewRequest,
    policy: DeviceStreamHttpPolicy<'_>,
    rpc_start: F,
) -> Result<(StatusCode, Json<MediaOperationSummary>), HttpError>
where
    F: FnOnce(BusinessControl, String, String, String, DeviceStreamOptions) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<StreamSummary, GuardError>> + Send + 'static,
{
    request.trans_mode = match request.trans_mode.trim().to_ascii_lowercase().as_str() {
        "" | "udp" => "udp".to_string(),
        "tcp_active" => "tcp_active".to_string(),
        "tcp_passive" => "tcp_passive".to_string(),
        _ => {
            return Err(HttpError::bad_request(
                "invalid_media_transport: expected udp, tcp_active, or tcp_passive",
            ));
        }
    };
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
                    requested_stream_profile: http_stream_profile_name(
                        item.requested_stream_profile,
                    ),
                    effective_stream_profile: http_stream_profile_name(
                        item.effective_stream_profile,
                    ),
                    stream_profile_verification: http_profile_verification_name(
                        item.stream_profile_verification,
                    ),
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

fn operation_visible_to(record: &OperationRecord, session: &RequestPrincipal) -> bool {
    session.role == Role::Admin || record.requested_by == session.username
}

fn require_operation_owner(
    record: &OperationRecord,
    session: &RequestPrincipal,
) -> Result<(), HttpError> {
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
    stream_summaries(&state.api.store())
}

pub(crate) fn stream_summaries(store: &InMemoryGuardStore) -> Vec<StreamSummary> {
    let leases = store.leases();
    store
        .routes()
        .into_iter()
        .filter(|route| !route.resource_id.starts_with("ai-"))
        .map(|route| {
            let owner = store.get_stream_session_owner(&route.resource_id);
            let lease = leases.iter().find(|lease| lease.route_id == route.route_id);
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
                mime_codec: String::new(),
                broadcast_profile: String::new(),
                requested_stream_profile: String::new(),
                effective_stream_profile: String::new(),
                stream_profile_verification: String::new(),
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
                state: if route.state == RouteState::Closed
                    || lease.is_some_and(|lease| lease.state == LeaseState::Released)
                {
                    StreamSummaryState::Stopped
                } else if lease
                    .map(|lease| {
                        lease.state == LeaseState::Failed || lease.state == LeaseState::Expired
                    })
                    .unwrap_or(false)
                    || matches!(route.state, RouteState::Orphaned | RouteState::Conflict)
                {
                    StreamSummaryState::Failed
                } else if route.state == RouteState::Reconciling {
                    StreamSummaryState::Stopping
                } else {
                    StreamSummaryState::Running
                },
            }
        })
        .collect()
}

fn real_ai_tasks(state: &HttpState) -> Vec<AiTaskSummary> {
    ai_task_summaries(&state.api.store())
}

pub(crate) fn ai_task_summaries(store: &InMemoryGuardStore) -> Vec<AiTaskSummary> {
    let leases = store.leases();
    store
        .routes()
        .into_iter()
        .filter(|route| route.resource_id.starts_with("ai-"))
        .map(|route| {
            let lease = leases.iter().find(|lease| lease.route_id == route.route_id);
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
                } else if route.state == RouteState::Closed
                    || lease.is_some_and(|lease| lease.state == LeaseState::Released)
                {
                    AiTaskSummaryState::Cancelled
                } else if matches!(route.state, RouteState::Orphaned | RouteState::Conflict) {
                    AiTaskSummaryState::Failed
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

fn require_role(
    auth: &AuthState,
    headers: &HeaderMap,
    role: Role,
) -> Result<RequestPrincipal, HttpError> {
    let session = authenticated(auth, headers)?;
    session.require_role(role)?;
    Ok(session)
}

fn require_write(
    auth: &AuthState,
    headers: &HeaderMap,
    role: Role,
) -> Result<RequestPrincipal, HttpError> {
    require_write_with_token(auth, headers, role).map(|(_, session)| session)
}

fn require_write_with_token(
    auth: &AuthState,
    headers: &HeaderMap,
    role: Role,
) -> Result<(String, RequestPrincipal), HttpError> {
    if let Some(principal) = integration_principal::current() {
        let principal = RequestPrincipal::from_integration(principal);
        principal.require_role(role)?;
        return Ok((String::new(), principal));
    }
    verify_origin(auth, headers)?;
    let (token, session) = authenticated_with_token(auth, headers)?;
    auth.require_role(&session, role)
        .map_err(|_| HttpError::forbidden("UI role is not allowed"))?;
    verify_csrf(auth, &session, headers)?;
    Ok((token, RequestPrincipal::from_ui(session)))
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

    fn secondary_auth_failed() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "secondary_auth_failed".to_string(),
            message: "secondary authentication failed".to_string(),
            user_message: Some("当前密码验证失败".to_string()),
            retryable: Some(false),
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
        | GmvGuardErrorCode::SnapshotRejected
        | GmvGuardErrorCode::StreamProfileMismatch => StatusCode::CONFLICT,
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
        asyncapi_contract, endpoint_with_playback_token, gb_preview_request,
        mapping_source_matches_callback_contract, media_startup_timeout_ms,
        mqtt_action_payload_schemas, mqtt_broker_connected, node_connection_label,
        node_health_label, node_scheduling_label, open_business_scope, openapi_contract,
        openapi_operation_parameters, openapi_operation_summary, openapi_request_body,
        openapi_responses, openapi_success_schema, playback_control_owner_matches,
        playback_token_from_endpoint, valid_event_mapping_source,
    };
    use crate::auth::Role;
    use crate::core::{ConnectionState, HealthState, SchedulingState};
    use crate::integration::model::{MqttRuntimeApplyState, MqttRuntimeConfig};
    use crate::store::model::PlaybackTicketRecord;
    use axum::http::Method;

    #[test]
    fn mqtt_broker_connection_requires_connack_for_desired_revision() {
        let mut config = MqttRuntimeConfig {
            protocol_version: "v5".to_string(),
            broker: "broker.example.test".to_string(),
            port: 1883,
            client_id: "guard-test".to_string(),
            username: None,
            password_configured: false,
            tls: false,
            publish_event_ttl_sec: 86_400,
            desired_revision: 2,
            active_revision: Some(1),
            config_version: 2,
            apply_state: MqttRuntimeApplyState::Connected,
            last_error_code: None,
            last_error_summary: None,
            last_transition_at_ms: 1,
            updated_by: "test".to_string(),
            updated_at_ms: 1,
        };
        assert!(!mqtt_broker_connected(Some(&config)));
        config.active_revision = Some(2);
        assert!(mqtt_broker_connected(Some(&config)));
        config.apply_state = MqttRuntimeApplyState::Degraded;
        assert!(!mqtt_broker_connected(Some(&config)));
        assert!(!mqtt_broker_connected(None));
    }

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
                stream_profile: "sub".to_string(),
            },
        );

        assert_eq!(request.session_node_id, "session-b");
        assert_eq!(request.stream_profile, "sub");
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
            openapi_responses("get", "/dashboard", "查询业务数据")["200"]["description"]
                .as_str()
                .is_some_and(contains_chinese)
        );
    }

    #[test]
    fn public_contract_has_explicit_success_and_nested_request_schemas() {
        assert_eq!(OPEN_BUSINESS_OPERATIONS.len(), 58);
        for (path, methods) in OPEN_BUSINESS_OPERATIONS {
            for method in *methods {
                let schema =
                    openapi_success_schema(method, path, openapi_operation_summary(method, path));
                assert_ne!(
                    schema["type"],
                    base::serde_json::json!(["object", "array"]),
                    "success response type must be explicit: {method} {path}"
                );
                if schema["type"] == "array" {
                    assert!(schema.get("items").is_some(), "array response needs items");
                } else {
                    assert!(
                        schema["properties"]
                            .as_object()
                            .is_some_and(|value| !value.is_empty()),
                        "object response needs properties: {method} {path}"
                    );
                }
            }
        }
        let preview = openapi_request_body(
            "post",
            "/gb28181/devices/{device_id}/channels/{channel_id}/preview",
        )
        .unwrap();
        assert!(
            preview["content"]["application/json"]["schema"]["properties"]
                .get("stream_profile")
                .is_some()
        );
        let broadcast = openapi_request_body("post", "/gb28181/broadcasts/start").unwrap();
        assert_eq!(
            broadcast["content"]["application/json"]["schema"]["properties"]["targets"]["items"]["type"],
            "object"
        );
        let heartbeat = openapi_request_body("post", "/playbacks/presence/heartbeat").unwrap();
        assert_eq!(
            heartbeat["content"]["application/json"]["schema"]["properties"]["items"]["items"]["type"],
            "object"
        );
        let mqtt_schemas = mqtt_action_payload_schemas();
        for schema_name in [
            "StreamStartPayload",
            "StreamStopPayload",
            "StreamPlaybackPayload",
            "StreamDownloadPayload",
            "DeviceBroadcastPayload",
            "DevicePtzPayload",
            "AiStartPayload",
            "AiCancelPayload",
            "PlaybackTicketRenewPayload",
        ] {
            assert!(
                mqtt_schemas.get(schema_name).is_some(),
                "missing {schema_name}"
            );
        }
        assert_eq!(
            preview["content"]["application/json"]["schema"]["properties"]["trans_mode"]["enum"],
            base::serde_json::json!(["udp", "tcp_active", "tcp_passive"])
        );
        assert_eq!(
            preview["content"]["application/json"]["schema"]["properties"]["stream_profile"]["enum"],
            base::serde_json::json!(["main", "sub"])
        );
        assert_eq!(
            mqtt_schemas["DevicePtzPayload"]["properties"]["zoomSpeed"]["maximum"],
            15
        );
        let asyncapi = asyncapi_contract();
        assert_eq!(
            asyncapi["components"]["messages"]["CommandResult"]["payload"]["properties"]["action"]
                ["enum"],
            base::serde_json::json!(crate::integration::model::MQTT_COMMAND_ACTIONS)
        );
        assert!(
            asyncapi["components"]["messages"]["CommandResult"]["payload"]["properties"]
                .get("result")
                .is_some()
        );
        assert_eq!(
            asyncapi["x-gmv-action-usage"]
                .as_object()
                .map(base::serde_json::Map::len),
            Some(crate::integration::model::MQTT_COMMAND_ACTIONS.len())
        );
    }

    #[test]
    fn stream_contract_exposes_actual_codec_and_mime_metadata() {
        let streams = openapi_success_schema("get", "/streams", "查询媒体流");
        let stream_properties = &streams["items"]["properties"];
        assert!(stream_properties.get("video_codec").is_some());
        assert!(stream_properties.get("audio_codec").is_some());
        assert!(stream_properties.get("mime_codec").is_some());

        let outputs = openapi_success_schema("get", "/streams/{stream_id}/outputs", "查询媒体输出");
        let output_properties = &outputs["items"]["properties"];
        assert!(output_properties.get("video_codec").is_some());
        assert!(output_properties.get("audio_codec").is_some());
        assert!(output_properties.get("mime_codec").is_some());

        let mqtt = mqtt_action_payload_schemas();
        assert!(
            mqtt["StreamCommandResult"]["properties"]
                .get("mime_codec")
                .is_some()
        );
    }

    #[test]
    fn phase9_contract_keeps_http_mqtt_capabilities_and_examples_complete() {
        let openapi = openapi_contract();
        let asyncapi = asyncapi_contract();
        let action_usage = asyncapi["x-gmv-action-usage"].as_object().unwrap();
        let action_examples = asyncapi["x-gmv-action-examples"].as_object().unwrap();
        let stop_all_usage = &action_usage["broadcast.stop_all"];
        assert_eq!(stop_all_usage["summary"], "停止语音广播全部目标");
        assert!(
            stop_all_usage["http_equivalent_operations"]
                .as_array()
                .is_some_and(|items| items.contains(&base::serde_json::json!({
                    "method": "POST",
                    "path": "/openapi/v1/gb28181/broadcasts/{broadcast_id}/stop-all",
                    "summary": "停止语音广播全部目标"
                })))
        );
        let mut operation_count = 0;
        let mut special_count = 0;

        for (path, methods) in OPEN_BUSINESS_OPERATIONS {
            for method in *methods {
                operation_count += 1;
                let http_method = if *method == "get" {
                    Method::GET
                } else {
                    Method::POST
                };
                let scope = open_business_scope(&http_method, path).unwrap();
                let action = crate::integration::model::mqtt_action_for_http(method, path);
                let special = crate::integration::model::mqtt_special_for_http(method, path);
                assert_ne!(
                    action.is_some(),
                    special.is_some(),
                    "HTTP operation needs exactly one MQTT mapping: {method} {path}"
                );
                if let Some(action) = action {
                    assert!(crate::integration::model::MQTT_COMMAND_ACTIONS.contains(&action));
                    assert_eq!(
                        crate::integration::model::mqtt_action_scope(action),
                        Some(scope)
                    );
                    let usage = &action_usage[action];
                    assert_eq!(usage["required_scope"], scope);
                    assert!(usage["http_equivalents"].as_array().is_some_and(|items| {
                        items.contains(&base::serde_json::json!(format!(
                            "{} /openapi/v1{path}",
                            method.to_uppercase()
                        )))
                    }));
                    assert!(
                        usage["summary"]
                            .as_str()
                            .is_some_and(|summary| !summary.is_empty())
                    );
                    assert!(
                        usage["http_equivalent_operations"]
                            .as_array()
                            .is_some_and(|items| items.contains(&base::serde_json::json!({
                                "method": method.to_uppercase(),
                                "path": format!("/openapi/v1{path}"),
                                "summary": openapi_operation_summary(method, path)
                            })))
                    );
                } else {
                    special_count += 1;
                    assert_eq!((*method, *path), ("get", "/events"));
                }

                let operation = &openapi["paths"][format!("/openapi/v1{path}")][*method];
                assert!(operation.get("x-gmv-request-example").is_some());
                let responses = operation["responses"].as_object().unwrap();
                let successes = responses
                    .iter()
                    .filter(|(status, _)| status.starts_with('2'))
                    .collect::<Vec<_>>();
                assert_eq!(successes.len(), 1, "one success response: {method} {path}");
                if successes[0].0.as_str() != "204" {
                    let media = &successes[0].1["content"]["application/json"];
                    assert!(media.get("schema").is_some());
                    assert!(media.get("example").is_some());
                }
                assert!(
                    responses["400"]["content"]["application/json"]
                        .get("example")
                        .is_some()
                );
            }
        }

        assert_eq!(operation_count, 65);
        assert_eq!(special_count, 1);
        assert_eq!(
            action_usage.len(),
            crate::integration::model::MQTT_COMMAND_ACTIONS.len()
        );
        assert_eq!(
            action_examples.len(),
            crate::integration::model::MQTT_COMMAND_ACTIONS.len()
        );
        for action in crate::integration::model::MQTT_COMMAND_ACTIONS {
            let usage = &action_usage[*action];
            assert!(
                usage["required_scope"].is_string(),
                "missing scope: {action}"
            );
            let payload_schema = usage["payload_schema"].as_str().unwrap();
            let result_schema = usage["result_schema"].as_str().unwrap();
            assert!(
                asyncapi["components"]["schemas"]
                    .get(payload_schema)
                    .is_some()
            );
            assert!(
                asyncapi["components"]["schemas"]
                    .get(result_schema)
                    .is_some()
            );
            let examples = &action_examples[*action];
            assert_eq!(examples["request"]["action"], *action);
            assert_eq!(examples["success"]["action"], *action);
            assert_eq!(examples["failure"]["action"], *action);
            assert_eq!(examples["success"]["state"], "succeeded");
            assert_eq!(examples["failure"]["state"], "failed");
        }
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
        assert!(include_str!("api_docs.js").contains("MQTT 调用闭环"));
        assert!(include_str!("api_docs.js").contains("MQTT Action 请求与结果"));
        assert!(include_str!("api_docs.js").contains("HTTP 等价接口"));
        assert!(include_str!("api_docs.js").contains("usage.summary"));
        assert!(include_str!("api_docs.js").contains("取值约束"));
        assert!(include_str!("api_docs.js").contains("成功响应字段"));
        assert!(include_str!("api_docs.js").contains("失败响应示例"));
        assert!(include_str!("api_docs.js").contains("可回调事件接口"));
        assert!(include_str!("api_docs.js").contains("可发布事件列表"));
    }

    #[test]
    fn callback_event_contract_is_shared_by_openapi_and_asyncapi() {
        let expected = crate::integration::model::INTEGRATION_CALLBACK_EVENTS
            .iter()
            .map(|event| event.event_type)
            .collect::<Vec<_>>();
        let openapi = openapi_contract();
        let asyncapi = asyncapi_contract();

        let mut webhook_events = openapi["webhooks"]
            .as_object()
            .unwrap()
            .values()
            .flat_map(|path| path["post"]["x-gmv-event-types"].as_array().unwrap())
            .map(|event| event["event_type"].as_str().unwrap())
            .collect::<Vec<_>>();
        webhook_events.sort_unstable();
        let mut expected_webhook_events = expected.clone();
        expected_webhook_events.sort_unstable();
        assert_eq!(webhook_events, expected_webhook_events);
        for event in crate::integration::model::INTEGRATION_CALLBACK_EVENTS {
            let operation = &openapi["webhooks"][event.event_type.replace('.', "_")]["post"];
            assert_eq!(
                operation["x-gmv-callback-path"],
                format!("{{callback_url}}/{}", event.event_type.replace('.', "/"))
            );
            assert_eq!(
                operation["requestBody"]["content"]["application/json"]["schema"]["properties"]["event_type"]
                    ["const"],
                event.event_type
            );
            assert!(
                operation["x-gmv-event-types"][0]["payload_schema"]["properties"]
                    .as_object()
                    .is_some_and(|properties| !properties.is_empty())
            );
            assert!(operation["x-gmv-event-types"][0]["envelope_example"]["payload"].is_object());
        }
        assert_eq!(
            openapi["components"]["schemas"]["IntegrationEventEnvelope"]["properties"]["event_type"]
                ["enum"],
            base::serde_json::json!(expected)
        );
        assert_eq!(
            asyncapi["channels"]["events"]["address"],
            "gmv/events/{integration_id}/{event_type}"
        );
        assert_eq!(
            asyncapi["channels"]["events"]["x-gmv-event-types"],
            asyncapi["x-gmv-event-types"]
        );
        assert_eq!(
            asyncapi["components"]["messages"]["EventEnvelope"]["payload"]["oneOf"]
                .as_array()
                .unwrap()
                .iter()
                .map(|schema| schema["properties"]["event_type"]["const"]
                    .as_str()
                    .unwrap())
                .collect::<Vec<_>>(),
            expected
        );
        assert!(
            asyncapi["components"]["messages"]
                .get("PlaybackRenewalRequest")
                .is_none()
        );
    }

    #[test]
    fn callback_mapping_patterns_use_segment_wildcards() {
        for valid in [
            "session.alarm",
            "session.*",
            "stream.**",
            "node-health.changed",
        ] {
            assert!(valid_event_mapping_source(valid), "valid pattern: {valid}");
        }
        for invalid in [
            "",
            "session.",
            ".session",
            "session.***",
            "session.*suffix",
            "session/#",
        ] {
            assert!(
                !valid_event_mapping_source(invalid),
                "invalid pattern: {invalid}"
            );
        }
        for matching in ["session.alarm", "session.*", "integration.**", "**"] {
            assert!(
                mapping_source_matches_callback_contract(matching),
                "contract pattern: {matching}"
            );
        }
        for unsupported in ["stream.**", "node-health.changed"] {
            assert!(
                !mapping_source_matches_callback_contract(unsupported),
                "unsupported callback pattern: {unsupported}"
            );
        }
    }
}
