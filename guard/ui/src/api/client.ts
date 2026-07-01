export type Role = 'viewer' | 'operator' | 'admin';
export interface SessionInfo { username: string; role: Role; nickname: string; csrf_token: string; expires_at_ms: number }
export class ApiError extends Error {
  constructor(public readonly status: number, message: string) { super(message); this.name = 'ApiError'; }
}
export interface UserInfo { username: string; role: Role; nickname: string; enabled: boolean; created_at_ms: number; updated_at_ms: number }
export interface DashboardInfo { node_count: number; event_count: number; next_after_id: string | null }
export interface HostMetricsInfo { cpu_usage_percent: number; load_average_1m: number; load_average_5m: number; load_average_15m: number; memory_total_bytes: number; memory_used_bytes: number; swap_total_bytes: number; swap_used_bytes: number; disk_read_bytes_per_sec: number; disk_write_bytes_per_sec: number; network_receive_bytes_per_sec: number; network_transmit_bytes_per_sec: number; process_resident_memory_bytes: number; process_threads: number }
export interface NodeInfo { node_id: string; instance_id: string; kind: string; service: string; protocol: string | null; display_name: string; connection: string; health: string; scheduling: string; capabilities: string[]; capacity: number; pending_leases: number; host_metrics: HostMetricsInfo; business_metrics: Record<string, string>; config: Record<string, string>; zone: string | null; last_seen_at_ms: number; generation: number; sequence: number }
export interface EventItem { event_id: string; topic: string; priority: number; payload: string }
export interface EventPage { items: EventItem[]; next_after_id: string | null }
export interface LeaseInfo { lease_id: string; route_id: string; resource_id: string; node_id: string; instance_id: string; state: 'allocated' | 'confirmed' | 'failed' | 'released' | 'expired'; expires_at_ms: number }
export interface OperationInfo { operation_id: string; kind: string; requested_by: string; required_role: Role; status: 'accepted' | 'running' | 'succeeded' | 'failed' | 'cancelled'; progress_percent: number; message: string; error: string | null }
export interface SystemJobInfo { job_id: string; job_type: 'backup' | 'restore' | 'migrate' | 'reconcile'; status: 'pending' | 'running' | 'succeeded' | 'failed'; progress_percent: number; message: string; error: string | null }
export interface OutboxInfo { outbox_id: string; event_id: string; destination_kind: 'mqtt' | 'webhook'; destination: string; state: 'pending' | 'sending' | 'delivered' | 'retry_wait' | 'dead'; attempts: number; next_attempt_at_ms: number; last_error: string | null; created_at_ms: number; updated_at_ms: number }
export interface DeviceSummary { device_id: string; name: string; session_node_id: string; channels: string[]; online: boolean }
export interface StreamSummary { stream_id: string; device_id: string; channel_id: string; node_id: string; lease_id: string; endpoint: string; state: 'running' | 'stopped' | 'failed' }
export interface AiTaskSummary { task_id: string; model: string; stream_id: string; node_id: string; state: 'running' | 'cancelled' | 'failed' }
export interface RuntimeStatus { guard_available: boolean; streams: number; running_streams: number; ai_tasks: number; running_ai_tasks: number; ptz_commands: number }
export interface HealthInfo { status: string }
export interface CreateUserPayload { username: string; role: Role; nickname: string; password: string; enabled: boolean }
export interface UpdateUserPayload { role: Role; nickname?: string; password?: string | null; enabled: boolean }
export interface UpdateProfilePayload { nickname?: string; password?: string }

export const liveApi = import.meta.env.VITE_GMV_API_MODE !== 'mock';
let csrfToken = '';
let unauthorizedHandler: (() => void) | undefined;
export function setUnauthorizedHandler(handler: () => void): void { unauthorizedHandler = handler; }

