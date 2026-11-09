import { expect, test, type Page } from '@playwright/test';
import AxeBuilder from '@axe-core/playwright';
import {
  attention,
  board,
  context,
  draft,
  issue,
  options,
  person,
  pr,
  sync,
  runtimeMode,
  sprint,
} from '../src/app/testing/fixtures';

const own = pr({
  title: 'Publish atomic snapshots',
  attention: {
    ...attention(),
    reasons: ['author_feedback'],
    threads: [{ ...attention().threads[0], reason: 'author_feedback' }],
  },
});
const review = pr({
  id: 'personal:acme/widget:43',
  source_id: 'personal',
  number: 43,
  title: 'Retain last known good data',
  is_mine: false,
  author: { ...person, username: 'sam' },
  attention: {
    ...attention(),
    reasons: ['review_request'],
    threads: [{ ...attention().threads[0], reason: 'review_request' }],
  },
});
const snapshot = board({
  issues: [
    issue(),
    issue({
      id: 'jira:DEV-2',
      key: 'DEV-2',
      title: 'Mention-only triage',
      is_assigned_to_me: false,
      sprint_ids: [],
    }),
  ],
  pull_requests: [own, review],
});

async function mockApi(
  page: Page,
  overrides: {
    empty?: boolean;
    failure?: boolean;
    degraded?: boolean;
    snapshot?: ReturnType<typeof board>;
    liveLink?: { issueKey: string; prId: string };
    deferRefreshedBoard?: boolean;
  } = {},
) {
  const current = structuredClone(overrides.snapshot ?? snapshot);
  let pendingLiveLink = false;
  let refreshRequested = false;
  let releaseRefreshedBoard!: () => void;
  const refreshedBoard = new Promise<void>((resolve) => (releaseRefreshedBoard = resolve));
  if (overrides.degraded)
    current.sources[0] = {
      ...current.sources[0],
      status: 'backoff',
      error: 'Source rate limit reached.',
      retry_at: '2026-11-06T23:00:00Z',
      coverage_notes: ['Older review history is unavailable.'],
    };
  const writes: { url: string; body: Record<string, unknown>; headers: Record<string, string> }[] =
    [];
  await page.route('**/api/**', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (request.method() === 'POST') {
      writes.push({ url: url.pathname, body: request.postDataJSON(), headers: request.headers() });
      if (url.pathname === '/api/ticket-drafts')
        return route.fulfill({
          json: { ...draft(request.postDataJSON().purpose), pull_request_id: own.id },
        });
      if (url.pathname === '/api/jira/issues')
        return route.fulfill({
          json: {
            outcome: 'simulated',
            issue: null,
            message: 'Ticket creation simulated (read-only mode).',
            warnings: [],
          },
        });
      if (url.pathname.endsWith('/links')) {
        if (overrides.liveLink) {
          pendingLiveLink = true;
          return route.fulfill({
            json: {
              outcome: 'linked',
              issue_key: overrides.liveLink.issueKey,
              pull_request_id: overrides.liveLink.prId,
              message: 'Pull request linked.',
            },
          });
        }
        return route.fulfill({
          json: {
            outcome: 'simulated',
            issue_key: 'DEV-1',
            pull_request_id: own.id,
            message: 'Linking simulated (read-only mode).',
          },
        });
      }
      if (url.pathname === '/api/sync') {
        if (pendingLiveLink && overrides.liveLink) {
          const issue = current.issues.find((item) => item.key === overrides.liveLink!.issueKey)!;
          issue.linked_pr_ids = [...new Set([...issue.linked_pr_ids, overrides.liveLink.prId])];
          pendingLiveLink = false;
        }
        refreshRequested = true;
        return route.fulfill({ status: 202, json: sync('syncing') });
      }
      return route.fulfill({ status: 202, json: sync('syncing') });
    }
    if (overrides.failure)
      return route.fulfill({
        status: 503,
        json: {
          error: {
            code: 'source_not_ready',
            message: 'Local server is unavailable. Retry shortly.',
          },
        },
      });
    switch (url.pathname) {
      case '/api/context':
        return route.fulfill({
          json: overrides.empty
            ? context({
                identities: [],
                current_project: null,
                current_sprints: [],
                jira_projects: [],
              })
            : context(),
        });
      case '/api/runtime-mode':
        return route.fulfill({ json: runtimeMode() });
      case '/api/board':
        if (overrides.deferRefreshedBoard && refreshRequested) await refreshedBoard;
        return route.fulfill({
          json: overrides.empty
            ? board({
                issues: [],
                pull_requests: [],
                upstream_issues: [],
                sprints: [],
                sources: [],
              })
            : current,
        });
      case '/api/sync':
        return route.fulfill({
          json: {
            ...sync(),
            sources: overrides.empty ? [] : current.sources,
            progress: overrides.empty ? [] : sync().progress,
          },
        });
      case '/api/health':
        return route.fulfill({ json: { status: 'ok', version: 'e2e', ...runtimeMode() } });
      case '/api/jira/options':
        return route.fulfill({
          json: {
            ...options(
              url.searchParams.get('purpose') === 'implementation' ? 'implementation' : 'review',
            ),
            issue_type: url.searchParams.get('issue_type') ?? 'Task',
          },
        });
      default:
        return route.fulfill({
          status: 404,
          json: { error: { code: 'not_found', message: 'Unknown API route.' } },
        });
    }
  });
  return Object.assign(writes, { releaseRefreshedBoard });
}

