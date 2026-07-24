export type Role = 'viewer' | 'operator' | 'admin';
export interface SessionInfo { username: string; role: Role; nickname: string; csrf_token: string; expires_at_ms: number }
export interface ApiErrorPayload { code?: string; message?: string; user_message?: string; operation_id?: string; trace_id?: string; retryable?: boolean; details?: Record<string, string> }
export class ApiError extends Error {
  public readonly code: string;
  public readonly userMessage: string;
  public readonly diagnosticMessage: string;
  public readonly operationId?: string;
  public readonly traceId?: string;
  public readonly retryable?: boolean;
  public readonly details: Record<string, string>;

  constructor(public readonly status: number, payload: ApiErrorPayload | string) {
    const data = typeof payload === 'string' ? { message: payload } : payload;
    const diagnosticMessage = data.message || 'HTTP ' + status;
    const userMessage = data.user_message || friendlyMessage(data.code, status) || diagnosticMessage;
    super(userMessage);
    this.name = 'ApiError';
    this.code = data.code || 'http_error';
    this.userMessage = userMessage;
    this.diagnosticMessage = diagnosticMessage;
    this.operationId = data.operation_id;
    this.traceId = data.trace_id;
    this.retryable = data.retryable;
    this.details = data.details || {};
  }
}
export interface UserInfo { username: string; role: Role; nickname: string; enabled: boolean; created_at_ms: number; updated_at_ms: number }
export interface DashboardInfo { node_count: number; event_count: number; next_after_id: string | null }
export interface HostMetricsInfo { cpu_usage_percent: number; load_average_1m: number; load_average_5m: number; load_average_15m: number; memory_total_bytes: number; memory_used_bytes: number; swap_total_bytes: number; swap_used_bytes: number; disk_read_bytes_per_sec: number; disk_write_bytes_per_sec: number; network_receive_bytes_per_sec: number; network_transmit_bytes_per_sec: number; process_resident_memory_bytes: number; process_threads: number }
export interface NodeInfo { node_id: string; instance_id: string; kind: string; service: string; protocol: string | null; display_name: string; connection: string; health: string; scheduling: string; capabilities: string[]; pending_leases: number; host_metrics: HostMetricsInfo; business_metrics: Record<string, string>; config: Record<string, string>; zone: string | null; last_seen_at_ms: number; generation: number; sequence: number }
export interface EventItem { event_id: string; topic: string; priority: number; payload: string }
export interface EventPage { items: EventItem[]; next_after_id: string | null }
export interface LeaseInfo { lease_id: string; route_id: string; resource_id: string; node_id: string; instance_id: string; state: 'allocated' | 'confirmed' | 'failed' | 'released' | 'expired'; expires_at_ms: number }
export interface OutboxInfo { outbox_id: string; event_id: string; destination_kind: 'mqtt' | 'webhook'; destination: string; state: 'pending' | 'sending' | 'delivered' | 'retry_wait' | 'dead'; attempts: number; next_attempt_at_ms: number; last_error: string | null; created_at_ms: number; updated_at_ms: number }
export interface DeviceSummary { device_id: string; name: string; session_node_id: string; channels: string[]; online: boolean }
export interface StreamSummary { stream_id: string; device_id: string; channel_id: string; node_id: string; lease_id: string; endpoint: string; video_codec?: string; audio_codec?: string; mime_codec?: string; subscription_id?: string; session_node_id?: string; session_instance_id?: string; playback_id?: string; playback_generation?: number; playback_start_time_sec?: number; playback_end_time_sec?: number; state: 'running' | 'stopping' | 'stopped' | 'failed' }
export interface ActiveStreamViewerFormat { media_format: string; viewer_count: number }
export interface ActiveStreamMonitorItem { stream_id: string; session_node_id: string; session_instance_id: string; stream_node_id: string; device_id: string; channel_id: string; ssrc: string; state: 'starting' | 'running' | 'stopping' | 'failed' | 'unknown' | 'conflict'; dialog_state: string; media_state: string; media_ready: boolean; created_at_ms: number; established_at_ms: number; started_at_ms: number; diagnostic_reason: string; session_type: string; viewer_count: number; viewer_formats: ActiveStreamViewerFormat[]; supported_formats: string[]; output_format: string }
export interface ActiveStreamDialogItem { stream_id: string; session_node_id: string; session_instance_id: string; stream_node_id: string; device_id: string; channel_id: string; ssrc: string; dialog_state: 'INVITING' | 'ESTABLISHED' | 'TERMINATING'; created_at_ms: number; established_at_ms: number; started_at_ms: number; session_type: string }
export interface ActiveStreamDialogPage { items: ActiveStreamDialogItem[]; total: number; page: number; page_size: number; server_time_ms: number }
export interface StreamHistoryMonitorItem { stream_id: string; session_node_id: string; stream_node_id: string; device_id: string; channel_id: string; ssrc: string; session_type: string; state: 'TERMINATED' | 'ORPHAN'; created_at_ms: number; established_at_ms: number; terminated_at_ms: number; duration_ms: number; terminal_reason: string; terminal_reason_label: string; error_code: string; legacy_terminal_time: boolean; stop_reason: string }
export interface StreamHistoryMonitorPage { items: StreamHistoryMonitorItem[]; total: number; page: number; page_size: number; server_time_ms: number }
export interface ActiveStreamManagementInfo { state: 'active' | 'ended'; active: ActiveStreamMonitorItem | null; ended: StreamHistoryMonitorItem | null }
export interface StreamMonitorQuery { stream_id?: string; stream_node_id?: string; device_id?: string; channel_id?: string; ssrc?: string; state?: string }
export interface StreamOutputSummary { output_id: string; stream_id: string; output_type: 'flv' | 'hls' | 'll_hls' | 'fmp4'; endpoint: string; state: 'preparing' | 'ready' | 'closed' | 'failed' }
export type MediaOperationState = 'preparing' | 'ready' | 'failed' | 'cancelled';
export interface MediaOperationError { code: string; message: string; user_message: string; retryable: boolean }
export interface MediaOperationSummary<T = unknown> {
  operation_id: string;
  state: MediaOperationState;
  stage: string;
  elapsed_ms: number;
  last_progress_at_ms: number;
  checkpoint_ms: number;
  hard_timeout_ms: number;
  can_continue: boolean;
  result: T | null;
  error: MediaOperationError | null;
}
export interface MediaTransportCapability { scheme: 'http' | 'https'; http_version: 'http/1.1' | 'h2'; multi_view_limit: number }
export interface AiTaskSummary { task_id: string; model: string; stream_id: string; node_id: string; state: 'running' | 'cancelled' | 'failed' }
export interface RuntimeStatus { guard_available: boolean; streams: number; running_streams: number; ai_tasks: number; running_ai_tasks: number; ptz_commands: number }
export interface CreateUserPayload { username: string; role: Role; nickname: string; password: string; enabled: boolean }
export interface UpdateUserPayload { role: Role; nickname?: string; password?: string | null; enabled: boolean }
export interface UpdateProfilePayload { nickname?: string; password?: string }