async function requestAt<T>(url: string, init: RequestInit = {}, redirectOnUnauthorized = true): Promise<T> {
  const method = (init.method ?? 'GET').toUpperCase();
  if (method !== 'GET' && method !== 'POST') throw new Error('HTTP method is not allowed: ' + method);
  const headers = new Headers(init.headers);
  if (init.body) headers.set('content-type', 'application/json');
  if (csrfToken && method === 'POST') headers.set('x-csrf-token', csrfToken);
  const response = await fetch(url, { ...init, headers, credentials: 'include' });
  if (!response.ok) {
    const error = await response.json().catch(() => ({ message: 'HTTP ' + response.status }));
    if (response.status === 401 && redirectOnUnauthorized) { csrfToken = ''; unauthorizedHandler?.(); }
    throw new ApiError(response.status, error.message ?? 'HTTP ' + response.status);
  }
  if (response.status === 204) return undefined as T;
  return response.json() as Promise<T>;
}
const request = <T>(path: string, init: RequestInit = {}, redirectOnUnauthorized = true) => requestAt<T>('/api/v2' + path, init, redirectOnUnauthorized);

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
export const listOperations = () => request<OperationInfo[]>('/operations');
export const startOperation = (kind: string, dangerous = false) => request<OperationInfo>('/operations', { method: 'POST', body: JSON.stringify({ operation_id: 'ui-' + kind.replaceAll('.', '-') + '-' + Date.now(), kind, dangerous, confirmation: dangerous ? kind : null }) });
export const listSystemJobs = () => request<SystemJobInfo[]>('/system/jobs');
export const startSystemJob = (jobType: SystemJobInfo['job_type']) => request<SystemJobInfo>('/system/jobs', { method: 'POST', body: JSON.stringify({ job_id: 'ui-' + jobType + '-' + Date.now(), job_type: jobType }) });
export const listOutbox = (limit = 100) => request<OutboxInfo[]>('/integrations/outbox?limit=' + limit);
export const retryOutbox = (outboxId: string) => request<OutboxInfo>('/integrations/outbox/' + encodeURIComponent(outboxId) + '/retry', { method: 'POST', body: '{}' });
export const listDevices = () => request<DeviceSummary[]>('/devices');
export const startPreview = (deviceId: string, channelId: string, requestId: string) => request<StreamSummary>('/devices/' + deviceId + '/preview', { method: 'POST', body: JSON.stringify({ channel_id: channelId, request_id: requestId }) });
export const sendPtz = (deviceId: string, channelId: string) => request<{ accepted: boolean; count: number }>('/devices/' + deviceId + '/ptz', { method: 'POST', body: JSON.stringify({ channel_id: channelId }) });
export const listStreams = () => request<StreamSummary[]>('/streams');
export const stopStream = (streamId: string) => request<StreamSummary>('/streams/' + streamId + '/stop', { method: 'POST', body: '{}' });
export const listAiTasks = () => request<AiTaskSummary[]>('/ai/tasks');
export const startAiTask = (streamId: string, model: string, requestId: string) => request<AiTaskSummary>('/ai/tasks', { method: 'POST', body: JSON.stringify({ stream_id: streamId, model, request_id: requestId }) });
export const cancelAiTask = (taskId: string) => request<AiTaskSummary>('/ai/tasks/' + taskId + '/cancel', { method: 'POST', body: '{}' });
export const runtimeStatus = () => request<RuntimeStatus>('/runtime/status');
export const healthLive = () => requestAt<HealthInfo>('/health/live');
export const healthReady = () => requestAt<HealthInfo>('/health/ready');


