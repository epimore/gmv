import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  fullyParallel: false,
  use: {
    baseURL: 'http://127.0.0.1:5174',
    screenshot: 'only-on-failure',
  },
  webServer: {
    command: 'VITE_GMV_API_MODE=mock pnpm dev --host 127.0.0.1 --port 5174',
    url: 'http://127.0.0.1:5174/login',
    reuseExistingServer: true,
  },
});
