import { expect, test, type BrowserContext, type Locator } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import { board, context, draft, options, pr, runtimeMode, sync } from '../src/app/testing/fixtures';
import type { RuntimeMode, SyncStatus, SourceKind } from '../src/app/core/api-types';

async function sharedApi(browser: BrowserContext, long = false) {
  let mode: RuntimeMode = runtimeMode();
  let status: SyncStatus = sync('syncing');
  let revision = 1;
  let boardReads = 0;
  let creates = 0;
  let links = 0;
  let toggles = 0;
  let actionAttempts = 0;
  const text = 'Long upstream text with no trustworthy markup. '.repeat(40);
  await browser.route('**/api/**', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (path === '/api/runtime-mode') {
      if (request.method() === 'POST') {
        const body = request.postDataJSON();
        if (body.expected_mode_revision !== mode.mode_revision)
          return route.fulfill({
            status: 409,
            json: { error: { code: 'mode_conflict', message: 'Mode changed. Review again.' } },
          });
        mode = {
          read_only_test_mode: body.read_only_test_mode,
          mode_revision: `revision-${++toggles}`,
        };
      }
      return route.fulfill({ json: mode });
    }
    if (path === '/api/context') return route.fulfill({ json: context(mode) });
    if (path === '/api/health')
      return route.fulfill({ json: { status: 'ok', version: '1.2.0', ...mode } });
    if (path === '/api/board') {
      boardReads++;
      const snapshot = board({ revision });
      if (long) {
        snapshot.issues[0].description = text;
        snapshot.issues[0].title = text;
        snapshot.pull_requests[0] = pr({
          title: text,
          description: text,
          repository: 'repository'.repeat(30),
          reviewers: Array.from({ length: 12 }, (_, i) => ({
            id: `r-${i}`,
            username: `reviewer-${i}-${'long'.repeat(20)}`,
            display_name: text,
            url: 'https://forge.example/person',
          })),
          linked_issue_keys: Array.from({ length: 20 }, (_, i) => `DEV-${i}`),
        });
      }
      return route.fulfill({ json: snapshot });
    }
    if (path === '/api/sync')
      return route.fulfill({ json: status, status: request.method() === 'POST' ? 202 : 200 });
    if (path === '/api/ticket-drafts') return route.fulfill({ json: draft() });
    if (path === '/api/jira/options') return route.fulfill({ json: options() });
    if (path === '/api/jira/issues' || path.endsWith('/links')) {
      actionAttempts++;
      const body = request.postDataJSON();
      if (body.expected_mode_revision !== mode.mode_revision)
        return route.fulfill({
          status: 409,
          json: { error: { code: 'mode_conflict', message: 'Mode changed. Review again.' } },
        });
      if (path.endsWith('/links')) {
        links++;
        return route.fulfill({
          json: {
            outcome: mode.read_only_test_mode ? 'simulated' : 'linked',
            issue_key: 'DEV-1',
            pull_request_id: pr().id,
            message: 'Explicit link accepted.',
          },
        });
      }
      creates++;
      return route.fulfill({
        json: mode.read_only_test_mode
          ? { outcome: 'simulated', issue: null, message: 'Test creation simulated.', warnings: [] }
          : {
              outcome: 'created',
              issue: null,
              message: 'Jira accepted creation; inspect Jira before resubmitting.',
              warnings: ['No confirmed key. Do not create again.'],
            },
      });
    }
    return route.fulfill({
      status: 404,
      json: { error: { code: 'not_found', message: 'Unknown path.' } },
    });
  });
  return {
    setMode: (value: RuntimeMode) => {
      mode = value;
    },
    progress: (value: SyncStatus, nextRevision = revision) => {
      status = value;
      revision = nextRevision;
    },
    counts: () => ({ boardReads, creates, links, toggles, actionAttempts }),
  };
}