export const liveApi = import.meta.env.VITE_GMV_API_MODE !== 'mock';
let csrfToken = '';
let unauthorizedHandler: (() => void) | undefined;
export function setUnauthorizedHandler(handler: () => void): void { unauthorizedHandler = handler; }

async function requestAt<T>(url: string, init: RequestInit = {}, redirectOnUnauthorized = true, timeoutMs = 0): Promise<T> {
  const method = (init.method ?? 'GET').toUpperCase();
  if (method !== 'GET' && method !== 'POST') throw new Error('HTTP method is not allowed: ' + method);
  const headers = new Headers(init.headers);
  if (init.body) headers.set('content-type', 'application/json');
  if (csrfToken && method === 'POST') headers.set('x-csrf-token', csrfToken);
  const timeoutController = timeoutMs > 0 && !init.signal ? new AbortController() : undefined;
  const timeoutId = timeoutController ? window.setTimeout(() => timeoutController.abort(), timeoutMs) : undefined;
  let response: Response;
  try {
    response = await fetch(url, { ...init, signal: init.signal ?? timeoutController?.signal, headers, credentials: 'include' });
  } catch {
    if (timeoutController?.signal.aborted) {
      throw new ApiError(0, { code: 'request_timeout', message: 'request timed out', user_message: '控制请求响应超时，请稍后重试', retryable: true });
    }
    throw new ApiError(0, { code: 'network_error', message: 'fetch failed', user_message: '无法连接 Guard 服务，请检查网络或刷新页面', retryable: true });
  } finally {
    if (timeoutId !== undefined) window.clearTimeout(timeoutId);
  }
  if (!response.ok) {
    const error = await response.json().catch(() => ({ message: 'HTTP ' + response.status }));
    if (response.status === 401 && redirectOnUnauthorized) { csrfToken = ''; unauthorizedHandler?.(); }
    throw new ApiError(response.status, error);
  }
  if (response.status === 204) return undefined as T;
  return response.json() as Promise<T>;
}
const request = <T>(path: string, init: RequestInit = {}, redirectOnUnauthorized = true, timeoutMs = 0) => requestAt<T>('/api/v2' + path, init, redirectOnUnauthorized, timeoutMs);

export function errorMessage(error: unknown, fallback: string): string {
  if (error instanceof ApiError) return error.userMessage || fallback;
  return fallback;
}