export interface GbSessionConfigInfo { domain: string; domain_id: string; wan_ip: string; wan_port: number }
export interface GbDeviceInfo { device_id: string; session_node_id: string; domain_id: string; domain: string; longitude: string | null; latitude: string | null; address: string | null; pwd: string | null; pwd_check: number; alias: string | null; status: number; heartbeat_sec: number; del: number; create_time: string | null; tenant_id: string | null; sys_org_code: string | null; create_by: string | null; update_by: string | null; update_time: string | null; channel_count: number }
export interface GbDevicePayload { device_id?: string; session_node_id?: string; domain_id?: string; domain?: string; longitude?: string; latitude?: string; address?: string; pwd?: string; pwd_check?: number; alias?: string; status?: number; heartbeat_sec?: number; tenant_id?: string; sys_org_code?: string; create_by?: string; update_by?: string }
export interface GbChannelInfo { device_id: string; channel_id: string; name: string; manufacturer: string; model: string; owner: string; status: string; civil_code: string; address: string; parent_id: string; ip_address: string; port: number; longitude: string; latitude: string; ptz_type: string; alias_name: string; pic_url: string; snapshot: number; over_pic_id: string; ptz_enable: number; talk_enable: number; audio_enable: number; record_enable: number; playback_enable: number; alarm_enable: number; biz_enable: number; sort_no: number; created_at_ms: number; updated_at_ms: number }
export interface GbChannelPayload { channel_id: string; name?: string; manufacturer?: string; model?: string; owner?: string; status?: string; civil_code?: string; address?: string; parent_id?: string; ip_address?: string; port?: number; longitude?: string; latitude?: string; ptz_type?: string; alias_name?: string; pic_url?: string; snapshot?: number; over_pic_id?: string; ptz_enable?: number; talk_enable?: number; audio_enable?: number; record_enable?: number; playback_enable?: number; alarm_enable?: number; biz_enable?: number; sort_no?: number }
export interface GbChannelImageInfo { image_id: string; device_id: string; channel_id: string; image_url: string; created_at_ms: number }
export interface GbStreamPayload { request_id: string; token?: string; start_time_sec?: number; end_time_sec?: number; trans_mode?: string; output_type?: string }

const gbPath = (value: string) => encodeURIComponent(value);
export const getGbSessionNodeConfig = (nodeId: string) => request<GbSessionConfigInfo>('/gb28181/session-nodes/' + gbPath(nodeId) + '/config');
export const listGbDevices = () => request<GbDeviceInfo[]>('/gb28181/devices');
export const createGbDevice = (payload: GbDevicePayload) => request<GbDeviceInfo>('/gb28181/devices', { method: 'POST', body: JSON.stringify(payload) });
export const updateGbDevice = (deviceId: string, payload: GbDevicePayload) => request<GbDeviceInfo>('/gb28181/devices/' + gbPath(deviceId), { method: 'POST', body: JSON.stringify(payload) });
export const deleteGbDevice = (deviceId: string) => request<void>('/gb28181/devices/' + gbPath(deviceId) + '/delete', { method: 'POST', body: '{}' });
export const listGbChannels = (deviceId: string) => request<GbChannelInfo[]>('/gb28181/devices/' + gbPath(deviceId) + '/channels');
export const createGbChannel = (deviceId: string, payload: GbChannelPayload) => request<GbChannelInfo>('/gb28181/devices/' + gbPath(deviceId) + '/channels', { method: 'POST', body: JSON.stringify(payload) });
export const updateGbChannel = (deviceId: string, channelId: string, payload: GbChannelPayload) => request<GbChannelInfo>('/gb28181/devices/' + gbPath(deviceId) + '/channels/' + gbPath(channelId), { method: 'POST', body: JSON.stringify(payload) });
export const deleteGbChannel = (deviceId: string, channelId: string) => request<void>('/gb28181/devices/' + gbPath(deviceId) + '/channels/' + gbPath(channelId) + '/delete', { method: 'POST', body: '{}' });
export const listGbChannelImages = (deviceId: string, channelId: string) => request<GbChannelImageInfo[]>('/gb28181/devices/' + gbPath(deviceId) + '/channels/' + gbPath(channelId) + '/images');
export const startGbPreview = (deviceId: string, channelId: string, payload: GbStreamPayload) => request<StreamSummary>('/gb28181/devices/' + gbPath(deviceId) + '/channels/' + gbPath(channelId) + '/preview', { method: 'POST', body: JSON.stringify(payload) });
export const startGbPlayback = (deviceId: string, channelId: string, payload: GbStreamPayload) => request<StreamSummary>('/gb28181/devices/' + gbPath(deviceId) + '/channels/' + gbPath(channelId) + '/playback', { method: 'POST', body: JSON.stringify(payload) });
export const sendGbPtz = (deviceId: string, channelId: string) => request<{ accepted: boolean; count: number }>('/gb28181/devices/' + gbPath(deviceId) + '/channels/' + gbPath(channelId) + '/ptz', { method: 'POST', body: '{}' });
export const takeGbSnapshot = (deviceId: string, channelId: string) => request<{ accepted: boolean }>('/gb28181/devices/' + gbPath(deviceId) + '/channels/' + gbPath(channelId) + '/snapshot', { method: 'POST', body: '{}' });
