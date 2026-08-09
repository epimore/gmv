import { expect, test, type Page } from '@playwright/test';

const session = {
  username: 'admin',
  nickname: '舰桥管理员',
  role: 'admin',
  csrf_token: 'csrf-test-token',
  expires_at_ms: Date.now() + 60_000,
};

const readySessionNode = {
  node_id: 'session-1', instance_id: 'instance-1', kind: 'SESSION', service: 'session-gb28181',
  protocol: 'gb28181', display_name: 'GB28181 Session', connection: 'CONNECTED', health: 'READY',
  scheduling: 'ENABLED', capabilities: ['protocol.gb28181'], pending_leases: 0,
  host_metrics: {}, business_metrics: {}, config: {}, zone: null,
  last_seen_at_ms: Date.now(), generation: 1, sequence: 1,
};

const defaultRoutes = [
  ['/dashboard', 'Dashboard'],
  ['/gb28181/register', '注册管理'],
  ['/gb28181/monitor', '监控信息'],
  ['/streams', '流媒监控'],
  ['/system', '系统健康'],
] as const;

async function mockAuth(page: Page, initiallyAuthenticated = false, authSession = session) {
  let authenticated = initiallyAuthenticated;
  const readBodies = new Map<string, unknown>([
    ['/api/v2/events', { items: [], next_after_id: null }],
    ['/api/v2/streams', []],
    ['/api/v2/ai/tasks', []],
    ['/api/v2/leases', []],
    ['/api/v2/integrations/outbox', []],
    ['/api/v2/integrations/master-key', { configured: true, key_version: 1, created_at_ms: 1, updated_by: 'system:init', updated_at_ms: 1 }],
    ['/api/v2/integrations/mqtt/runtime', { configured: false, broker_connected: false, config: null, connection_scope: 'deployment', qos: 1, retain: false }],
    ['/api/v2/runtime/status', { guard_available: true, streams: 0, running_streams: 0, ai_tasks: 0, running_ai_tasks: 0, ptz_commands: 0 }],
    ['/api/v2/media/transport', { scheme: 'http', http_version: 'http/1.1', multi_view_limit: 6 }],
  ]);

  await page.route('**/api/v2/**', async (route) => {
    const path = new URL(route.request().url()).pathname;
    const body = readBodies.get(path);
    if (body === undefined) {
      await route.fallback();
      return;
    }
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(body) });
  });
  await page.route('**/api/v2/auth/session', async (route) => {
    await route.fulfill({
      status: authenticated ? 200 : 401,
      contentType: 'application/json',
      body: JSON.stringify(authenticated ? authSession : { message: 'invalid UI session' }),
    });
  });
  await page.route('**/api/v2/auth/login', async (route) => {
    const body = route.request().postDataJSON() as { username: string; password: string };
    if (body.username !== 'admin' || body.password !== 'secret') {
      await route.fulfill({ status: 401, contentType: 'application/json', body: JSON.stringify({ message: '用户名或密码错误' }) });
      return;
    }
    authenticated = true;
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(authSession) });
  });
  await page.route('**/api/v2/auth/logout', async (route) => {
    expect(route.request().headers()['x-csrf-token']).toBe(authSession.csrf_token);
    authenticated = false;
    await route.fulfill({ status: 204, body: '' });
  });
  await page.route('**/api/v2/users', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify([{ ...authSession, enabled: true, created_at_ms: 0, updated_at_ms: 0 }]),
    });
  });
  await page.route('**/api/v2/me', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ ...authSession, enabled: true, created_at_ms: 0, updated_at_ms: 0 }),
    });
  });
  await page.route('**/api/v2/nodes', async (route) => {
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify([]) });
  });
  await page.route('**/api/v2/gb28181/devices**', async (route) => {
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ items: [], total: 0, page: 1, page_size: 20 }) });
  });
  await page.route('**/api-docs/openapi.json', async (route) => {
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ paths: {}, webhooks: {} }) });
  });
  await page.route('**/api-docs/asyncapi.json', async (route) => {
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ channels: {} }) });
  });
}

test('未登录禁止 URL 直达，登录后恢复目标页面并可退出', async ({ page }) => {
  await mockAuth(page);

  await page.goto('/nodes');
  await expect(page).toHaveURL((url) => url.pathname === '/login' && url.searchParams.get('redirect') === '/system/health');
  await expect(page.getByRole('heading', { name: '登录' })).toBeVisible();

  await page.getByLabel('用户名').fill('admin');
  await page.getByLabel('密码').fill('secret');
  await page.getByRole('button', { name: '安全登录' }).click();
  await expect(page).toHaveURL((url) => url.pathname === '/system/health');
  const userMenu = page.getByRole('button', { name: /舰桥管理员 admin/ });
  await expect(userMenu).toBeVisible();

  await page.reload();
  await expect(page.getByRole('heading', { name: '系统健康', level: 1 })).toBeVisible();
  await expect(page.getByRole('menuitem', { name: '节点监控' })).toHaveCount(0);

  await userMenu.click();
  await page.getByRole('menuitem', { name: '退出登录' }).click();
  await expect(page).toHaveURL((url) => url.pathname === '/login');

  await page.goto('/system');
  await expect(page).toHaveURL((url) => url.pathname === '/login' && url.searchParams.get('redirect') === '/system/health');
});

test('密码框按 Enter 可提交登录', async ({ page }) => {
  await mockAuth(page);

  await page.goto('/login');
  await page.getByLabel('用户名').fill('admin');
  await page.getByLabel('密码').fill('secret');

  const loginRequest = page.waitForRequest('**/api/v2/auth/login');
  await page.getByLabel('密码').press('Enter');

  expect((await loginRequest).postDataJSON()).toEqual({ username: 'admin', password: 'secret' });
  await expect(page).toHaveURL((url) => url.pathname === '/dashboard');
});

