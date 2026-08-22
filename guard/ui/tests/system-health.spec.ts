import { expect, test, type Page } from '@playwright/test';

const session = {
  username: 'admin',
  nickname: '系统管理员',
  role: 'admin',
  csrf_token: 'csrf-system-health',
  expires_at_ms: Date.now() + 60_000,
};

const hostMetrics = {
  cpu_usage_percent: 36,
  load_average_1m: 1.25,
  load_average_5m: 1.1,
  load_average_15m: 0.95,
  memory_total_bytes: 8 * 1024 ** 3,
  memory_used_bytes: 3 * 1024 ** 3,
  swap_total_bytes: 0,
  swap_used_bytes: 0,
  disk_read_bytes_per_sec: 1024,
  disk_write_bytes_per_sec: 2048,
  network_receive_bytes_per_sec: 4096,
  network_transmit_bytes_per_sec: 8192,
  process_resident_memory_bytes: 256 * 1024 ** 2,
  process_threads: 12,
};

function node(nodeId: string, health = 'READY', connection = 'CONNECTED', scheduling = 'ENABLED', kind = 'stream') {
  const healthLabels: Record<string, string> = { READY: '就绪', DRAINING: '排空中', OFFLINE: '离线' };
  return {
    node_id: nodeId,
    instance_id: `${nodeId}-instance`,
    kind,
    service: kind,
    protocol: null,
    display_name: `${kind}:${nodeId}`,
    connection,
    connection_label: connection === 'CONNECTED' ? '已连接' : '已断开',
    health,
    health_label: healthLabels[health] ?? health,
    scheduling,
    scheduling_label: scheduling === 'ENABLED' ? '可调度' : '不可调度',
    capabilities: ['stream.receive'],
    pending_leases: 0,
    host_metrics: hostMetrics,
    business_metrics: { receiving_streams: '2' },
    config: {},
    zone: null,
    last_seen_at_ms: Date.now(),
    generation: 1,
    sequence: 10,
  };
}

function lease(nodeId: string, index: number, state: string) {
  return {
    lease_id: `${nodeId}-lease-${index}`,
    route_id: `${nodeId}-route-${index}`,
    resource_id: `${nodeId}-resource-${index}`,
    node_id: nodeId,
    instance_id: `${nodeId}-instance`,
    state,
    expires_at_ms: Date.now() + 30_000,
  };
}

async function mockSystemHealth(page: Page) {
  const nodes = [
    node('ready-zero-b'),
    node('ready-busy'),
    node('offline-node', 'OFFLINE', 'DISCONNECTED', 'DISABLED'),
    node('ready-small'),
    node('draining-node', 'DRAINING', 'CONNECTED', 'DISABLED'),
    node('ready-zero-a'),
    node('stream-20'),
    node('stream-2'),
    node('session-10', 'READY', 'CONNECTED', 'ENABLED', 'session'),
    node('stream-10'),
    node('session-2', 'READY', 'CONNECTED', 'ENABLED', 'session'),
    node('stream-11'),
  ];
  const leases = [
    ...Array.from({ length: 3 }, (_, index) => lease('ready-busy', index, 'confirmed')),
    ...Array.from({ length: 2 }, (_, index) => lease('ready-busy', index + 3, 'allocated')),
    lease('ready-small', 0, 'confirmed'),
    lease('ready-small', 1, 'allocated'),
    ...Array.from({ length: 4 }, (_, index) => lease('draining-node', index, 'confirmed')),
    lease('offline-node', 0, 'confirmed'),
    lease('offline-node', 1, 'allocated'),
    lease('ready-zero-a', 0, 'released'),
    lease('ready-zero-a', 1, 'failed'),
    lease('ready-zero-a', 2, 'expired'),
  ];

  await page.route('**/api/v2/auth/session', async (route) => {
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(session) });
  });
  await page.route('**/api/v2/nodes', async (route) => {
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(nodes) });
  });
  await page.route('**/api/v2/leases', async (route) => {
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(leases) });
  });
}

