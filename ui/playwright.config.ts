import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './e2e',
  // Playwright clears its outputDir before execution. Keep prior Vitest
  // feedback/measurement evidence as siblings under the shared artifact root.
  outputDir: './test-results/playwright',
  testMatch: '**/*.pw.ts',
  use: {
    baseURL: 'http://127.0.0.1:4177',
    browserName: 'chromium',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    launchOptions: {
      chromiumSandbox: true,
      ...(process.env.POLARIS_TEST_CHROMIUM_PATH
        ? { executablePath: process.env.POLARIS_TEST_CHROMIUM_PATH }
        : {}),
    },
  },
  webServer: {
    command: 'pnpm exec vite --host 127.0.0.1 --port 4177 --strictPort',
    url: 'http://127.0.0.1:4177/harness.html',
    reuseExistingServer: !process.env.CI,
    timeout: 30_000,
  },
});