test('登录页不主动恢复会话', async ({ page }) => {
  let sessionRequests = 0;
  await page.route('**/api/v2/auth/session', async (route) => {
    sessionRequests += 1;
    await route.fulfill({ status: 401, contentType: 'application/json', body: JSON.stringify({ message: 'invalid UI session' }) });
  });

  await page.goto('/login');
  await expect(page.getByRole('heading', { name: '登录' })).toBeVisible();
  expect(sessionRequests).toBe(0);
});

test('已登录会话可访问中文页面与移动端布局', async ({ page }) => {
  await mockAuth(page, true);
  await page.setViewportSize({ width: 390, height: 844 });

  for (const [path, heading] of defaultRoutes) {
    await page.goto(path);
    await expect(page.getByRole('heading', { name: heading, level: 1 })).toBeVisible();
  }

  await page.goto('/dashboard');
  await expect(page.getByRole('heading', { name: 'Dashboard', level: 1 })).toBeVisible();
  const layout = await page.evaluate(() => ({
    innerWidth: window.innerWidth,
    scrollWidth: document.documentElement.scrollWidth,
    mainLeft: document.querySelector('.main')?.getBoundingClientRect().left,
    mainWidth: document.querySelector('.main')?.getBoundingClientRect().width,
  }));

  expect(layout.scrollWidth).toBe(layout.innerWidth);
  expect(layout.mainLeft).toBe(0);
  expect(layout.mainWidth).toBe(layout.innerWidth);
});

test('核心管理页面卡片铺满可视窗口且页面不产生纵向滚动', async ({ page }) => {
  await mockAuth(page, true);
  await page.setViewportSize({ width: 1440, height: 900 });

  const routes = [
    ['/gb28181/register', '注册管理'],
    ['/gb28181/monitor', '监控信息'],
    ['/streams', '流媒监控'],
    ['/system/health', '系统健康'],
    ['/system/users', '用户管理'],
  ] as const;

  for (const [path, heading] of routes) {
    await page.goto(path);
    await expect(page.getByRole('heading', { name: heading, level: 1 })).toBeVisible();

    const layout = await page.locator('.viewport-card-page').evaluate((grid) => {
      const gridRect = grid.getBoundingClientRect();
      const fillPanel = grid.querySelector<HTMLElement>(':scope > .fill-panel');
      const panelRect = fillPanel?.getBoundingClientRect();
      return {
        viewportHeight: window.innerHeight,
        documentHeight: document.documentElement.scrollHeight,
        gridBottom: Math.round(gridRect.bottom),
        panelBottom: Math.round(panelRect?.bottom || 0),
      };
    });

    expect(layout.documentHeight).toBe(layout.viewportHeight);
    expect(layout.gridBottom).toBe(layout.viewportHeight - 32);
    expect(layout.panelBottom).toBe(layout.gridBottom);
  }
});

test('Dashboard 展示边端状态、能力入口和待处理事项', async ({ page }) => {
  await mockAuth(page, true);

  await page.goto('/dashboard');

  await expect(page.getByRole('heading', { name: '等待节点接入', level: 2 })).toBeVisible();
  await expect(page.getByRole('heading', { name: '边端能力矩阵', level: 2 })).toBeVisible();
  await expect(page.getByRole('button', { name: /GB28181/ })).toBeVisible();
  await expect(page.getByRole('button', { name: /ONVIF/ })).toHaveCount(0);
  await expect(page.getByRole('button', { name: /MQTT 接入/ })).toHaveCount(0);
  await expect(page.getByText('尚未发现业务节点')).toBeVisible();
  await expect(page.getByRole('heading', { name: '业务异常概览', level: 2 })).toBeVisible();
  await expect(page.getByText('星图拓扑')).toHaveCount(0);
  await expect(page.getByText('资源分布')).toHaveCount(0);
});

test('Dashboard 将单次流失败归入业务异常而不是待处理事项', async ({ page }) => {
  await mockAuth(page, true);
  await page.route('**/api/v2/nodes', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify([readySessionNode]),
    });
  });
  await page.route('**/api/v2/streams', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify([{
        stream_id: 'failed-1', device_id: 'device-1', channel_id: 'channel-1', node_id: 'stream-1',
        lease_id: '', endpoint: '', state: 'failed',
      }]),
    });
  });

  await page.goto('/dashboard');

  await expect(page.getByText('1 条失败流记录')).toBeVisible();
  await expect(page.getByText('当前无待处理事项')).toBeVisible();
  await expect(page.getByText('1 路流失败')).toHaveCount(0);
});