test('系统健康按页签查询、排序并分页展示节点', async ({ page }) => {
  await mockSystemHealth(page);
  await page.goto('/system/health');

  await expect(page.getByRole('heading', { name: '系统健康', level: 1 })).toBeVisible();
  await expect(page.getByRole('tab', { name: '任务拥堵' })).toHaveAttribute('aria-selected', 'true');
  await expect(page.getByRole('tab', { name: '节点矩阵' })).toHaveAttribute('aria-selected', 'false');
  await expect(page.locator('.el-drawer')).not.toBeVisible();
  await expect(page.getByRole('menuitem', { name: '节点监控' })).toHaveCount(0);

  const taskPanel = page.getByRole('tabpanel', { name: '任务拥堵' });
  const taskFilters = taskPanel.locator('.task-filters');
  await expect(taskFilters.locator('.el-form-item')).toHaveCount(4);
  const filterLayout = await taskFilters.evaluate((element) => {
    const style = getComputedStyle(element);
    const items = Array.from(element.children) as HTMLElement[];
    return {
      display: style.display,
      paddingLeft: style.paddingLeft,
      paddingRight: style.paddingRight,
      widths: items.map((item) => Math.round(item.getBoundingClientRect().width)),
      tops: items.map((item) => Math.round(item.getBoundingClientRect().top)),
    };
  });
  expect(filterLayout.display).toBe('grid');
  expect(filterLayout.paddingLeft).toBe('16px');
  expect(filterLayout.paddingRight).toBe('16px');
  expect(new Set(filterLayout.widths.slice(0, 3)).size).toBe(1);
  expect(filterLayout.widths[3]).toBe(180);
  expect(new Set(filterLayout.tops).size).toBe(1);
  const taskBodyHeight = await taskPanel.locator('.health-tab-body').evaluate((element) =>
    Math.round(element.getBoundingClientRect().height));
  expect(taskBodyHeight).toBeGreaterThan(0);
  const taskCardHeight = await taskPanel.locator('xpath=ancestor::section').evaluate((element) =>
    Math.round(element.getBoundingClientRect().height));
  const rows = taskPanel.locator('.node-load-row');
  await expect(rows).toHaveCount(10);
  expect(await rows.evaluateAll((items) => items.map((item) => item.getAttribute('data-node-id')))).toEqual([
    'draining-node',
    'offline-node',
    'session-2',
    'session-10',
    'ready-busy',
    'ready-small',
    'ready-zero-a',
    'ready-zero-b',
    'stream-2',
    'stream-10',
  ]);

  const offline = page.locator('[data-node-id="offline-node"]');
  await expect(offline).toHaveAttribute('data-alert-level', 'offline');
  await expect(offline).toContainText('离线');
  await expect(offline).toContainText('不可调度');
  await expect(offline).toContainText('执行 1');
  await expect(offline).toContainText('排队 1');
  await expect(offline).toContainText('合计 2');

  const busy = page.locator('[data-node-id="ready-busy"]');
  await expect(busy).toContainText('执行 3');
  await expect(busy).toContainText('排队 2');
  await expect(busy).toContainText('合计 5');

  const terminalOnly = page.locator('[data-node-id="ready-zero-a"]');
  await expect(terminalOnly).toContainText('执行 0');
  await expect(terminalOnly).toContainText('排队 0');
  await expect(terminalOnly).toContainText('合计 0');

  const tones = await Promise.all([
    offline.evaluate((element) => ({ background: getComputedStyle(element).backgroundImage, border: getComputedStyle(element).borderColor })),
    busy.evaluate((element) => ({ background: getComputedStyle(element).backgroundImage, border: getComputedStyle(element).borderColor })),
  ]);
  expect(tones[0].background).not.toBe(tones[1].background);
  expect(tones[0].border).not.toBe(tones[1].border);

  await taskPanel.locator('.el-pagination .btn-next').click();
  await expect(rows).toHaveCount(2);
  expect(await rows.evaluateAll((items) => items.map((item) => item.getAttribute('data-node-id')))).toEqual([
    'stream-11',
    'stream-20',
  ]);

  const taskNodeId = taskPanel.getByRole('combobox', { name: '节点 ID' });
  await taskNodeId.click();
  await taskNodeId.fill('10');
  const nodeOptions = page.getByRole('option');
  await expect(nodeOptions).toHaveCount(2);
  await expect(nodeOptions).toHaveText(['session-10', 'stream-10']);
  await page.getByRole('option', { name: 'stream-10', exact: true }).click();
  await taskPanel.getByRole('button', { name: '查询' }).click();
  await expect(rows).toHaveCount(1);
  await expect(rows.first()).toHaveAttribute('data-node-id', 'stream-10');

  await page.getByRole('tab', { name: '节点矩阵' }).click();
  const matrixPanel = page.getByRole('tabpanel', { name: '节点矩阵' });
  const matrixRows = matrixPanel.locator('.node-matrix-table .el-table__body tr');
  const matrixBodyHeight = await matrixPanel.locator('.health-tab-body').evaluate((element) =>
    Math.round(element.getBoundingClientRect().height));
  expect(matrixBodyHeight).toBe(taskBodyHeight);
  const matrixCardHeight = await matrixPanel.locator('xpath=ancestor::section').evaluate((element) =>
    Math.round(element.getBoundingClientRect().height));
  expect(matrixCardHeight).toBe(taskCardHeight);
  await expect(matrixPanel.locator('.matrix-filters .el-form-item')).toHaveCount(4);
  await expect(matrixPanel.locator('.node-matrix-table thead th .cell')).toHaveText([
    '节点 ID',
    '类型',
    '协议',
    '健康',
    'CPU',
    '内存',
    '磁盘 IO',
    '网络 IO',
  ]);
  await expect(matrixRows).toHaveCount(10);
  await expect(matrixRows.first()).toContainText('draining-node');
  await expect(matrixRows.first()).toContainText('排空中');
  expect(await matrixRows.first().locator('td .cell').evaluateAll((cells) =>
    [...new Set(cells.map((cell) => getComputedStyle(cell).whiteSpace))])).toEqual(['nowrap']);

  await matrixPanel.locator('.matrix-filters .el-form-item').nth(1).locator('.el-select').click();
  await page.getByRole('option', { name: '离线', exact: true }).click();
  await matrixPanel.getByRole('button', { name: '查询' }).click();
  await expect(matrixRows).toHaveCount(1);
  await matrixRows.first().click();
  const nodeDetail = page.locator('.el-drawer');
  await expect(nodeDetail).toBeVisible();
  await expect(nodeDetail).toContainText('实例围栏 · stream:offline-node');
  await expect(nodeDetail.getByText('节点名称', { exact: true })).toHaveCount(0);
  await expect(nodeDetail.locator('.chart')).toHaveCount(0);
  await expect(nodeDetail).toContainText('已断开');
  await expect(nodeDetail).toContainText('不可调度');
  await expect(nodeDetail).toContainText('代次');
  await expect(nodeDetail).toContainText('Load');
  await expect(nodeDetail).toContainText('receiving_streams=2');
  const detailLayout = await nodeDetail.locator('.node-detail-grid').evaluate((element) =>
    Array.from(element.children).map((child) => ({
      label: child.querySelector('.node-detail-label')?.textContent?.trim(),
      top: Math.round(child.getBoundingClientRect().top),
      stacked: child.classList.contains('is-stacked'),
    })));
  const detailRows = Object.fromEntries(detailLayout.map((item) => [item.label, item.top]));
  expect(detailRows['健康']).toBe(detailRows['连接']);
  expect(detailRows['调度']).toBe(detailRows['CPU']);
  expect(detailRows['代次']).toBe(detailRows['线程']);
  expect(detailLayout.filter((item) => item.stacked).map((item) => item.label)).toEqual([
    'Load（1m / 5m / 15m）',
    '能力',
    '业务指标',
  ]);
  await nodeDetail.getByRole('button', { name: '关闭' }).click();
  await expect(nodeDetail).not.toBeVisible();

  await page.setViewportSize({ width: 390, height: 844 });
  const layout = await page.evaluate(() => ({
    innerWidth: window.innerWidth,
    scrollWidth: document.documentElement.scrollWidth,
  }));
  expect(layout.scrollWidth).toBe(layout.innerWidth);
});