function friendlyMessage(code: string | undefined, status: number): string {
  if (code === 'network_error') return '无法连接 Guard 服务，请检查网络或刷新页面';
  if (code === 'unauthorized' || status === 401) return '登录已过期，请重新登录';
  if (code === 'forbidden' || status === 403) return '当前账号无权执行此操作';
  if (code === 'csrf_invalid') return '页面会话已失效，请刷新后重试';
  if (code === 'node_rpc_timeout' || status === 504) return '节点响应超时，请稍后重试或检查节点负载/网络';
  if (code === 'node_rpc_connect_failed' || code === 'node_rpc_unavailable' || status === 503) return '无法连接目标节点，请检查节点进程、地址和网络';
  if (code === 'node_unavailable') return '节点离线或不可调度，请等待恢复或切换节点';
  if (code === 'node_endpoint_missing') return '节点未上报 RPC 地址，请检查节点配置';
  if (code === 'stream_input_timeout') return '设备未在限定时间内推流，请检查设备网络和编码配置';
  if (code === 'capacity_exceeded' || status === 429) return '当前系统繁忙，请稍后重试';
  return '';
}

export async function login(username: string, password: string): Promise<SessionInfo> { const session = await request<SessionInfo>('/auth/login', { method: 'POST', body: JSON.stringify({ username, password }) }, false); csrfToken = session.csrf_token; return session; }
export async function currentSession(redirectOnUnauthorized = true): Promise<SessionInfo> { const session = await request<SessionInfo>('/auth/session', {}, redirectOnUnauthorized); csrfToken = session.csrf_token; return session; }
export async function logout(): Promise<void> { await request<void>('/auth/logout', { method: 'POST' }); csrfToken = ''; }
export const currentProfile = () => request<UserInfo>('/me');
export const updateProfile = (payload: UpdateProfilePayload) => request<UserInfo>('/me', { method: 'POST', body: JSON.stringify(payload) });
export const listUsers = () => request<UserInfo[]>('/users');
export const createUser = (payload: CreateUserPayload) => request<UserInfo>('/users', { method: 'POST', body: JSON.stringify(payload) });
export const updateUser = (username: string, payload: UpdateUserPayload) => request<UserInfo>('/users/' + encodeURIComponent(username), { method: 'POST', body: JSON.stringify(payload) });
export const fetchDashboard = () => request<DashboardInfo>('/dashboard');
export const listNodes = () => request<NodeInfo[]>('/nodes');
export function pollEvents(afterId?: string, limit = 100, minPriority?: number, topicPrefix?: string): Promise<EventPage> { const query = new URLSearchParams({ limit: String(limit) }); if (afterId) query.set('after_id', afterId); if (minPriority) query.set('min_priority', String(minPriority)); if (topicPrefix) query.set('topic_prefix', topicPrefix); return request<EventPage>('/events?' + query); }
export const listLeases = () => request<LeaseInfo[]>('/leases');
export const listOutbox = (limit = 100) => request<OutboxInfo[]>('/integrations/outbox?limit=' + limit);
export const retryOutbox = (outboxId: string) => request<OutboxInfo>('/integrations/outbox/' + encodeURIComponent(outboxId) + '/retry', { method: 'POST', body: '{}' });
export const listDevices = () => request<DeviceSummary[]>('/devices');
export const startPreview = async (deviceId: string, channelId: string, requestId: string) => {
  const operation = await request<MediaOperationSummary<StreamSummary>>('/devices/' + deviceId + '/preview', { method: 'POST', body: JSON.stringify({ channel_id: channelId, request_id: requestId }) }, true, 3_000);
  return waitMediaOperation(operation);
};
export const sendPtz = (deviceId: string, channelId: string) => request<{ accepted: boolean; count: number }>('/devices/' + deviceId + '/ptz', { method: 'POST', body: JSON.stringify({ channel_id: channelId }) });
export const listStreams = () => request<StreamSummary[]>('/streams');
export const stopStream = (streamId: string) => request<StreamSummary>('/streams/' + streamId + '/stop', { method: 'POST', body: '{}' });
function streamMonitorParams(sessionNodeId: string, query: StreamMonitorQuery): URLSearchParams { const params = new URLSearchParams({ session_node_id: sessionNodeId }); for (const [key, value] of Object.entries(query)) if (value?.trim()) params.set(key, value.trim()); return params; }
export function listActiveStreamMonitor(sessionNodeId: string, query: StreamMonitorQuery, page = 1, pageSize = 20): Promise<ActiveStreamDialogPage> { const params = streamMonitorParams(sessionNodeId, query); const state = params.get('state'); params.delete('state'); if (state) params.set('dialog_state', state); params.set('page', String(page)); params.set('page_size', String(pageSize)); return request<ActiveStreamDialogPage>('/gb28181/streams?' + params); }
export function listStreamHistoryMonitor(sessionNodeId: string, query: StreamMonitorQuery, page = 1, pageSize = 20): Promise<StreamHistoryMonitorPage> { const params = streamMonitorParams(sessionNodeId, query); params.set('page', String(page)); params.set('page_size', String(pageSize)); return request<StreamHistoryMonitorPage>('/gb28181/stream-history?' + params); }
export const getActiveStreamManagement = (sessionNodeId: string, streamId: string) => request<ActiveStreamManagementInfo>('/gb28181/streams/' + encodeURIComponent(streamId) + '/management?session_node_id=' + encodeURIComponent(sessionNodeId));
export const stopMonitoredStream = (sessionNodeId: string, streamId: string, requestId: string, stopReason: string) => request<{ stream_id: string; state: 'stopping' | 'stopped'; session_node_id: string; session_instance_id: string }>('/gb28181/streams/' + encodeURIComponent(streamId) + '/stop', { method: 'POST', body: JSON.stringify({ session_node_id: sessionNodeId, request_id: requestId, stop_reason: stopReason }) });
export const releaseStream = (streamId: string, subscriptionId: string, requestId: string) => request<StreamSummary>('/streams/' + encodeURIComponent(streamId) + '/release', { method: 'POST', body: JSON.stringify({ request_id: requestId, subscription_id: subscriptionId }) });
export const setStreamPlaybackSpeed = (streamId: string, speedRate: number) => request<{ accepted: boolean; speed_rate: number }>('/streams/' + encodeURIComponent(streamId) + '/speed', { method: 'POST', body: JSON.stringify({ speed_rate: speedRate }) });
export interface PlaybackControlResponse { accepted: boolean; generation: number }
export interface PlaybackPresenceHeartbeatItem { playback_id: string; stream_id: string; subscription_id: string; generation: number }
export interface PlaybackPresenceHeartbeatResult { playback_id: string; stream_id: string; accepted: boolean; terminal: boolean; generation: number; presence_deadline_ms?: number }
export interface PlaybackPresenceHeartbeatResponse { server_time_ms: number; items: PlaybackPresenceHeartbeatResult[] }
export const seekGbPlayback = (playbackId: string, payload: { request_id: string; stream_id: string; position_sec: number; expected_generation: number }) => request<PlaybackControlResponse>('/playbacks/' + encodeURIComponent(playbackId) + '/seek', { method: 'POST', body: JSON.stringify(payload) });
export const setGbPlaybackSpeed = (playbackId: string, payload: { request_id: string; stream_id: string; speed_rate: number; expected_generation: number }) => request<PlaybackControlResponse>('/playbacks/' + encodeURIComponent(playbackId) + '/speed', { method: 'POST', body: JSON.stringify(payload) });
export const setGbPlaybackState = (playbackId: string, payload: { request_id: string; stream_id: string; paused: boolean; expected_generation: number }) => request<PlaybackControlResponse>('/playbacks/' + encodeURIComponent(playbackId) + '/state', { method: 'POST', body: JSON.stringify(payload) });
export const heartbeatGbPlaybackPresence = (items: PlaybackPresenceHeartbeatItem[]) => request<PlaybackPresenceHeartbeatResponse>('/playbacks/presence/heartbeat', { method: 'POST', body: JSON.stringify({ items }) });
export const listStreamOutputs = (streamId: string) => request<StreamOutputSummary[]>('/streams/' + encodeURIComponent(streamId) + '/outputs');
export const createStreamOutput = async (
  streamId: string,
  outputType: StreamOutputSummary['output_type'],
  requestId: string,
  options: MediaOperationWaitOptions<StreamOutputSummary> = {},
) => {
  const operation = await request<MediaOperationSummary<StreamOutputSummary>>('/streams/' + encodeURIComponent(streamId) + '/outputs', { method: 'POST', body: JSON.stringify({ request_id: requestId, output_type: outputType, audio_codec: 'aac', subscription_id: options.subscriptionId || '' }) }, true, 3_000);
  return waitMediaOperation(operation, options);
};
export const closeStreamOutput = (streamId: string, outputId: string) => request<{ closed: boolean; output_id: string }>('/streams/' + encodeURIComponent(streamId) + '/outputs/' + encodeURIComponent(outputId) + '/close', { method: 'POST', body: '{}' });
export const listAiTasks = () => request<AiTaskSummary[]>('/ai/tasks');
export const startAiTask = (streamId: string, model: string, requestId: string) => request<AiTaskSummary>('/ai/tasks', { method: 'POST', body: JSON.stringify({ stream_id: streamId, model, request_id: requestId }) });
export const cancelAiTask = (taskId: string) => request<AiTaskSummary>('/ai/tasks/' + taskId + '/cancel', { method: 'POST', body: '{}' });
export const runtimeStatus = () => request<RuntimeStatus>('/runtime/status');
export const getMediaTransport = () => request<MediaTransportCapability>('/media/transport');