test('接入应用页面选择并启用 MQTT 后直接进入详细配置', async ({ page }) => {
  await mockAuth(page, true);
  let integration: Record<string, unknown> | null = null;
  let savedPayload: Record<string, unknown> | null = null;
  await page.route('**/api/v2/integrations/business', async (route) => {
    if (route.request().method() === 'GET') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ state: integration ? 'ready' : 'unconfigured', integration }),
      });
      return;
    }
    savedPayload = route.request().postDataJSON() as Record<string, unknown>;
    integration = {
      integration_id: 'business-mqtt-1', name: '园区业务平台', transport: 'mqtt',
      inbound_enabled: true, outbound_enabled: true, enabled: true, scopes: [],
      expires_at_ms: null, config_version: 1, created_by: 'admin', created_at_ms: 1, updated_at_ms: 1,
    };
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(integration) });
  });
  await page.route('**/api/v2/integrations/master-key/rotate', async (route) => {
    expect(route.request().postDataJSON()).toMatchObject({ expected_key_version: 1 });
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ configured: true, key_version: 2, created_at_ms: 1, updated_by: 'admin', updated_at_ms: 2 }),
    });
  });
  await page.goto('/integrations/apps');
  await expect(page.getByRole('heading', { name: '接入应用', level: 1 })).toBeVisible();
  await expect(page.getByText('未集成', { exact: true }).first()).toBeVisible();
  await expect(page.getByText('HTTP', { exact: true })).toBeVisible();
  await expect(page.getByText('MQTT', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: '保存应用' })).toBeDisabled();
  const inboundSwitch = page.locator('.el-form-item').filter({ hasText: '接收入站命令' }).getByRole('switch');
  const outboundSwitch = page.locator('.el-form-item').filter({ hasText: '发送回调 / 事件' }).getByRole('switch');
  const enabledSwitch = page.locator('.el-form-item').filter({ hasText: '启用应用' }).getByRole('switch');
  const scopeSelect = page.locator('.el-form-item').filter({ hasText: '授权范围' }).getByRole('combobox');
  await expect(inboundSwitch).toBeDisabled();
  await expect(outboundSwitch).toBeDisabled();
  await expect(scopeSelect).toBeDisabled();
  await page.getByPlaceholder('例如：园区业务平台').fill('园区业务平台');
  await page.getByText('MQTT', { exact: true }).click();
  await expect(inboundSwitch).toBeEnabled();
  await expect(outboundSwitch).toBeEnabled();
  await expect(scopeSelect).toBeEnabled();
  await expect(enabledSwitch).toBeEnabled();
  await expect(page.getByText('应用保存后将进入 MQTT 接入页面维护 Broker Runtime。')).toBeVisible();
  await expect(page.getByRole('button', { name: '保存应用' })).toBeDisabled();
  await expect(page.getByRole('heading', { name: '集成主密钥', level: 2 })).toBeVisible();
  await expect(page.getByText('版本 1')).toBeVisible();
  await page.getByRole('button', { name: '轮换主密钥' }).click();
  await page.getByRole('button', { name: '确认轮换' }).click();
  await expect(page.getByText('版本 2')).toBeVisible();
  await page.locator('.el-form-item').filter({ hasText: '启用应用' }).locator('.el-switch').click();
  const saveButton = page.getByRole('button', { name: '保存并进入 MQTT 接入' });
  await expect(saveButton).toBeEnabled();
  await saveButton.click();

  await expect.poll(() => savedPayload).toMatchObject({
    name: '园区业务平台', transport: 'mqtt', enabled: true, expected_config_version: 0,
  });
  await expect(page).toHaveURL((url) => url.pathname === '/integrations/mqtt');
});

test('运行中的 MQTT 应用切换为 HTTP 后直接进入详细配置', async ({ page }) => {
  await mockAuth(page, true);
  let integration = {
    integration_id: 'business-app-1', name: '园区业务平台', transport: 'mqtt',
    inbound_enabled: true, outbound_enabled: true, enabled: true, scopes: ['devices:read'],
    expires_at_ms: null, config_version: 3, created_by: 'admin', created_at_ms: 1, updated_at_ms: 1,
  };
  const requests: Array<Record<string, unknown>> = [];

  await page.route('**/api/v2/integrations/business', async (route) => {
    if (route.request().method() === 'GET') {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ state: 'ready', integration }),
      });
      return;
    }

    const payload = route.request().postDataJSON() as Record<string, unknown>;
    requests.push(payload);
    if (requests.length === 1) {
      expect(payload).toMatchObject({ transport: 'mqtt', enabled: false, expected_config_version: 3 });
      integration = { ...integration, enabled: false, config_version: 4, updated_at_ms: 2 };
    } else {
      expect(payload).toMatchObject({ transport: 'http', enabled: true, expected_config_version: 4 });
      integration = { ...integration, transport: 'http', enabled: true, config_version: 5, updated_at_ms: 3 };
    }
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(integration) });
  });

  await page.goto('/integrations/apps');
  const transportMetric = page.locator('.metric-card').filter({ hasText: '配置方式' });
  const statusMetric = page.locator('.metric-card').filter({ hasText: '接入状态' });
  const enabledFormItem = page.locator('.el-form-item').filter({ hasText: '启用应用' });
  const enabledSwitch = enabledFormItem.getByRole('switch');
  await expect(transportMetric.locator('.metric-value')).toHaveText('MQTT');
  await expect(statusMetric.locator('.metric-value')).toHaveText('待配置');
  await expect(enabledSwitch).toBeChecked();
  await expect(page.getByRole('button', { name: '保存并进入 MQTT 接入' })).toBeDisabled();
  await page.getByText('HTTP', { exact: true }).click();
  await expect(transportMetric.locator('.metric-value')).toHaveText('MQTT');
  await expect(page.getByText('本次选择：HTTP（尚未保存）')).toBeVisible();
  await expect(enabledSwitch).not.toBeChecked();
  await expect(page.getByRole('button', { name: '保存应用' })).toBeDisabled();
  await enabledFormItem.locator('.el-switch').click();
  const saveButton = page.getByRole('button', { name: '保存并进入 HTTP 接入' });
  await expect(saveButton).toBeEnabled();
  await saveButton.click();
  await expect(page.getByText('当前 MQTT 应用仍在运行。切换会先停用 MQTT，再启用 HTTP。')).toBeVisible();
  await page.getByRole('button', { name: '确认切换' }).click();

  await expect.poll(() => requests.length).toBe(2);
  await expect(page).toHaveURL((url) => url.pathname === '/integrations/http');
});

