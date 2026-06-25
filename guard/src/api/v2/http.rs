use axum::extract::{Path, Query, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE, ORIGIN, REFERRER_POLICY,
    SET_COOKIE, X_CONTENT_TYPE_OPTIONS,
};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use tower_http::cors::CorsLayer;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::api::v2::paths;
use crate::api::v2::{ApiV2, CursorQuery, EventQuery};
use crate::auth::session::{SESSION_COOKIE, cookie_value};
use crate::auth::{AuthState, Role, UiSession};
use crate::core::{GuardError, HealthState};
use crate::job::{SystemJobRecord, SystemJobRequest, SystemJobStatus, SystemJobType};
use crate::operation::{OperationRecord, OperationRequest, OperationStatus};
use crate::outbox::OutboxRepository;
use crate::sim::{SimAiTask, SimDevice, SimStatus, SimStream, Simulator};
use crate::store::model::{
    EventRecord, NodeRecord, OutboxDestinationKind, OutboxRecord, OutboxState,
};

const CSRF_HEADER: &str = "x-csrf-token";

#[derive(Debug, Clone)]
pub struct HttpState {
    pub api: ApiV2,
    pub auth: AuthState,
    pub outbox: OutboxRepository,
    pub simulator: Option<Simulator>,
}

pub fn router(state: HttpState) -> Router {
    let root_state = state.clone();
    let origin = HeaderValue::from_str(state.auth.allowed_origin())
        .expect("validated UI allowed origin must be a valid header value");
    let csrf_header = HeaderName::from_static(CSRF_HEADER);
    let api = Router::new()
        .route("/auth/login", post(login))
        .route("/auth/session", get(current_session))
        .route("/auth/logout", post(logout))
        .route("/dashboard", get(dashboard))
        .route("/nodes", get(nodes))
        .route("/events", get(events))
        .route("/operations", post(start_operation))
        .route("/operations/{operation_id}", get(operation))
        .route("/system/jobs", post(start_system_job))
        .route("/system/jobs/{job_id}", get(system_job))
        .route("/integrations/outbox", get(outbox_records))
        .route("/integrations/outbox/{outbox_id}/retry", post(retry_outbox))
        .route("/devices", get(sim_devices))
        .route("/devices/{device_id}/preview", post(sim_preview))
        .route("/devices/{device_id}/ptz", post(sim_ptz))
        .route("/streams", get(sim_streams))
        .route("/streams/{stream_id}/stop", post(sim_stop_stream))
        .route("/ai/tasks", get(sim_ai_tasks).post(sim_start_ai))
        .route("/ai/tasks/{task_id}/cancel", post(sim_cancel_ai))
        .route("/sim/status", get(sim_status))
        .route("/sim/availability", post(sim_availability))
        .with_state(state)
        .layer(
            CorsLayer::new()
                .allow_origin(origin)
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
    csrf_token: String,
    expires_at_ms: u64,
}

async fn login(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<LoginRequest>,
) -> Result<Response, HttpError> {
    verify_origin(&state.auth, &headers)?;
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
    zone: Option<String>,
    last_seen_at_ms: i64,
    generation: u64,
    sequence: u64,
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
    let session = authenticated(&state.auth, &headers)?;
    state
        .auth
        .require_role(&session, Role::Viewer)
        .map_err(|_| HttpError::forbidden("UI role is not allowed"))?;
    Ok(Json(
        state.api.list_nodes().into_iter().map(Into::into).collect(),
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

#[derive(Debug, base::serde::Deserialize)]
#[serde(crate = "base::serde")]
struct AvailabilityRequest {
    available: bool,
}

async fn sim_devices(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<SimDevice>>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    Ok(Json(require_simulator(&state)?.devices()))
}

async fn sim_preview(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Json(request): Json<PreviewRequest>,
) -> Result<(StatusCode, Json<SimStream>), HttpError> {
    require_write(&state.auth, &headers, Role::Operator)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(require_simulator(&state)?.start_stream(
            &request.request_id,
            &device_id,
            &request.channel_id,
            http_now_ms()?,
        )?),
    ))
}

async fn sim_ptz(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Json(request): Json<PtzRequest>,
) -> Result<Json<base::serde_json::Value>, HttpError> {
    require_write(&state.auth, &headers, Role::Operator)?;
    let count = require_simulator(&state)?.ptz(&device_id, &request.channel_id)?;
    Ok(Json(
        base::serde_json::json!({ "accepted": true, "count": count }),
    ))
}

async fn sim_streams(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<SimStream>>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    Ok(Json(require_simulator(&state)?.streams()))
}

async fn sim_stop_stream(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(stream_id): Path<String>,
) -> Result<Json<SimStream>, HttpError> {
    require_write(&state.auth, &headers, Role::Operator)?;
    Ok(Json(require_simulator(&state)?.stop_stream(&stream_id)?))
}

async fn sim_ai_tasks(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<Vec<SimAiTask>>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    Ok(Json(require_simulator(&state)?.ai_tasks()))
}

async fn sim_start_ai(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<StartAiRequest>,
) -> Result<(StatusCode, Json<SimAiTask>), HttpError> {
    require_write(&state.auth, &headers, Role::Operator)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(require_simulator(&state)?.start_ai(
            &request.request_id,
            &request.stream_id,
            &request.model,
            http_now_ms()?,
        )?),
    ))
}

async fn sim_cancel_ai(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(task_id): Path<String>,
) -> Result<Json<SimAiTask>, HttpError> {
    require_write(&state.auth, &headers, Role::Operator)?;
    Ok(Json(require_simulator(&state)?.cancel_ai(&task_id)?))
}

async fn sim_status(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Result<Json<SimStatus>, HttpError> {
    require_role(&state.auth, &headers, Role::Viewer)?;
    Ok(Json(require_simulator(&state)?.status()))
}

async fn sim_availability(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<AvailabilityRequest>,
) -> Result<Json<SimStatus>, HttpError> {
    require_write(&state.auth, &headers, Role::Admin)?;
    let simulator = require_simulator(&state)?;
    simulator.set_guard_available(request.available);
    if request.available {
        simulator.reconcile()?;
    }
    Ok(Json(simulator.status()))
}

fn require_simulator(state: &HttpState) -> Result<&Simulator, HttpError> {
    state.simulator.as_ref().ok_or_else(|| HttpError {
        status: StatusCode::NOT_IMPLEMENTED,
        code: "simulator_disabled",
        message: "simulator is disabled".to_string(),
    })
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
