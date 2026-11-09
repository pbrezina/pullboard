import { expect, test, type BrowserContext } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import {
  board,
  context,
  options,
  runtimeMode,
  sync,
  upstreamDraft,
  upstreamIssue,
} from '../src/app/testing/fixtures';
import type { RuntimeMode, UpstreamIssue } from '../src/app/core/api-types';

const closed = upstreamIssue({
  id: 'manual:closed:99',
  number: 99,
  url: 'https://forge.example/acme/widget/issues/99',
  state: 'closed',
  assignees: [],
  is_assigned_to_me: false,
  title: 'Closed unassigned upstream issue',
});
const warning = 'This upstream issue is closed. Review it before creating a Jira ticket.';
const assigned = [
  upstreamIssue({ id: 'z', title: 'zulu issue' }),
  upstreamIssue({ id: 'b', title: 'Alpha issue', labels: ['Accepted', 'bug'] }),
  upstreamIssue({ id: 'a', title: 'alpha issue', reporter: null }),
  upstreamIssue({
    id: 'personal-issue',
    title: 'Personal issue',
    source_id: 'personal',
    repository: 'other',
  }),
];

async function mockApi(browser: BrowserContext, issues = assigned) {
  let mode: RuntimeMode = runtimeMode();
  let rejection: { code: string; message: string } | null = null;
  let createOutcome: 'simulated' | 'created' | 'accepted' | 'unknown' | 'conflict' = 'simulated';
  let pause = false;
  let release!: () => void;
  let waiting!: () => void;
  const lookupStarted = new Promise<void>((resolve) => (waiting = resolve));
  const lookupReleased = new Promise<void>((resolve) => (release = resolve));
  const posts: { path: string; body: Record<string, unknown>; header: string | undefined }[] = [];
  const queries: URL[] = [];
  const snapshot = board({ upstream_issues: issues });
  const projects = [
    ...context().jira_projects,
    { key: 'OPS', name: 'Operations', issue_types: ['Task', 'Bug'] },
  ];
  let selected: UpstreamIssue = issues[0] ?? upstreamIssue();
  await browser.route('**/api/**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    if (request.method() === 'POST') {
      const body = request.postDataJSON();
      posts.push({ path, body, header: request.headers()['x-pullboard-request'] });
      if (path === '/api/runtime-mode') {
        mode = {
          read_only_test_mode: body.read_only_test_mode,
          mode_revision: `toggle-${posts.length}`,
        };
        return route.fulfill({ json: mode });
      }
      if (path === '/api/upstream-issues/resolve') {
        waiting();
        if (pause) await lookupReleased;
        if (rejection) return route.fulfill({ status: 422, json: { error: rejection } });
        selected = closed;
        return route.fulfill({ json: { issue: closed, warning } });
      }
      if (path === '/api/upstream-issues/ticket-drafts') {
        const issue =
          body.upstream_issue_id === closed.id
            ? closed
            : issues.find((i) => i.id === body.upstream_issue_id)!;
        selected = issue;
        return route.fulfill({ json: { ...upstreamDraft(issue), project_key: 'OPS' } });
      }
      if (path === '/api/upstream-issues/jira-issues') {
        if (createOutcome === 'conflict') {
          mode = runtimeMode({ mode_revision: 'server-restart' });
          return route.fulfill({
            status: 409,
            json: {
              error: { code: 'mode_conflict', message: 'Mode changed. Review the current mode.' },
            },
          });
        }
        if (createOutcome === 'unknown')
          return route.fulfill({
            status: 502,
            json: {
              error: { code: 'write_outcome_unknown', message: 'Creation could not be confirmed.' },
            },
          });
        return route.fulfill({
          json: {
            outcome: createOutcome === 'simulated' ? 'simulated' : 'created',
            issue:
              createOutcome === 'created'
                ? {
                    key: 'OPS-23',
                    url: 'https://jira.example/browse/OPS-23',
                    title: 'Confirmed response fixture',
                  }
                : null,
            message:
              createOutcome === 'simulated'
                ? 'Upstream issue creation simulated.'
                : 'Jira accepted creation.',
            warnings:
              createOutcome === 'accepted'
                ? ['No confirmed key; inspect Jira, do not resubmit.']
                : [],
          },
        });
      }
      if (path === '/api/sync') return route.fulfill({ status: 202, json: sync('syncing') });
    }
    if (path === '/api/runtime-mode') return route.fulfill({ json: mode });
    if (path === '/api/context')
      return route.fulfill({ json: context({ ...mode, jira_projects: projects }) });
    if (path === '/api/board') return route.fulfill({ json: snapshot });
    if (path === '/api/sync') return route.fulfill({ json: sync() });
    if (path === '/api/health')
      return route.fulfill({ json: { status: 'ok', version: '1.3.0', ...mode } });
    if (path === '/api/jira/options') {
      queries.push(url);
      return route.fulfill({
        json: {
          ...options('implementation'),
          project_key: url.searchParams.get('project_key'),
          issue_type: url.searchParams.get('issue_type') ?? 'Task',
        },
      });
    }
    return route.fulfill({
      status: 404,
      json: { error: { code: 'not_found', message: 'Unknown mock API route.' } },
    });
  });
  return {
    posts,
    queries,
    lookupStarted,
    release,
    pauseLookup: () => (pause = true),
    reject: (value: typeof rejection) => (rejection = value),
    outcome: (value: typeof createOutcome) => (createOutcome = value),
    setMode: (value: RuntimeMode) => (mode = value),
    mode: () => mode,
    selected: () => selected,
  };
}