async function cardGeometry(card: Locator) {
  await card.scrollIntoViewIfNeeded();
  return card.evaluate((element) => {
    const bounds = element.getBoundingClientRect();
    const overflowPx = { top: 0, right: 0, bottom: 0, left: 0 };
    let clippedElements = 0;
    let hiddenElements = 0;
    const scrollContainers: string[] = [];
    const truncatingElements: string[] = [];
    for (const child of [element, ...element.querySelectorAll('*')]) {
      // Only the baseline two-line description preview may be clipped.
      if (child.closest('.card-preview')) continue;
      const css = getComputedStyle(child);
      const name = `${child.tagName.toLowerCase()}.${child.className}`;
      if (/auto|scroll|hidden|clip/.test(`${css.overflowX} ${css.overflowY}`))
        scrollContainers.push(name);
      if (css.textOverflow === 'ellipsis' || !['', 'none', '0'].includes(css.webkitLineClamp))
        truncatingElements.push(name);
      if (css.display === 'contents') continue;
      if (css.display === 'none' || css.visibility !== 'visible' || css.opacity === '0') {
        hiddenElements++;
        continue;
      }
      const rects = [...child.getClientRects()];
      if (!rects.length) hiddenElements++;
      let clipped = false;
      for (const rect of rects) {
        if (!rect.width || !rect.height) continue;
        const excess = {
          top: Math.max(0, bounds.top - rect.top),
          right: Math.max(0, rect.right - bounds.right),
          bottom: Math.max(0, rect.bottom - bounds.bottom),
          left: Math.max(0, bounds.left - rect.left),
        };
        for (const side of ['top', 'right', 'bottom', 'left'] as const) {
          overflowPx[side] = Math.max(overflowPx[side], Number(excess[side].toFixed(2)));
          if (excess[side] > 0) clipped = true;
        }
      }
      if (clipped) clippedElements++;
    }
    return {
      height: bounds.height,
      width: bounds.width,
      overflowPx,
      clippedElements,
      hiddenElements,
      scrollContainers,
      truncatingElements,
      // content-visibility:auto implicitly paint-clips overflowing descendants;
      // DOM presence and Playwright's toBeVisible alone cannot prove containment.
      hostContentVisibility: getComputedStyle(element.parentElement!).contentVisibility,
      hostOverflowPx: Math.max(
        0,
        Number((bounds.bottom - element.parentElement!.getBoundingClientRect().bottom).toFixed(2)),
      ),
    };
  });
}

const containedCard = {
  overflowPx: { top: 0, right: 0, bottom: 0, left: 0 },
  clippedElements: 0,
  hiddenElements: 0,
  scrollContainers: [],
  truncatingElements: [],
  hostOverflowPx: 0,
};

test('two tabs invalidate edited ticket and require explicit reviewed mode; accepted/null is success', async ({
  context: browser,
  page,
}) => {
  const api = await sharedApi(browser);
  const other = await browser.newPage();
  await Promise.all([page.goto('/'), other.goto('/')]);
  await page.getByRole('button', { name: 'Create review ticket' }).click();
  const ticket = page.getByRole('dialog', { name: 'Create review ticket' });
  await ticket.getByLabel('Title', { exact: true }).fill('Preserved cross-tab edits');
  await other.getByRole('button', { name: 'Switch to Live', exact: true }).click();
  const confirmation = other.getByRole('dialog');
  await expect(confirmation).toContainText('Admitted live actions may finish');
  expect(api.counts().toggles).toBe(0);
  expect((await new AxeBuilder({ page: other }).analyze()).violations).toEqual([]);
  await confirmation.getByRole('button', { name: 'Enable Live writes', exact: true }).click();
  await expect(ticket.getByRole('button', { name: 'Simulate creation' })).toBeDisabled();
  await expect(ticket.getByLabel('Title', { exact: true })).toHaveValue(
    'Preserved cross-tab edits',
  );
  expect(api.counts().creates).toBe(0);
  await ticket.getByRole('button', { name: 'Review current mode' }).click();
  const review = page.getByRole('dialog', { name: 'Review action mode' });
  await expect(review).toContainText('Live mode');
  await review.getByRole('button', { name: 'Use reviewed mode' }).click();
  await ticket.getByRole('button', { name: 'Create Jira ticket', exact: true }).click();
  await expect(ticket).toBeHidden();
  await expect(
    page.getByText('Jira accepted creation; inspect Jira before resubmitting.'),
  ).toBeVisible();
  await expect(page.getByText('No confirmed key. Do not create again.')).toBeVisible();
  expect(api.counts().creates).toBe(1);
  await expect(page.locator('.jira-card')).toHaveCount(1);
});

