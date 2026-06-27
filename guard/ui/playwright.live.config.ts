import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests-live',
  fullyParallel: false,
  use: {
    baseURL: 'http://127.0.0.1:8080',
    screenshot: 'only-on-failure',
  },
});
