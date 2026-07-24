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

function node(nodeId: string, health = 'READY', connection = 'CONNECTED', scheduling = 'ENABLED') {
  return {
    node_id: nodeId,
    instance_id: `${nodeId}-instance`,
    kind: 'stream',
    service: 'stream',
    protocol: null,
    display_name: `stream:${nodeId}`,
    connection,
    health,
    scheduling,
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

test('系统健康展示全部节点的执行与排队负载，并突出离线节点', async ({ page }) => {
  await mockSystemHealth(page);
  await page.goto('/system/health');

  await expect(page.getByRole('heading', { name: '系统健康', level: 1 })).toBeVisible();
  await expect(page.getByRole('heading', { name: '任务拥堵', level: 2 })).toBeVisible();
  const nodeMatrixHeading = page.getByRole('heading', { name: '节点矩阵', level: 2 });
  await expect(nodeMatrixHeading).toBeVisible();
  await expect(nodeMatrixHeading.locator('xpath=ancestor::section')).toHaveClass(/span-12/);
  await expect(page.locator('.el-drawer')).not.toBeVisible();
  await expect(page.getByRole('menuitem', { name: '节点监控' })).toHaveCount(0);

  const rows = page.locator('.node-load-row');
  await expect(rows).toHaveCount(6);
  await expect(rows.first()).toHaveAttribute('data-node-id', 'offline-node');
  expect(await rows.evaluateAll((items) => items.map((item) => item.getAttribute('data-node-id')))).toEqual([
    'offline-node',
    'draining-node',
    'ready-busy',
    'ready-small',
    'ready-zero-a',
    'ready-zero-b',
  ]);

  const offline = page.locator('[data-node-id="offline-node"]');
  await expect(offline).toHaveAttribute('data-alert-level', 'offline');
  await expect(offline).toContainText('OFFLINE');
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

  await page.locator('.node-matrix-table .el-table__body tr').filter({ hasText: 'offline-node' }).click();
  const nodeDetail = page.locator('.el-drawer');
  await expect(nodeDetail).toBeVisible();
  await expect(nodeDetail).toContainText('实例围栏 · stream:offline-node');
  await expect(nodeDetail).toContainText('DISCONNECTED');
  await expect(nodeDetail).toContainText('receiving_streams=2');
  await nodeDetail.getByRole('button', { name: '关闭' }).click();
  await expect(nodeDetail).not.toBeVisible();

  await page.setViewportSize({ width: 390, height: 844 });
  const layout = await page.evaluate(() => ({
    innerWidth: window.innerWidth,
    scrollWidth: document.documentElement.scrollWidth,
  }));
  expect(layout.scrollWidth).toBe(layout.innerWidth);
});
