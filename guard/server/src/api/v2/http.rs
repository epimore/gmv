use axum::body::Bytes;
use axum::extract::{
    ConnectInfo, FromRequest, FromRequestParts, Multipart, Path, Query, Request, State,
};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE, ORIGIN, REFERRER_POLICY,
    SET_COOKIE, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::api::v2::control::{BusinessControl, DeviceStreamOptions};
use crate::api::v2::model::{
    AiTaskSummary, AiTaskSummaryState, DeviceSummary, RuntimeStatus, StreamSummary,
    StreamSummaryState,
};
use crate::api::v2::paths;
use crate::api::v2::{ApiV2, CursorQuery, EventQuery};
use crate::app_config::PictureUploadConfig;
use crate::auth::session::{SESSION_COOKIE, cookie_value};
use crate::auth::{AuthState, Role, UiSession, UserProfile, hash_password as hash_ui_password};
use crate::core::{GuardError, HealthState, LeaseState, RouteState};
use crate::job::{SystemJobRecord, SystemJobRequest, SystemJobStatus, SystemJobType};
use crate::media::{PictureUploadResult, save_picture_upload};
use crate::operation::{OperationRecord, OperationRequest, OperationStatus};
use crate::outbox::OutboxRepository;
use crate::runtime::event_forwarder::EventForwarder;
use crate::store::model::{
    EventRecord, LeaseRecord, NodeRecord, OutboxDestinationKind, OutboxRecord, OutboxState,
};
use crate::store::persistent::{MediaRepository, UserRepository};

const CSRF_HEADER: &str = "x-csrf-token";

#[derive(Debug, Clone)]
pub struct HttpState {
    pub api: ApiV2,
    pub auth: AuthState,
    pub outbox: OutboxRepository,
    pub users: Option<UserRepository>,
    pub media: PictureUploadConfig,
    pub media_files: Option<MediaRepository>,
    pub event_forwarder: Option<EventForwarder>,
}