export interface MediaOperationWaitOptions<T> {
  signal?: AbortSignal;
  onUpdate?: (operation: MediaOperationSummary<T>) => void;
  subscriptionId?: string;
}

export const getMediaOperation = <T = unknown>(operationId: string) => request<MediaOperationSummary<T>>('/media/operations/' + encodeURIComponent(operationId), {}, true, 2_000);
export const getMediaOperations = <T = unknown>(operationIds: string[]) => {
  const query = new URLSearchParams({ ids: operationIds.join(',') });
  return request<MediaOperationSummary<T>[]>('/media/operations?' + query, {}, true, 2_000);
};
export const continueMediaOperation = <T = unknown>(operationId: string) => request<MediaOperationSummary<T>>('/media/operations/' + encodeURIComponent(operationId) + '/continue', { method: 'POST', body: '{}' }, true, 3_000);
export const cancelMediaOperation = <T = unknown>(operationId: string) => request<MediaOperationSummary<T>>('/media/operations/' + encodeURIComponent(operationId) + '/cancel', { method: 'POST', body: '{}' }, true, 3_000);

export async function waitMediaOperation<T>(initial: MediaOperationSummary<T>, options: MediaOperationWaitOptions<T> = {}): Promise<T> {
  let operation = initial;
  while (true) {
    options.onUpdate?.(operation);
    if (operation.state === 'ready' && operation.result) return operation.result;
    if (operation.state === 'failed') {
      const error = operation.error;
      throw new ApiError(409, {
        code: error?.code || 'media_operation_failed',
        message: error?.message || 'media operation failed',
        user_message: error?.user_message || '媒体操作失败，请稍后重试',
        operation_id: operation.operation_id,
        retryable: error?.retryable ?? true,
      });
    }
    if (operation.state === 'cancelled') {
      throw new ApiError(409, {
        code: 'media_operation_cancelled',
        message: 'media operation cancelled',
        user_message: '媒体操作已取消',
        operation_id: operation.operation_id,
        retryable: true,
      });
    }
    const pollMs = operation.elapsed_ms < operation.checkpoint_ms ? 1_000 : 2_000;
    await abortableDelay(pollMs, options.signal);
    operation = await getMediaOperationBatched<T>(operation.operation_id);
  }
}

