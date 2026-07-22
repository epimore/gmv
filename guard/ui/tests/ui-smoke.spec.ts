import { expect, test, type Page } from '@playwright/test';

const session = {
  username: 'admin',
  nickname: '舰桥管理员',
  role: 'admin',
  csrf_token: 'csrf-test-token',
  expires_at_ms: Date.now() + 60_000,
};

const routes = [
  ['/dashboard', 'Dashboard'],
  ['/nodes', '节点监控'],
  ['/devices', '设备'],
  ['/gb28181/register', '注册管理'],
  ['/gb28181/monitor', '监控信息'],
  ['/streams', '流媒监控'],
  ['/ai', '智能分析'],
  ['/events', '事件中心'],
  ['/integrations', '三方集成'],
  ['/system', '系统健康'],
] as const;

async function mockAuth(page: Page, initiallyAuthenticated = false) {
  let authenticated = initiallyAuthenticated;
  const readBodies = new Map<string, unknown>([
    ['/api/v2/dashboard', { node_count: 0, event_count: 0, next_after_id: null }],
    ['/api/v2/events', { items: [], next_after_id: null }],
    ['/api/v2/devices', []],
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
      body: JSON.stringify(authenticated ? session : { message: 'invalid UI session' }),
    });
  });
  await page.route('**/api/v2/auth/login', async (route) => {
    const body = route.request().postDataJSON() as { username: string; password: string };
    if (body.username !== 'admin' || body.password !== 'secret') {
      await route.fulfill({ status: 401, contentType: 'application/json', body: JSON.stringify({ message: '用户名或密码错误' }) });
      return;
    }
    authenticated = true;
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(session) });
  });
  await page.route('**/api/v2/auth/logout', async (route) => {
    expect(route.request().headers()['x-csrf-token']).toBe(session.csrf_token);
    authenticated = false;
    await route.fulfill({ status: 204, body: '' });
  });
  await page.route('**/api/v2/users', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify([{ ...session, enabled: true, created_at_ms: 0, updated_at_ms: 0 }]),
    });
  });
  await page.route('**/api/v2/me', async (route) => {
    await route.fulfill({
      status: 200,
      contentType: 'application/json',
      body: JSON.stringify({ ...session, enabled: true, created_at_ms: 0, updated_at_ms: 0 }),
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
  await expect(page).toHaveURL((url) => url.pathname === '/login' && url.searchParams.get('redirect') === '/nodes');
  await expect(page.getByRole('heading', { name: '登录' })).toBeVisible();

  await page.getByLabel('用户名').fill('admin');
  await page.getByLabel('密码').fill('secret');
  await page.getByRole('button', { name: '安全登录' }).click();
  await expect(page).toHaveURL((url) => url.pathname === '/nodes');
  const userMenu = page.getByRole('button', { name: /舰桥管理员 admin/ });
  await expect(userMenu).toBeVisible();

  await page.reload();
  await expect(page.getByRole('heading', { name: '节点监控', level: 1 })).toBeVisible();

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

  for (const [path, heading] of routes) {
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