test('compact full-width title-ordered rows, independent persisted PR-style filters, light/dark axe', async ({
  page,
  context: browser,
}) => {
  await mockApi(browser);
  await page.goto('/');
  await page.getByLabel('PR title', { exact: true }).fill('snapshot');
  await page.getByRole('button', { name: 'Upstream issues', exact: true }).click();
  await expect(page.locator('.upstream-row h3')).toHaveText([
    'alpha issue',
    'Alpha issue',
    'Personal issue',
    'zulu issue',
  ]);
  await expect(page.getByText('Showing 4 of 4 upstream issues')).toBeVisible();
  await expect(page.locator('.work-card')).toHaveCount(0);
  const row = page.locator('.upstream-row').first();
  await expect(row).toContainText('Reporter: unavailable');
  await expect(row).toContainText('acme/widget');
  await expect(row).toContainText('Keep the first line.');
  const bounds = await row.boundingBox();
  expect(bounds!.height).toBeGreaterThan(80);
  expect(bounds!.height).toBeLessThan(115);
  expect(bounds!.width).toBeGreaterThan(1000);
  for (const theme of ['Light theme', 'Dark theme']) {
    await page.getByRole('button', { name: theme, exact: true }).click();
    expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
    await page.screenshot({
      path: `test-results/upstream-${theme.startsWith('Light') ? 'light' : 'dark'}.png`,
      fullPage: true,
    });
  }
  await page.getByLabel('Issue title', { exact: true }).fill('ALPHA');
  await expect(page.locator('.upstream-row')).toHaveCount(2);
  await page
    .getByRole('combobox', { name: 'Forge instance', exact: true })
    .selectOption('personal');
  await expect(page.getByRole('heading', { name: 'No matching issues' })).toBeVisible();
  await page.getByLabel('Issue title', { exact: true }).fill('');
  await page
    .getByRole('combobox', { name: 'Repository', exact: true })
    .selectOption('work\u001facme\u001fwidget');
  await expect(page.getByRole('combobox', { name: 'Forge instance', exact: true })).toHaveValue(
    'all',
  );
  await expect(page.locator('.upstream-row')).toHaveCount(3);
  await page.getByRole('button', { name: /Needs Attention/ }).click();
  await expect(page.getByLabel('PR title', { exact: true })).toHaveValue('snapshot');
  await page.getByRole('button', { name: 'Upstream issues', exact: true }).click();
  await expect(page).toHaveURL(/view=upstream/);
  await page.reload();
  await expect(page.getByRole('combobox', { name: 'Repository', exact: true })).toHaveValue(
    'work\u001facme\u001fwidget',
  );
  await expect(page.getByLabel('Issue URL', { exact: true })).toHaveValue('');
});

