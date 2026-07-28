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

  await expect(page.getByRole('heading', { name: '边端能力等待接入', level: 2 })).toBeVisible();
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