pub fn router(state: HttpState) -> Router {
    let root_state = state.clone();
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
        .route("/nodes", get(nodes))
        .route("/leases", get(leases))
        .route("/events", get(events))
        .route("/operations", get(operations).post(start_operation))
        .route("/operations/{operation_id}", get(operation))
        .route("/system/jobs", get(system_jobs).post(start_system_job))
        .route("/system/jobs/{job_id}", get(system_job))
        .route("/users", get(list_users).post(create_user))
        .route("/users/{username}", post(update_user))
        .route("/integrations/outbox", get(outbox_records))
        .route("/edge/upload/picture/{token}", post(upload_picture))
        .route("/integrations/outbox/{outbox_id}/retry", post(retry_outbox))
        .route("/devices", get(devices))
        .route("/devices/{device_id}/preview", post(preview))
        .route("/devices/{device_id}/playback", post(playback))
        .route("/devices/{device_id}/download", post(download))
        .route("/devices/{device_id}/talk", post(talk))
        .route("/devices/{device_id}/ptz", post(ptz))
        .route("/streams", get(streams))
        .route("/streams/{stream_id}/stop", post(stop_stream))
        .route("/ai/tasks", get(ai_tasks).post(start_ai_task))
        .route("/ai/tasks/{task_id}/cancel", post(cancel_ai_task))
        .route("/runtime/status", get(runtime_status))
        .with_state(state)
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
        .nest(paths::API_PREFIX, api)
        .with_state(root_state)
        .layer(SetResponseHeaderLayer::if_not_present(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
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
) -> Result<Json<SessionResponse>, HttpError> {
    Ok(Json(session_response(authenticated(
        &state.auth,
        &headers,
    )?)))
}

async fn logout(State(state): State<HttpState>, headers: HeaderMap) -> Result<Response, HttpError> {
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
    connection: String,
    health: String,
    scheduling: String,
    capabilities: Vec<String>,
    capacity: u32,
    pending_leases: u32,
    host_metrics: HostMetricsResponse,
    business_metrics: std::collections::HashMap<String, String>,
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
        Self {
            node_id: node.identity.node_id,
            instance_id: node.identity.instance_id,
            kind: format!("{:?}", node.identity.kind).to_lowercase(),
            connection: format!("{:?}", node.connection).to_uppercase(),
            health: format!("{:?}", node.health).to_uppercase(),
            scheduling: format!("{:?}", node.scheduling).to_uppercase(),
            capabilities: node.capabilities,
            capacity: node.capacity,
            pending_leases: node.pending_leases,
            host_metrics: node.host_metrics.into(),
            business_metrics: node.business_metrics,
            zone: node.zone,
            last_seen_at_ms: node.last_seen_at_ms,
            generation: node.generation,
            sequence: node.sequence,
        }
    }
}

async fn nodes(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<NodeResponse>>, HttpError> {
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

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct OperationResponse {
    operation_id: String,
    kind: String,
    requested_by: String,
    required_role: &'static str,
    status: &'static str,
    progress_percent: u8,
    message: String,
    error: Option<String>,
}

impl From<OperationRecord> for OperationResponse {
    fn from(record: OperationRecord) -> Self {
        Self {
            operation_id: record.operation_id,
            kind: record.kind,
            requested_by: record.requested_by,
            required_role: role_name(record.required_role),
            status: operation_status(record.status),
            progress_percent: record.progress_percent,
            message: record.message,
            error: record.error.map(|error| error.to_string()),
        }
    }
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct StartOperationRequest {
    operation_id: String,
    kind: String,
    dangerous: bool,
    confirmation: Option<String>,
}

async fn start_operation(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<StartOperationRequest>,
) -> Result<(StatusCode, Json<OperationResponse>), HttpError> {
    verify_origin(&state.auth, &headers)?;
    let session = authenticated(&state.auth, &headers)?;
    state
        .auth
        .require_role(&session, Role::Operator)
        .map_err(|_| HttpError::forbidden("UI role is not allowed"))?;
    verify_csrf(&state.auth, &session, &headers)?;
    let record = state.api.start_operation(OperationRequest {
        operation_id: request.operation_id,
        kind: request.kind,
        requested_by: session.username,
        caller_role: session.role,
        required_role: Role::Operator,
        dangerous: request.dangerous,
        confirmation: request.confirmation,
    })?;
    Ok((StatusCode::ACCEPTED, Json(record.into())))
}

async fn operations(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<OperationResponse>>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    Ok(Json(
        state
            .api
            .list_operations()
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

async fn operation(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(operation_id): Path<String>,
) -> Result<Json<OperationResponse>, HttpError> {
    let session = authenticated(&state.auth, &headers)?;
    state
        .auth
        .require_role(&session, Role::Viewer)
        .map_err(|_| HttpError::forbidden("UI role is not allowed"))?;
    Ok(Json(state.api.get_operation(&operation_id)?.into()))
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct SystemJobResponse {
    job_id: String,
    job_type: &'static str,
    status: &'static str,
    progress_percent: u8,
    message: String,
    error: Option<String>,
}

impl From<SystemJobRecord> for SystemJobResponse {
    fn from(record: SystemJobRecord) -> Self {
        Self {
            job_id: record.job_id,
            job_type: job_type(record.job_type),
            status: job_status(record.status),
            progress_percent: record.progress_percent,
            message: record.message,
            error: record.error.map(|error| error.to_string()),
        }
    }
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct StartSystemJobRequest {
    job_id: String,
    job_type: String,
}

async fn start_system_job(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<StartSystemJobRequest>,
) -> Result<(StatusCode, Json<SystemJobResponse>), HttpError> {
    verify_origin(&state.auth, &headers)?;
    let session = authenticated(&state.auth, &headers)?;
    state
        .auth
        .require_role(&session, Role::Admin)
        .map_err(|_| HttpError::forbidden("UI role is not allowed"))?;
    verify_csrf(&state.auth, &session, &headers)?;
    let job_type = match request.job_type.as_str() {
        "backup" => SystemJobType::Backup,
        "restore" => SystemJobType::Restore,
        "migrate" => SystemJobType::Migrate,
        "reconcile" => SystemJobType::Reconcile,
        _ => {
            return Err(GuardError::InvalidConfig(
                "job_type must be backup, restore, migrate, or reconcile".to_string(),
            )
            .into());
        }
    };
    let record = state.api.start_system_job(SystemJobRequest {
        job_id: request.job_id,
        job_type,
    })?;
    Ok((StatusCode::ACCEPTED, Json(record.into())))
}

async fn system_jobs(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<SystemJobResponse>>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    Ok(Json(
        state
            .api
            .list_system_jobs()
            .into_iter()
            .map(Into::into)
            .collect(),
    ))
}

async fn system_job(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<Json<SystemJobResponse>, HttpError> {
    let session = authenticated(&state.auth, &headers)?;
    state
        .auth
        .require_role(&session, Role::Viewer)
        .map_err(|_| HttpError::forbidden("UI role is not allowed"))?;
    Ok(Json(state.api.get_system_job(&job_id)?.into()))
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

fn operation_status(status: OperationStatus) -> &'static str {
    match status {
        OperationStatus::Accepted => "accepted",
        OperationStatus::Running => "running",
        OperationStatus::Succeeded => "succeeded",
        OperationStatus::Failed => "failed",
        OperationStatus::Cancelled => "cancelled",
    }
}

fn job_type(job_type: SystemJobType) -> &'static str {
    match job_type {
        SystemJobType::Backup => "backup",
        SystemJobType::Restore => "restore",
        SystemJobType::Migrate => "migrate",
        SystemJobType::Reconcile => "reconcile",
    }
}

fn job_status(status: SystemJobStatus) -> &'static str {
    match status {
        SystemJobStatus::Pending => "pending",
        SystemJobStatus::Running => "running",
        SystemJobStatus::Succeeded => "succeeded",
        SystemJobStatus::Failed => "failed",
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
            current.enabled,
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
    require_role(&state.auth, &headers, Role::Admin)?;
    let users = require_user_repository(&state)?.list_profiles().await?;
    Ok(Json(users.into_iter().map(user_response).collect()))
}

async fn create_user(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<UserResponse>), HttpError> {
    require_write(&state.auth, &headers, Role::Admin)?;
    let username = request.username.trim().to_string();
    let role = Role::parse(&request.role)?;
    let hash = password_hash(&request.password)?;
    let now_ms = http_now_ms()?;
    let users = require_user_repository(&state)?;
    users
        .upsert_user(
            &username,
            role,
            Some(&hash),
            Some(&request.nickname),
            request.enabled,
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
    require_write(&state.auth, &headers, Role::Admin)?;
    let username = username.trim().to_string();
    let role = Role::parse(&request.role)?;
    let hash = request.password.as_deref().map(password_hash).transpose()?;
    let now_ms = http_now_ms()?;
    let users = require_user_repository(&state)?;
    users
        .upsert_user(
            &username,
            role,
            hash.as_deref(),
            request.nickname.as_deref(),
            request.enabled,
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

fn user_response(profile: UserProfile) -> UserResponse {
    UserResponse {
        username: profile.username,
        role: profile.role.as_str(),
        nickname: profile.nickname,
        enabled: profile.enabled,
        created_at_ms: profile.created_at_ms,
        updated_at_ms: profile.updated_at_ms,
    }
}

fn require_user_repository(state: &HttpState) -> Result<&UserRepository, HttpError> {
    state.users.as_ref().ok_or_else(|| HttpError {
        status: StatusCode::NOT_IMPLEMENTED,
        code: "user_store_disabled",
        message: "persistent user store is disabled".to_string(),
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
            code: "invalid_user",
            message: "password is required".to_string(),
        });
    }
    hash_ui_password(password).map_err(|_| HttpError::internal("password hash failed"))
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct OutboxResponse {
    outbox_id: String,
    event_id: String,
    destination_kind: &'static str,
    destination: String,
    state: &'static str,
    attempts: u32,
    next_attempt_at_ms: i64,
    last_error: Option<String>,
    created_at_ms: i64,
    updated_at_ms: i64,
}

impl From<OutboxRecord> for OutboxResponse {
    fn from(record: OutboxRecord) -> Self {
        Self {
            outbox_id: record.outbox_id,
            event_id: record.event_id,
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
        }
    }
}

async fn upload_picture(
    State(state): State<HttpState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    request: Request,
) -> Result<Json<PictureUploadResult>, HttpError> {
    let repository = state
        .media_files
        .as_ref()
        .ok_or_else(|| HttpError::internal("media repository is unavailable"))?;
    let session_id = params
        .get("SessionID")
        .or_else(|| params.get("session_id"))
        .ok_or_else(|| HttpError {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_upload",
            message: "SessionID is required".to_string(),
        })?;
    let file_id = params
        .iter()
        .find(|(key, _)| key.to_ascii_lowercase().ends_with("fileid"))
        .map(|(_, value)| value.as_str());
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let result = if content_type.starts_with("multipart/form-data") {
        let (bytes, field_content_type) = read_multipart_image(request).await?;
        save_picture_upload(
            &state.media,
            repository,
            &token,
            session_id,
            file_id,
            &field_content_type,
            bytes,
        )
        .await?
    } else if content_type.starts_with("image/") {
        let bytes = Bytes::from_request(request, &())
            .await
            .map_err(|error| HttpError::bad_request(format!("invalid upload body: {error}")))?;
        save_picture_upload(
            &state.media,
            repository,
            &token,
            session_id,
            file_id,
            &content_type,
            bytes,
        )
        .await?
    } else {
        return Err(HttpError::bad_request(
            "Unsupported Content-Type. Use multipart/form-data or image/*",
        ));
    };
    publish_picture_event(&state, &result).await?;
    Ok(Json(result))
}

async fn read_multipart_image(request: Request) -> Result<(Bytes, String), HttpError> {
    let mut multipart = Multipart::from_request(request, &())
        .await
        .map_err(|error| HttpError::bad_request(format!("invalid multipart body: {error}")))?;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| HttpError::bad_request(format!("invalid multipart field: {error}")))?
    {
        let content_type = field.content_type().unwrap_or_default().to_string();
        if !content_type.starts_with("image/") {
            continue;
        }
        let bytes = field
            .bytes()
            .await
            .map_err(|error| HttpError::bad_request(format!("invalid image field: {error}")))?;
        return Ok((bytes, content_type));
    }
    Err(HttpError::bad_request("multipart image field is required"))
}

async fn publish_picture_event(
    state: &HttpState,
    result: &PictureUploadResult,
) -> Result<(), HttpError> {
    let event_id = format!("picture-uploaded-{}", result.session_id);
    let topic = "guard.picture.uploaded".to_string();
    let payload = base::serde_json::to_vec(result)
        .map_err(|_| HttpError::internal("encode picture upload event failed"))?;
    let inserted = state.api.store().insert_event_once(EventRecord {
        event_id: event_id.clone(),
        topic: topic.clone(),
        priority: 2,
        payload: payload.clone(),
    })?;
    if inserted {
        if let Some(forwarder) = &state.event_forwarder {
            forwarder.forward(event_id, topic, payload).await?;
        }
    }
    Ok(())
}

async fn outbox_records(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<OutboxQuery>,
) -> Result<Json<Vec<OutboxResponse>>, HttpError> {
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
    talk_codec: String,
    #[serde(default)]
    talk_sample_rate: u32,
    #[serde(default)]
    talk_channel_count: u32,
    #[serde(default)]
    talk_frame_duration_ms: u32,
}

fn device_stream_options(request: &PreviewRequest) -> DeviceStreamOptions {
    DeviceStreamOptions {
        token: request.token.clone(),
        start_time_sec: request.start_time_sec,
        end_time_sec: request.end_time_sec,
        trans_mode: request.trans_mode.clone(),
        output_type: request.output_type.clone(),
        talk_codec: request.talk_codec.clone(),
        talk_sample_rate: request.talk_sample_rate,
        talk_channel_count: request.talk_channel_count,
        talk_frame_duration_ms: request.talk_frame_duration_ms,
    }
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct PtzRequest {
    channel_id: String,
}

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct StartAiRequest {
    request_id: String,
    stream_id: String,
    model: String,
}

async fn devices(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<DeviceSummary>>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    Ok(Json(Vec::new()))
}

async fn preview(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Json(request): Json<PreviewRequest>,
) -> Result<(StatusCode, Json<StreamSummary>), HttpError> {
    let session = require_write(&state.auth, &headers, Role::Operator)?;
    let operation_id = request.request_id.clone();
    state.api.start_operation(operation_request(
        operation_id.clone(),
        "stream.start",
        &session,
        Role::Operator,
    ))?;
    let start_result = BusinessControl::new(state.api.store())
        .start_live_with_options(
            &request.request_id,
            &device_id,
            &request.channel_id,
            device_stream_options(&request),
        )
        .await;
    match start_result {
        Ok(stream) => {
            state
                .api
                .succeed_operation(&operation_id, "stream started")?;
            Ok((StatusCode::ACCEPTED, Json(stream)))
        }
        Err(error) => {
            let _ = state.api.fail_operation(&operation_id, error.clone());
            Err(error.into())
        }
    }
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
        "stream.playback",
        "playback started",
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
        "stream.download",
        "download started",
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
        "device.talk",
        "talk started",
        |control, operation_id, device_id, channel_id, options| async move {
            control
                .start_talk_with_options(&operation_id, &device_id, &channel_id, options)
                .await
        },
    )
    .await
}

async fn start_device_stream_http<F, Fut>(
    state: HttpState,
    headers: HeaderMap,
    device_id: String,
    request: PreviewRequest,
    operation_kind: &str,
    success_message: &str,
    rpc_start: F,
) -> Result<(StatusCode, Json<StreamSummary>), HttpError>
where
    F: FnOnce(BusinessControl, String, String, String, DeviceStreamOptions) -> Fut,
    Fut: std::future::Future<Output = Result<StreamSummary, GuardError>>,
{
    let session = require_write(&state.auth, &headers, Role::Operator)?;
    let operation_id = request.request_id.clone();
    state.api.start_operation(operation_request(
        operation_id.clone(),
        operation_kind,
        &session,
        Role::Operator,
    ))?;
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
            state
                .api
                .succeed_operation(&operation_id, success_message)?;
            Ok((StatusCode::ACCEPTED, Json(stream)))
        }
        Err(error) => {
            let _ = state.api.fail_operation(&operation_id, error.clone());
            Err(error.into())
        }
    }
}

async fn ptz(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Json(request): Json<PtzRequest>,
) -> Result<Json<base::serde_json::Value>, HttpError> {
    let session = require_write(&state.auth, &headers, Role::Operator)?;
    let operation_id = format!("ptz-{}", http_now_ms()?);
    state.api.start_operation(operation_request(
        operation_id.clone(),
        "device.ptz",
        &session,
        Role::Operator,
    ))?;
    let ptz_result = BusinessControl::new(state.api.store())
        .ptz(&device_id, &request.channel_id)
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
            Err(error.into())
        }
    }
}

async fn streams(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<StreamSummary>>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    Ok(Json(real_streams(&state)))
}

async fn stop_stream(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(stream_id): Path<String>,
) -> Result<Json<StreamSummary>, HttpError> {
    let session = require_write(&state.auth, &headers, Role::Operator)?;
    let operation_id = format!("stop-{stream_id}");
    state.api.start_operation(operation_request(
        operation_id.clone(),
        "stream.stop",
        &session,
        Role::Operator,
    ))?;
    let stop_result = BusinessControl::new(state.api.store())
        .stop_stream(&stream_id)
        .await;
    match stop_result {
        Ok(stream) => {
            state
                .api
                .succeed_operation(&operation_id, "stream stopped")?;
            Ok(Json(stream))
        }
        Err(error) => {
            let _ = state.api.fail_operation(&operation_id, error.clone());
            Err(error.into())
        }
    }
}

async fn ai_tasks(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<AiTaskSummary>>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    Ok(Json(real_ai_tasks(&state)))
}

async fn start_ai_task(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<StartAiRequest>,
) -> Result<(StatusCode, Json<AiTaskSummary>), HttpError> {
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
            Err(error.into())
        }
    }
}

async fn cancel_ai_task(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> Result<Json<AiTaskSummary>, HttpError> {
    let session = require_write(&state.auth, &headers, Role::Operator)?;
    let operation_id = format!("cancel-{task_id}");
    state.api.start_operation(operation_request(
        operation_id.clone(),
        "ai.cancel",
        &session,
        Role::Operator,
    ))?;
    let cancel_result = BusinessControl::new(state.api.store())
        .cancel_ai(&task_id)
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
            Err(error.into())
        }
    }
}

async fn runtime_status(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<RuntimeStatus>, HttpError> {
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
    verify_origin(auth, headers)?;
    let session = require_role(auth, headers, role)?;
    verify_csrf(auth, &session, headers)?;
    Ok(session)
}

#[derive(Debug, base::serde::Serialize)]
#[serde(crate = "base::serde")]
struct ErrorResponse {
    code: &'static str,
    message: String,
}

struct HttpError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl HttpError {
    fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: "authentication required".to_string(),
        }
    }

    fn forbidden(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "forbidden",
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal",
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: message.into(),
        }
    }

    fn from_auth(error: GuardError) -> Self {
        match error {
            GuardError::Capacity(message) => Self {
                status: StatusCode::TOO_MANY_REQUESTS,
                code: "rate_limited",
                message,
            },
            GuardError::InvalidIdentity(_) => Self::unauthorized(),
            other => other.into(),
        }
    }
}

impl From<GuardError> for HttpError {
    fn from(error: GuardError) -> Self {
        let status = match error {
            GuardError::InvalidConfig(_) | GuardError::InvalidIdentity(_) => {
                StatusCode::BAD_REQUEST
            }
            GuardError::Conflict(_)
            | GuardError::DuplicateEvent(_)
            | GuardError::StaleInstance(_) => StatusCode::CONFLICT,
            GuardError::NotFound(_) => StatusCode::NOT_FOUND,
            GuardError::Capacity(_) => StatusCode::TOO_MANY_REQUESTS,
            GuardError::TimeUnsynced(_) => StatusCode::SERVICE_UNAVAILABLE,
        };
        Self {
            status,
            code: "guard_error",
            message: error.to_string(),
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
            }),
        )
            .into_response()
    }
}
