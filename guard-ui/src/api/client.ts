export interface SessionInfo {
  username: string;
  role: 'viewer' | 'operator' | 'admin';
  csrf_token: string;
  expires_at_ms: number;
}

export interface EventItem {
  event_id: string;
  topic: string;
  priority: number;
  payload: string;
}

export interface EventPage {
  items: EventItem[];
  next_after_id: string | null;
}

export const liveApi = import.meta.env.VITE_GMV_API_MODE === 'live';
let csrfToken = '';

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const headers = new Headers(init.headers);
  if (init.body) headers.set('content-type', 'application/json');
  if (csrfToken && init.method && init.method !== 'GET') headers.set('x-csrf-token', csrfToken);
  const response = await fetch(`/api/v2${path}`, {
    ...init,
    headers,
    credentials: 'include',
  });
  if (!response.ok) {
    const error = await response.json().catch(() => ({ message: `HTTP ${response.status}` }));
    throw new Error(error.message ?? `HTTP ${response.status}`);
  }
  if (response.status === 204) return undefined as T;
  return response.json() as Promise<T>;
}

export async function login(username: string, password: string): Promise<SessionInfo> {
  const session = await request<SessionInfo>('/auth/login', {
    method: 'POST',
    body: JSON.stringify({ username, password }),
  });
  csrfToken = session.csrf_token;
  return session;
}

export async function currentSession(): Promise<SessionInfo> {
  const session = await request<SessionInfo>('/auth/session');
  csrfToken = session.csrf_token;
  return session;
}

export async function logout(): Promise<void> {
  await request<void>('/auth/logout', { method: 'POST' });
  csrfToken = '';
}

export function pollEvents(afterId?: string, limit = 100): Promise<EventPage> {
  const query = new URLSearchParams({ limit: String(limit) });
  if (afterId) query.set('after_id', afterId);
  return request<EventPage>(`/events?${query}`);
}


export interface SimDevice {
  device_id: string;
  name: string;
  session_node_id: string;
  channels: string[];
  online: boolean;
}

export interface SimStream {
  stream_id: string;
  device_id: string;
  channel_id: string;
  node_id: string;
  lease_id: string;
  endpoint: string;
  state: 'running' | 'stopped' | 'failed';
}

export interface SimAiTask {
  task_id: string;
  model: string;
  stream_id: string;
  node_id: string;
  state: 'running' | 'cancelled' | 'failed';
}

export const listDevices = () => request<SimDevice[]>('/devices');
export const startPreview = (deviceId: string, channelId: string, requestId: string) =>
  request<SimStream>(`/devices/${deviceId}/preview`, {
    method: 'POST',
    body: JSON.stringify({ channel_id: channelId, request_id: requestId }),
  });
export const sendPtz = (deviceId: string, channelId: string) =>
  request<{ accepted: boolean; count: number }>(`/devices/${deviceId}/ptz`, {
    method: 'POST',
    body: JSON.stringify({ channel_id: channelId }),
  });
export const listStreams = () => request<SimStream[]>('/streams');
export const stopStream = (streamId: string) =>
  request<SimStream>(`/streams/${streamId}/stop`, { method: 'POST', body: '{}' });
export const listAiTasks = () => request<SimAiTask[]>('/ai/tasks');
export const startAiTask = (streamId: string, model: string, requestId: string) =>
  request<SimAiTask>('/ai/tasks', {
    method: 'POST',
    body: JSON.stringify({ stream_id: streamId, model, request_id: requestId }),
  });
export const cancelAiTask = (taskId: string) =>
  request<SimAiTask>(`/ai/tasks/${taskId}/cancel`, { method: 'POST', body: '{}' });
