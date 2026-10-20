import { defineConfig } from '@playwright/test';

// Expects a running devnet whose bootstrap serves web/dist:
//   DETACH=1 scripts/devnet.sh 4   (after scripts/build-web.sh)
// CHROME overrides the browser binary (default: system chromium).
export default defineConfig({
  testDir: 'e2e',
  timeout: 240_000,
  use: {
    baseURL: process.env.SIMU_URL ?? 'http://127.0.0.1:8080',
    launchOptions: { executablePath: process.env.CHROME ?? '/usr/bin/chromium', args: ['--no-sandbox'] },
  },
});
