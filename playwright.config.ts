import { existsSync } from 'node:fs';
import { defineConfig, devices } from '@playwright/test';

const systemChrome = '/usr/bin/google-chrome';
const port = Number(process.env.CAM_E2E_PORT ?? 1420);
const output = process.env.CAM_E2E_OUTPUT ?? 'test-results/e2e';
if (!Number.isInteger(port) || port < 1024 || port > 65535) throw new Error('Invalid CAM_E2E_PORT');
const launchOptions = existsSync(systemChrome)
  ? { launchOptions: { executablePath: systemChrome } }
  : {};
const inheritedEnvironment = Object.fromEntries(
  Object.entries(process.env).filter((entry): entry is [string, string] => entry[1] !== undefined),
);

export default defineConfig({
  testDir: './e2e',
  testIgnore: '**/usability/**',
  fullyParallel: false,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 2 : 0,
  outputDir: `${output}/results`,
  reporter: [['list'], ['html', { open: 'never', outputFolder: `${output}/report` }]],
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
    video: 'retain-on-failure',
    ...launchOptions,
  },
  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],
  webServer: {
    command: `npm run dev -- --port ${port} --strictPort`,
    env: { ...inheritedEnvironment, VITE_USE_MOCK_API: 'true' },
    url: `http://127.0.0.1:${port}`,
    reuseExistingServer: false,
    timeout: 60_000,
  },
});
