import { expect, test } from '@playwright/test';

test('Guard 模拟节点完成点播、PTZ、AI 和停止闭环', async ({ page }) => {
  await page.goto('/login');
  await page.getByLabel('用户名').fill('admin');
  await page.getByLabel('密码').fill('secret');
  await page.getByRole('button', { name: '安全登录' }).click();
  await expect(page).toHaveURL(/\/dashboard$/);

  await page.locator('a[href="/devices"]').click();
  await expect(page.getByRole('heading', { name: '设备', level: 1 })).toBeVisible();
  await expect(page.getByText('SIM LIVE')).toBeVisible();
  await page.getByRole('button', { name: '发起预览' }).click();
  await expect(page.getByText('预览已创建')).toBeVisible();
  await page.getByRole('button', { name: '云台测试' }).click();
  await expect(page.getByText('PTZ 命令已接受')).toBeVisible();

  await page.locator('a[href="/ai"]').click();
  await page.getByRole('button', { name: '创建车辆分析' }).click();
  await expect(page.getByText('AI 任务已创建')).toBeVisible();
  await page.locator('button:has-text("取消"):not([disabled])').first().click();
  await expect(page.getByText('AI 任务已取消')).toBeVisible();

  await page.locator('a[href="/streams"]').click();
  await page.locator('button:has-text("停止"):not([disabled])').first().click();
  await expect(page.getByText('流已停止')).toBeVisible();
});