test('已停用的 MQTT 应用可重新启用并直接进入详细配置', async ({ page }) => {
  await mockAuth(page, true);
  await page.route('**/api/v2/integrations/mqtt/runtime', async (route) => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({
      configured: true,
      broker_connected: false,
      config: {
        protocol_version: 'v5', broker: 'broker.example.test', port: 1883,
        client_id: 'guard-business', username: null, password_configured: false, tls: false,
        publish_event_ttl_sec: 86400, desired_revision: 1, active_revision: null,
        config_version: 1, apply_state: 'DEGRADED', last_error_code: 'connection_failed',
        last_error_summary: 'connection failed', last_transition_at_ms: 1,
        updated_by: 'admin', updated_at_ms: 1,
      },
      connection_scope: 'deployment', qos: 1, retain: false,
    }),
  }));
  let integration = {
    integration_id: 'business-mqtt-1', name: 'MQTT 业务平台', transport: 'mqtt',
    inbound_enabled: true, outbound_enabled: true, enabled: false, scopes: ['devices:read'],
    expires_at_ms: null, config_version: 10, created_by: 'admin', created_at_ms: 1, updated_at_ms: 1,
  };
  let savedPayload: Record<string, unknown> | null = null;

  await page.route('**/api/v2/integrations/business', async (route) => {
    if (route.request().method() === 'GET') {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ state: 'ready', integration }) });
      return;
    }
    savedPayload = route.request().postDataJSON() as Record<string, unknown>;
    integration = { ...integration, enabled: true, config_version: 11, updated_at_ms: 2 };
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(integration) });
  });

  await page.goto('/integrations/apps');
  const statusMetric = page.locator('.metric-card').filter({ hasText: '接入状态' });
  const transportMetric = page.locator('.metric-card').filter({ hasText: '配置方式' });
  const appSwitchMetric = page.locator('.metric-card').filter({ hasText: '应用开关' });
  const enabledFormItem = page.locator('.el-form-item').filter({ hasText: '启用应用' });
  const enabledSwitch = enabledFormItem.getByRole('switch');
  const initialSaveButton = page.getByRole('button', { name: '保存应用' });
  await expect(statusMetric.locator('.metric-value')).toHaveText('未集成');
  await expect(transportMetric.locator('.metric-value')).toHaveText('未集成');
  await expect(appSwitchMetric.locator('.metric-value')).toHaveText('未启用');
  await expect(enabledSwitch).toBeDisabled();

  await page.getByText('MQTT', { exact: true }).click();
  await expect(enabledSwitch).toBeEnabled();
  await expect(initialSaveButton).toBeDisabled();
  await enabledFormItem.locator('.el-switch').click();
  await expect(page.getByText('本次修改为启用，尚未保存。')).toBeVisible();
  const saveButton = page.getByRole('button', { name: '保存并进入 MQTT 接入' });
  await expect(saveButton).toBeEnabled();
  await saveButton.click();

  await expect.poll(() => savedPayload).toMatchObject({ transport: 'mqtt', enabled: true, expected_config_version: 10 });
  await expect(page).toHaveURL((url) => url.pathname === '/integrations/mqtt');
});

test('停用已启用应用时只保存停用动作并丢弃未启用草稿', async ({ page }) => {
  await mockAuth(page, true);
  const current = {
    integration_id: 'business-mqtt-stop', name: '当前 MQTT 应用', transport: 'mqtt',
    inbound_enabled: true, outbound_enabled: true, enabled: true, scopes: ['devices:read'],
    expires_at_ms: null, config_version: 7, created_by: 'admin', created_at_ms: 1, updated_at_ms: 1,
  };
  let savedPayload: Record<string, unknown> | null = null;

  await page.route('**/api/v2/integrations/business', async (route) => {
    if (route.request().method() === 'GET') {
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ state: 'ready', integration: current }) });
      return;
    }
    savedPayload = route.request().postDataJSON() as Record<string, unknown>;
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ ...current, enabled: false, config_version: 8, updated_at_ms: 2 }),
    });
  });

  await page.goto('/integrations/apps');
  await page.getByPlaceholder('例如：园区业务平台').fill('不应保存的草稿名称');
  await page.locator('.el-radio-group').getByText('未集成', { exact: true }).click();
  await expect(page.locator('.el-form-item').filter({ hasText: '接收入站命令' }).getByRole('switch')).toBeDisabled();
  await expect(page.locator('.el-form-item').filter({ hasText: '发送回调 / 事件' }).getByRole('switch')).toBeDisabled();
  await expect(page.locator('.el-form-item').filter({ hasText: '授权范围' }).getByRole('combobox')).toBeDisabled();
  const saveButton = page.getByRole('button', { name: '停用应用' });
  await expect(saveButton).toBeEnabled();
  await saveButton.click();

  await expect.poll(() => savedPayload).toMatchObject({
    name: '当前 MQTT 应用', transport: 'mqtt', enabled: false,
    inbound_enabled: true, outbound_enabled: true, scopes: ['devices:read'], expected_config_version: 7,
  });
  await expect(page.locator('.metric-card').filter({ hasText: '接入状态' }).locator('.metric-value')).toHaveText('未集成');
  await expect(page.locator('.metric-card').filter({ hasText: '配置方式' }).locator('.metric-value')).toHaveText('未集成');
  await expect(page.getByRole('button', { name: '保存应用' })).toBeDisabled();
});

