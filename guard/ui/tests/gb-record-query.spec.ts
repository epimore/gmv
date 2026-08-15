import { expect, test } from '@playwright/test';

test('录像查询仅按用户操作发起，并通过票据展示抓拍图集', async ({ page }) => {
  let recordQueries = 0;
  const recordLists: URLSearchParams[] = [];
  let cloudRecordingLists = 0;
  let cloudRecordingCreateRange: { start_time_sec: number; end_time_sec: number } | undefined;
  let recordQueryRange: { start_time_sec: number; end_time_sec: number } | undefined;
  let previewOutputType = '';
  let previewStarts = 0;
  let playbackOutputType = '';
  let multiSeekRequests = 0;
  let releaseMultiSeek: (() => void) | undefined;
  let imageAccesses = 0;
  const releasedStreams: string[] = [];
  let coverUpdates = 0;
  const imageLists: URLSearchParams[] = [];
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
    snapshot: 1, over_pic_id: '', ptz_enable: 1, broadcast_enable: 2, audio_enable: 2, record_enable: 1,
    playback_enable: 1, alarm_enable: 1, biz_enable: 1, sort_no: 1, created_at_ms: 0, updated_at_ms: 0,
    cover_image_id: '16873',
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
    next_query_at_ms: 0, server_time_ms: Date.now(), total: 21, page: 1, page_size: 10,
  };
  const cloudRecording = {
    task_id: 'cloud-task-1', request_id: 'cloud-request-1', session_node_id: 'session-1',
    device_id: device.device_id, channel_id: channel.channel_id,
    start_time_sec: 1_753_000_000, end_time_sec: 1_753_001_800, requested_duration_sec: 1_800,
    status: 'RUNNING', file_state: 'WRITING', progress_percent: 50, recorded_duration_ms: 900_000,
    progress_stale: false, current_size_bytes: 1_024, final_size_bytes: 0, file_format: 'mp4',
    requested_by: session.username, created_at_ms: Date.now(), started_at_ms: Date.now(), finished_at_ms: 0,
    updated_at_ms: Date.now(), error_code: '', error_message: '', can_stop: true, can_play: false,
    can_download: false, can_delete: false,
  };
  const playbackStream = {
    stream_id: 'playback-stream-1', device_id: device.device_id, channel_id: channel.channel_id,
    node_id: 'stream-node-1', lease_id: 'lease-1', endpoint: '/test-playback.flv',
    subscription_id: 'subscription-1', session_node_id: 'session-1', session_instance_id: 'instance-1',
    playback_id: 'playback-1', playback_generation: 1,
    playback_start_time_sec: current.segments[0].start_time_sec,
    playback_end_time_sec: current.segments[0].end_time_sec, state: 'running',
  };
  const previewStream = {
    ...playbackStream,
    stream_id: 'preview-stream-1', endpoint: '/test-preview/index.m3u8',
    playback_id: null, playback_generation: null, playback_start_time_sec: null, playback_end_time_sec: null,
  };
  const image = {
    image_id: '16873', device_id: device.device_id, channel_id: channel.channel_id, image_url: '',
    created_at_ms: Date.now(), file_name: 'snapshot.jpeg', content_type: 'image/jpeg', file_size: 128,
    can_preview: true, session_node_id: 'session-1',
  };

  await page.route('**/test-snapshot.svg', (route) => route.fulfill({
    status: 200,
    contentType: 'image/svg+xml',
    body: '<svg xmlns="http://www.w3.org/2000/svg" width="16" height="12"><rect width="16" height="12" fill="navy"/></svg>',
  }));

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
    else if (path.endsWith('/images/16873/access')) {
      imageAccesses += 1;
      expect(request.method()).toBe('POST');
      expect(request.headers()['x-csrf-token']).toBe(session.csrf_token);
      expect(request.postDataJSON()).toEqual({ session_node_id: 'session-1', mode: 'inline' });
      body = { url: '/test-snapshot.svg', expires_at_ms: Date.now() + 300_000, content_type: 'image/jpeg', file_name: image.file_name, file_size: image.file_size };
    } else if (path.endsWith('/images/16873/cover')) {
      coverUpdates += 1;
      expect(request.method()).toBe('POST');
      expect(request.postDataJSON()).toEqual({ session_node_id: 'session-1' });
      body = { ...channel, over_pic_id: image.image_id, cover_image_id: image.image_id };
    } else if (path.endsWith('/images')) {
      imageLists.push(new URL(request.url()).searchParams);
      body = { items: [image], total: 1, page: 1, page_size: 12 };
    }
    else if (path.endsWith('/preview') && request.method() === 'POST') {
      previewStarts += 1;
      previewOutputType = request.postDataJSON().output_type;
      body = {
        operation_id: 'preview-operation-1', state: 'ready', stage: 'ready', elapsed_ms: 10,
        last_progress_at_ms: Date.now(), checkpoint_ms: 8_000, hard_timeout_ms: 30_000,
        can_continue: false, result: previewStream, error: null,
      };
    } else if (path.includes('/streams/') && path.endsWith('/release') && request.method() === 'POST') {
      releasedStreams.push(path);
      if (releasedStreams.length === 1) {
        status = 409;
        body = {
          code: 'conflict',
          message: `session owner for stream ${previewStream.stream_id} is unavailable or stale`,
          user_message: '操作未完成，请检查目标资源状态后重试',
          retryable: false,
        };
      } else {
        body = previewStream;
      }
    } else if (path.endsWith('/playback') && request.method() === 'POST') {
      const playbackRequest = request.postDataJSON();
      playbackOutputType = playbackRequest.output_type;
      body = {
        operation_id: 'playback-operation-1', state: 'ready', stage: 'ready', elapsed_ms: 10,
        last_progress_at_ms: Date.now(), checkpoint_ms: 8_000, hard_timeout_ms: 30_000,
        can_continue: false,
        result: {
          ...playbackStream,
          playback_start_time_sec: playbackRequest.start_time_sec,
          playback_end_time_sec: playbackRequest.end_time_sec,
        },
        error: null,
      };
    } else if (path.endsWith('/playbacks/playback-1/seek') && request.method() === 'POST') {
      multiSeekRequests += 1;
      await new Promise<void>((resolve) => { releaseMultiSeek = resolve; });
      body = { generation: 2 };
    }
    else if (path.endsWith('/records/query')) {
      recordQueries += 1;
      recordQueryRange = request.postDataJSON();
      expect(request.headers()['x-csrf-token']).toBe(session.csrf_token);
      status = 202;
      body = { ...current, attempt_batch: { batch_id: 'new', status: 'QUERYING', start_time_sec: 1_753_000_000, end_time_sec: 1_753_003_600, created_at_ms: Date.now() } };
    } else if (path.endsWith('/records')) {
      const url = new URL(request.url());
      recordLists.push(url.searchParams);
      body = {
        ...current,
        page: Number(url.searchParams.get('page') || 1),
        page_size: Number(url.searchParams.get('page_size') || 10),
      };
    }
    else if (path.endsWith('/cloud-recordings') && request.method() === 'POST') {
      cloudRecordingCreateRange = request.postDataJSON();
      body = cloudRecording;
    } else if (path.endsWith('/cloud-recordings')) {
      cloudRecordingLists += 1;
      body = { items: [cloudRecording], total: 1, page: 1, page_size: 50 };
    }
    await route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
  });

  await page.goto('/gb28181/monitor');
  await page.getByRole('button', { name: '相机' }).click();
  await expect(page.locator('.channel-cover img')).toHaveAttribute('src', '/test-snapshot.svg');
  await page.locator('.channel-live-dropdown').getByRole('button', { name: '直播', exact: true }).click();
  await page.getByRole('menuitem', { name: 'LL-HLS', exact: true }).click();
  await expect.poll(() => previewOutputType).toBe('ll_hls');
  await expect(page.getByRole('dialog', { name: /^实时直播 ·/ })).toBeVisible();
  const livePlayer = page.locator('.monitor-player-dialog .gmv-player');
  const liveVideo = livePlayer.locator('.gmv-video').first();
  await livePlayer.getByLabel('更多操作').click();
  await livePlayer.getByLabel('切换媒体信息').click();
  await livePlayer.getByLabel('更多操作').click();
  await livePlayer.getByLabel('切换云台控制').click();
  await livePlayer.getByLabel('更多操作').click();
  await liveVideo.click({ position: { x: 20, y: 20 } });
  await expect(livePlayer).toHaveClass(/player-chrome-hidden/);
  await expect(livePlayer.locator('.media-info-panel')).toBeHidden();
  await expect(livePlayer.locator('.ptz-panel')).toBeHidden();
  await expect(livePlayer.locator('.overflow-menu')).toHaveCount(0);

  await liveVideo.click({ position: { x: 20, y: 20 } });
  await expect(livePlayer).not.toHaveClass(/player-chrome-hidden/);
  await expect(livePlayer.locator('.media-info-panel')).toBeVisible();
  await expect(livePlayer.locator('.ptz-panel')).toBeVisible();
  await page.waitForTimeout(3_000);
  await expect(livePlayer).toHaveClass(/player-chrome-hidden/);
  await expect(livePlayer.locator('.media-info-panel')).toHaveCount(0);
  await expect(livePlayer.locator('.ptz-panel')).toHaveCount(0);
  await page.locator('.monitor-player-dialog .el-dialog__headerbtn').click();
  await expect.poll(() => releasedStreams.length).toBe(1);
  await expect(page.getByRole('dialog', { name: /^实时直播 ·/ })).toBeHidden();
  await page.locator('.channel-live-dropdown').getByRole('button', { name: '直播', exact: true }).click();
  await page.getByRole('menuitem', { name: 'LL-HLS', exact: true }).click();
  await expect.poll(() => previewStarts).toBe(2);
  await page.locator('.monitor-player-dialog .el-dialog__headerbtn').click();
  await expect.poll(() => releasedStreams.length).toBe(2);
  const channelCard = page.locator('.channel-card');
  const actionRows = await channelCard.locator('.channel-actions').evaluate((footer) =>
    [...footer.children].map((child) => (child as HTMLElement).offsetTop),
  );
  expect(actionRows).toHaveLength(6);
  expect(new Set(actionRows).size).toBe(2);
  await expect(channelCard.getByRole('button', { name: '广播', exact: true })).toHaveCount(0);
  await expect(channelCard.getByRole('button', { name: '配置', exact: true })).toHaveCount(0);
  await channelCard.getByRole('button', { name: '更多', exact: true }).click();
  await expect(page.getByRole('menuitem', { name: '广播', exact: true })).toBeVisible();
  await page.getByRole('menuitem', { name: '配置', exact: true }).click();
  await expect(page.getByRole('heading', { name: '相机业务配置' })).toBeVisible();
  await page.locator('.camera-config-drawer .el-drawer__close-btn').click();
  await channelCard.getByRole('button', { name: '图集', exact: true }).click();
  await expect(page.getByRole('heading', { name: '抓拍图集', exact: true })).toBeVisible();
  await expect.poll(() => imageAccesses).toBe(2);
  expect(imageLists[0].get('page')).toBe('1');
  expect(imageLists[0].get('page_size')).toBe('12');
  expect(imageLists[0].get('session_node_id')).toBe('session-1');
  await expect(page.locator('.gallery-image img')).toHaveAttribute('src', '/test-snapshot.svg');
  await expect(page.locator('.image-pagination .el-pagination__total')).toContainText('1');
  await expect(page.locator('.image-pagination .el-pagination__sizes')).toBeVisible();
  await expect(page.locator('.image-pagination .btn-prev')).toBeVisible();
  await expect(page.locator('.image-pagination .btn-next')).toBeVisible();
  await expect(page.locator('.image-pagination .el-pagination__jump')).toBeVisible();
  await expect.poll(() => page.evaluate(() => {
    const root = document.scrollingElement;
    return !!root && root.scrollHeight <= root.clientHeight + 1;
  })).toBe(true);
  await page.getByRole('button', { name: '设为封面', exact: true }).click();
  await expect.poll(() => coverUpdates).toBe(1);
  await expect(page.getByRole('button', { name: '当前封面', exact: true })).toBeDisabled();
  const imageTimeInputs = page.locator('.image-time-filter input');
  await imageTimeInputs.nth(0).fill('2026-07-21 00:00:00');
  await imageTimeInputs.nth(0).press('Enter');
  await page.locator('.image-time-filter').getByRole('button', { name: '查询', exact: true }).click();
  await expect.poll(() => imageLists.length).toBe(2);
  expect(imageLists.at(-1)!.get('start_time_ms')).toBeTruthy();
  expect(imageLists.at(-1)!.get('end_time_ms')).toBeNull();
  await imageTimeInputs.nth(0).clear();
  await imageTimeInputs.nth(1).fill('2026-07-22 00:00:00');
  await imageTimeInputs.nth(1).press('Enter');
  await page.locator('.image-time-filter').getByRole('button', { name: '查询', exact: true }).click();
  await expect.poll(() => imageLists.length).toBe(3);
  expect(imageLists.at(-1)!.get('start_time_ms')).toBeNull();
  expect(imageLists.at(-1)!.get('end_time_ms')).toBeTruthy();
  await page.getByRole('button', { name: '返回通道', exact: true }).click();
  await page.locator('.channel-play-main', { hasText: '回放' }).click();

  const playbackRangeDialog = page.getByRole('dialog', { name: '历史回放', exact: true });
  await expect(playbackRangeDialog.locator('.record-functional-block')).toHaveCount(2);
  await expect(playbackRangeDialog.locator('.record-playback-panel').getByRole('heading', { name: '历史回放' })).toBeVisible();
  await expect(playbackRangeDialog.locator('.device-record-panel').getByRole('heading', { name: '设备录像片段' })).toBeVisible();
  await expect(playbackRangeDialog.getByRole('button', { name: '下载', exact: true })).toHaveCount(0);
  await expect(page.getByText('30分钟')).toBeVisible();
  await expect(page.getByRole('button', { name: '自定义' })).toHaveCount(0);
  await expect.poll(() => recordLists.length).toBe(1);
  expect(recordLists[0].get('start_time_sec')).toBeNull();
  expect(recordLists[0].get('end_time_sec')).toBeNull();
  expect(recordQueries).toBe(0);

  await page.locator('.record-pagination .btn-next').click();
  await expect.poll(() => recordLists.length).toBe(2);
  expect(recordLists.at(-1)!.get('page')).toBe('2');

  const databaseTimeInputs = page.locator('.record-database-query input');
  await databaseTimeInputs.nth(0).fill('2026-07-21 00:00:00');
  await databaseTimeInputs.nth(0).press('Enter');
  await page.getByRole('button', { name: '查询', exact: true }).click();
  await expect.poll(() => recordLists.length).toBe(3);
  expect(recordLists.at(-1)!.get('page')).toBe('1');
  expect(recordLists.at(-1)!.get('start_time_sec')).toBeTruthy();
  expect(recordLists.at(-1)!.get('end_time_sec')).toBeNull();

  await databaseTimeInputs.nth(0).clear();
  await databaseTimeInputs.nth(1).fill('2026-07-22 00:00:00');
  await databaseTimeInputs.nth(1).press('Enter');
  await page.getByRole('button', { name: '查询', exact: true }).click();
  await expect.poll(() => recordLists.length).toBe(4);
  expect(recordLists.at(-1)!.get('start_time_sec')).toBeNull();
  expect(recordLists.at(-1)!.get('end_time_sec')).toBeTruthy();

  await page.getByRole('button', { name: '更新', exact: true }).click();
  await expect(page.getByText('请先选择录像检索时段')).toBeVisible();
  expect(recordQueries).toBe(0);

  await page.getByRole('button', { name: '近一周' }).click();
  await page.getByRole('button', { name: '更新', exact: true }).click();
  await expect.poll(() => recordQueries).toBe(1);
  expect(recordQueryRange!.end_time_sec - recordQueryRange!.start_time_sec).toBe(7 * 24 * 60 * 60);
  await expect(page.getByText('设备录像正在更新，当前仍展示上一次完整结果')).toBeVisible();

  await playbackRangeDialog.locator('.el-dialog__headerbtn').click();
  await expect(playbackRangeDialog).toBeHidden();
  await page.locator('.channel-card').getByRole('button', { name: '下载', exact: true }).click();
  await expect(page.getByRole('heading', { name: '设备录像下载', exact: true })).toBeVisible();
  await expect(page.locator('.cloud-recording-drawer-title').getByText('下载任务', { exact: true })).toBeVisible();
  await expect(page.getByText('将设备历史录像下载到平台，完成后可在线播放或下载到本地。')).toBeVisible();
  await expect(page.getByRole('button', { name: '本地下载', exact: true })).toBeVisible();
  await expect.poll(() => cloudRecordingLists).toBe(1);
  await expect(page.getByPlaceholder('请选择开始时间')).toBeVisible();
  await expect(page.getByPlaceholder('请选择结束时间')).toBeVisible();
  await expect(page.getByRole('button', { name: '创建当前时段任务' })).toHaveCount(0);
  await expect(page.getByText(/^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2} 至 \d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$/).last()).toBeVisible();
  await expect(page.getByText('开始时间', { exact: true })).toHaveCount(1);
  await expect(page.getByText('结束时间', { exact: true })).toHaveCount(1);
  await page.waitForTimeout(2_200);
  expect(cloudRecordingLists).toBe(1);
  await page.getByRole('button', { name: '刷新', exact: true }).click();
  await expect.poll(() => cloudRecordingLists).toBe(2);
  const cloudRecordingTimeInputs = page.locator('.cloud-recording-create-form input');
  await cloudRecordingTimeInputs.nth(0).fill('2026-07-22 00:00:00');
  await cloudRecordingTimeInputs.nth(0).press('Enter');
  await cloudRecordingTimeInputs.nth(1).fill('2026-07-22 00:08:00');
  await cloudRecordingTimeInputs.nth(1).press('Enter');
  await page.getByRole('button', { name: '开始下载', exact: true }).click();
  await expect.poll(() => cloudRecordingCreateRange).toBeTruthy();
  expect(cloudRecordingCreateRange!.end_time_sec - cloudRecordingCreateRange!.start_time_sec).toBe(8 * 60);

  await page.locator('.cloud-recording-drawer .el-drawer__close-btn').click();
  await expect(page.getByRole('heading', { name: '设备录像下载', exact: true })).toBeHidden();
  await page.locator('.channel-play-main', { hasText: '回放' }).click();
  await page.locator('.record-output-select').click();
  await page.getByRole('option', { name: 'HLS-fMP4', exact: true }).click();
  await page.locator('.record-segment-table .el-table__body tr').first().click();
  await page.getByRole('button', { name: '开始播放' }).click();
  await expect.poll(() => playbackOutputType).toBe('hls');
  await expect(page.getByRole('dialog', { name: /^历史回放 ·/ })).toBeVisible();
  const playbackPlayer = page.locator('.monitor-player-dialog .gmv-player');
  const playbackProgress = playbackPlayer.getByLabel('回放进度');
  await expect(playbackProgress).toHaveAttribute('max', String(30 * 60 * 1_000));
  await playbackPlayer.hover({ position: { x: 20, y: 20 } });
  await expect(playbackProgress).toBeVisible();
  await playbackPlayer.locator('.gmv-video').first().click({ position: { x: 20, y: 20 } });
  await expect(playbackProgress).toBeHidden();
  await playbackPlayer.locator('.gmv-video').first().click({ position: { x: 20, y: 20 } });
  await expect(playbackProgress).toBeVisible();
  await page.getByLabel('回放操作模式').selectOption('clip');
  cloudRecordingCreateRange = undefined;
  const createClip = page.getByRole('button', { name: '创建截取录像' });
  await expect(createClip).toBeEnabled();
  await createClip.click();

  await expect.poll(() => cloudRecordingCreateRange).toBeTruthy();
  await expect(page.getByRole('heading', { name: '设备录像下载', exact: true })).toBeVisible();
  const lockedDown = page.getByRole('button', { name: '截取录像创建中' });
  await expect(lockedDown).toBeDisabled();
  await expect(lockedDown.locator('.clip-down-spinner')).toBeVisible();

  await page.locator('.cloud-recording-drawer .el-drawer__close-btn').click();
  await page.locator('.monitor-player-dialog .el-dialog__headerbtn').click();
  await page.getByLabel('加入多画面回放').click();

  const defaultRangeInputs = page.locator('.multi-default-range input');
  await defaultRangeInputs.nth(0).fill('2026-07-22 10:00:00');
  await defaultRangeInputs.nth(0).press('Enter');
  await defaultRangeInputs.nth(1).fill('2026-07-22 10:30:00');
  await defaultRangeInputs.nth(1).press('Enter');
  await page.locator('.device-channel-tree .tree-device-node').first().click();
  await page.locator('.tree-channel-node').first().click();
  await page.getByRole('button', { name: '确认播放', exact: true }).click();

  const multiProgress = page.locator('.grid-cell').first().getByLabel('回放进度');
  await expect(multiProgress).toBeVisible();
  const multiPlayer = page.locator('.grid-cell .gmv-player').first();
  await multiPlayer.locator('.gmv-video').first().click({ position: { x: 20, y: 20 } });
  await expect(multiProgress).toBeHidden();
  await multiPlayer.locator('.gmv-video').first().click({ position: { x: 20, y: 20 } });
  await expect(multiProgress).toBeVisible();
  const emitMultiProgress = async (mediaTimeMs: number) => {
    await page.locator('.grid-cell .gmv-player').first().evaluate((element, value) => {
      const instance = (element as HTMLElement & {
        __vueParentComponent?: { emit: (event: string, payload: unknown) => void };
      }).__vueParentComponent;
      if (!instance) throw new Error('GmvPlayerView instance is unavailable');
      instance.emit('playbackProgress', { mediaTimeMs: value });
    }, mediaTimeMs);
  };

  await emitMultiProgress(0);
  await emitMultiProgress(120_000);
  await expect(multiProgress).toHaveValue('120000');
  await multiProgress.evaluate((element) => {
    const input = element as HTMLInputElement;
    input.value = '600000';
    input.dispatchEvent(new Event('change', { bubbles: true }));
  });
  await expect.poll(() => multiSeekRequests).toBe(1);
  await emitMultiProgress(180_000);
  await expect(multiProgress).toHaveValue('600000');

  const seekResponse = page.waitForResponse((response) => (
    new URL(response.url()).pathname.endsWith('/playbacks/playback-1/seek')
  ));
  releaseMultiSeek?.();
  await seekResponse;
  await emitMultiProgress(240_000);
  await expect(multiProgress).toHaveValue('600000');
  await emitMultiProgress(0);
  await expect(multiProgress).toHaveValue('600000');
  await emitMultiProgress(5_000);
  await expect(multiProgress).toHaveValue('605000');
});