type PendingOperationRead = {
  resolve: (operation: MediaOperationSummary<unknown>) => void;
  reject: (error: unknown) => void;
};
const pendingOperationReads = new Map<string, PendingOperationRead[]>();
let pendingOperationReadTimer: number | undefined;

function getMediaOperationBatched<T>(operationId: string): Promise<MediaOperationSummary<T>> {
  return new Promise((resolve, reject) => {
    const readers = pendingOperationReads.get(operationId) ?? [];
    readers.push({
      resolve: (operation) => resolve(operation as MediaOperationSummary<T>),
      reject,
    });
    pendingOperationReads.set(operationId, readers);
    if (pendingOperationReadTimer === undefined) {
      pendingOperationReadTimer = window.setTimeout(flushMediaOperationReads, 25);
    }
  });
}

async function flushMediaOperationReads() {
  pendingOperationReadTimer = undefined;
  const batch = new Map(pendingOperationReads);
  pendingOperationReads.clear();
  try {
    const operations = await getMediaOperations([...batch.keys()]);
    const byId = new Map(operations.map((operation) => [operation.operation_id, operation]));
    for (const [operationId, readers] of batch) {
      const operation = byId.get(operationId);
      if (!operation) {
        const error = new ApiError(404, { code: 'media_operation_not_found', message: 'media operation not found', operation_id: operationId, retryable: false });
        readers.forEach((reader) => reader.reject(error));
      } else {
        readers.forEach((reader) => reader.resolve(operation));
      }
    }
  } catch (error) {
    for (const readers of batch.values()) readers.forEach((reader) => reader.reject(error));
  }
}

function abortableDelay(delayMs: number, signal?: AbortSignal): Promise<void> {
  if (signal?.aborted) return Promise.reject(new DOMException('Aborted', 'AbortError'));
  return new Promise((resolve, reject) => {
    const timer = window.setTimeout(() => {
      signal?.removeEventListener('abort', onAbort);
      resolve();
    }, delayMs);
    const onAbort = () => {
      window.clearTimeout(timer);
      signal?.removeEventListener('abort', onAbort);
      reject(new DOMException('Aborted', 'AbortError'));
    };
    signal?.addEventListener('abort', onAbort, { once: true });
  });
}