test('HTTP 子页面直接使用唯一业务应用且不重复选择', async ({ page }) => {
  await mockAuth(page, true);
  await page.route('**/api/v2/integrations/business', async (route) => route.fulfill({
    status: 200, contentType: 'application/json', body: JSON.stringify({ state: 'unconfigured', integration: null }),
  }));
  await page.route('**/api-docs/openapi.json', async (route) => route.fulfill({
    status: 200, contentType: 'application/json', body: JSON.stringify({ paths: {} }),
  }));
  await page.goto('/integrations/http');
  await expect(page.getByRole('heading', { name: 'HTTP 接入', level: 1 })).toBeVisible();
  await expect(page.getByText('当前未选择 HTTP 接入')).toBeVisible();
  await expect(page.getByPlaceholder('请选择 HTTP 应用')).toHaveCount(0);
});

test('HTTP 凭证首屏隐藏 Secret 且查看需要二次密码鉴权', async ({ page }) => {
  await mockAuth(page, true);
  let savedHttpConfig: Record<string, unknown> | null = null;
  const integration = {
    integration_id: 'http-app-1', name: '业务 HTTP', transport: 'http', inbound_enabled: true,
    outbound_enabled: true, enabled: true, scopes: ['*'], expires_at_ms: null,
    config_version: 1, created_by: 'admin', created_at_ms: 1, updated_at_ms: 1,
  };
  await page.route('**/api/v2/integrations/business', async (route) => route.fulfill({
    status: 200, contentType: 'application/json', body: JSON.stringify({ state: 'ready', integration }),
  }));
  await page.route('**/api/v2/integrations/http-app-1/http', async (route) => {
    const baseConfig = {
      integration_id: 'http-app-1', callback_url: null, callback_timeout_ms: 5000,
      private_network_policy: 'deny', private_network_allowlist: [], max_attempts: 5,
      event_ttl_ms: 259200000, max_response_bytes: 65536, updated_at_ms: 1,
    };
    if (route.request().method() === 'POST') savedHttpConfig = route.request().postDataJSON();
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(route.request().method() === 'POST'
        ? { ...baseConfig, ...(savedHttpConfig ?? {}), updated_at_ms: 2 }
        : baseConfig),
    });
  });
  await page.route('**/api/v2/integrations/http-app-1/mappings', async (route) => route.fulfill({
    status: 200, contentType: 'application/json', body: '[]',
  }));
  await page.route('**/api/v2/integrations/http-app-1/credentials', async (route) => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify([{
      credential_id: 'cred-1', access_key: 'ak_http_1', integration_id: 'http-app-1',
      purpose: 'http_inbound_verify', key_version: 1, status: 'active', not_before_ms: 1,
      expires_at_ms: null, revoked_at_ms: null, created_by: 'admin', created_at_ms: 1, updated_at_ms: 1,
    }]),
  }));
  await page.route('**/api/v2/integrations/outbox?limit=500', async (route) => route.fulfill({
    status: 200, contentType: 'application/json', body: '[]',
  }));
  await page.route('**/api/v2/integrations/http-app-1/credentials/cred-1/reveal', async (route) => {
    expect(route.request().postDataJSON()).toEqual({ password: 'current-password' });
    await route.fulfill({
      status: 200, contentType: 'application/json', body: JSON.stringify({ secret: 'revealed-hmac-secret' }),
    });
  });
  await page.route('**/api-docs/openapi.json', async (route) => route.fulfill({
    status: 200, contentType: 'application/json', body: JSON.stringify({ paths: {} }),
  }));

  await page.goto('/integrations/http');
  await expect(page.getByRole('heading', { name: 'HMAC 凭证管理' })).toBeVisible();
  await expect(page.getByText('*******')).toBeVisible();
  await expect(page.getByText('revealed-hmac-secret')).toHaveCount(0);
  await page.getByRole('button', { name: '查看', exact: true }).click();
  await expect(page.getByRole('dialog', { name: '查看 HMAC Secret' })).toBeVisible();
  await expect(page.getByRole('button', { name: '验证并查看' })).toBeDisabled();
  await page.getByPlaceholder('请输入当前登录密码').fill('current-password');
  await page.getByRole('button', { name: '验证并查看' }).click();
  await expect(page.getByText('revealed-hmac-secret')).toBeVisible();
  await page.getByRole('button', { name: '关闭' }).click();
  await expect(page.getByText('revealed-hmac-secret')).toHaveCount(0);

  await page.getByPlaceholder('https://partner.example.com/gmv/events 或 http://192.168.0.8/events').fill('http://192.168.0.8/events');
  await page.locator('.config-form .el-switch').click();
  await page.getByPlaceholder(/每行一个 hostname/).fill('192.168.0.8');
  await page.getByRole('button', { name: '保存配置' }).click();
  await expect.poll(() => savedHttpConfig).toMatchObject({
    callback_url: 'http://192.168.0.8/events',
    private_network_policy: 'allowlist',
    private_network_allowlist: ['192.168.0.8'],
  });
});

