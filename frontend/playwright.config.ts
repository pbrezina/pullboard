import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  use: {
    baseURL: 'http://127.0.0.1:4209',
    viewport: { width: 1440, height: 1080 },
    trace: 'retain-on-failure',
  },
  webServer: {
    command: 'python3 -m http.server 4209 --bind 127.0.0.1 --directory dist/frontend/browser',
    url: 'http://127.0.0.1:4209',
    reuseExistingServer: false,
  },
});
