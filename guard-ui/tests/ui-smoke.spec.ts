import { expect, test } from '@playwright/test';

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

test('登录、轮询控制与节点导航', async ({ page }) => {
  const errors: string[] = [];
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text());
  });
  page.on('pageerror', (error) => errors.push(error.message));

  await page.goto('/login');
  await page.getByRole('button', { name: '安全登录' }).click();
  await expect(page).toHaveURL(/\/dashboard$/);
  await expect(page.getByRole('heading', { name: '总览', level: 1 })).toBeVisible();

  await page.getByRole('button', { name: '暂停' }).click();
  await expect(page.getByRole('button', { name: '恢复' })).toBeVisible();

  await page.locator('a[href="/nodes"]').click();
  await expect(page).toHaveURL(/\/nodes$/);
  await expect(page.getByRole('heading', { name: '节点', level: 1 })).toBeVisible();
  expect(errors).toEqual([]);
});

test('中文页面与移动端布局', async ({ page }) => {
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