test('stale Live confirmation and link selections survive test/live/test round trip and restart', async ({
  context: browser,
  page,
}) => {
  const api = await sharedApi(browser);
  await page.goto('/');
  await page.getByRole('button', { name: 'Switch to Live', exact: true }).click();
  api.setMode(runtimeMode({ mode_revision: 'new-boot-test' }));
  await expect(
    page.getByRole('dialog').getByRole('button', { name: 'Enable Live writes' }),
  ).toBeDisabled();
  expect(api.counts().toggles).toBe(0);
  await page.getByRole('dialog').getByRole('button', { name: 'Cancel' }).click();
  await page.locator('.pr-card').getByRole('button', { name: 'Link to Jira', exact: true }).click();
  const link = page.getByRole('dialog', { name: 'Link PR to Jira' });
  await link.getByLabel('Jira ticket', { exact: true }).selectOption('DEV-1');
  api.setMode(runtimeMode({ read_only_test_mode: false, mode_revision: 'live-trip' }));
  await expect(link.getByRole('button', { name: 'Simulate link' })).toBeDisabled();
  api.setMode(runtimeMode({ mode_revision: 'test-trip' }));
  await expect(link.getByRole('button', { name: 'Simulate link' })).toBeDisabled();
  await expect(link.getByLabel('Jira ticket', { exact: true })).toHaveValue('DEV-1');
  expect(api.counts().links).toBe(0);
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
});

test('a conflict before polling preserves the editor and does not auto retry with the refreshed token', async ({
  context: browser,
  page,
}) => {
  const api = await sharedApi(browser);
  await page.goto('/');
  await page.getByRole('button', { name: 'Create review ticket' }).click();
  const ticket = page.getByRole('dialog', { name: 'Create review ticket' });
  await ticket.getByLabel('Title', { exact: true }).fill('Never replace these edits');
  api.setMode(runtimeMode({ read_only_test_mode: false, mode_revision: 'not-yet-polled' }));
  await ticket.getByRole('button', { name: 'Simulate creation' }).click();
  await expect(ticket.getByText('Mode changed. Review again.', { exact: true })).toBeVisible();
  await expect(ticket.getByRole('button', { name: 'Simulate creation' })).toBeDisabled();
  await expect(ticket.getByLabel('Title', { exact: true })).toHaveValue(
    'Never replace these edits',
  );
  await expect(ticket.getByRole('button', { name: 'Review current mode' })).toBeVisible();
  expect(api.counts().actionAttempts).toBe(1);
  expect(api.counts().creates).toBe(0);
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
});

test('a drag freezes mode at pointer down and cannot silently upgrade to a live link', async ({
  context: browser,
  page,
}) => {
  const api = await sharedApi(browser);
  await page.goto('/');
  const handle = await page.locator('.pr-card .drag-handle').boundingBox();
  const target = await page.locator('.jira-card').boundingBox();
  await page.mouse.move(handle!.x + handle!.width / 2, handle!.y + handle!.height / 2);
  await page.mouse.down();
  api.setMode(runtimeMode({ read_only_test_mode: false, mode_revision: 'live-during-gesture' }));
  await expect(page.getByRole('button', { name: 'Switch to Test', exact: true })).toBeVisible();
  await page.mouse.move(handle!.x - 15, handle!.y + 15, { steps: 5 });
  await page.mouse.move(target!.x + target!.width / 2, target!.y + 130, { steps: 20 });
  await page.mouse.up();
  await expect(
    page.getByText(
      'Runtime mode changed. Use Link PR to review the current mode and submit explicitly.',
    ),
  ).toBeVisible();
  expect(api.counts().actionAttempts).toBe(0);
});

