import { expect, test } from '@playwright/test';

test('录像片段弹窗只在点击更新后发起设备查询', async ({ page }) => {
  let recordQueries = 0;
  let cloudRecordingLists = 0;
  let recordQueryRange: { start_time_sec: number; end_time_sec: number } | undefined;
  const session = {
    username: 'operator',
    nickname: '值班员',
    role: 'operator',
    csrf_token: 'csrf-record-test',
    expires_at_ms: Date.now() + 60_000,
  };
  const node = {
    node_id: 'session-1', instance_id: 'instance-1', kind: 'session', service: 'session-gb28181',
    protocol: 'gb28181', display_name: '国标节点', connection: 'CONNECTED', health: 'HEALTHY',
    scheduling: 'ENABLED', capabilities: ['protocol.gb28181'], pending_leases: 0, host_metrics: {},
    business_metrics: {}, config: { service: 'session-gb28181', protocol: 'gb28181' }, zone: null,
    last_seen_at_ms: Date.now(), generation: 1, sequence: 1,
  };
  const device = {
    device_id: '34020000001110000001', session_node_id: 'session-1', domain_id: '34020000002000000001',
    domain: '3402000000', longitude: null, latitude: null, address: null, pwd: null, pwd_check: 0,
    alias: '测试设备', status: 1, heartbeat_sec: 60, del: 0, create_time: null, tenant_id: null,
    sys_org_code: null, create_by: null, update_by: null, update_time: null, monitor_status: 1,
    device_type: 'NVR', manufacturer: 'GMV', model: 'test', firmware: null, gb_version: '2016',
    max_camera: 1, camera_in_count: 1, camera_off_count: 0, register_time: '2026-07-20 10:00:00',
  };
  const channel = {
    device_id: device.device_id, channel_id: '34020000001320000001', name: '入口相机', manufacturer: 'GMV',
    model: 'test', owner: '', status: 'ON', civil_code: '', address: '', parent_id: device.device_id,
    ip_address: '', port: 0, longitude: '', latitude: '', ptz_type: '3', alias_name: '', pic_url: '',
    snapshot: 1, over_pic_id: '', ptz_enable: 1, talk_enable: 2, audio_enable: 2, record_enable: 1,
    playback_enable: 1, alarm_enable: 1, biz_enable: 1, sort_no: 1, created_at_ms: 0, updated_at_ms: 0,
  };
  const resource = {
    device_id: device.device_id, resource_id: channel.channel_id, name: channel.name, status: 'ON',
    parent_id: device.device_id, type_code: '131', enum_id: '', enum_name: '', suggested_kind: 'video',
    classification_mode: 'default', effective_kind: 'video', effective_owner_scope: 'resource',
    effective_owner_id: channel.channel_id, warning: '', biz_enable: 1, owner_biz_enable: 1,
    supported: true, available: true, unavailable_reason: '', confirmation: null,
  };
  const current = {
    current_batch: { batch_id: 'old', status: 'READY', start_time_sec: 1_753_000_000, end_time_sec: 1_753_003_600, created_at_ms: Date.now() - 600_000 },
    attempt_batch: null,
    segments: [{ segment_id: 1, batch_id: 'old', device_id: device.device_id, channel_id: channel.channel_id,
      remote_device_id: channel.channel_id, name: '定时录像', file_path: '/record/1', address: '',
      start_time_sec: 1_753_000_000, end_time_sec: 1_753_001_800, secrecy: 0, record_type: 'time', recorder_id: '', file_size: 0 }],
    next_query_at_ms: 0, server_time_ms: Date.now(),
  };

  await page.route('**/api/v2/**', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    let body: unknown = [];
    let status = 200;
    if (path === '/api/v2/auth/session') body = session;
    else if (path === '/api/v2/nodes') body = [node];
    else if (path === '/api/v2/media/transport') body = { scheme: 'http', http_version: 'http/1.1', multi_view_limit: 6 };
    else if (path === '/api/v2/gb28181/session-nodes/session-1/config') body = { domain: '3402000000', domain_id: device.domain_id, wan_ip: '127.0.0.1', wan_port: 5060 };
    else if (path === '/api/v2/gb28181/devices') body = { items: [device], total: 1, page: 1, page_size: 20 };
    else if (path.endsWith('/channels')) body = [channel];
    else if (path.endsWith('/resources')) body = [resource];
    else if (path.endsWith('/records/query')) {
      recordQueries += 1;
      recordQueryRange = request.postDataJSON();
      expect(request.headers()['x-csrf-token']).toBe(session.csrf_token);
      status = 202;
      body = { ...current, attempt_batch: { batch_id: 'new', status: 'QUERYING', start_time_sec: 1_753_000_000, end_time_sec: 1_753_003_600, created_at_ms: Date.now() } };
    } else if (path.endsWith('/records')) body = current;
    else if (path.endsWith('/cloud-recordings')) {
      cloudRecordingLists += 1;
      body = { items: [], total: 0, page: 1, page_size: 50 };
    }
    await route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
  });

  await page.goto('/gb28181/monitor');
  await page.getByRole('button', { name: '相机' }).click();
  await page.locator('.channel-play-main', { hasText: '回放' }).click();

  await expect(page.getByText('设备录像片段')).toBeVisible();
  await expect(page.getByText('定时录像')).toBeVisible();
  expect(recordQueries).toBe(0);

  await page.getByRole('button', { name: '更新', exact: true }).click();
  await expect(page.getByText('请先选择录像检索时段')).toBeVisible();
  expect(recordQueries).toBe(0);

  await page.getByRole('button', { name: '近一周' }).click();
  await page.getByRole('button', { name: '更新', exact: true }).click();
  await expect.poll(() => recordQueries).toBe(1);
  expect(recordQueryRange!.end_time_sec - recordQueryRange!.start_time_sec).toBe(7 * 24 * 60 * 60);
  await expect(page.getByText('设备录像正在更新，当前仍展示上一次完整结果')).toBeVisible();

  await page.getByRole('button', { name: '云端录像', exact: true }).click();
  await expect(page.getByRole('heading', { name: '云端录像' })).toBeVisible();
  await expect.poll(() => cloudRecordingLists).toBe(1);
  await expect(page.getByText('暂无云端录像任务')).toBeVisible();
});
