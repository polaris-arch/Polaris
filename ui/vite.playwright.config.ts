import path from 'node:path';
import { defineConfig, mergeConfig } from 'vite';
import baseConfig from './vite.config';

export default mergeConfig(baseConfig, defineConfig({
  // One reusable Playwright slot, separate from the Vitest fixture servers.
  cacheDir: path.resolve(import.meta.dirname, 'node_modules/.vite/playwright'),
  optimizeDeps: {
    // The production build inputs omit these fixtures. Scan their dependencies
    // up front so initial module requests do not race a newly discovered bundle
    // generation before the browser's HMR connection can receive its reload.
    entries: ['harness.html', 'tray-harness.html'],
  },
}));