test('Enter resolves a closed/unassigned URL and opens an editable deterministic copy; explicit simulation adds no state', async ({
  page,
  context: browser,
}) => {
  const api = await mockApi(browser);
  await page.goto('/?view=upstream');
  const input = page.getByLabel('Issue URL', { exact: true });
  await input.fill(closed.url);
  await input.press('Enter');
  const dialog = page.getByRole('dialog', { name: 'Create Jira ticket' });
  await expect(dialog).toContainText(warning);
  await expect(dialog).toContainText('Deterministic copy');
  await expect(dialog.getByLabel('Title', { exact: true })).toHaveValue(closed.title);
  await expect(dialog.getByLabel('Description', { exact: true })).toHaveValue(
    upstreamDraft(closed).description,
  );
  await expect(dialog.getByRole('combobox', { name: 'Project', exact: true })).toHaveValue('OPS');
  await expect(dialog.getByRole('combobox', { name: 'Assignee', exact: true })).toHaveValue(
    'Unassigned',
  );
  await expect(dialog.getByLabel('Workflow status')).toHaveValue('');
  expect(api.posts.map((p) => p.path)).toEqual([
    '/api/upstream-issues/resolve',
    '/api/upstream-issues/ticket-drafts',
  ]);
  expect(api.posts[0].body).toEqual({ source_id: null, url: closed.url });
  expect(api.posts[1].body).toEqual({ upstream_issue_id: closed.id, project_key: null });
  await dialog.getByLabel('Title', { exact: true }).fill('My edited copy');
  await dialog.getByLabel('Team').selectOption('team-platform');
  await dialog.getByLabel('Sprint', { exact: false }).selectOption('future-1');
  await dialog.getByLabel('Issue type').selectOption('Bug');
  await expect(dialog.getByLabel('Team')).toHaveValue('team-platform');
  await expect(dialog.getByLabel('Sprint', { exact: false })).toHaveValue('future-1');
  expect(api.queries.every((q) => q.searchParams.get('purpose') === 'implementation')).toBe(true);
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await dialog.getByRole('button', { name: 'Simulate creation' }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByText('Simulated: Upstream issue creation simulated.')).toBeVisible();
  await expect(input).toBeFocused();
  const create = api.posts.find((p) => p.path === '/api/upstream-issues/jira-issues')!;
  expect(create.header).toBe('1');
  expect(create.body).toEqual({
    upstream_issue_id: closed.id,
    expected_mode_revision: 'mode-test-1',
    purpose: 'implementation',
    project_key: 'OPS',
    issue_type: 'Bug',
    title: 'My edited copy',
    description: upstreamDraft(closed).description,
    story_points: null,
    assignee_id: null,
    sprint_id: 'future-1',
    team: 'team-platform',
    status: null,
  });
  expect(
    api.posts.some((p) => ['/api/jira/issues', '/api/ticket-drafts', '/api/sync'].includes(p.path)),
  ).toBe(false);
  await expect(page.locator('.upstream-row')).toHaveCount(4);
  const saved = await page.evaluate(() => localStorage.getItem('pullboard.display.v1'));
  expect(saved).not.toContain(closed.url);
  expect(saved).not.toContain('My edited copy');
  expect(saved).not.toContain('mode-test-1');
  await page.reload();
  await expect(input).toHaveValue('');
  await expect(page.getByRole('dialog')).toHaveCount(0);
});

test('URL form shows local and authoritative errors; optional instance selection resolves ambiguity', async ({
  page,
  context: browser,
}) => {
  const api = await mockApi(browser);
  await page.goto('/?view=upstream');
  const input = page.getByLabel('Issue URL', { exact: true });
  const open = page.getByRole('button', { name: 'Clone to Jira', exact: true });
  await input.fill('not a URL');
  await input.press('Enter');
  await expect(page.getByRole('alert')).toContainText('Enter a complete HTTP(S)');
  expect(api.posts).toHaveLength(0);
  for (const rejection of [
    { code: 'invalid_upstream_url', message: 'Use an issue URL on a configured instance.' },
    { code: 'not_upstream_issue', message: 'This URL is a pull request, not an issue.' },
    { code: 'repository_not_allowed', message: 'Repository is not allowed by configured policy.' },
    { code: 'ambiguous_upstream_source', message: 'Select a matching forge instance.' },
  ]) {
    api.reject(rejection);
    await input.fill('https://forge.example/acme/widget/issues/7');
    await open.click();
    await expect(page.getByRole('alert')).toHaveText(rejection.message);
    await expect(page.getByRole('dialog')).toHaveCount(0);
  }
  api.reject(null);
  await page
    .getByRole('combobox', { name: 'Lookup instance (optional)', exact: true })
    .selectOption('work');
  await open.click();
  await expect(page.getByRole('dialog')).toBeVisible();
  expect(api.posts.filter((p) => p.path.endsWith('/resolve')).at(-1)?.body.source_id).toBe('work');
});

