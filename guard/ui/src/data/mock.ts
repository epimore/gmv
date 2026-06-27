export const metrics = [
  { label: '在线节点', value: 18, trend: '+3 READY', hint: '2 DRAINING' },
  { label: '活跃流', value: 246, trend: '+18 / 5m', hint: '3 orphan' },
  { label: 'AI 任务', value: 72, trend: 'GPU 68%', hint: '队列 9' },
  { label: '事件延迟', value: '42ms', trend: 'REST polling', hint: 'after_id' },
];

export const nodes = [
  { node_id: 'session-a01', service: 'session', status: 'READY', instance_id: '018f-7a2', cpu: 42, memory: 61, drift: '12ms', generation: 91 },
  { node_id: 'stream-s03', service: 'stream', status: 'DRAINING', instance_id: '018f-81c', cpu: 66, memory: 72, drift: '18ms', generation: 44 },
  { node_id: 'avai-g02', service: 'avai', status: 'READY', instance_id: '018f-91d', cpu: 53, memory: 64, drift: '9ms', generation: 37 },
  { node_id: 'stream-s07', service: 'stream', status: 'TIME_UNSYNCED', instance_id: '018f-a22', cpu: 35, memory: 48, drift: '1280ms', generation: 22 },
];

export const devices = [
  { device_id: '34020000001320000001', name: '东门枪机', status: 'ONLINE', session: 'session-a01', channels: 16, active: 7 },
  { device_id: '34020000001320000002', name: '停车场球机', status: 'ONLINE', session: 'session-b02', channels: 8, active: 2 },
  { device_id: '34020000001320000003', name: '仓库通道', status: 'OFFLINE', session: '-', channels: 12, active: 0 },
  { device_id: '34020000001320000004', name: '办公楼 NVR', status: 'ORPHAN', session: 'session-a01', channels: 32, active: 1 },
];

export const streams = [
  { stream_id: 'str_9121', device_id: '34020000001320000001', channel_id: 'ch_001', state: 'CONFIRMED', endpoint: 'rtp://10.20.1.4:30220', lease_id: 'lea_771' },
  { stream_id: 'str_9122', device_id: '34020000001320000002', channel_id: 'ch_004', state: 'PENDING', endpoint: 'rtp://10.20.1.5:30224', lease_id: 'lea_772' },
  { stream_id: 'str_9123', device_id: '34020000001320000004', channel_id: 'ch_013', state: 'ORPHAN', endpoint: 'rtp://10.20.1.8:30238', lease_id: 'lea_773' },
];

export const aiTasks = [
  { task_id: 'ai_1042', model: 'vehicle-detect', state: 'RUNNING', node: 'avai-g02', fps: 24, gpu: 72 },
  { task_id: 'ai_1043', model: 'face-quality', state: 'PENDING', node: 'avai-g04', fps: 0, gpu: 41 },
  { task_id: 'ai_1044', model: 'intrusion', state: 'FAILED', node: 'avai-g01', fps: 0, gpu: 55 },
  { task_id: 'ai_1045', model: 'helmet', state: 'COMPLETED', node: 'avai-g02', fps: 18, gpu: 68 },
];

export const leases = [
  { allocation_id: 'alc_3101', lease_id: 'lea_771', resource: 'str_9121', type: 'stream', state: 'CONFIRMED', score: 0.92 },
  { allocation_id: 'alc_3102', lease_id: 'lea_772', resource: 'str_9122', type: 'stream', state: 'PENDING', score: 0.81 },
  { allocation_id: 'alc_3103', lease_id: 'lea_781', resource: 'ai_1042', type: 'ai task', state: 'CONFIRMED', score: 0.88 },
  { allocation_id: 'alc_3104', lease_id: 'lea_790', resource: 'str_9019', type: 'stream', state: 'EXPIRED', score: 0.44 },
];

