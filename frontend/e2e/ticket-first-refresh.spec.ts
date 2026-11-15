import { expect, test } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import {
  board,
  context,
  draft,
  jiraOptionsStatus,
  options,
  runtimeMode,
  sync,
} from '../src/app/testing/fixtures';
import type { Board, Context, SyncStatus } from '../src/app/core/api-types';

test('mid-job tickets, indeterminate groups, exact project checks and independent readiness remain accessible', async ({
  page,
}) => {
  let snapshot: Board = board({
    revision: 0,
    generated_at: null,
    issues: [],
    pull_requests: [],
    sprints: [],
  });
  let ctx: Context = context({
    identities: [],
    current_project: null,
    current_sprints: [],
    jira_projects: [],
  });
  let status: SyncStatus = { ...sync('syncing'), board_revision: 0 };
  const unavailable = jiraOptionsStatus('syncing');
  unavailable.ready = false;
  unavailable.health = { ...unavailable.health, status: 'pending', last_success_at: null };
  status.jira_options = unavailable;
  let boardReads = 0;
  let optionReads = 0;
  const submissions: Record<string, unknown>[] = [];
  await page.route('**/api/**', (route) => {
    const req = route.request();
    switch (new URL(req.url()).pathname) {
      case '/api/runtime-mode':
        return route.fulfill({ json: runtimeMode() });
      case '/api/health':
        return route.fulfill({ json: { status: 'ok', version: '1.5', ...runtimeMode() } });
      case '/api/context':
        return route.fulfill({ json: ctx });
      case '/api/sync':
        return route.fulfill({ json: status, status: req.method() === 'POST' ? 202 : 200 });
      case '/api/board':
        boardReads++;
        return route.fulfill({ json: snapshot });
      case '/api/jira/options':
        optionReads++;
        return route.fulfill({ json: options() });
      case '/api/ticket-drafts':
        return route.fulfill({ json: draft() });
      case '/api/jira/issues':
        submissions.push(req.postDataJSON());
        return route.fulfill({
          json: {
            outcome: 'simulated',
            issue: null,
            warnings: [],
            message: 'Ticket creation simulated.',
          },
        });
      default:
        return route.fulfill({
          status: 404,
          json: { error: { code: 'not_found', message: 'Unknown path.' } },
        });
    }
  });
  await page.goto('/');
  await expect(page.getByText('Snapshot 0 / Updated', { exact: false })).toBeVisible();
  snapshot = board({ revision: 2 });
  ctx = context({
    current_project: { key: 'DEV', name: 'Development', issue_types: [] },
    jira_projects: [{ key: 'DEV', name: 'Development', issue_types: [] }],
  });
  status = {
    ...sync('syncing'),
    board_revision: 2,
    progress: sync().progress,
    jira_options: {
      ...unavailable,
      progress: {
        ...unavailable.progress,
        phase: 'discovering',
        started_at: status.started_at,
        elapsed_ms: 3000,
        groups: [
          { name: 'Reviewers', phase: 'discovering', completed_items: 0, total_items: null },
        ],
        projects: [{ name: 'DEV', phase: 'queued', completed_items: 0, total_items: null }],
      },
    },
  };
  await expect(page.getByText('Snapshot 2 / Updated', { exact: false })).toBeVisible();
  await expect(page.locator('.work-card')).toHaveCount(2);
  const create = page.getByRole('button', { name: 'Create review ticket', exact: true });
  await expect(create).toBeDisabled();
  await expect(page.getByRole('region', { name: 'Jira refresh', exact: true })).toContainText(
    'completed',
  );
  const choices = page.locator('.jira-options');
  await choices.locator('summary').focus();
  await page.keyboard.press('Enter');
  await expect(
    choices.getByRole('progressbar', { name: 'Reviewers group membership' }),
  ).not.toHaveAttribute('value');
  await expect(
    choices.getByRole('progressbar', { name: 'Jira creation metadata and statuses' }),
  ).toBeVisible();
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  const reads = boardReads;
  status = {
    ...status,
    jira_options: {
      ...status.jira_options!,
      progress: {
        ...status.jira_options!.progress,
        phase: 'fetching',
        elapsed_ms: 65000,
        groups: [{ name: 'Reviewers', phase: 'completed', completed_items: 75, total_items: 75 }],
        projects: [{ name: 'DEV', phase: 'fetching', completed_items: 28, total_items: 75 }],
      },
    },
  };
  const checks = choices.getByRole('progressbar', { name: 'DEV project eligibility checks' });
  await expect(checks).toHaveAttribute('value', '28');
  await expect(checks).toHaveAttribute('max', '75');
  await expect(choices.getByText('28 / 75 checks finished')).toBeVisible();
  expect(boardReads).toBe(reads);
  expect(optionReads).toBe(0);
  await page.getByRole('button', { name: 'Dark theme', exact: true }).click();
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await page.screenshot({ path: 'test-results/ticket-first-options-progress.png', fullPage: true });

  // Initial failed options do not undo the completed ticket snapshot.
  status = {
    ...status,
    board_revision: 3,
    jira_options: {
      ...status.jira_options!,
      health: {
        ...unavailable.health,
        status: 'error',
        error: 'Eligibility read failed.',
      },
      progress: {
        ...status.jira_options!.progress,
        phase: 'failed',
        finished_at: '2026-11-06T12:01:00Z',
        projects: [{ name: 'DEV', phase: 'failed', completed_items: 28, total_items: 75 }],
      },
    },
  };
  snapshot = board({ revision: 3 });
  await expect(page.getByText('Snapshot 3 / Updated', { exact: false })).toBeVisible();
  await expect(choices).toContainText('No usable choices');
  await expect(create).toBeDisabled();
  await expect(page.locator('.work-card')).toHaveCount(2);
  await expect(page.locator('.source-progress-grid .source-failed')).toHaveCount(0);

  status = {
    ...sync('syncing'),
    board_revision: 4,
    progress: sync().progress,
    jira_options: jiraOptionsStatus(),
  };
  snapshot = board({ revision: 4 });
  ctx = context();
  await expect(create).toBeEnabled();
  await expect(page.getByText('Snapshot 4 / Updated', { exact: false })).toBeVisible();
  await page.getByLabel('Jira title', { exact: true }).fill('cache');
  await page.getByLabel('PR title', { exact: true }).fill('snapshot');
  await create.click();
  const editor = page.getByRole('dialog', { name: 'Create review ticket' });
  await editor.getByLabel('Title', { exact: true }).fill('Keep my edited title');
  await editor.getByLabel('Workflow status', { exact: false }).selectOption('Review');
  status = {
    ...status,
    board_revision: 5,
    jira_options: {
      ...jiraOptionsStatus(),
      health: { ...jiraOptionsStatus().health, status: 'error', error: 'Options refresh failed.' },
      progress: { ...jiraOptionsStatus().progress, phase: 'failed' },
    },
  };
  snapshot = board({ revision: 5 });
  ctx = context({ current_sprints: [] });
  await expect(editor.getByText('Using cached Jira choices;', { exact: false })).toBeVisible();
  await expect(editor.getByLabel('Title', { exact: true })).toHaveValue('Keep my edited title');
  await expect(editor.getByLabel('Workflow status', { exact: false })).toHaveValue('Review');
  await expect(editor.getByRole('button', { name: 'Simulate creation' })).toBeEnabled();
  expect(optionReads).toBe(1);
  expect(submissions).toHaveLength(0);
  await editor.getByRole('button', { name: 'Simulate creation' }).click();
  await expect(editor).toBeHidden();
  expect(submissions).toHaveLength(1);
  expect(submissions[0]).toMatchObject({
    title: 'Keep my edited title',
    status: 'Review',
    expected_mode_revision: 'mode-test-1',
  });
  await expect(page.getByLabel('Jira title', { exact: true })).toHaveValue('cache');
  await expect(page.getByLabel('PR title', { exact: true })).toHaveValue('snapshot');
  await expect(page.getByText('Snapshot 5 / Updated', { exact: false })).toBeVisible();
});