test('dense three-column workbench, matching card widths and natural heights, light/dark accessibility', async ({
  page,
}) => {
  await mockApi(page);
  await page.goto('/');
  await expect(page.getByRole('heading', { name: 'Your PR feedback' })).toBeVisible();
  await expect(page.getByText('Showing 4 of 4')).toBeVisible();
  const boxes = await page.locator('.work-card').evaluateAll((cards) =>
    cards.map((c) => ({
      width: Math.round(c.getBoundingClientRect().width),
      height: Math.round(c.getBoundingClientRect().height),
      containsFooter:
        c.querySelector('.card-footer')!.getBoundingClientRect().bottom <=
        c.getBoundingClientRect().bottom,
    })),
  );
  expect(boxes).toHaveLength(4);
  expect(new Set(boxes.map((b) => b.height)).size).toBeGreaterThan(1);
  expect(new Set(boxes.map((b) => b.width)).size).toBe(1);
  expect(Math.max(...boxes.map((b) => b.height))).toBeLessThan(300);
  expect(boxes.every((b) => b.containsFooter)).toBe(true);
  const headings = await page
    .locator('.queue-heading')
    .evaluateAll((items) => items.map((i) => Math.round(i.getBoundingClientRect().top)));
  expect(new Set(headings).size).toBe(1);
  await page.evaluate(() => document.fonts.ready);
  await page.screenshot({ path: 'test-results/workbench-light.png', fullPage: true });
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await page.getByRole('button', { name: 'Dark theme', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await page.screenshot({ path: 'test-results/workbench-dark.png', fullPage: true });
});

test('card links, tags and actions stay visible without disclosures or internal scrolling in both tabs', async ({
  page,
}) => {
  await mockApi(page);
  const linked = structuredClone(snapshot);
  linked.pull_requests[0].title =
    'Publish atomic snapshots and retain last known good data when a source is unavailable';
  linked.pull_requests[0].description =
    'Replace snapshots atomically and retain last good data. Keep cache entries consistent when a configured source is unavailable.';
  linked.pull_requests[0].reviewers = [
    { ...person, id: 'reviewer-pat', username: 'pat' },
    { ...person, id: 'reviewer-sam', username: 'sam' },
  ];
  linked.pull_requests[0].assignees = [{ ...person, id: 'assignee-pat', username: 'pat' }];
  linked.pull_requests[0].linked_issue_keys = ['DEV-1', 'DEV-99'];
  linked.issues[0].linked_pr_ids = [own.id, 'unknown:pr'];
  linked.pull_requests.push(
    pr({ id: 'work:acme/widget:44', number: 44, title: 'No unanswered feedback', attention: null }),
  );
  linked.issues.push(
    issue({ id: 'jira:DEV-3', key: 'DEV-3', title: 'No unanswered mention', attention: null }),
  );
  await page.route('**/api/board', (route) => route.fulfill({ json: linked }));
  await page.goto('/');
  await page.evaluate(() => document.fonts.ready);
  for (const tab of ['Needs Attention', 'Upstream Pull Requests']) {
    if (tab === 'Upstream Pull Requests')
      await page.getByRole('button', { name: tab, exact: true }).click();
    await page.getByRole('button', { name: 'Show all work', exact: true }).click();
    const card = page.locator('.pr-card').filter({ hasText: linked.pull_requests[0].title });
    const jira = page
      .locator('.jira-card')
      .filter({ has: page.getByRole('link', { name: 'DEV-1', exact: true }) });
    await expect(
      card.locator('.linked-items').getByRole('link', { name: 'DEV-1', exact: true }),
    ).toHaveAttribute('href', issue().url);
    await expect(card.locator('.linked-items')).toContainText('DEV-99');
    await expect(card.locator('.linked-items a')).toHaveCount(1);
    await expect(jira.locator('.linked-items a')).toHaveAttribute('href', own.url);
    await expect(jira.locator('.linked-items a')).toContainText(linked.pull_requests[0].title);
    await expect(jira.locator('.linked-pr-url')).toHaveText(own.url);
    await expect(jira.locator('.linked-items')).toContainText('Ready for review');
    await expect(jira.locator('.linked-items')).toContainText('unknown:pr');
    await expect(page.locator('.work-card details, .work-card summary')).toHaveCount(0);
    await expect(card.locator('.card-tags .card-tag')).toHaveText([
      'Ready for review',
      'backend',
      'cache',
    ]);
    await expect(card.getByRole('button', { name: 'Link to Jira', exact: true })).toHaveClass(
      'card-secondary-action',
    );
    await expect(card.locator('.card-primary-action')).toHaveText(
      tab === 'Upstream Pull Requests' ? 'Create Jira ticket' : 'Create review ticket',
    );
    await expect(
      card.getByRole('button', { name: 'Create review ticket', exact: true }),
    ).toBeEnabled();
    const handle = card.locator('.drag-handle');
    const title = card.locator('.card-title a');
    const handleBox = (await handle.boundingBox())!;
    const titleBox = (await title.boundingBox())!;
    const metaBox = (await card.locator('.card-meta').boundingBox())!;
    expect(handleBox.x + handleBox.width).toBeLessThanOrEqual(titleBox.x);
    expect(handleBox.y).toBeLessThan(metaBox.y);
    expect(Math.abs(handleBox.y - titleBox.y)).toBeLessThan(8);
    const neutralAppearance = await page.evaluate(() => {
      const probe = document.createElement('article');
      probe.className = 'work-card';
      document.body.append(probe);
      const css = getComputedStyle(probe);
      const appearance = { background: css.backgroundColor, leading: css.borderLeftColor };
      probe.remove();
      return appearance;
    });
    if (tab === 'Upstream Pull Requests') {
      await expect(
        page.locator('.pr-card').filter({ hasText: 'No unanswered feedback' }),
      ).not.toHaveClass(/needs-attention/);
      await expect(
        page.locator('.jira-card').filter({ hasText: 'No unanswered mention' }),
      ).not.toHaveClass(/needs-attention/);
    }
    for (const item of [card, jira]) {
      await expect(item).toHaveClass(/needs-attention/);
      const appearance = await item.evaluate((element) => {
        const css = getComputedStyle(element);
        return { background: css.backgroundColor, leading: css.borderLeftColor };
      });
      expect(appearance.background).not.toBe(neutralAppearance.background);
      expect(appearance.leading).not.toBe(neutralAppearance.leading);
    }
    const overflow = await page.locator('.work-card').evaluateAll((cards) =>
      cards.flatMap((card) => {
        const bounds = card.getBoundingClientRect();
        const bottom = Math.max(
          ...[...card.querySelectorAll('*')].map(
            (element) => element.getBoundingClientRect().bottom,
          ),
        );
        const scrollers = [...card.querySelectorAll('*')].filter(
          (element) =>
            /auto|scroll|hidden/.test(getComputedStyle(element).overflowY) &&
            !element.classList.contains('card-preview'),
        );
        return bottom > bounds.bottom + 1 || scrollers.length
          ? [
              {
                title: card.getAttribute('aria-label'),
                excess: Math.round(bottom - bounds.bottom),
                scrollers: scrollers.length,
              },
            ]
          : [];
      }),
    );
    await page.screenshot({
      path: `test-results/cards-${tab === 'Upstream Pull Requests' ? 'overview' : 'attention'}.png`,
      fullPage: true,
    });
    expect(
      overflow,
      'Representative linked cards must contain their content without clipping or scrolling',
    ).toEqual([]);
  }
});

test('representative partial-history card hides coverage warnings and preserves stale status', async ({
  page,
}) => {
  await mockApi(page, { degraded: true });
  const current = structuredClone(snapshot);
  current.pull_requests[0] = {
    ...own,
    title: 'Publish atomic snapshots and retain last known good data when a source is unavailable',
    description:
      'Replace snapshots atomically and retain last good data. Keep cache entries consistent when a configured source is unavailable.',
    reviewers: [
      { ...person, id: 'reviewer-pat', username: 'pat' },
      { ...person, id: 'reviewer-sam', username: 'sam' },
    ],
    assignees: [{ ...person, id: 'assignee-pat', username: 'pat' }],
    linked_issue_keys: ['DEV-1'],
    attention: { ...own.attention!, coverage: 'partial' },
  };
  await page.route('**/api/board', (route) => route.fulfill({ json: current }));
  await page.goto('/');
  await page.evaluate(() => document.fonts.ready);
  const card = page.locator('.pr-card').filter({ hasText: current.pull_requests[0].title });
  await expect(card.getByText('Partial history.', { exact: false })).toHaveCount(0);
  await expect(card.getByText('Older review history is unavailable.')).toHaveCount(0);
  await expect(card.locator('.attention-tag a')).toHaveText('Feedback on your PR');
  await expect(card.locator('.attention-tag a')).toHaveAttribute(
    'href',
    own.attention!.threads[0].url!,
  );
  await expect(card.locator('.attention-threads, details, summary')).toHaveCount(0);
  await expect(card.getByText('Stale snapshot', { exact: false })).toBeVisible();
  const bounds = await card.evaluate((element) => {
    const cardBounds = element.getBoundingClientRect();
    const footerBounds = element.querySelector('.card-footer')!.getBoundingClientRect();
    return {
      cardHeight: cardBounds.height,
      contentHeight: Math.ceil(footerBounds.bottom - cardBounds.top),
      overflow: Math.max(0, Math.ceil(footerBounds.bottom - cardBounds.bottom)),
    };
  });
  await page.screenshot({ path: 'test-results/cards-partial-history.png', fullPage: true });
  expect(bounds.cardHeight).toBeGreaterThanOrEqual(bounds.contentHeight - 1);
  expect(
    bounds,
    'Representative warnings must fit without hiding source or attention information',
  ).toMatchObject({ overflow: 0 });
  await page.locator('.health-panel summary').click();
  await expect(
    page.locator('.health-panel').getByText('Older review history is unavailable.'),
  ).toBeVisible();
  await page.locator('.health-panel summary').click();
  for (const width of [390, 320]) {
    await page.setViewportSize({ width, height: 844 });
    await card.scrollIntoViewIfNeeded();
    await expect(card.getByText('Partial history.', { exact: false })).toHaveCount(0);
    await expect(card.getByText('Stale snapshot', { exact: false })).toBeVisible();
    await expect(card.locator('details, summary')).toHaveCount(0);
    const geometry = await card.evaluate((element) => {
      const bounds = element.getBoundingClientRect();
      const footer = element.querySelector('.card-footer')!.getBoundingClientRect();
      const nonPreviewOverflowPx = { top: 0, right: 0, bottom: 0, left: 0 };
      const scrollContainers: string[] = [];
      for (const child of [element, ...element.querySelectorAll('*')]) {
        if (child.closest('.card-preview')) continue;
        const css = getComputedStyle(child);
        if (/auto|scroll|hidden|clip/.test(`${css.overflowX} ${css.overflowY}`))
          scrollContainers.push(`${child.tagName.toLowerCase()}.${child.className}`);
        for (const rect of child.getClientRects()) {
          if (!rect.width || !rect.height) continue;
          const excess = {
            top: Math.max(0, bounds.top - rect.top),
            right: Math.max(0, rect.right - bounds.right),
            bottom: Math.max(0, rect.bottom - bounds.bottom),
            left: Math.max(0, bounds.left - rect.left),
          };
          for (const side of ['top', 'right', 'bottom', 'left'] as const)
            nonPreviewOverflowPx[side] = Math.max(
              nonPreviewOverflowPx[side],
              Number(excess[side].toFixed(2)),
            );
        }
      }
      return {
        height: bounds.height,
        width: bounds.width,
        footerOverflowPx: Math.max(0, Number((footer.bottom - bounds.bottom).toFixed(2))),
        nonPreviewOverflowPx,
        scrollContainers,
        hostContentVisibility: getComputedStyle(element.parentElement!).contentVisibility,
      };
    });
    await test.info().attach(`representative-${width}-geometry.json`, {
      body: JSON.stringify(geometry, null, 2),
      contentType: 'application/json',
    });
    await page.screenshot({
      path: `test-results/cards-partial-history-${width}.png`,
      fullPage: true,
    });
    expect
      .soft(
        geometry,
        `Unchanged representative fixture at ${width}px: paint-clipped content is not visible containment`,
      )
      .toMatchObject({
        footerOverflowPx: 0,
        nonPreviewOverflowPx: { top: 0, right: 0, bottom: 0, left: 0 },
        scrollContainers: [],
      });
  }
});

test('baseline previews preserve newlines and accepted labels stay distinct in both themes', async ({
  page,
}) => {
  await mockApi(page);
  const current = structuredClone(snapshot);
  const description = 'First description line.\nSecond description line.\nThird description line.';
  current.pull_requests[0].description = description;
  current.pull_requests[0].labels = ['Accepted', 'backend'];
  current.issues[0].description = description;
  await page.route('**/api/board', (route) => route.fulfill({ json: current }));
  await page.goto('/');
  await page.evaluate(() => document.fonts.ready);
  const card = page.locator('.pr-card').filter({ hasText: own.title });
  const jira = page.locator('.jira-card').filter({
    has: page.getByRole('link', { name: 'DEV-1', exact: true }),
  });
  for (const theme of ['Light theme', 'Dark theme']) {
    await page.getByRole('button', { name: theme, exact: true }).click();
    for (const item of [card, jira]) {
      const preview = item.locator('.card-preview');
      await expect(preview).toHaveCSS('white-space', 'pre-line');
      const bounds = await preview.evaluate((element) => ({
        height: element.getBoundingClientRect().height,
        lineHeight: Number.parseFloat(getComputedStyle(element).lineHeight),
      }));
      expect(bounds.height).toBeCloseTo(bounds.lineHeight * 2, 0);
    }
    const accepted = card.locator('.label-tag.accepted');
    await expect(accepted).toHaveText('Accepted');
    const acceptedColor = await accepted.evaluate(
      (element) => getComputedStyle(element).backgroundColor,
    );
    const otherColor = await card
      .locator('.label-tag')
      .filter({ hasText: 'backend' })
      .evaluate((element) => getComputedStyle(element).backgroundColor);
    expect(acceptedColor).not.toBe(otherColor);
    await expect(card.locator('.card-title a')).toHaveCSS('text-decoration-line', 'none');
    await card.locator('.card-title a').hover();
    await expect(card.locator('.card-title a')).toHaveCSS('text-decoration-line', 'underline');
    expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  }
});

test('editable simulated ticket uses exact request, normal success and no invented cache items', async ({
  page,
}) => {
  const writes = await mockApi(page);
  await page.goto('/');
  const create = page
    .locator('.pr-card')
    .filter({ hasText: own.title })
    .getByRole('button', { name: 'Create review ticket' });
  await create.click();
  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText('Template draft', { exact: true })).toBeVisible();
  await dialog.getByRole('button', { name: 'Simulate creation' }).focus();
  await page.keyboard.press('Tab');
  expect(await dialog.evaluate((element) => element.contains(document.activeElement))).toBe(true);
  await dialog.getByLabel('Title', { exact: true }).fill('My editable review title');
  await dialog.getByLabel('Description', { exact: true }).fill('My review checklist.');
  await dialog.getByLabel('Workflow status').selectOption('');
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await dialog.getByRole('button', { name: 'Simulate creation' }).click();
  await expect(dialog).toBeHidden();
  await expect(
    page.getByText('Simulated: Ticket creation simulated (read-only mode).'),
  ).toBeVisible();
  expect(writes.filter((w) => w.url === '/api/jira/issues')).toHaveLength(1);
  const write = writes.find((w) => w.url === '/api/jira/issues')!;
  expect(write.headers['x-pullboard-request']).toBe('1');
  expect(write.body).toEqual({
    expected_mode_revision: 'mode-test-1',
    pull_request_id: own.id,
    purpose: 'review',
    project_key: 'DEV',
    issue_type: 'Task',
    title: 'My editable review title',
    description: 'My review checklist.',
    story_points: null,
    assignee_id: null,
    sprint_id: null,
    team: null,
    status: null,
  });
  await expect(page.locator('.work-card')).toHaveCount(4);
  expect(writes.filter((w) => w.url === '/api/sync')).toHaveLength(0);
  await expect(create).toBeFocused();
});

for (const purpose of ['review', 'implementation'] as const) {
  test(`own PR without attention opens and submits ${purpose} from Upstream Pull Requests`, async ({
    page,
  }) => {
    const writes = await mockApi(page, {
      snapshot: { ...snapshot, pull_requests: [{ ...own, attention: null }] },
    });
    const queries: URL[] = [];
    page.on('request', (request) => {
      const url = new URL(request.url());
      if (url.pathname === '/api/jira/options') queries.push(url);
    });
    await page.goto('/');
    await page.getByRole('button', { name: 'Upstream Pull Requests', exact: true }).click();
    const card = page.getByRole('article', { name: own.title, exact: true });
    await expect(card).not.toHaveClass(/needs-attention/);
    await expect(card.getByRole('button', { name: 'Link to Jira', exact: true })).toBeEnabled();
    await expect(
      card.getByRole('button', { name: 'Create Jira ticket', exact: true }),
    ).toBeEnabled();
    await expect(
      card.getByRole('button', { name: 'Create review ticket', exact: true }),
    ).toBeEnabled();
    const label = purpose === 'review' ? 'Create review ticket' : 'Create Jira ticket';
    const create = card.getByRole('button', { name: label, exact: true });
    await create.focus();
    await page.keyboard.press('Enter');
    const dialog = page.getByRole('dialog', { name: label, exact: true });
    const submit = dialog.getByRole('button', { name: 'Simulate creation', exact: true });
    await expect(submit).toBeEnabled();
    expect(writes.filter((write) => write.url === '/api/ticket-drafts').map((w) => w.body)).toEqual(
      [{ pull_request_id: own.id, purpose, project_key: null }],
    );
    expect(queries.length).toBeGreaterThan(0);
    expect(queries.every((url) => url.searchParams.get('purpose') === purpose)).toBe(true);
    await expect(dialog.getByLabel('Assignee', { exact: true })).toHaveValue('Unassigned');
    await expect(dialog.locator('#ticket-sprint')).toHaveValue('');
    await expect(dialog.locator('#ticket-status')).toHaveValue('');
    await dialog.getByLabel('Assignee', { exact: true }).click();
    await expect(dialog.getByRole('option').filter({ hasText: person.display_name })).toHaveCount(
      purpose === 'review' ? 0 : 1,
    );
    await page.keyboard.press('Escape');
    expect(writes.filter((write) => write.url === '/api/jira/issues')).toHaveLength(0);
    expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
    await submit.click();
    await expect(dialog).toBeHidden();
    const creates = writes.filter((write) => write.url === '/api/jira/issues');
    expect(creates).toHaveLength(1);
    expect(creates[0].body).toMatchObject({
      pull_request_id: own.id,
      purpose,
      expected_mode_revision: 'mode-test-1',
      assignee_id: null,
      sprint_id: null,
      status: null,
    });
    await expect(
      page.getByText('Simulated: Ticket creation simulated (read-only mode).'),
    ).toBeVisible();
    await expect(page.locator('.work-card')).toHaveCount(2);
    expect(writes.filter((write) => write.url === '/api/sync')).toHaveLength(0);
    await expect(create).toBeFocused();
  });
}

test('keyboard-accessible link dialog and escape restore focus', async ({ page }) => {
  const writes = await mockApi(page);
  await page.goto('/');
  const link = page
    .locator('.pr-card')
    .filter({ hasText: own.title })
    .getByRole('button', { name: 'Link to Jira', exact: true });
  await link.focus();
  await page.keyboard.press('Enter');
  const dialog = page.getByRole('dialog');
  await expect(dialog).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(dialog).toBeHidden();
  await expect(link).toBeFocused();
  await page.keyboard.press('Enter');
  await dialog.getByLabel('Jira ticket', { exact: true }).selectOption('DEV-1');
  await dialog.getByRole('button', { name: 'Simulate link' }).click();
  await expect(page.getByText('Simulated: Linking simulated (read-only mode).')).toBeVisible();
  expect(writes.find((w) => w.url.endsWith('/links'))?.body).toEqual({
    pull_request_id: own.id,
    expected_mode_revision: 'mode-test-1',
  });
  await expect(
    page.locator('.jira-card').filter({ hasText: 'DEV-1' }).locator('.linked-items'),
  ).toContainText('None linked');
});

test('lazy ticket loading freezes gesture mode, coalesces repeated opens and restores focus', async ({
  page,
}) => {
  const writes = await mockApi(page);
  let mode = runtimeMode();
  await page.route('**/api/runtime-mode', (route) => route.fulfill({ json: mode }));
  await page.goto('/');
  const create = page
    .locator('.pr-card')
    .filter({ hasText: own.title })
    .getByRole('button', { name: 'Create review ticket' });
  await expect(create).toBeVisible();
  let release!: () => void;
  let started!: () => void;
  const loading = new Promise<void>((resolve) => (started = resolve));
  const loaded = new Promise<void>((resolve) => (release = resolve));
  let chunks = 0;
  await page.route('**/chunk-*.js', async (route) => {
    chunks++;
    started();
    await loaded;
    await route.continue();
  });
  await create.click();
  await loading;
  await create.click();
  expect(writes.filter((write) => write.url === '/api/ticket-drafts')).toHaveLength(0);
  mode = runtimeMode({ read_only_test_mode: false, mode_revision: 'live-during-lazy-load' });
  await expect(page.getByRole('button', { name: 'Switch to Test', exact: true })).toBeVisible();
  release();
  const dialog = page.getByRole('dialog', { name: 'Create review ticket' });
  await expect(dialog.getByRole('combobox', { name: 'Assignee', exact: true })).toHaveValue(
    'Unassigned',
  );
  await expect(dialog).toContainText('Runtime mode changed');
  await expect(dialog.getByRole('button', { name: 'Simulate creation' })).toBeDisabled();
  expect(chunks).toBe(1);
  expect(writes.filter((write) => write.url === '/api/ticket-drafts')).toHaveLength(1);
  expect(writes.filter((write) => write.url === '/api/jira/issues')).toHaveLength(0);
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(create).toBeFocused();
});

test('a failed lazy ticket chunk reports a safe error without drafts or writes', async ({
  page,
}) => {
  const writes = await mockApi(page);
  await page.goto('/');
  const create = page
    .locator('.pr-card')
    .first()
    .getByRole('button', { name: 'Create review ticket' });
  await expect(create).toBeVisible();
  await page.route('**/chunk-*.js', (route) => route.abort('failed'));
  await create.click();
  await expect(page.getByText('The ticket editor could not load.', { exact: false })).toBeVisible();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(create).toBeFocused();
  expect(writes).toHaveLength(0);
});

test('type-scoped discovered choices preserve independent manual selections and searchable eligible assignees', async ({
  page,
}) => {
  await mockApi(page);
  const queries: URL[] = [];
  await page.route('**/api/jira/options?**', async (route) => {
    const url = new URL(route.request().url());
    queries.push(url);
    const o = options();
    await route.fulfill({
      json: {
        ...o,
        issue_type: url.searchParams.get('issue_type') ?? 'Task',
        assignees: o.assignees,
        sprints: [
          ...o.sprints,
          {
            ...o.sprints[0],
            id: 'foreign',
            name: 'CORE foreign-origin',
            team: null,
            default_eligible: false,
          },
        ],
        diagnostics: [],
      },
    });
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Dark theme', exact: true }).click();
  await page
    .locator('.pr-card')
    .filter({ hasText: own.title })
    .getByRole('button', { name: 'Create review ticket' })
    .click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.locator('#ticket-assignee')).toHaveValue('Unassigned');
  await expect(dialog.locator('#ticket-assignee option[value="person-1"]')).toHaveCount(0);
  await dialog.locator('#ticket-team').selectOption('team-platform');
  await dialog.locator('#ticket-sprint').selectOption('foreign');
  await dialog.locator('#ticket-assignee').fill('PAT@EXAMPLE.TEST');
  await expect(dialog.getByRole('option', { name: /Pat Reviewer/ })).toBeVisible();
  await dialog.locator('#ticket-assignee').press('ArrowDown');
  await dialog.locator('#ticket-assignee').press('Enter');
  await expect(dialog.locator('#ticket-assignee')).toHaveValue('Pat Reviewer');
  await dialog.getByLabel('Issue type').selectOption('Bug');
  await expect.poll(() => queries.at(-1)?.searchParams.get('issue_type')).toBe('Bug');
  await expect(dialog.locator('#ticket-team')).toHaveValue('team-platform');
  await expect(dialog.locator('#ticket-sprint')).toHaveValue('foreign');
  const beforeSearch = queries.length;
  await dialog.locator('#ticket-assignee').fill('not in eligible groups');
  await expect(dialog.getByText('No matching assignees.')).toBeVisible();
  await expect(dialog.getByText('Selected: Pat Reviewer')).toBeVisible();
  await dialog.locator('#ticket-assignee').press('Escape');
  await expect(dialog.locator('#ticket-assignee')).toHaveValue('Pat Reviewer');
  expect(queries).toHaveLength(beforeSearch);
  expect(queries.every((q) => !q.searchParams.get('query'))).toBe(true);
  await dialog.locator('#ticket-team').selectOption('');
  await expect(dialog.locator('#ticket-sprint')).toHaveValue('foreign');
  await dialog.locator('#ticket-sprint').selectOption('');
  await dialog.getByLabel('Issue type').selectOption('Task');
  await expect.poll(() => queries.at(-1)?.searchParams.get('issue_type')).toBe('Task');
  await expect(dialog.locator('#ticket-team')).toHaveValue('');
  await expect(dialog.locator('#ticket-sprint')).toHaveValue('');
  expect(queries.every((q) => q.searchParams.get('purpose') === 'review')).toBe(true);
  await expect(
    dialog.getByText('Sprint will be applied after creation,', { exact: false }),
  ).toBeVisible();
  await page.screenshot({ path: 'test-results/ticket-discovered-options.png', fullPage: true });
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
});

test('successful creation with sprint follow-up warning closes the editor without a duplicate retry', async ({
  page,
}) => {
  await mockApi(page);
  let creates = 0;
  await page.route('**/api/jira/issues', (route) => {
    creates++;
    return route.fulfill({
      json: {
        outcome: 'created',
        issue: {
          key: 'DEV-123',
          url: 'https://jira.example/browse/DEV-123',
          title: 'Real response fixture',
        },
        message: 'Ticket DEV-123 created.',
        warnings: [
          'Sprint assignment failed. Assign the existing ticket in Jira; do not create it again.',
        ],
      },
    });
  });
  await page.goto('/');
  await page
    .locator('.pr-card')
    .filter({ hasText: own.title })
    .getByRole('button', { name: 'Create review ticket' })
    .click();
  const dialog = page.getByRole('dialog');
  await dialog.getByRole('button', { name: 'Simulate creation' }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByText('Sprint assignment failed.', { exact: false })).toBeVisible();
  await expect(page.getByRole('link', { name: 'DEV-123' })).toBeVisible();
  const notification = page.locator('.toast.toast-warning');
  await expect(notification).toContainText('Ticket created.');
  await expect(notification.getByRole('link', { name: 'DEV-123' })).toHaveAttribute(
    'href',
    'https://jira.example/browse/DEV-123',
  );
  await page.waitForTimeout(1200);
  await expect(notification).toBeVisible();
  await expect(page.locator('.jira-card')).toHaveCount(2);
  expect(creates).toBe(1);
});

for (const count of [0, 2]) {
  test(`header/current project and Overview explicitly handle ${count} matching active sprints; reset and refresh preserve intentional filters`, async ({
    page,
  }) => {
    await mockApi(page);
    let current = context({
      current_project: { key: 'FIRST', name: 'First configured project', issue_types: ['Task'] },
      current_sprints: Array.from({ length: count }, (_, index) => ({
        ...sprint,
        id: String(index),
        team: null,
        name: `2026-IDM-SSSD-IDENTITY-S${index}`,
        board_url: 'https://jira.example/secure/RapidBoard.jspa?rapidView=12',
      })),
    });
    const core = { ...sprint, id: 'core', name: '2026-IDM-SSSD-CORE-S99', default_eligible: false };
    await page.route('**/api/context', (route) => route.fulfill({ json: current }));
    await page.route('**/api/board', (route) =>
      route.fulfill({
        json: board({
          sprints: [
            ...current.current_sprints,
            core,
            {
              ...sprint,
              id: 'future',
              state: 'future',
              default_eligible: false,
              name: 'Other team future',
            },
          ],
        }),
      }),
    );
    await page.goto('/?view=overview');
    const header = page.getByRole('region', { name: 'Who am I' });
    await expect(header).toContainText('FIRST / First configured project');
    await expect(header).not.toContainText('CORE');
    await expect(header).not.toContainText('Ownership unknown');
    await expect(header).not.toContainText('Board 12');
    if (count) {
      await expect(header).toContainText('Multiple matching active sprints');
      const link = header.getByRole('link', { name: '2026-IDM-SSSD-IDENTITY-S0' });
      await expect(link).toHaveAttribute(
        'href',
        'https://jira.example/secure/RapidBoard.jspa?rapidView=12&selectedSprint=0',
      );
      await expect(header).toContainText('Ends');
    } else await expect(header).toContainText('No matching active sprint');
    const filter = page.getByRole('combobox', { name: 'Sprint', exact: true });
    await expect(filter).toHaveValue('');
    await expect(page.locator('.jira-card')).toHaveCount(0);
    await expect(filter.locator('option[value="active"]')).toHaveCount(0);
    await expect(filter.locator('optgroup[label="Active"] option[value="core"]')).toHaveCount(1);
    await expect(filter.locator('optgroup[label="Future"] option')).toContainText(
      'Other team future',
    );
    await filter.selectOption('all');
    await page.getByLabel('Jira title', { exact: true }).fill('CACHE');
    await page.getByLabel('PR title', { exact: true }).fill('snapshot');
    await page.getByLabel('Forge instance').selectOption('work');
    current = { ...current, current_sprints: [sprint] };
    await page.reload();
    await expect(filter).toHaveValue('all');
    await expect(page.getByLabel('Jira title', { exact: true })).toHaveValue('CACHE');
    await expect(page.getByLabel('Forge instance')).toHaveValue('work');
    await page.getByRole('button', { name: 'Reset filters', exact: true }).click();
    await expect(filter).toHaveValue(sprint.id);
    await expect(page.getByLabel('Jira title', { exact: true })).toHaveValue('');
    await expect(page.getByLabel('PR title', { exact: true })).toHaveValue('');
    await expect(page.getByLabel('Forge instance')).toHaveValue('all');
    expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  });
}

test('saved actual sprint and other filters survive initial discovery pending and later publication', async ({
  page,
}) => {
  await mockApi(page);
  let pending = true;
  await page.route('**/api/context', (route) =>
    route.fulfill({
      json: pending
        ? context({ current_project: null, current_sprints: [], jira_projects: [], identities: [] })
        : context(),
    }),
  );
  await page.route('**/api/board', (route) =>
    route.fulfill({
      json: pending
        ? board({ generated_at: null, issues: [], pull_requests: [], sprints: [], sources: [] })
        : board(),
    }),
  );
  await page.goto(
    '/?view=overview&overview.sprint=saved-actual&overview.jiraTitle=cache&overview.source=personal&overview.repository=saved-repo',
  );
  await expect(page.getByRole('combobox', { name: 'Sprint', exact: true })).toHaveValue(
    'saved-actual',
  );
  await expect(page.getByLabel('Forge instance')).toHaveValue('personal');
  await expect(page.getByLabel('Repository')).toHaveValue('saved-repo');
  pending = false;
  await page.reload();
  await expect(page.getByRole('combobox', { name: 'Sprint', exact: true })).toHaveValue(
    'saved-actual',
  );
  await expect(page.getByLabel('Jira title', { exact: true })).toHaveValue('cache');
  await expect(page.locator('.jira-card')).toHaveCount(0);
});

test('large cached assignee pool filters name/email locally with keyboard, bounded DOM, stale choices and name-based statuses', async ({
  page,
}) => {
  const writes = await mockApi(page);
  const queries: URL[] = [];
  const o = options();
  const people = Array.from({ length: 2000 }, (_, index) => ({
    ...o.assignees[0],
    id: `user-${index}`,
    display_name: `Person ${index}`,
    username: `user${index}`,
    email: index === 1999 ? 'last@example.test' : null,
  }));
  await page.route('**/api/jira/options?**', (route) => {
    queries.push(new URL(route.request().url()));
    return route.fulfill({
      json: {
        ...o,
        assignees: people,
        statuses: [
          { id: 'status-987', name: 'Review' },
          { id: 'duplicate-a', name: 'Ambiguous' },
          { id: 'duplicate-b', name: 'Ambiguous' },
        ],
      },
    });
  });
  const health = {
    source_id: 'jira',
    name: 'Jira',
    kind: 'jira',
    status: 'error',
    last_success_at: '2026-11-01T00:00:00Z',
    last_attempt_at: '2026-11-02T00:00:00Z',
    error: 'Choice refresh failed.',
    retry_at: null,
    coverage_notes: [],
  };
  await page.route('**/api/sync', (route) =>
    route.request().method() === 'GET'
      ? route.fulfill({ json: { ...sync(), sources: [health] } })
      : route.fulfill({ status: 202, json: { ...sync('syncing'), sources: [health] } }),
  );
  await page.goto('/');
  await page
    .locator('.pr-card')
    .first()
    .getByRole('button', { name: 'Create review ticket' })
    .click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByText('Using cached Jira choices;', { exact: false })).toBeVisible();
  await expect(dialog.locator('#ticket-sprint')).toHaveValue('');
  await expect(dialog.locator('#ticket-status')).toHaveValue('');
  const assignee = dialog.getByRole('combobox', { name: 'Assignee', exact: true });
  await assignee.click();
  await expect(dialog.getByRole('option').filter({ hasText: 'Person 0' })).toBeVisible();
  expect(await dialog.locator('[role="option"]').count()).toBeLessThan(50);
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await assignee.fill('LAST@EXAMPLE.TEST');
  await expect(dialog.getByRole('option', { name: 'Person 1999 last@example.test' })).toBeVisible();
  await assignee.press('ArrowDown');
  await assignee.press('Enter');
  await expect(assignee).toHaveValue('Person 1999');
  await assignee.fill('no such person');
  await expect(dialog.getByText('No matching assignees.')).toBeVisible();
  await expect(dialog.getByText('Selected: Person 1999')).toBeVisible();
  await assignee.press('Escape');
  await expect(dialog).toBeVisible();
  await expect(assignee).toHaveValue('Person 1999');
  expect(queries).toHaveLength(1);
  expect(queries.every((q) => !q.searchParams.get('query'))).toBe(true);
  await expect(dialog.locator('#ticket-status option[value="Ambiguous"]').first()).toBeDisabled();
  await dialog.locator('#ticket-status').selectOption('Review');
  await dialog.getByRole('button', { name: 'Refresh Jira choices' }).click();
  await expect(assignee).toHaveValue('Person 1999');
  await expect(dialog.locator('#ticket-status')).toHaveValue('Review');
  await dialog.getByRole('button', { name: 'Simulate creation' }).click();
  await expect(dialog).toBeHidden();
  expect(writes.find((w) => w.url === '/api/jira/issues')?.body).toMatchObject({
    assignee_id: 'user-1999',
    sprint_id: null,
    status: 'Review',
  });
});

for (const tab of ['Needs Attention', 'Upstream Pull Requests']) {
  test(`CDK drag from PR onto Jira submits a link without optimistic changes in ${tab}`, async ({
    page,
  }) => {
    const writes = await mockApi(page);
    await page.goto('/');
    if (tab === 'Upstream Pull Requests')
      await page.getByRole('button', { name: tab, exact: true }).click();
    const jira = page.locator('.jira-card').filter({ hasText: 'DEV-1' });
    await expect(jira).toBeVisible();
    const handle = await page
      .locator('.pr-card')
      .filter({ hasText: own.title })
      .locator('.drag-handle')
      .boundingBox();
    const target = await jira.boundingBox();
    await page.mouse.move(handle!.x + handle!.width / 2, handle!.y + handle!.height / 2);
    await page.mouse.down();
    await page.mouse.move(handle!.x - 15, handle!.y + 15, { steps: 5 });
    await page.mouse.move(target!.x + target!.width / 2, target!.y + 130, { steps: 20 });
    await page.mouse.up();
    await expect(page.getByText('Simulated: Linking simulated (read-only mode).')).toBeVisible();
    expect(writes.filter((w) => w.url.endsWith('/links'))).toHaveLength(1);
    await expect(jira.locator('.linked-items')).toContainText('None linked');
  });
}

test('PR dragging distinguishes eligible Jira targets without changing Jira card geometry', async ({
  page,
}) => {
  const dragSnapshot = board({
    issues: [
      issue(),
      issue({
        id: 'jira:DEV-2',
        key: 'DEV-2',
        title: 'Another configured ticket',
      }),
      issue({
        id: 'jira:FOREIGN-1',
        key: 'FOREIGN-1',
        project_key: 'FOREIGN',
        title: 'Mention-only foreign ticket',
      }),
    ],
    pull_requests: [own],
  });
  await mockApi(page, { snapshot: dragSnapshot });
  await page.goto('/');
  const firstEligible = page.locator('.jira-card').filter({ hasText: 'DEV-1' });
  const secondEligible = page.locator('.jira-card').filter({ hasText: 'DEV-2' });
  const ineligible = page.locator('.jira-card').filter({ hasText: 'FOREIGN-1' });
  const appearance = async (card: typeof firstEligible) =>
    card.evaluate((element) => {
      const style = getComputedStyle(element);
      const bounds = element.getBoundingClientRect();
      return {
        background: style.backgroundColor,
        border: `${style.borderTopWidth} ${style.borderTopStyle} ${style.borderTopColor}`,
        geometry: { width: bounds.width, height: bounds.height },
      };
    });
  const [firstBefore, secondBefore, foreignBefore] = await Promise.all([
    appearance(firstEligible),
    appearance(secondEligible),
    appearance(ineligible),
  ]);
  const handle = await page.locator('.pr-card .drag-handle').boundingBox();
  await page.mouse.move(handle!.x + handle!.width / 2, handle!.y + handle!.height / 2);
  await page.mouse.down();
  await page.mouse.move(handle!.x - 15, handle!.y + 15, { steps: 5 });
  await expect
    .poll(async () => (await appearance(firstEligible)).background !== firstBefore.background)
    .toBe(true);
  const [firstPale, secondPale, foreignDragging] = await Promise.all([
    appearance(firstEligible),
    appearance(secondEligible),
    appearance(ineligible),
  ]);
  expect(firstPale.background).not.toBe(firstBefore.background);
  expect(secondPale.background).not.toBe(secondBefore.background);
  expect(foreignDragging).toEqual(foreignBefore);
  expect(firstPale.geometry).toEqual(firstBefore.geometry);
  expect(secondPale.geometry).toEqual(secondBefore.geometry);
  expect(foreignDragging.geometry).toEqual(foreignBefore.geometry);
  const target = await firstEligible.boundingBox();
  await page.mouse.move(target!.x + target!.width / 2, target!.y + target!.height / 2, {
    steps: 20,
  });
  await expect
    .poll(async () => (await appearance(firstEligible)).background !== firstPale.background)
    .toBe(true);
  const [receiving, otherEligible] = await Promise.all([
    appearance(firstEligible),
    appearance(secondEligible),
  ]);
  expect(receiving.background).not.toBe(firstPale.background);
  expect(receiving.background).not.toBe(otherEligible.background);
  expect(receiving.geometry).toEqual(firstBefore.geometry);
  expect(otherEligible.geometry).toEqual(secondBefore.geometry);
  await page.mouse.up();
});

test('a real second PR link is additive only after the refreshed board snapshot arrives', async ({
  page,
}) => {
  const existing = own;
  const second = pr({
    id: 'work:acme/widget:43',
    number: 43,
    title: 'Add cache refresh regression coverage',
    attention: null,
  });
  const liveSnapshot = board({
    issues: [issue({ linked_pr_ids: [existing.id] })],
    pull_requests: [existing, second],
  });
  const writes = await mockApi(page, {
    snapshot: liveSnapshot,
    liveLink: { issueKey: 'DEV-1', prId: second.id },
    deferRefreshedBoard: true,
  });
  await page.goto('/');
  await page.getByRole('button', { name: 'Upstream Pull Requests', exact: true }).click();
  const jira = page.locator('.jira-card').filter({ hasText: 'DEV-1' });
  const linked = jira.locator('.linked-pull-requests');
  await expect(linked).toContainText(existing.title);
  await expect(linked).not.toContainText(second.title);
  await page
    .locator('.pr-card')
    .filter({ hasText: second.title })
    .getByRole('button', { name: 'Link to Jira', exact: true })
    .click();
  const dialog = page.getByRole('dialog', { name: 'Link PR to Jira' });
  await dialog.getByLabel('Jira ticket', { exact: true }).selectOption('DEV-1');
  await dialog.getByRole('button', { name: 'Simulate link' }).click();
  await expect(page.getByText('Linked: Pull request linked.')).toBeVisible();
  await expect(linked).toContainText(existing.title);
  await expect(linked).not.toContainText(second.title);
  writes.releaseRefreshedBoard();
  await expect(linked).toContainText(second.title, { timeout: 5_000 });
  expect(writes.filter((write) => write.url.endsWith('/links'))).toHaveLength(1);
  expect(writes.filter((write) => write.url === '/api/sync')).toHaveLength(1);
});

test('independent tab filters and instance IDs, assigned-only overview, persistent preferences', async ({
  page,
}) => {
  await mockApi(page);
  await page.goto('/');
  await page.getByLabel('Forge instance').selectOption('personal');
  await expect(page.locator('.pr-card')).toHaveCount(1);
  await expect(page.locator('.pr-card')).toContainText(review.title);
  await page.getByRole('button', { name: 'Upstream Pull Requests', exact: true }).click();
  await expect(page.getByLabel('Forge instance')).toHaveValue('all');
  await expect(page.locator('.pr-card')).toHaveCount(1);
  await page.getByRole('button', { name: 'Show all work', exact: true }).click();
  await expect(page.locator('.jira-card')).toHaveCount(1);
  await expect(page.locator('.jira-card')).not.toContainText('Mention-only triage');
  await page.getByRole('button', { name: /Needs Attention/ }).click();
  await expect(page.getByLabel('Forge instance')).toHaveValue('personal');
  await page.reload();
  await expect(page.getByLabel('Forge instance')).toHaveValue('personal');
});

test('repository and forge filters are mutually exclusive', async ({ page }) => {
  await mockApi(page);
  await page.goto('/');
  const forge = page.getByLabel('Forge instance');
  const repository = page.getByLabel('Repository');
  await forge.selectOption('personal');
  await repository.selectOption('work\u001facme\u001fwidget');
  await expect(forge).toHaveValue('all');
  await expect(repository).toHaveValue('work\u001facme\u001fwidget');
  await expect(page.locator('.pr-card')).toHaveCount(1);
  await expect(page.locator('.pr-card')).toContainText(own.title);
  await forge.selectOption('personal');
  await expect(repository).toHaveValue('all');
  await expect(page.locator('.pr-card')).toHaveCount(1);
  await expect(page.locator('.pr-card')).toContainText(review.title);
});

test('mobile layout has no horizontal overflow and keeps accessible controls', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await mockApi(page);
  await page.goto('/');
  await expect(page.locator('.work-card').first()).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await page.screenshot({ path: 'test-results/workbench-mobile.png', fullPage: true });
  await page.setViewportSize({ width: 320, height: 700 });
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(
    true,
  );
});

