// Optional integration check against the real Rust process, not mocked API routes.
// First build backend/target/debug/pullboard, then run npm run test:server.
import { chromium, expect } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import { access, mkdtemp, writeFile, unlink, rmdir } from 'node:fs/promises';
import { constants } from 'node:fs';
import { spawn } from 'node:child_process';
import { createServer } from 'node:net';
import { once } from 'node:events';
import assert from 'node:assert/strict';
import { resolve } from 'node:path';

const root = resolve('..');
const binary = process.argv[2]
  ? resolve(process.argv[2])
  : `${root}/backend/target/debug/pullboard`;
await access(binary, constants.X_OK);
const reserve = createServer();
reserve.listen(0, '127.0.0.1');
await once(reserve, 'listening');
const port = reserve.address().port;
await new Promise((done) => reserve.close(done));
const base = `http://127.0.0.1:${port}`;
const directory = await mkdtemp('/tmp/opencode/pullboard-native-smoke-');
const config = `${directory}/config.yaml`;
await writeFile(
  config,
  `server:
  bind: 127.0.0.1
  port: ${port}
  allowed_origins: [${base}]
  static_directory: ${JSON.stringify(`${root}/frontend/dist/frontend/browser`)}
  database: ':memory:'
read_only_test_mode: true
jira: null
forges: []
ai: {enabled: false}
`,
  { mode: 0o600 },
);

const child = spawn(binary, ['--config', config], {
  cwd: root,
  stdio: ['ignore', 'pipe', 'pipe'],
});
const exited = once(child, 'exit');
let serverErrors = '';
child.stderr.on('data', (chunk) => (serverErrors += chunk));
let browser;
try {
  await expect(async () => {
    assert.equal(child.exitCode, null, serverErrors);
    assert.equal((await fetch(`${base}/api/health`)).status, 200);
  }).toPass({ timeout: 10_000 });
  browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 1600, height: 1080 } });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  page.on('console', (message) => {
    if (message.type() === 'error') errors.push(message.text());
  });
  const response = await page.goto(base);
  assert.match(response.headers()['content-security-policy'], /script-src 'self'/);
  await expect(page.getByText('No verified identities yet.', { exact: false })).toBeVisible();
  await expect(page.getByText('No matching active sprint.', { exact: false })).toBeVisible();
  await expect(page.locator('.app-header')).toHaveCSS('background-color', 'rgb(32, 54, 80)');
  assert.equal((await new AxeBuilder({ page }).analyze()).violations.length, 0);
  assert.deepEqual((await (await fetch(`${base}/api/board`)).json()).upstream_issues, []);
  await page.getByRole('button', { name: 'Upstream issues', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'No assigned upstream issues' })).toBeVisible();
  await expect(page.getByLabel('Issue URL', { exact: true })).toBeVisible();
  assert.equal((await new AxeBuilder({ page }).analyze()).violations.length, 0);
  const initialMode = await (await fetch(`${base}/api/runtime-mode`)).json();
  assert.equal(initialMode.read_only_test_mode, true);
  assert.equal(typeof initialMode.mode_revision, 'string');
  const second = await context.newPage();
  await second.goto(base);
  await page.getByRole('button', { name: 'Switch to Live', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Enable Live writes' }).click();
  await expect(second.getByRole('button', { name: 'Switch to Test', exact: true })).toBeVisible();
  const liveMode = await (await fetch(`${base}/api/runtime-mode`)).json();
  assert.equal(liveMode.read_only_test_mode, false);
  assert.notEqual(liveMode.mode_revision, initialMode.mode_revision);
  // No Jira/forges configured: toggling is local and makes zero upstream calls.
  const stale = await fetch(`${base}/api/runtime-mode`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-PullBoard-Request': '1' },
    body: JSON.stringify({
      read_only_test_mode: true,
      expected_mode_revision: initialMode.mode_revision,
    }),
  });
  assert.equal(stale.status, 409);
  assert.equal((await stale.json()).error.code, 'mode_conflict');
  await second.getByRole('button', { name: 'Switch to Test', exact: true }).click();
  await second
    .getByRole('dialog')
    .getByRole('button', { name: 'Switch to Test', exact: true })
    .click();
  await expect(page.getByRole('button', { name: 'Switch to Live', exact: true })).toBeVisible();
  const testMode = await (await fetch(`${base}/api/runtime-mode`)).json();
  assert.equal(testMode.read_only_test_mode, true);
  assert.notEqual(testMode.mode_revision, initialMode.mode_revision);
  await second.close();
  await page.getByRole('button', { name: 'Dark theme', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  assert.equal((await new AxeBuilder({ page }).analyze()).violations.length, 0);
  const refresh = await page.evaluate(
    async () =>
      (
        await fetch('/api/sync', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', 'X-PullBoard-Request': '1' },
          body: '{}',
        })
      ).status,
  );
  assert.equal(refresh, 202);
  const csrf = await fetch(`${base}/api/sync`, {
    method: 'POST',
    headers: {
      Origin: 'https://evil.example',
      'Content-Type': 'application/json',
      'X-PullBoard-Request': '1',
    },
    body: '{}',
  });
  assert.equal(csrf.status, 403);
  for (const path of ['/api/not-found', '/api/']) {
    const missing = await fetch(`${base}${path}`, { headers: { Accept: 'text/html' } });
    assert.equal(missing.status, 404);
    assert.match(missing.headers.get('content-type'), /application\/json/);
  }
  assert.equal(errors.length, 0, errors.join('\n'));
  console.log(
    'PASS: real Rust + Angular browser smoke; CSP/assets, light/dark axe, confirmed multi-tab runtime mode and stale-token conflict, refresh, CSRF, JSON API 404; zero browser errors.',
  );
} finally {
  if (browser) await browser.close();
  child.kill('SIGTERM');
  await exited;
  await unlink(config);
  await rmdir(directory);
}
