import { existsSync } from 'node:fs';
import { defineConfig } from '@playwright/test';

const port = Number(process.env.CAM_USABILITY_PORT ?? 1439);
if (!Number.isInteger(port) || port < 1024 || port > 65535)
  throw new Error('Invalid CAM_USABILITY_PORT');
const output = process.env.CAM_USABILITY_OUTPUT ?? 'test-results/usability';
export default defineConfig({
  testDir: './e2e/usability',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 20000,
  expect: { timeout: 2500 },
  forbidOnly: true,
  outputDir: `${output}/results`,
  reporter: [['list'], ['html', { outputFolder: `${output}/report`, open: 'never' }]],
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    viewport: { width: 1180, height: 790 },
    deviceScaleFactor: 1,
    locale: 'pt-BR',
    timezoneId: 'America/Sao_Paulo',
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    launchOptions: existsSync('/usr/bin/google-chrome')
      ? { executablePath: '/usr/bin/google-chrome' }
      : {},
  },
  webServer: {
    command: `npm run dev -- --port ${port} --strictPort`,
    env: { VITE_USE_MOCK_API: 'true' },
    url: `http://127.0.0.1:${port}`,
    reuseExistingServer: false,
    timeout: 30000,
  },
});
