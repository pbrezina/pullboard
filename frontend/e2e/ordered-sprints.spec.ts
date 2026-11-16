import { expect, test } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import {
  draft,
  orderedSprintScenario,
  runtimeMode,
  sync,
  upstreamDraft,
} from '../src/app/testing/fixtures';

for (const editor of ['implementation', 'review', 'upstream'] as const) {
  test(`${editor} editor keeps committed pattern-allowed sprints separate from full multi-board filters`, async ({
    page,
  }) => {
    const purpose = editor === 'review' ? 'review' : 'implementation';
    const scenario = orderedSprintScenario(purpose);
    let snapshot = scenario.board;
    let context = scenario.context;
    let status = sync('syncing');
    let optionReads = 0;
    const submissions: { path: string; body: Record<string, unknown> }[] = [];
    await page.route('**/api/**', (route) => {
      const request = route.request();
      const path = new URL(request.url()).pathname;
      switch (path) {
        case '/api/runtime-mode':
          return route.fulfill({ json: runtimeMode() });
        case '/api/health':
          return route.fulfill({ json: { status: 'ok', version: '1.7.0', ...runtimeMode() } });
        case '/api/context':
          return route.fulfill({ json: context });
        case '/api/board':
          return route.fulfill({ json: snapshot });
        case '/api/sync':
          return route.fulfill({ json: status });
        case '/api/jira/options':
          optionReads++;
          return route.fulfill({ json: scenario.options });
        case '/api/ticket-drafts':
          return route.fulfill({ json: draft(purpose) });
        case '/api/upstream-issues/ticket-drafts':
          return route.fulfill({ json: upstreamDraft(snapshot.upstream_issues[0]) });
        case '/api/jira/issues':
        case '/api/upstream-issues/jira-issues':
          submissions.push({ path, body: request.postDataJSON() });
          return route.fulfill({
            json: {
              outcome: 'simulated',
              issue: null,
              message: 'Ticket creation simulated.',
              warnings: [],
            },
          });
        default:
          return route.fulfill({
            status: 404,
            json: { error: { code: 'not_found', message: 'Unknown mock API route.' } },
          });
      }
    });
    await page.goto('/?view=overview');
    const header = page.getByRole('region', { name: 'Who am I' });
    await expect(header.locator('.sprint-list a')).toHaveCount(1);
    await expect(header.getByRole('link', { name: scenario.primary.name })).toHaveAttribute(
      'href',
      'https://jira.example/secure/RapidBoard.jspa?rapidView=12&selectedSprint=sprint-1&view=planning',
    );
    await expect(header).not.toContainText(scenario.secondaryIdentity.name);
    await expect(header).not.toContainText(scenario.core.name);
    const filter = page.getByRole('combobox', { name: 'Sprint', exact: true });
    await expect(filter).toHaveValue(scenario.primary.id);
    for (const sprint of scenario.board.sprints) {
      await expect(filter.locator(`option[value="${sprint.id}"]`)).toHaveCount(1);
    }
    // Display filtering is independent of the server's creation-name policy.
    await filter.selectOption(scenario.excluded.id);
    if (editor === 'upstream') {
      await page.getByRole('button', { name: 'Upstream issues', exact: true }).click();
      await page
        .getByRole('button', { name: `Clone to Jira: ${snapshot.upstream_issues[0].title}` })
        .click();
    } else {
      await page
        .locator('.pr-card')
        .getByRole('button', {
          name: editor === 'review' ? 'Create review ticket' : 'Create Jira ticket',
          exact: true,
        })
        .click();
    }
    const dialog = page.getByRole('dialog');
    const sprint = dialog.locator('#ticket-sprint');
    await expect(sprint).toHaveValue('');
    await expect(dialog.locator('#ticket-assignee')).toHaveValue('Unassigned');
    await expect(dialog.locator('#ticket-status')).toHaveValue('');
    await expect(sprint.locator('option')).toHaveCount(scenario.options.sprints.length + 1);
    await expect(sprint.locator(`option[value="${scenario.excluded.id}"]`)).toHaveCount(0);
    await expect(sprint.locator('optgroup[label="Future"] option')).toHaveText([
      scenario.future.name,
    ]);
    await sprint.selectOption(scenario.core.id);
    await dialog.getByLabel('Title', { exact: true }).fill('Keep explicit CORE selection');

    // A new ticket snapshot can omit CORE while retained options still allow it.
    snapshot = { ...snapshot, revision: 2, sprints: [scenario.primary, scenario.excluded] };
    context = { ...context, current_sprints: [] };
    status = {
      ...status,
      board_revision: 2,
      jira_options: {
        ...status.jira_options!,
        health: {
          ...status.jira_options!.health,
          status: 'error',
          error: 'Options refresh failed.',
        },
        progress: {
          ...status.jira_options!.progress,
          phase: 'failed',
          finished_at: '2026-11-06T12:01:00Z',
        },
      },
    };
    await expect(dialog.getByText('Using cached Jira choices;', { exact: false })).toBeVisible();
    await expect(sprint).toHaveValue(scenario.core.id);
    await expect(dialog.getByLabel('Title', { exact: true })).toHaveValue(
      'Keep explicit CORE selection',
    );
    await expect(sprint.locator(`option[value="${scenario.excluded.id}"]`)).toHaveCount(0);
    expect(optionReads).toBe(1);
    expect(submissions).toHaveLength(0);
    expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
    await dialog.getByRole('button', { name: 'Simulate creation' }).click();
    await expect(dialog).toBeHidden();
    expect(submissions).toEqual([
      {
        path: editor === 'upstream' ? '/api/upstream-issues/jira-issues' : '/api/jira/issues',
        body: expect.objectContaining({
          sprint_id: scenario.core.id,
          expected_mode_revision: 'mode-test-1',
          assignee_id: null,
          status: null,
        }),
      },
    ]);
    if (editor === 'upstream')
      await page.getByRole('button', { name: 'Upstream Pull Requests', exact: true }).click();
    await expect(filter).toHaveValue(scenario.excluded.id);
    await expect(header).toContainText('No matching active sprint');
  });
}