export interface GbSessionConfigInfo { domain: string; domain_id: string; wan_ip: string; wan_port: number }
export interface GbDeviceInfo { device_id: string; session_node_id: string; domain_id: string; domain: string; longitude: string | null; latitude: string | null; address: string | null; pwd: string | null; pwd_check: number; alias: string | null; status: number; heartbeat_sec: number; del: number; create_time: string | null; tenant_id: string | null; sys_org_code: string | null; create_by: string | null; update_by: string | null; update_time: string | null; monitor_status: number; device_type: string | null; manufacturer: string | null; model: string | null; firmware: string | null; gb_version: string | null; max_camera: number; camera_in_count: number; camera_off_count: number; register_time: string | null }
export interface GbDevicePage { items: GbDeviceInfo[]; total: number; page: number; page_size: number }
export interface GbDevicePayload { device_id?: string; session_node_id?: string; domain_id?: string; domain?: string; longitude?: string; latitude?: string; address?: string; pwd?: string; pwd_check?: number; alias?: string; status?: number; heartbeat_sec?: number; tenant_id?: string; sys_org_code?: string; create_by?: string; update_by?: string }
export interface GbChannelInfo { device_id: string; channel_id: string; name: string; manufacturer: string; model: string; owner: string; status: string; civil_code: string; address: string; parent_id: string; ip_address: string; port: number; longitude: string; latitude: string; ptz_type: string; alias_name: string; pic_url: string; snapshot: number; over_pic_id: string; ptz_enable: number; talk_enable: number; audio_enable: number; record_enable: number; playback_enable: number; alarm_enable: number; biz_enable: number; sort_no: number; created_at_ms: number; updated_at_ms: number }
export interface GbChannelPayload { channel_id: string; name?: string; manufacturer?: string; model?: string; owner?: string; status?: string; civil_code?: string; address?: string; parent_id?: string; ip_address?: string; port?: number; longitude?: string; latitude?: string; ptz_type?: string; alias_name?: string; pic_url?: string; snapshot?: number; over_pic_id?: string; ptz_enable?: number; talk_enable?: number; audio_enable?: number; record_enable?: number; playback_enable?: number; alarm_enable?: number; biz_enable?: number; sort_no?: number }
export interface GbChannelImageInfo { image_id: string; device_id: string; channel_id: string; image_url: string; created_at_ms: number }
export interface GbResourceConfirmationInfo { status: number; resource_kind: string; owner_scope: string; owner_id: string; suggested_enum_id: string; source_parent_id: string; confirmed_by: string; confirmed_at_ms: number; remark: string }
export interface GbResourceInfo { device_id: string; resource_id: string; name: string; status: string; parent_id: string; type_code: string; enum_id: string; enum_name: string; suggested_kind: string; classification_mode: 'default' | 'manual' | 'manual_stale' | 'unknown' | 'conflict' | 'orphan'; effective_kind: string; effective_owner_scope: string; effective_owner_id: string; warning: string; biz_enable: number; owner_biz_enable: number; supported: boolean; available: boolean; unavailable_reason: string; confirmation: GbResourceConfirmationInfo | null }
export interface GbResourceConfirmationPayload { request_id: string; resource_kind: 'video' | 'audio_input' | 'audio_output' | 'other'; owner_scope: 'device' | 'resource'; owner_id: string; remark?: string }
export interface GbSnapshotInfo { session_id: string }
export type GbRecordQueryStatus = 'QUERYING' | 'READY' | 'EMPTY' | 'FAILED';
export interface GbRecordQueryBatchInfo { batch_id: string; status: GbRecordQueryStatus; start_time_sec: number; end_time_sec: number; created_at_ms: number }
export interface GbRecordSegmentInfo { segment_id: number; batch_id: string; device_id: string; channel_id: string; remote_device_id: string; name: string; file_path: string; address: string; start_time_sec: number; end_time_sec: number; secrecy: number; record_type: string; recorder_id: string; file_size: number }
export interface GbChannelRecordsInfo { current_batch: GbRecordQueryBatchInfo | null; attempt_batch: GbRecordQueryBatchInfo | null; segments: GbRecordSegmentInfo[]; next_query_at_ms: number; server_time_ms: number; total: number; page: number; page_size: number }
export interface GbRecordQueryPayload { request_id: string; session_node_id: string; start_time_sec: number; end_time_sec: number }
export interface GbRecordListParams { session_node_id: string; start_time_sec?: number; end_time_sec?: number; page?: number; page_size?: number }
export type CloudRecordingStatus = 'STARTING' | 'RUNNING' | 'STOPPING' | 'COMPLETED' | 'STOPPED' | 'PARTIAL' | 'FAILED' | 'DELETING' | 'DELETED';
export type CloudRecordingFileState = 'NONE' | 'WRITING' | 'READY' | 'MISSING' | 'DELETED';
export interface CloudRecordingSummary {
  task_id: string; request_id: string; session_node_id: string; device_id: string; channel_id: string;
  start_time_sec: number; end_time_sec: number; requested_duration_sec: number; status: CloudRecordingStatus;
  file_state: CloudRecordingFileState; progress_percent: number; recorded_duration_ms: number; progress_stale: boolean;
  current_size_bytes: number; final_size_bytes: number; file_format: string; requested_by: string;
  created_at_ms: number; started_at_ms: number; finished_at_ms: number; updated_at_ms: number;
  error_code: string; error_message: string; can_stop: boolean; can_play: boolean; can_download: boolean; can_delete: boolean;
}
export interface CloudRecordingList { items: CloudRecordingSummary[]; total: number; page: number; page_size: number }
export interface CloudRecordingAccess { url: string; expires_at_ms: number; content_type: string; file_name: string; file_size: number }
export interface GbStreamPayload { request_id: string; session_node_id?: string; token?: string; start_time_sec?: number; end_time_sec?: number; playback_id?: string; trans_mode?: string; output_type?: string; audio_codec?: 'aac' }
export interface GbBroadcastPayload extends GbStreamPayload { channel_id: string; talk_codec: 'PCMA'; talk_sample_rate: 8000; talk_channel_count: 1; talk_frame_duration_ms: 20 }