test('healthy idle source counters collapse into keyboard-accessible neutral coverage details', async ({
  context: browser,
  page,
}) => {
  const api = await sharedApi(browser);
  const status = sync();
  const kinds: SourceKind[] = ['jira', 'github', 'gitlab', 'forgejo', 'codeberg'];
  status.sources = kinds.map((kind) => ({
    ...status.sources[0],
    kind,
    source_id: kind,
    name: `Mock ${kind}`,
    coverage_notes: ['Mock API history scope; not an upstream coverage claim.'],
  }));
  status.progress = kinds.map((kind, i) => ({
    ...status.progress[0],
    source_id: kind,
    completed_items: i,
    total_items: i,
  }));
  api.progress(status);
  await page.goto('/');
  await expect(page.locator('.source-progress-grid')).toHaveCount(0);
  await expect(page.locator('.source-failed')).toHaveCount(0);
  await expect(page.getByText('All sources up to date', { exact: true })).toBeVisible();
  await expect(page.locator('.progress-history').first()).not.toBeVisible();
  await page.screenshot({ path: 'test-results/all-sources-success-collapsed.png', fullPage: true });
  await page.locator('.health-panel summary').focus();
  await page.keyboard.press('Enter');
  await expect(
    page
      .getByRole('region', { name: 'Mock jira health', exact: true })
      .locator('.progress-history'),
  ).toHaveText('completed: 0 / 0 evaluated / 1s');
  await expect(
    page
      .getByRole('region', { name: 'Mock codeberg health', exact: true })
      .locator('.progress-history'),
  ).toHaveText('completed: 4 / 4 evaluated / 1s');
  await expect(page.locator('.coverage-note')).toHaveCount(5);
  await expect(page.locator('.health-panel .chip.ready')).toHaveCount(5);
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await page.screenshot({ path: 'test-results/all-sources-success.png', fullPage: true });
  api.progress({
    ...status,
    state: 'syncing',
    started_at: '2026-11-06T13:00:00Z',
    finished_at: null,
    progress: status.progress.map((p) => ({
      ...p,
      phase: 'fetching',
      total_items: null,
      started_at: '2026-11-06T13:00:00Z',
      finished_at: null,
      elapsed_ms: 1250,
    })),
  });
  await page.getByRole('button', { name: 'Refresh', exact: true }).click();
  await expect(page.locator('.source-progress-grid .source-progress')).toHaveCount(5);
  await expect(page.locator('.source-progress-grid progress')).toHaveCount(5);
  await expect(
    page.getByRole('region', { name: 'Mock codeberg refresh', exact: true }),
  ).toContainText('4 evaluated / total unknown');
  await page.screenshot({ path: 'test-results/all-sources-active.png', fullPage: true });
});

