import { expect, test, type Page } from '@playwright/test';

const session = {
  username: 'admin',
  nickname: '舰桥管理员',
  role: 'admin',
  csrf_token: 'csrf-test-token',
  expires_at_ms: Date.now() + 60_000,
};

const routes = [
  ['/dashboard', '总览'],
  ['/nodes', '节点'],
  ['/devices', '设备'],
  ['/streams', '流媒体'],
  ['/ai', '智能分析'],
  ['/allocations', '调度与租约'],
  ['/events', '事件中心'],
  ['/integrations', '集成'],
  ['/system', '系统'],
] as const;

async function mockAuth(page: Page, initiallyAuthenticated = false) {
  let authenticated = initiallyAuthenticated;

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
  await expect(page.getByText('舰桥管理员 · admin')).toBeVisible();

  await page.reload();
  await expect(page.getByRole('heading', { name: '节点', level: 1 })).toBeVisible();

  await page.getByRole('button', { name: '退出登录' }).click();
  await expect(page).toHaveURL((url) => url.pathname === '/login');

  await page.goto('/system');
  await expect(page).toHaveURL((url) => url.pathname === '/login' && url.searchParams.get('redirect') === '/system');
});

test('已登录会话可访问中文页面与移动端布局', async ({ page }) => {
  await mockAuth(page, true);
  await page.setViewportSize({ width: 390, height: 844 });

  for (const [path, heading] of routes) {
    await page.goto(path);
    await expect(page.getByRole('heading', { name: heading, level: 1 })).toBeVisible();
  }

  await page.goto('/dashboard');
  await expect(page.getByRole('heading', { name: '总览', level: 1 })).toBeVisible();
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