const gbPath = (value: string) => encodeURIComponent(value);
export const getGbSessionNodeConfig = (nodeId: string) => request<GbSessionConfigInfo>('/gb28181/session-nodes/' + gbPath(nodeId) + '/config');
export const listGbDevicePage = (page = 1, pageSize = 20, sessionNodeId = '', domainId = '', deviceId = '', deviceName = '', registeredOnly = false) => {
  const query = new URLSearchParams({ page: String(page), page_size: String(pageSize), session_node_id: sessionNodeId, domain_id: domainId, device_id: deviceId, device_name: deviceName, registered_only: String(registeredOnly) });
  return request<GbDevicePage>('/gb28181/devices?' + query);
};
export async function listGbDevices(pageSize = 500, sessionNodeId = '', domainId = '') {
  const items: GbDeviceInfo[] = [];
  let page = 1;
  while (true) {
    const result = await listGbDevicePage(page, pageSize, sessionNodeId, domainId);
    items.push(...result.items);
    if (!result.total || items.length >= result.total || result.items.length === 0) return items;
    page += 1;
  }
}
export const createGbDevice = (payload: GbDevicePayload) => request<GbDeviceInfo>('/gb28181/devices', { method: 'POST', body: JSON.stringify(payload) });
export const updateGbDevice = (deviceId: string, payload: GbDevicePayload) => request<GbDeviceInfo>('/gb28181/devices/' + gbPath(deviceId), { method: 'POST', body: JSON.stringify(payload) });
export const deleteGbDevice = (deviceId: string, sessionNodeId: string, domainId: string) => request<void>('/gb28181/devices/' + gbPath(deviceId) + '/delete', { method: 'POST', body: JSON.stringify({ session_node_id: sessionNodeId, domain_id: domainId }) });
export const listGbChannels = (deviceId: string, sessionNodeId = '') => request<GbChannelInfo[]>('/gb28181/devices/' + gbPath(deviceId) + '/channels?session_node_id=' + gbPath(sessionNodeId));
export const listGbResources = (deviceId: string, sessionNodeId = '') => request<GbResourceInfo[]>('/gb28181/devices/' + gbPath(deviceId) + '/resources?session_node_id=' + gbPath(sessionNodeId));
export const saveGbResourceConfirmation = (deviceId: string, resourceId: string, payload: GbResourceConfirmationPayload) => request<GbResourceInfo>('/gb28181/devices/' + gbPath(deviceId) + '/resources/' + gbPath(resourceId) + '/confirmation', { method: 'POST', body: JSON.stringify(payload) });
export const resetGbResourceConfirmation = (deviceId: string, resourceId: string, requestId: string) => request<GbResourceInfo>('/gb28181/devices/' + gbPath(deviceId) + '/resources/' + gbPath(resourceId) + '/confirmation/reset', { method: 'POST', body: JSON.stringify({ request_id: requestId }) });
export const updateGbChannel = (deviceId: string, channelId: string, payload: GbChannelPayload) => request<GbChannelInfo>('/gb28181/devices/' + gbPath(deviceId) + '/channels/' + gbPath(channelId), { method: 'POST', body: JSON.stringify(payload) });
export const listGbChannelImages = (deviceId: string, channelId: string) => request<GbChannelImageInfo[]>('/gb28181/devices/' + gbPath(deviceId) + '/channels/' + gbPath(channelId) + '/images');
export const getGbChannelRecords = (deviceId: string, channelId: string, params: GbRecordListParams) => {
  const query = new URLSearchParams({
    session_node_id: params.session_node_id,
    page: String(params.page || 1),
    page_size: String(params.page_size || 10),
  });
  if (params.start_time_sec) query.set('start_time_sec', String(params.start_time_sec));
  if (params.end_time_sec) query.set('end_time_sec', String(params.end_time_sec));
  return request<GbChannelRecordsInfo>('/gb28181/devices/' + gbPath(deviceId) + '/channels/' + gbPath(channelId) + '/records?' + query);
};
export const queryGbChannelRecords = (deviceId: string, channelId: string, payload: GbRecordQueryPayload) => request<GbChannelRecordsInfo>('/gb28181/devices/' + gbPath(deviceId) + '/channels/' + gbPath(channelId) + '/records/query', { method: 'POST', body: JSON.stringify(payload) });
export const createCloudRecording = (deviceId: string, channelId: string, payload: { request_id: string; session_node_id: string; start_time_sec: number; end_time_sec: number }) => request<CloudRecordingSummary>('/gb28181/devices/' + gbPath(deviceId) + '/channels/' + gbPath(channelId) + '/cloud-recordings', { method: 'POST', body: JSON.stringify(payload) });
export const listCloudRecordings = (deviceId: string, channelId: string, sessionNodeId: string, page = 1, pageSize = 50) => request<CloudRecordingList>('/gb28181/devices/' + gbPath(deviceId) + '/channels/' + gbPath(channelId) + '/cloud-recordings?session_node_id=' + gbPath(sessionNodeId) + '&page=' + page + '&page_size=' + pageSize);
export const getCloudRecording = (taskId: string) => request<CloudRecordingSummary>('/gb28181/cloud-recordings/' + gbPath(taskId));
export const stopCloudRecording = (taskId: string, requestId: string) => request<CloudRecordingSummary>('/gb28181/cloud-recordings/' + gbPath(taskId) + '/stop', { method: 'POST', body: JSON.stringify({ request_id: requestId }) });
export const deleteCloudRecording = (taskId: string, requestId: string) => request<CloudRecordingSummary>('/gb28181/cloud-recordings/' + gbPath(taskId) + '/delete', { method: 'POST', body: JSON.stringify({ request_id: requestId }) });
export const issueCloudRecordingAccess = (taskId: string, mode: 'inline' | 'attachment') => request<CloudRecordingAccess>('/gb28181/cloud-recordings/' + gbPath(taskId) + '/access', { method: 'POST', body: JSON.stringify({ mode }) });
export async function startGbPreview(
  deviceId: string,
  channelId: string,
  payload: GbStreamPayload,
  options: MediaOperationWaitOptions<StreamSummary> = {},
): Promise<StreamSummary> {
  const operation = await request<MediaOperationSummary<StreamSummary>>('/gb28181/devices/' + gbPath(deviceId) + '/channels/' + gbPath(channelId) + '/preview', { method: 'POST', body: JSON.stringify(payload) }, true, 3_000);
  return waitMediaOperation(operation, options);
}
export async function startGbPlayback(
  deviceId: string,
  channelId: string,
  payload: GbStreamPayload,
  options: MediaOperationWaitOptions<StreamSummary> = {},
): Promise<StreamSummary> {
  const operation = await request<MediaOperationSummary<StreamSummary>>('/gb28181/devices/' + gbPath(deviceId) + '/channels/' + gbPath(channelId) + '/playback', { method: 'POST', body: JSON.stringify(payload) }, true, 3_000);
  return waitMediaOperation(operation, options);
}
export const startGbBroadcast = (deviceId: string, payload: GbBroadcastPayload) => request<StreamSummary>('/gb28181/devices/' + gbPath(deviceId) + '/broadcast/start', { method: 'POST', body: JSON.stringify(payload) });
export const stopGbBroadcast = (broadcastId: string) => request<StreamSummary>('/gb28181/broadcasts/' + gbPath(broadcastId) + '/stop', { method: 'POST', body: '{}' });
export interface GbPtzPayload { leftRight: number; upDown: number; inOut: number; horizonSpeed: number; verticalSpeed: number; zoomSpeed: number }
export const sendGbPtz = (deviceId: string, channelId: string, payload: GbPtzPayload) => request<{ accepted: boolean; count: number }>('/gb28181/devices/' + gbPath(deviceId) + '/channels/' + gbPath(channelId) + '/ptz', { method: 'POST', body: JSON.stringify({ deviceId, channelId, ...payload }) });
export const takeGbSnapshot = (deviceId: string, channelId: string) => request<GbSnapshotInfo>('/gb28181/devices/' + gbPath(deviceId) + '/channels/' + gbPath(channelId) + '/images', { method: 'POST', body: JSON.stringify({ request_id: 'ui-snapshot-' + Date.now() }) });