export const events = [
  { id: 'evt_000245', level: 'P1', type: 'TIME_UNSYNCED', source: 'stream-s07', message: '节点时间偏差超过接入阈值', cursor: 'cur_7f91' },
  { id: 'evt_000244', level: 'P2', type: 'ORPHAN_DETECTED', source: 'str_9123', message: '检测到孤儿流路由', cursor: 'cur_7f90' },
  { id: 'evt_000243', level: 'P1', type: 'CONFLICT_DETECTED', source: 'lea_790', message: '租约 generation 冲突', cursor: 'cur_7f89' },
  { id: 'evt_000242', level: 'P3', type: 'NODE_READY', source: 'session-a01', message: '节点进入 READY', cursor: 'cur_7f88' },
];

export const integrations = [
  { channel: 'MQTT', target: 'gmv/v2/commands/#', state: 'DELIVERED', retry: 0, security: 'QoS 1' },
  { channel: 'Webhook', target: 'https://ops.example/events', state: 'RETRY_WAIT', retry: 3, security: 'HMAC' },
  { channel: 'Outbox', target: 'evt_000245', state: 'PENDING', retry: 0, security: 'bounded queue' },
  { channel: 'Dead Letter', target: 'cmd_9a11', state: 'DEAD', retry: 8, security: 'TTL expired' },
];

export const systemJobs = [
  { job: 'SQLite 在线备份', state: 'READY', progress: 100, detail: 'guard.db -> backup-20260625.db' },
  { job: 'MySQL 迁移检查', state: 'WARNING', progress: 68, detail: '等待人工确认' },
  { job: 'TLS 证书轮转', state: 'READY', progress: 100, detail: '有效期 89 天' },
  { job: 'NTP / chrony 校验', state: 'READY', progress: 100, detail: '最大偏差 18ms' },
];

export const lineOption = (name: string, color = '#34d8ff') => ({
  backgroundColor: 'transparent',
  grid: { left: 28, right: 18, top: 24, bottom: 26 },
  tooltip: { trigger: 'axis' },
  xAxis: { type: 'category', data: ['12:00', '12:05', '12:10', '12:15', '12:20', '12:25'], axisLine: { lineStyle: { color: '#34506d' } } },
  yAxis: { type: 'value', splitLine: { lineStyle: { color: 'rgba(120,220,255,.1)' } } },
  series: [{ name, type: 'line', smooth: true, symbol: 'circle', data: [32, 48, 41, 66, 58, 72], lineStyle: { color, width: 3 }, areaStyle: { color: `${color}22` } }],
});

export const radarOption = {
  backgroundColor: 'transparent',
  radar: { indicator: [{ name: 'CPU', max: 100 }, { name: '内存', max: 100 }, { name: '带宽', max: 100 }, { name: '队列', max: 100 }, { name: 'FPS', max: 100 }], axisName: { color: '#9edfff' }, splitLine: { lineStyle: { color: 'rgba(120,220,255,.16)' } }, splitArea: { areaStyle: { color: ['rgba(52,216,255,.03)', 'rgba(168,117,255,.05)'] } } },
  series: [{ type: 'radar', data: [{ value: [62, 70, 54, 38, 82], name: '容量' }], areaStyle: { color: 'rgba(52,216,255,.20)' }, lineStyle: { color: '#34d8ff' } }],
};

export const graphOption = {
  backgroundColor: 'transparent',
  tooltip: {},
  series: [{
    type: 'graph', layout: 'force', roam: false, force: { repulsion: 140, edgeLength: 78 },
    label: { show: true, color: '#dff7ff' },
    data: [
      { name: 'device', value: 30, symbolSize: 52 }, { name: 'session', value: 20, symbolSize: 58 },
      { name: 'stream', value: 18, symbolSize: 58 }, { name: 'lease', value: 12, symbolSize: 44 },
      { name: 'avai', value: 16, symbolSize: 50 },
    ],
    links: [{ source: 'device', target: 'session' }, { source: 'session', target: 'stream' }, { source: 'stream', target: 'lease' }, { source: 'stream', target: 'avai' }],
    lineStyle: { color: '#34d8ff', opacity: .46, width: 2 },
    itemStyle: { color: '#17345f', borderColor: '#34d8ff', borderWidth: 2 },
  }],
};