test('mode captured before delayed lookup stays Test after a Live toggle; only explicit reviewed mode enables real create', async ({
  page,
  context: browser,
}) => {
  const api = await mockApi(browser);
  api.pauseLookup();
  await page.goto('/?view=upstream');
  await page.getByLabel('Issue URL', { exact: true }).fill(closed.url);
  await page.getByRole('button', { name: 'Clone to Jira', exact: true }).click();
  await api.lookupStarted;
  api.setMode(runtimeMode({ read_only_test_mode: false, mode_revision: 'live-2' }));
  await expect(page.getByRole('button', { name: 'Switch to Test', exact: true })).toBeVisible();
  api.release();
  const dialog = page.getByRole('dialog', { name: 'Create Jira ticket' });
  await expect(dialog.getByRole('button', { name: 'Simulate creation' })).toBeDisabled();
  await expect(dialog).toContainText('Runtime mode changed');
  await dialog.getByLabel('Title', { exact: true }).fill('Preserved gesture edits');
  expect(api.posts.filter((p) => p.path.endsWith('/jira-issues'))).toHaveLength(0);
  await dialog.getByRole('button', { name: 'Review current mode' }).click();
  await page
    .getByRole('dialog', { name: 'Review action mode' })
    .getByRole('button', { name: 'Use reviewed mode' })
    .click();
  await expect(dialog.getByLabel('Title', { exact: true })).toHaveValue('Preserved gesture edits');
  api.outcome('created');
  await dialog.getByRole('button', { name: 'Create Jira ticket', exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByRole('link', { name: 'Open OPS-23', exact: true })).toHaveAttribute(
    'href',
    'https://jira.example/browse/OPS-23',
  );
  expect(api.posts.find((p) => p.path.endsWith('/jira-issues'))?.body.expected_mode_revision).toBe(
    'live-2',
  );
  await expect(page.locator('.upstream-row')).toHaveCount(4);
  await expect(page.locator('.work-card')).toHaveCount(0);
});

test('server mode conflict preserves upstream edits and freezes revision without automatic create retry', async ({
  page,
  context: browser,
}) => {
  const api = await mockApi(browser);
  api.outcome('conflict');
  await page.goto('/?view=upstream');
  await page.getByRole('button', { name: 'Clone to Jira: Alpha issue', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Create Jira ticket' });
  await dialog.getByLabel('Title', { exact: true }).fill('Keep conflict edits');
  await dialog.getByRole('button', { name: 'Simulate creation' }).click();
  await expect(dialog).toContainText('Review the current mode');
  await expect(dialog.getByRole('button', { name: 'Simulate creation' })).toBeDisabled();
  await expect(dialog.getByLabel('Title', { exact: true })).toHaveValue('Keep conflict edits');
  expect(api.posts.filter((p) => p.path.endsWith('/jira-issues'))).toHaveLength(1);
  expect(api.posts.find((p) => p.path.endsWith('/jira-issues'))?.body.expected_mode_revision).toBe(
    'mode-test-1',
  );
  expect(api.posts.some((p) => p.path.endsWith('/resolve'))).toBe(false);
});

for (const outcome of ['unknown', 'accepted'] as const) {
  test(`upstream ${outcome} creation never invites duplicate replay or fabricates a key`, async ({
    page,
    context: browser,
  }) => {
    const api = await mockApi(browser);
    api.outcome(outcome);
    await page.goto('/?view=upstream');
    await page.getByRole('button', { name: 'Clone to Jira: Alpha issue', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Create Jira ticket' });
    await dialog.getByRole('button', { name: 'Simulate creation' }).click();
    if (outcome === 'unknown') {
      await expect(dialog).toContainText('Check Jira');
      await expect(dialog.getByRole('button', { name: 'Simulate creation' })).toBeDisabled();
    } else {
      await expect(dialog).toBeHidden();
      await expect(
        page.getByText('No confirmed key; inspect Jira, do not resubmit.', { exact: false }),
      ).toBeVisible();
      await expect(page.locator('.toast-region a')).toHaveCount(0);
    }
    expect(api.posts.filter((p) => p.path.endsWith('/jira-issues'))).toHaveLength(1);
    await expect(page.locator('.upstream-row')).toHaveCount(4);
  });
}

test('long upstream metadata wraps accessibly on narrow screens without horizontal overflow', async ({
  page,
  context: browser,
}) => {
  const long = 'VeryLongUpstreamContent'.repeat(20);
  await mockApi(browser, [
    upstreamIssue({
      title: long,
      repository: long,
      labels: [long, 'Accepted'],
      reporter: {
        id: 'reporter',
        username: long,
        display_name: long,
        url: 'https://forge.example/person',
      },
    }),
  ]);
  await page.goto('/?view=upstream');
  await page.getByRole('button', { name: 'Dark theme', exact: true }).click();
  for (const width of [390, 320]) {
    await page.setViewportSize({ width, height: 844 });
    await expect(page.locator('.upstream-row button')).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
      true,
    );
    expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  }
  await page.screenshot({ path: 'test-results/upstream-mobile-long.png', fullPage: true });
});