test('honest per-source progress retains old cards and fetches missed follow-up publication', async ({
  context: browser,
  page,
}) => {
  const api = await sharedApi(browser);
  await page.goto('/');
  const status = sync('syncing');
  status.progress[0] = {
    ...status.progress[0],
    phase: 'fetching',
    completed_items: 7,
    total_items: null,
    started_at: status.started_at,
    elapsed_ms: 65000,
  };
  api.progress(status);
  await expect(
    page.locator('.source-progress-grid').getByText('7 evaluated / total unknown'),
  ).toBeVisible();
  await expect(page.getByRole('progressbar')).not.toHaveAttribute('value');
  await expect(page.getByText('1m 5s', { exact: true })).toBeVisible();
  await page.screenshot({ path: 'test-results/source-progress.png', fullPage: true });
  expect(api.counts().boardReads).toBe(1);
  await expect(page.locator('.work-card')).toHaveCount(2);
  api.progress({
    ...status,
    progress: [{ ...status.progress[0], phase: 'publishing', total_items: 7 }],
  });
  await expect(page.getByText('Fetched; waiting for atomic publication.')).toBeVisible();
  expect(api.counts().boardReads).toBe(1);
  const failed = {
    ...status,
    sources: [
      { ...status.sources[0], status: 'error' as const, error: 'Actual upstream read failed.' },
    ],
    progress: [
      { ...status.progress[0], phase: 'failed' as const, finished_at: '2026-11-06T12:00:02Z' },
    ],
  };
  api.progress(failed);
  await expect(page.locator('.source-failed')).toContainText('Actual upstream read failed.');
  await page.screenshot({ path: 'test-results/source-progress-failed.png', fullPage: true });
  expect(api.counts().boardReads).toBe(1);
  api.progress({ ...sync('syncing'), started_at: '2026-11-06T12:05:00Z' }, 2);
  await expect(page.getByText('Snapshot 2 / Updated', { exact: false })).toBeVisible();
  expect(api.counts().boardReads).toBe(2);
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
});

test('natural-height flat cards fully contain adversarial upstream content and actions without scrolling or disclosures', async ({
  context: browser,
  page,
}) => {
  await sharedApi(browser, true);
  await page.goto('/');
  await page.evaluate(() => document.fonts.ready);
  const pull = page.locator('.pr-card');
  const issue = page.locator('.jira-card');
  const longText = 'Long upstream text with no trustworthy markup. '.repeat(40);
  await expect(pull.locator('.card-title a')).toHaveText(longText);
  await expect(issue.locator('.card-title a')).toHaveText(longText);
  await expect(pull.locator('.card-meta a').filter({ hasText: 'repositoryrepository' })).toHaveText(
    `acme/${'repository'.repeat(30)}`,
  );
  await expect(pull.locator('.card-people dd').nth(1).locator('a')).toHaveText(
    Array.from({ length: 12 }, (_, i) => `reviewer-${i}-${'long'.repeat(20)}`).sort((a, b) =>
      a.localeCompare(b),
    ),
  );
  await expect(pull.locator('.linked-items pb-link')).toHaveText(
    Array.from({ length: 20 }, (_, i) => `DEV-${i}`),
  );
  await expect(pull.getByRole('button', { name: 'Link to Jira', exact: true })).toBeEnabled();
  await expect(
    pull.getByRole('button', { name: 'Create review ticket', exact: true }),
  ).toBeEnabled();
  await expect(issue.getByRole('button', { name: 'Link pull request', exact: true })).toBeEnabled();
  await expect(pull.locator('.card-tags .card-tag')).toHaveText([
    'Ready for review',
    'backend',
    'cache',
  ]);
  for (const theme of ['Light theme', 'Dark theme']) {
    await page.getByRole('button', { name: theme, exact: true }).click();
    for (const [name, card] of [
      ['PR', pull],
      ['Jira', issue],
    ] as const) {
      await expect(card.locator('details, summary')).toHaveCount(0);
      await expect(card.locator('.card-title')).toBeVisible();
      await expect(card.locator('.card-preview')).toBeVisible();
      const geometry = await cardGeometry(card);
      expect(geometry.height).toBeGreaterThan(300);
      await test.info().attach(`long-${name}-${theme}-geometry.json`, {
        body: JSON.stringify(geometry, null, 2),
        contentType: 'application/json',
      });
      expect
        .soft(
          geometry,
          `${name}: every non-preview element and action must fit; content-visibility:auto paint-clips overflow`,
        )
        .toMatchObject(containedCard);
    }
    expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
    await page.screenshot({
      path: `test-results/flat-long-${theme === 'Light theme' ? 'light' : 'dark'}.png`,
      fullPage: true,
    });
  }
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
});