test('stale and backoff conditions retain cards and disclose source coverage', async ({ page }) => {
  await mockApi(page, { degraded: true });
  await page.goto('/');
  await expect(page.getByText('Stale snapshot', { exact: false }).first()).toBeVisible();
  await page.locator('.health-panel summary').click();
  await expect(
    page.locator('.health-panel').getByText('Source rate limit reached.', { exact: false }),
  ).toBeVisible();
  await expect(
    page.locator('.health-panel').getByText('Older review history is unavailable.'),
  ).toBeVisible();
  await expect(page.getByText('Retry at', { exact: false })).toBeVisible();
  await expect(page.locator('.work-card')).toHaveCount(4);
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
});

test('unconfigured sources have honest empty states and no active sprint fallback', async ({
  page,
}) => {
  await mockApi(page, { empty: true });
  await page.goto('/');
  await expect(page.getByText('No verified identities yet.', { exact: false })).toBeVisible();
  await expect(page.getByText('No matching active sprint.', { exact: false })).toBeVisible();
  await expect(page.getByText('No unanswered mentions')).toBeVisible();
  await expect(page.locator('.work-card')).toHaveCount(0);
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
});

test('API failures show sanitized recovery without demo or fabricated data', async ({ page }) => {
  await mockApi(page, { failure: true });
  await page.goto('/');
  await expect(page.getByRole('alert')).toContainText('Local server is unavailable');
  await expect(page.getByRole('button', { name: 'Retry loading', exact: true })).toBeVisible();
  await expect(page.locator('.work-card')).toHaveCount(0);
});

test('production assets work under the Rust server content security policy', async ({ page }) => {
  await mockApi(page);
  const violations: string[] = [];
  page.on('console', (message) => {
    if (
      message.type() === 'error' &&
      /content security policy|violates.*directive/i.test(message.text())
    )
      violations.push(message.text());
  });
  await page.route('http://127.0.0.1:4209/', async (route) => {
    const response = await route.fetch();
    await route.fulfill({
      response,
      headers: {
        ...response.headers(),
        'content-security-policy':
          "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'",
      },
    });
  });
  await page.goto('/');
  await expect(page.locator('.work-card')).toHaveCount(4);
  await expect(page.locator('.work-card').first()).toHaveCSS('display', 'flex');
  expect(
    await page
      .locator('.work-card')
      .first()
      .evaluate((card) => card.scrollHeight <= card.clientHeight),
  ).toBe(true);
  await page.getByRole('button', { name: 'Dark theme', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  await page
    .locator('.pr-card')
    .first()
    .getByRole('button', { name: 'Create review ticket' })
    .click();
  await expect(
    page.getByRole('dialog').getByRole('combobox', { name: 'Assignee', exact: true }),
  ).toBeVisible();
  expect(violations).toEqual([]);
});
