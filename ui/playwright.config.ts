import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './e2e',
  testMatch: '**/*.pw.ts',
  use: {
    baseURL: 'http://127.0.0.1:4177',
    browserName: 'chromium',
    launchOptions: process.env.POLARIS_TEST_CHROMIUM_PATH
      ? { executablePath: process.env.POLARIS_TEST_CHROMIUM_PATH }
      : undefined,
  },
  webServer: {
    command: 'pnpm exec vite --host 127.0.0.1 --port 4177 --strictPort',
    url: 'http://127.0.0.1:4177/harness.html',
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
  },
});