test('HTTP 回调映射展示就绪条件并闭环管理投递失败', async ({ page }) => {
  await mockAuth(page, true);
  const integration = {
    integration_id: 'http-app-1', name: '业务 HTTP', transport: 'http', inbound_enabled: true,
    outbound_enabled: true, enabled: true, scopes: ['*'], expires_at_ms: null,
    config_version: 1, created_by: 'admin', created_at_ms: 1, updated_at_ms: 1,
  };
  let mappingEnabled = true;
  let outboxState = 'dead';
  await page.route('**/api/v2/integrations/business', async (route) => route.fulfill({
    status: 200, contentType: 'application/json', body: JSON.stringify({ state: 'ready', integration }),
  }));
  await page.route('**/api/v2/integrations/http-app-1/http', async (route) => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({
      integration_id: 'http-app-1', callback_url: 'https://partner.example.com/gmv/events',
      callback_timeout_ms: 5000, private_network_policy: 'deny', private_network_allowlist: [],
      max_attempts: 5, event_ttl_ms: 259200000, max_response_bytes: 65536, updated_at_ms: 1,
    }),
  }));
  await page.route('**/api/v2/integrations/http-app-1/credentials', async (route) => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify([{
      credential_id: 'cred-callback', access_key: 'ak_callback', integration_id: 'http-app-1',
      purpose: 'http_callback_sign', key_version: 1, status: 'active', not_before_ms: 1,
      expires_at_ms: null, revoked_at_ms: null, created_by: 'admin', created_at_ms: 1, updated_at_ms: 1,
    }]),
  }));
  await page.route('**/api/v2/integrations/http-app-1/mappings', async (route) => {
    if (route.request().method() === 'POST') {
      mappingEnabled = route.request().postDataJSON().enabled;
    }
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify(route.request().method() === 'POST' ? {
        mapping_id: 'map-1', integration_id: 'http-app-1', direction: 'OUTBOUND',
        source_type: 'session.*', schema_version: 'v1', destination_kind: 'HTTP',
        destination: 'https://partner.example.com/gmv/events', payload_profile: 'event-envelope-v1',
        enabled: mappingEnabled, created_at_ms: 1, updated_at_ms: 2,
      } : [{
        mapping_id: 'map-1', integration_id: 'http-app-1', direction: 'OUTBOUND',
        source_type: 'session.*', schema_version: 'v1', destination_kind: 'HTTP',
        destination: 'https://old.example.com/events', payload_profile: 'event-envelope-v1',
        enabled: mappingEnabled, created_at_ms: 1, updated_at_ms: 1,
      }]),
    });
  });
  await page.route('**/api/v2/integrations/outbox/outbox-1/retry', async (route) => {
    outboxState = 'pending';
    await route.fulfill({
      status: 200, contentType: 'application/json', body: JSON.stringify({
        outbox_id: 'outbox-1', event_id: 'event-1', integration_id: 'http-app-1', mapping_id: 'map-1',
        destination_kind: 'webhook', destination: 'https://partner.example.com/gmv/events/session/alarm', state: outboxState,
        attempts: 0, next_attempt_at_ms: 2, last_error: null, created_at_ms: 1, updated_at_ms: 2, expires_at_ms: null,
      }),
    });
  });
  await page.route('**/api/v2/integrations/outbox?limit=500', async (route) => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify([{
      outbox_id: 'outbox-1', event_id: 'event-1', integration_id: 'http-app-1', mapping_id: 'map-1',
      destination_kind: 'webhook', destination: 'https://partner.example.com/gmv/events/session/alarm', state: outboxState,
      attempts: outboxState === 'dead' ? 5 : 0, next_attempt_at_ms: 2,
      last_error: outboxState === 'dead' ? 'webhook returned HTTP 503' : null,
      created_at_ms: 1, updated_at_ms: 2, expires_at_ms: null,
    }]),
  }));
  await page.route('**/api-docs/openapi.json', async (route) => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({
      paths: {},
      webhooks: {
        guardEventCallback: {
          post: {
            summary: 'Guard 事件回调',
            'x-gmv-callback-url-source': 'HTTP 接入配置 callback_url',
            'x-gmv-callback-path': '{callback_url}/{event_type}',
            'x-gmv-event-types': [
              {
                event_type: 'session.alarm', method: 'POST', payload_profile: 'event-envelope-v1',
                http_path_suffix: '/session/alarm',
                payload_schema: { properties: { priority: {}, method: {}, alarmType: {}, timeStr: {}, deviceId: {}, channelId: {} } },
                summary: '设备报警通知', description: '设备上报报警后回调第三方。',
              },
              {
                event_type: 'avai.task.result', method: 'POST', payload_profile: 'event-envelope-v1',
                http_path_suffix: '/avai/task/result',
                summary: '智能分析任务结果', description: '智能分析任务产生终态时回调第三方。',
              },
            ],
          },
        },
      },
    }),
  }));

  await page.goto('/integrations/http');
  await expect(page.getByText('已就绪')).toHaveCount(3);
  await expect(page.getByText('可回调事件接口')).toBeVisible();
  await expect(page.getByText('session.alarm', { exact: true })).toBeVisible();
  await expect(page.getByText('设备报警通知', { exact: true })).toBeVisible();
  await expect(page.locator('.callback-contract-table').getByText('https://partner.example.com/gmv/events/session/alarm', { exact: true })).toBeVisible();
  await expect(page.locator('.callback-contract-table').getByText('priority、method、alarmType、timeStr、deviceId、channelId', { exact: true })).toBeVisible();
  await expect(page.getByText('已映射')).toBeVisible();
  await expect(page.locator('.mapping-table').getByText('https://partner.example.com/gmv/events/session/*', { exact: true })).toBeVisible();
  await expect(page.getByText('https://old.example.com/events')).toHaveCount(0);
  await expect(page.getByText('webhook returned HTTP 503')).toBeVisible();
  await page.locator('.mapping-table .el-switch').click();
  await expect.poll(() => mappingEnabled).toBe(false);
  await page.getByRole('button', { name: '重试' }).click();
  await expect(page.getByText('待投递')).toBeVisible();
  await expect(page.getByText('webhook returned HTTP 503')).toHaveCount(0);
});