test('Upstream Pull Requests shows original-style flat cards with natural heights, equal widths and direct keyboard navigation', async ({
  context: browser,
  page,
}) => {
  const api = await sharedApi(browser);
  api.progress(sync());
  await page.goto('/');
  await page.getByRole('button', { name: 'Upstream Pull Requests', exact: true }).click();
  const issue = page.locator('.jira-card');
  const pull = page.locator('.pr-card');
  await expect(issue.locator('.card-meta a')).toHaveText('DEV-1');
  await expect(issue.locator('.issue-type')).toContainText('Task');
  await expect(issue.locator('.status-chip')).toHaveText('In progress');
  await expect(issue.locator('.linked-items h4')).toHaveText('Linked pull requests');
  await expect(issue.locator('.linked-items')).toContainText('None linked');
  await expect(issue.locator('.card-preview')).toBeVisible();
  await expect(pull.locator('.card-meta a').filter({ hasText: 'acme/widget' })).toHaveAttribute(
    'href',
    'https://forge.example/acme/widget',
  );
  await expect(pull.locator('.card-people dd').nth(0).locator('a')).toHaveText('alex');
  await expect(pull.locator('.card-people dd').nth(1)).toHaveText('None');
  await expect(pull.locator('.card-people dd').nth(2)).toHaveText('Unassigned');
  await expect(pull.locator('.card-preview')).toBeVisible();
  await expect(page.locator('.work-card details, .work-card summary')).toHaveCount(0);
  await expect(pull.getByRole('button', { name: 'Link to Jira', exact: true })).toBeEnabled();
  await expect(pull.getByRole('button', { name: 'Create Jira ticket', exact: true })).toBeEnabled();
  await expect(
    pull.getByRole('button', { name: 'Create review ticket', exact: true }),
  ).toBeEnabled();
  await expect(issue.getByRole('button', { name: 'Link pull request', exact: true })).toBeEnabled();
  await expect(page.locator('.source-progress-grid')).toHaveCount(0);
  for (const card of [issue, pull]) {
    await expect(card.locator('.card-meta')).toBeInViewport();
    await expect(card.locator('.card-preview')).toBeInViewport();
    await expect(card.locator('.card-footer')).toBeInViewport();
    const geometry = await cardGeometry(card);
    expect(geometry.height).toBeLessThan(300);
    expect(geometry).toMatchObject(containedCard);
  }
  expect((await issue.boundingBox())!.width).toBe((await pull.boundingBox())!.width);
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await page.screenshot({ path: 'test-results/overview-light.png', fullPage: true });
  await page.getByRole('button', { name: 'Dark theme', exact: true }).click();
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await page.screenshot({ path: 'test-results/overview-dark.png', fullPage: true });
  const repo = pull.getByRole('link', { name: 'acme/widget', exact: true });
  const pullHeight = (await pull.boundingBox())!.height;
  await pull.locator('.card-title a').focus();
  await page.keyboard.press('Tab');
  await expect(repo).toBeFocused();
  await expect(repo).toBeInViewport();
  expect((await pull.boundingBox())!.height).toBe(pullHeight);
  await pull.getByRole('button', { name: 'Link to Jira', exact: true }).focus();
  await page.keyboard.press('Tab');
  await expect(
    pull.getByRole('button', { name: 'Create review ticket', exact: true }),
  ).toBeFocused();
  await page.keyboard.press('Tab');
  await expect(pull.getByRole('button', { name: 'Create Jira ticket', exact: true })).toBeFocused();
  await page.setViewportSize({ width: 390, height: 844 });
  await pull.locator('.card-title a').focus();
  await page.keyboard.press('Tab');
  await expect(repo).toBeFocused();
  await expect(repo).toBeInViewport();
  for (const card of [issue, pull]) expect(await cardGeometry(card)).toMatchObject(containedCard);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
});
