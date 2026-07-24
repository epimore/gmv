import { expect, test } from '@playwright/test';

test('流监控识别 GB28181 Session 并使用中文状态', async ({ page }) => {
  const session = {
    username: 'admin', nickname: '管理员', role: 'admin', csrf_token: 'csrf-stream-test',
    expires_at_ms: Date.now() + 60_000,
  };
  const node = {
    node_id: '34020000002000000001', instance_id: 'session-instance-1',
    kind: 'session-gb28181', service: 'session-gb28181', protocol: 'gb28181',
    display_name: '国标 Session', connection: 'CONNECTED', health: 'READY', scheduling: 'ENABLED',
    capabilities: ['protocol.gb28181'], pending_leases: 0, host_metrics: {}, business_metrics: {},
    config: { service: 'session-gb28181', protocol: 'gb28181' }, zone: null,
    last_seen_at_ms: Date.now(), generation: 1, sequence: 1,
  };
  const streamNode = {
    ...node,
    node_id: 'stream-node-1', instance_id: 'stream-instance-1', kind: 'stream', service: 'stream',
    protocol: null, display_name: '流媒体节点一', capabilities: [], config: {},
  };
  const offlineSessionNode = {
    ...node,
    node_id: '34020000002000000002', instance_id: 'old-session-instance', connection: 'DISCONNECTED',
    display_name: '离线国标 Session',
  };
  const monitorServerTimeMs = Date.now();
  const active = {
    stream_id: 'stream-1', session_node_id: node.node_id, session_instance_id: node.instance_id,
    stream_node_id: 'stream-node-1', device_id: 'device-1', channel_id: 'channel-1', ssrc: '0100000001',
    state: 'running', dialog_state: 'ESTABLISHED', media_state: 'receiving', media_ready: true,
    created_at_ms: monitorServerTimeMs - 10_000, established_at_ms: monitorServerTimeMs - 9_000,
    started_at_ms: monitorServerTimeMs - 9_000, diagnostic_reason: '', session_type: 'LIVE',
    viewer_count: 1, viewer_formats: [{ media_format: 'hls', viewer_count: 1 }],
    supported_formats: ['flv', 'fmp4', 'hls', 'll_hls'],
    output_format: 'hls',
  };
  const download = {
    ...active,
    stream_id: 'stream-download-1', ssrc: '0100000003', session_type: 'DOWNLOAD',
    created_at_ms: monitorServerTimeMs - 6_000, established_at_ms: monitorServerTimeMs - 5_000,
    started_at_ms: monitorServerTimeMs - 5_000, viewer_count: 0, viewer_formats: [],
    supported_formats: ['flv', 'fmp4', 'hls', 'mp4'], output_format: 'mp4',
  };
  const history = {
    stream_id: 'stream-history-1', session_node_id: node.node_id, stream_node_id: 'stream-node-1',
    device_id: 'device-1', channel_id: 'channel-1', ssrc: '0100000002', session_type: 'PLAYBACK',
    state: 'TERMINATED', created_at_ms: monitorServerTimeMs - 20_000, established_at_ms: monitorServerTimeMs - 19_000,
    terminated_at_ms: monitorServerTimeMs - 1_000, duration_ms: 18_000, terminal_reason: 'manual_stop',
    terminal_reason_label: '手动停止', error_code: '', legacy_terminal_time: false,
  };
  let stopRequested = false;
  let activeListRequests = 0;

  await page.route('**/api/v2/**', async (route) => {
    const path = new URL(route.request().url()).pathname;
    let body: unknown = [];
    if (path === '/api/v2/auth/session') body = session;
    else if (path === '/api/v2/nodes') body = [offlineSessionNode, node, streamNode];
    else if (path === '/api/v2/gb28181/streams') {
      activeListRequests += 1;
      const items = !stopRequested
        ? [active, download]
        : [{ ...active, state: 'stopping', dialog_state: 'TERMINATING', media_ready: false }, download];
      body = { items, next_after_id: '', server_time_ms: monitorServerTimeMs };
    } else if (path === '/api/v2/gb28181/streams/stream-1/stop') {
      stopRequested = true;
      body = { stream_id: active.stream_id, state: 'stopping', session_node_id: node.node_id, session_instance_id: node.instance_id };
    } else if (path === '/api/v2/gb28181/stream-history') body = { items: [history], total: 1, page: 1, page_size: 20, server_time_ms: monitorServerTimeMs };
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(body) });
  });

  await page.goto('/streams');
  await expect(page.getByText('当前运行资源监控', { exact: true })).toHaveCount(0);
  await expect(page.getByText('业务事实来自所选 Session，Guard 仅鉴权与转发', { exact: true })).toHaveCount(0);
  await page.getByRole('combobox').first().click();
  const onlineSession = page.getByRole('option', { name: `SESSION-GB28181 · ${node.node_id} 在线` });
  const offlineSession = page.getByRole('option', { name: `SESSION-GB28181 · ${offlineSessionNode.node_id} 离线` });
  await expect(onlineSession).toBeEnabled();
  await expect(offlineSession).toBeDisabled();
  await expect(page.getByText(node.instance_id, { exact: true })).toHaveCount(0);
  await onlineSession.click();

  const currentPanel = page.getByRole('tabpanel', { name: '当前运行' });
  await expect(currentPanel.getByText('运行中', { exact: true }).first()).toBeVisible();
  await expect(currentPanel.getByText('直播', { exact: true })).toBeVisible();
  await expect(currentPanel.getByText('下载', { exact: true })).toBeVisible();
  await expect(currentPanel.getByText('观看人数', { exact: true })).toHaveCount(0);
  await expect(currentPanel.getByText('媒体格式', { exact: true })).toHaveCount(0);
  await expect(currentPanel.locator('.el-pagination')).toBeVisible();
  const duration = currentPanel.getByText('9秒', { exact: true });
  await expect(duration).toBeVisible();
  await page.waitForTimeout(1_200);
  await expect(duration).toBeVisible();
  const liveRow = currentPanel.getByRole('row').filter({ hasText: active.stream_id });
  await liveRow.getByRole('button', { name: '详情', exact: true }).click();
  const details = page.getByRole('dialog', { name: '流详情' });
  await expect(details.getByText('总观看人数', { exact: true })).toBeVisible();
  await expect(details.getByRole('heading', { name: '媒体格式', exact: true })).toBeVisible();
  await expect(details.getByRole('columnheader', { name: '观看人数', exact: true })).toBeVisible();
  const hlsRow = details.getByRole('row').filter({ hasText: 'HLS' });
  await expect(hlsRow.getByText('1', { exact: true })).toBeVisible();
  const flvRow = details.getByRole('row').filter({ hasText: 'HTTP-FLV' });
  await expect(flvRow.getByText('0', { exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  const downloadRow = currentPanel.getByRole('row').filter({ hasText: download.stream_id });
  await downloadRow.getByRole('button', { name: '详情', exact: true }).click();
  await expect(details.getByText('下载格式', { exact: true })).toBeVisible();
  await expect(details.getByText('MP4', { exact: true })).toBeVisible();
  await expect(details.getByText('总观看人数', { exact: true })).toHaveCount(0);
  await expect(details.getByRole('heading', { name: '媒体格式', exact: true })).toHaveCount(0);
  await expect(details.getByRole('columnheader', { name: '观看人数', exact: true })).toHaveCount(0);
  await page.keyboard.press('Escape');
  await expect(page.getByText('Session 服务', { exact: true })).toHaveCount(0);
  await expect(page.getByText('请先选择 Session 节点；页面不会从 Guard route/lease 推断当前业务流。')).toHaveCount(0);
  await expect(page.getByRole('button', { name: '刷新', exact: true })).toHaveCount(0);
  await expect(page.locator('.stream-filter-row')).toHaveCount(2);
  const filterLayout = await page.locator('.stream-toolbar').evaluate((toolbar) => {
    const style = window.getComputedStyle(toolbar);
    const widths = Array.from(toolbar.querySelector('.stream-filter-row')?.children || []).map((item) => item.getBoundingClientRect().width);
    return { paddingLeft: style.paddingLeft, paddingRight: style.paddingRight, widths };
  });
  expect(filterLayout.paddingLeft).toBe('16px');
  expect(filterLayout.paddingRight).toBe('16px');
  expect(Math.max(...filterLayout.widths) - Math.min(...filterLayout.widths)).toBeLessThan(1);

  await page.locator('.stream-filter-row').first().locator('.el-select').nth(1).click();
  await expect(page.getByRole('option', { name: '流媒体节点一', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');

  await page.locator('.stream-filter-row').nth(1).locator('.el-select').click();
  await expect(page.getByRole('option', { name: '启动中', exact: true })).toBeVisible();
  await expect(page.getByRole('option', { name: '运行中', exact: true })).toBeVisible();
  await expect(page.getByRole('option', { name: '停止中', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');

  await liveRow.getByRole('button', { name: '停止', exact: true }).click();
  await page.getByRole('button', { name: '确认停止', exact: true }).click();
  await expect(currentPanel.getByText('停止中', { exact: true })).toBeVisible();
  const requestsAfterStop = activeListRequests;
  await page.waitForTimeout(2_200);
  expect(activeListRequests).toBe(requestsAfterStop);

  await page.getByRole('tab', { name: '历史记录' }).click();
  const historyPanel = page.getByRole('tabpanel', { name: '历史记录' });
  await expect(historyPanel.getByText('回放', { exact: true })).toBeVisible();
  await expect(historyPanel.getByText('手动停止', { exact: true })).toBeVisible();
  await expect(historyPanel.getByText('manual_stop', { exact: true })).toHaveCount(0);
});