test('未启用 MQTT 应用时详细配置保持只读并引导先选择接入方式', async ({ page }) => {
  await mockAuth(page, true);
  await page.route('**/api/v2/integrations/business', async (route) => route.fulfill({
    status: 200, contentType: 'application/json', body: JSON.stringify({ state: 'unconfigured', integration: null }),
  }));
  await page.route('**/api/v2/integrations/mqtt/runtime', async (route) => route.fulfill({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify({ configured: false, broker_connected: false, config: null, connection_scope: 'deployment', qos: 1, retain: false }),
  }));
  await page.route('**/api-docs/asyncapi.json', async (route) => route.fulfill({
    status: 200, contentType: 'application/json', body: JSON.stringify({ channels: {} }),
  }));

  await page.goto('/integrations/mqtt');
  await expect(page.getByText('当前未启用 MQTT 业务接入')).toBeVisible();
  await expect(page.getByText('请先在“接入应用”选择 MQTT、打开“启用应用”并保存')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'MQTT Runtime 配置' })).toBeVisible();
  await expect(page.getByRole('button', { name: '保存 Runtime 配置' })).toBeDisabled();
});

test('MQTT 子页面展示连接状态、Topic 契约、文档与可靠投递', async ({ page }) => {
  await mockAuth(page, true);
  const integration = {
    integration_id: 'mqtt-app-1', name: '边缘 MQTT', transport: 'mqtt', inbound_enabled: true,
    outbound_enabled: true, enabled: true, scopes: ['*'], expires_at_ms: null,
    config_version: 1, created_by: 'admin', created_at_ms: 1, updated_at_ms: 1,
  };
  let savedVersion = '';

  await page.route('**/api/v2/integrations/business', async (route) => route.fulfill({
    status: 200, contentType: 'application/json', body: JSON.stringify({ state: 'ready', integration }),
  }));
  await page.route('**/api/v2/integrations/mqtt/runtime', (route) => route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ configured: true, broker_connected: true, config: { protocol_version: 'v5', broker: 'broker.example.test', port: 1883, client_id: 'guard-test', username: 'guard', password_configured: true, tls: false, publish_event_ttl_sec: 86400, desired_revision: 2, active_revision: 2, config_version: 2, apply_state: 'CONNECTED', last_error_code: null, last_error_summary: null, last_transition_at_ms: 1, updated_by: 'admin', updated_at_ms: 1 }, connection_scope: 'deployment', qos: 1, retain: false }) }));
  await page.route('**/api/v2/integrations/business/mqtt/runtime', async (route) => {
    const payload = route.request().postDataJSON() as { protocol_version: string };
    savedVersion = payload.protocol_version;
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ protocol_version: payload.protocol_version, broker: 'broker.example.test', port: 1883, client_id: 'guard-test', username: 'guard', password_configured: true, tls: false, publish_event_ttl_sec: 86400, desired_revision: 3, active_revision: 2, config_version: 3, apply_state: 'PENDING', last_error_code: null, last_error_summary: null, last_transition_at_ms: 2, updated_by: 'admin', updated_at_ms: 2 }) });
  });
  await page.route('**/api/v2/integrations/mqtt-app-1/mqtt', async (route) => {
    const config = {
      integration_id: 'mqtt-app-1',
      command_topic: 'gmv/commands/mqtt-app-1', result_topic: 'gmv/command-results/mqtt-app-1',
      event_topic_prefix: 'gmv/events/mqtt-app-1', updated_at_ms: 1,
    };
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(config) });
  });
  await page.route('**/api/v2/integrations/mqtt-app-1/mappings', (route) => route.fulfill({
    status: 200, contentType: 'application/json', body: JSON.stringify([]),
  }));
  await page.route('**/api-docs/asyncapi.json', (route) => route.fulfill({
    status: 200, contentType: 'application/json', body: JSON.stringify({ channels: {
      commands: { address: 'gmv/commands/{integration_id}', messages: { MqttCommand: {} } },
      commandResults: { address: 'gmv/command-results/{integration_id}', messages: { MqttCommandResult: {} } },
      events: {
        address: 'gmv/events/{integration_id}/{event_type}',
        messages: { EventEnvelope: {} },
        'x-gmv-event-types': [{
          event_type: 'session.alarm', mqtt_topic_suffix: 'session/alarm',
          summary: '设备报警通知', description: '设备上报报警后发布 MQTT 事件。',
          payload_profile: 'event-envelope-v1',
          payload_schema: { properties: { priority: {}, method: {}, alarmType: {}, timeStr: {}, deviceId: {}, channelId: {} } },
        }],
      },
    } }),
  }));
  await page.goto('/integrations/mqtt');
  await expect(page).toHaveURL(/\/integrations\/mqtt$/);
  await expect(page.getByRole('heading', { name: 'MQTT 接入', level: 1 })).toBeVisible();
  await expect(page.getByText('MQTT Runtime 配置')).toBeVisible();
  await expect(page.getByText('第三方业务应用已启用')).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Broker 连接状态' })).toBeVisible();
  await expect(page.getByText('已连接 Broker')).toBeVisible();
  await expect(page.getByText('Topic 契约预览')).toBeVisible();
  await expect(page.getByRole('button', { name: '查看在线文档' })).toBeVisible();
  await expect(page.getByText('文档随 Guard Server 发布')).toBeVisible();
  await expect(page.getByText('边端可靠投递')).toBeVisible();
  await expect(page.getByText('允许动作')).toHaveCount(0);
  await expect(page.getByText('MQTT 应用策略')).toHaveCount(0);
  await expect(page.getByText('gmv/commands/{integration_id}', { exact: true })).toBeVisible();
  await expect(page.getByText('gmv/events/{integration_id}/{event_type} 可发布事件')).toBeVisible();
  await expect(page.getByText('gmv/events/{integration_id}/session/alarm', { exact: true })).toBeVisible();
  await expect(page.getByText('priority、method、alarmType、timeStr、deviceId、channelId', { exact: true })).toBeVisible();
  await expect(page.getByText('设备报警通知', { exact: true })).toBeVisible();
  await expect(page.locator('.el-loading-mask')).toBeHidden();
  await page.getByText('MQTT 5.0', { exact: true }).first().click();
  await page.getByText('MQTT 3.1.1', { exact: true }).click();
  await page.getByRole('button', { name: '保存 Runtime 配置' }).click();
  await expect.poll(() => savedVersion).toBe('v3');
});

test('Dashboard 将持续 Catalog 重建失败归入运维待处理事项', async ({ page }) => {
  await mockAuth(page, true);
  await page.route('**/api/v2/nodes', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify([{
        ...readySessionNode,
        business_metrics: { catalog_subscription_degraded_devices: '2' },
      }]),
    });
  });

  await page.goto('/dashboard');

  await expect(page.getByText('2 个设备目录订阅持续异常')).toBeVisible();
  await expect(page.getByText('当前无待处理事项')).toHaveCount(0);
});

test('Dashboard 将会话与运行态冲突归入运维待处理事项', async ({ page }) => {
  await mockAuth(page, true);
  await page.route('**/api/v2/nodes', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify([{
        ...readySessionNode,
        business_metrics: { dialog_runtime_conflicts: '1' },
      }]),
    });
  });

  await page.goto('/dashboard');

  await expect(page.getByText('1 个流运行态与会话记录不一致')).toBeVisible();
  await expect(page.getByText('当前无待处理事项')).toHaveCount(0);
});

test('管理员可恢复并再次隐藏实验功能，直达路由与偏好保持一致', async ({ page }) => {
  await mockAuth(page, true);

  await page.goto('/onvif');
  await expect(page).toHaveURL((url) => url.pathname === '/dashboard');
  await expect(page.getByText('该功能当前处于实验隐藏状态')).toBeVisible();
  await expect(page.getByRole('menuitem', { name: /ONVIF/ })).toHaveCount(0);

  await page.getByRole('button', { name: /舰桥管理员 admin/ }).click();
  await page.getByRole('menuitem', { name: '显示实验性功能' }).click();
  await expect(page.getByRole('menuitem', { name: /ONVIF（实验）/ })).toBeVisible();
  await page.goto('/onvif');
  await expect(page.getByRole('heading', { name: 'ONVIF', level: 1 })).toBeVisible();
  await expect(page.getByText('实验性 · 未闭环')).toBeVisible();

  await page.reload();
  await expect(page.getByRole('heading', { name: 'ONVIF', level: 1 })).toBeVisible();
  await page.getByRole('button', { name: /舰桥管理员 admin/ }).click();
  await page.getByRole('menuitem', { name: '隐藏实验性功能' }).click();
  await expect(page).toHaveURL((url) => url.pathname === '/dashboard');
  await expect(page.getByRole('menuitem', { name: /ONVIF/ })).toHaveCount(0);
});

test('非管理员不能显示实验功能，本地状态不构成授权', async ({ page }) => {
  const viewerSession = { ...session, username: 'viewer', nickname: '观察员', role: 'viewer' as const };
  await mockAuth(page, true, viewerSession);

  await page.goto('/dashboard');
  await page.evaluate(() => window.localStorage.setItem('gmv.preview.experimental_features.viewer', 'true'));
  await page.getByRole('button', { name: /观察员 viewer/ }).click();
  await expect(page.getByRole('menuitem', { name: '显示实验性功能' })).toHaveCount(0);
  await page.goto('/ai');
  await expect(page).toHaveURL((url) => url.pathname === '/dashboard');
});

test('事件中心隐藏 cursor 并提供面向用户的筛选和详情入口', async ({ page }) => {
  await mockAuth(page, true);

  await page.goto('/dashboard');
  await page.getByRole('button', { name: /舰桥管理员 admin/ }).click();
  await page.getByRole('menuitem', { name: '显示实验性功能' }).click();
  await page.goto('/events');

  await expect(page.getByRole('heading', { name: '告警与事件', level: 2 })).toBeVisible();
  await expect(page.getByRole('combobox', { name: '事件级别' })).toBeVisible();
  await expect(page.getByRole('combobox', { name: '事件领域' })).toBeVisible();
  await expect(page.getByLabel('事件搜索')).toBeVisible();
  await expect(page.getByRole('button', { name: '暂停接收' })).toBeVisible();
  await expect(page.getByText(/after_id|next cursor/)).toHaveCount(0);
  await expect(page.getByText('获取时间不是事件发生时间')).toBeVisible();
});

test('多画面工作台已选通道空状态居中', async ({ page }) => {
  await mockAuth(page, true);
  await page.goto('/gb28181/monitor');
  await page.getByRole('button', { name: '多画面工作台', exact: true }).click();

  const mediaTransport = page.getByRole('combobox', { name: '媒体传输模式' });
  await expect(mediaTransport).toBeVisible();
  await page.locator('.monitor-actions .el-select').filter({ has: mediaTransport }).click();
  await expect(page.getByRole('option', { name: 'UDP', exact: true })).toBeVisible();
  await expect(page.getByRole('option', { name: 'TCP 主动', exact: true })).toBeVisible();
  await expect(page.getByRole('option', { name: 'TCP 被动', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');

  const list = page.locator('.selected-channel-list.empty');
  const empty = list.locator('.el-empty');
  await expect(empty).toBeVisible();
  await expect(empty).toContainText('暂无已选通道');
  const [listBox, emptyBox] = await Promise.all([list.boundingBox(), empty.boundingBox()]);
  expect(listBox).not.toBeNull();
  expect(emptyBox).not.toBeNull();
  expect(Math.abs((listBox!.x + listBox!.width / 2) - (emptyBox!.x + emptyBox!.width / 2))).toBeLessThan(2);
  expect(Math.abs((listBox!.y + listBox!.height / 2) - (emptyBox!.y + emptyBox!.height / 2))).toBeLessThan(2);
});
