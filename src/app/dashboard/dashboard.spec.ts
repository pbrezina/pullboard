import { ComponentFixture, TestBed } from '@angular/core/testing';
import { HttpErrorResponse } from '@angular/common/http';
import { vi } from 'vitest';
import { ConfigService } from '../core/config';
import { ForgeAggregatorService } from '../core/forge-aggregator';
import { JiraService } from '../core/jira';
import { LlmService } from '../core/llm';
import { JiraTicket } from '../core/models';
import { PartialSyncError, SyncService } from '../core/sync';
import { testConfig, testPr } from '../core/testing';
import { Dashboard } from './dashboard';
import { CdkDrag, CdkDragDrop, CdkDropList } from '@angular/cdk/drag-drop';
import { MatDialog } from '@angular/material/dialog';
import { By } from '@angular/platform-browser';
import { of } from 'rxjs';
import { TEST_MODE } from '../core/test-mode';
import { UnifiedPullRequest } from '../core/models';

describe('Dashboard', () => {
  let fixture: ComponentFixture<Dashboard>;
  const ticket: JiraTicket = {
    key: 'DEV-1',
    summary: 'Track worker shutdown',
    description: 'Stop the worker cleanly and flush pending requests.',
    status: 'Review',
    issueType: 'Task',
    url: 'https://team.atlassian.net/browse/DEV-1',
    links: [{ id: 1, title: 'PR', url: testPr.url }],
    pullRequests: [],
  };
  const forges = {
    getOpenPullRequests: vi.fn(),
    supportsUrl: vi.fn(),
    getPullRequestStatus: vi.fn(),
  };
  const jira = {
    getTickets: vi.fn(),
    getWebLinks: vi.fn(),
    transition: vi.fn(),
    getPullRequestLinks: vi.fn(),
  };
  const sync = { resume: vi.fn(), create: vi.fn() };
  beforeEach(async () => {
    localStorage.clear();
    vi.resetAllMocks();
    jira.getPullRequestLinks.mockResolvedValue({
      links: {},
      complete: true,
      checkedTickets: 0,
      failedTickets: 0,
    });
    forges.getOpenPullRequests.mockResolvedValue({
      pullRequests: [testPr],
      warnings: [],
      identities: [
        { providerId: 'forge-0', label: 'GitHub', host: 'github.com', username: 'developer' },
      ],
    });
    forges.supportsUrl.mockResolvedValue(true);
    forges.getPullRequestStatus.mockResolvedValue({ state: 'merged', draft: false });
    jira.getTickets.mockResolvedValue({
      context: {
        boardId: 1,
        defaultSprintId: 2,
        accountId: 'account-1',
        username: 'Developer (dev@example.com)',
        projectKey: 'DEV',
        sprints: [
          { id: 2, name: 'Current sprint', state: 'active' },
          { id: 3, name: 'Next sprint', state: 'future' },
        ],
      },
      selection: 2,
      tickets: [ticket],
    });
    jira.getWebLinks.mockResolvedValue(ticket.links);
    jira.transition.mockResolvedValue(undefined);
    sync.resume.mockResolvedValue({ key: ticket.key, url: ticket.url });
    await TestBed.configureTestingModule({
      imports: [Dashboard],
      providers: [
        { provide: ConfigService, useValue: { load: async () => testConfig } },
        { provide: ForgeAggregatorService, useValue: forges },
        { provide: JiraService, useValue: jira },
        { provide: LlmService, useValue: { suggest: vi.fn() } },
        { provide: SyncService, useValue: sync },
      ],
    }).compileComponents();
    fixture = TestBed.createComponent(Dashboard);
    await fixture.whenStable();
    await vi.waitFor(() => expect(fixture.componentInstance['loading']()).toBe(false));
    await fixture.whenStable();
  });
  afterEach(() => {
    fixture.destroy();
    localStorage.clear();
  });

  it('renders both columns and detects a merged linked PR', () => {
    const element: HTMLElement = fixture.nativeElement;
    expect(element.textContent).toContain('Your pull requests');
    expect(element.textContent).toContain('Your JIRA tickets');
    expect(element.textContent).toContain(ticket.description);
    expect(element.querySelector('.header-context')?.textContent).toContain('developer');
    expect(element.querySelector('.header-context')?.textContent).toContain(
      'Developer (dev@example.com)',
    );
    expect(element.querySelector('.header-context')?.textContent).toContain('Current sprint');
    expect(element.querySelector('.linked-pr')?.textContent).toContain(testPr.url);
    expect(element.querySelector('.closure-card')).toBeTruthy();
    expect(element.textContent).toContain('Close JIRA Ticket');
  });

  it('shows sorted labels and accepted highlighting on sync cards too', async () => {
    fixture.componentInstance['pullRequests'].set([
      { ...testPr, labels: ['Zulu', 'Accepted', 'alpha'] },
    ]);
    await fixture.whenStable();
    const card: HTMLElement = fixture.nativeElement.querySelector('.accepted-card');
    expect(card).toBeTruthy();
    expect(
      [...card.querySelectorAll('.label-badge')].map((badge) => badge.textContent?.trim()),
    ).toEqual(['Accepted', 'alpha', 'Zulu']);
    expect(card.textContent).toContain('Ready for review');
    expect(card.querySelector('.last-activity strong')).toBeTruthy();
    expect(card.querySelector('.card-meta app-pr-activity .last-activity')).toBeTruthy();
    expect(card.querySelector('p.last-activity')).toBeNull();
  });

  it('shows the PR title only once at the top of the card', () => {
    const element: HTMLElement = fixture.nativeElement;
    const card = element.querySelector('.pr-card')!;
    expect(card.querySelectorAll('h3')).toHaveLength(1);
    const title = card.querySelector('mat-card-content > h3:first-child');
    expect(title?.textContent?.trim()).toBe(testPr.title);
    expect(title?.nextElementSibling?.classList.contains('card-meta')).toBe(true);
    expect(title?.querySelector('a')?.href).toBe(testPr.url);
    expect(card.querySelector('.card-meta + app-pr-badges')).toBeTruthy();
  });

  it('shows known sprint links without scanning the rest of the project', async () => {
    await fixture.componentInstance['refresh']();
    await fixture.whenStable();
    const links = [...fixture.nativeElement.querySelectorAll('.linked-tickets a')].map((link) =>
      (link as HTMLElement).textContent?.trim(),
    );
    expect(links).toEqual(['DEV-1']);
    expect(jira.getPullRequestLinks).not.toHaveBeenCalled();
    expect(fixture.nativeElement.textContent).not.toContain('Loading JIRA links');
    expect(fixture.nativeElement.textContent).not.toContain('JIRA links unavailable');
  });

  it('does not refresh automatically after two minutes', async () => {
    const calls = forges.getOpenPullRequests.mock.calls.length;
    vi.useFakeTimers();
    try {
      await vi.advanceTimersByTimeAsync(5 * 60000);
      expect(forges.getOpenPullRequests).toHaveBeenCalledTimes(calls);
    } finally {
      vi.useRealTimers();
    }
  });
  it('keeps the PR template and adds compact links without allowing duplicate creation', () => {
    const link: HTMLAnchorElement = fixture.nativeElement.querySelector('.linked-tickets a');
    expect(link.href).toBe(ticket.url);
    expect(link.textContent).toContain('DEV-1');
    const card: HTMLElement = fixture.nativeElement.querySelector('.pr-card');
    expect(card.querySelector('.pr-description')?.textContent).toContain(testPr.description);
    expect(card.querySelector('.repository-name')?.textContent).toContain(testPr.repository);
    expect(card.classList.contains('synced-card')).toBe(true);
    expect(card.querySelector<HTMLButtonElement>('.card-actions button')?.disabled).toBe(true);
    expect(card.querySelector<HTMLButtonElement>('[aria-label^="Link PR"]')?.disabled).toBe(false);
    expect(fixture.nativeElement.querySelector('.issue-type').textContent).toBe('Task');
    expect(
      localStorage.getItem('pullboard:v1:https://team.atlassian.net:DEV:dev@example.com'),
    ).not.toContain('test-only');
  });

  it('shows reviewers and assignees on both unlinked and linked PR cards', async () => {
    const dashboard = fixture.componentInstance;
    dashboard['pullRequests'].set([
      { ...testPr, reviewers: ['alice', 'bob'], assignees: ['dev', 'carol'] },
    ]);
    await fixture.whenStable();
    const people: HTMLElement = fixture.nativeElement.querySelector('.pr-people');
    expect(people.textContent).toContain('alice, bob');
    expect(people.textContent).toContain('dev, carol');
    dashboard['states'].set({});
    dashboard['tickets'].set([{ ...ticket, links: [] }]);
    await fixture.whenStable();
    expect(fixture.nativeElement.querySelector('.pr-people').textContent).toBe(people.textContent);
    expect(fixture.nativeElement.querySelector('.linked-tickets')).toBeNull();
  });

  it('hides the drag placeholder only inside ticket targets to keep their layout stable', () => {
    const card: HTMLElement = fixture.nativeElement.querySelector('.pr-card');
    const source: HTMLElement = fixture.nativeElement.querySelector('#pull-requests');
    const target: HTMLElement = fixture.nativeElement.querySelector('.jira-card');
    const placeholder = card.cloneNode(true) as HTMLElement;
    placeholder.classList.add('cdk-drag-placeholder');
    source.append(placeholder);
    expect(getComputedStyle(placeholder).display).not.toBe('none');
    target.append(placeholder);
    expect(getComputedStyle(placeholder).display).toBe('none');
    placeholder.remove();
  });

  it('allows dragging linked PRs but rejects already linked issue targets', async () => {
    const dashboard = fixture.componentInstance;
    const second = {
      ...ticket,
      key: 'DEV-2',
      url: 'https://team.atlassian.net/browse/DEV-2',
      links: [],
    };
    dashboard['tickets'].update((tickets) => [...tickets, second]);
    await fixture.whenStable();
    const drag = fixture.debugElement.query(By.directive(CdkDrag)).injector.get(CdkDrag);
    const targets = fixture.debugElement
      .queryAll(By.directive(CdkDropList))
      .map((element) => element.injector.get(CdkDropList));
    const first = targets.find((target) => target.id === ticket.key)!;
    const additional = targets.find((target) => target.id === second.key)!;
    expect(drag.disabled).toBe(false);
    expect(first.enterPredicate(drag, first)).toBe(false);
    expect(additional.enterPredicate(drag, additional)).toBe(true);
  });

  it('discovers all issues for one PR and deduplicates canonical URLs per issue', async () => {
    const result = await jira.getTickets();
    jira.getTickets.mockResolvedValue({
      ...result,
      tickets: [
        ticket,
        {
          ...ticket,
          key: 'DEV-2',
          url: 'https://team.atlassian.net/browse/DEV-2',
          links: [
            { id: 2, title: 'PR', url: `${testPr.url}/?source=jira` },
            { id: 3, title: 'PR again', url: testPr.url },
          ],
        },
      ],
    });
    await fixture.componentInstance['refresh']();
    await fixture.whenStable();
    expect(fixture.componentInstance['linkedTickets'](testPr).map((entry) => entry.key)).toEqual([
      'DEV-1',
      'DEV-2',
    ]);
    expect(fixture.nativeElement.querySelectorAll('.linked-tickets a')).toHaveLength(2);
    expect(fixture.componentInstance['unlinkedCount']()).toBe(0);
  });

  it('links an already tracked PR to another issue and retains both across views and reloads', async () => {
    const dashboard = fixture.componentInstance;
    const second = {
      ...ticket,
      key: 'DEV-2',
      url: 'https://team.atlassian.net/browse/DEV-2',
      links: [],
    };
    const receipt = { key: second.key, url: second.url };
    sync.resume.mockResolvedValue(receipt);
    expect(dashboard['canLink'](testPr, second)).toBe(true);
    await dashboard['link'](testPr, second);
    expect(sync.resume).toHaveBeenCalledWith(testPr, receipt);
    // A successful mutation updates receipts without reloading either remote board.
    expect(jira.getTickets).toHaveBeenCalledTimes(1);
    expect(forges.getOpenPullRequests).toHaveBeenCalledTimes(1);
    expect(dashboard['linkedTickets'](testPr).map((entry) => entry.key)).toEqual([
      'DEV-1',
      'DEV-2',
    ]);
    dashboard['states'].set({});
    dashboard['tickets'].set([]);
    dashboard['restoreReceipts']('https://team.atlassian.net');
    expect(dashboard['linkedTickets'](testPr).map((entry) => entry.key)).toEqual([
      'DEV-1',
      'DEV-2',
    ]);
    dashboard['pullRequests'].set([]);
    expect(dashboard['savedLinks']().map((saved) => saved.receipt.key)).toEqual(['DEV-1', 'DEV-2']);
  });

  it('rejects duplicate PR/issue pairs without blocking a different PR on the same issue', async () => {
    const dashboard = fixture.componentInstance;
    await dashboard['link'](testPr, ticket);
    expect(sync.resume).not.toHaveBeenCalled();
    const otherPr = {
      ...testPr,
      id: 'other',
      number: 43,
      url: 'https://github.com/org/repo/pull/43',
    };
    await dashboard['link'](otherPr, ticket);
    expect(sync.resume).toHaveBeenCalledWith(otherPr, { key: ticket.key, url: ticket.url });
    expect(dashboard['linkedTickets'](testPr).map((entry) => entry.key)).toEqual(['DEV-1']);
    expect(dashboard['linkedTickets'](otherPr).map((entry) => entry.key)).toEqual(['DEV-1']);
  });

  it('preserves earlier links and the exact failed destination while resuming another link', async () => {
    const dashboard = fixture.componentInstance;
    const second = {
      ...ticket,
      key: 'DEV-2',
      url: 'https://team.atlassian.net/browse/DEV-2',
      links: [],
    };
    const receipt = { key: second.key, url: second.url };
    sync.resume.mockRejectedValueOnce(
      new PartialSyncError(receipt, new Error('Transition denied')),
    );
    await dashboard['link'](testPr, second);
    expect(dashboard['states']()[testPr.url].status).toBe('partial');
    expect(dashboard['linkedTickets'](testPr).map((entry) => entry.key)).toEqual(['DEV-1']);
    expect(dashboard['canLink'](testPr)).toBe(false);
    dashboard['states'].set({});
    dashboard['restoreReceipts']('https://team.atlassian.net');
    expect(dashboard['states']()[testPr.url].receipt).toEqual(receipt);
    sync.resume.mockResolvedValue(receipt);
    await dashboard['resume'](testPr);
    expect(sync.resume).toHaveBeenLastCalledWith(testPr, receipt, undefined);
    expect(dashboard['linkedTickets'](testPr).map((entry) => entry.key)).toEqual([
      'DEV-1',
      'DEV-2',
    ]);
  });

  it('retains partial operation state when refresh discovers other linked issues', async () => {
    const dashboard = fixture.componentInstance;
    dashboard['states'].set({
      [testPr.url]: {
        status: 'partial',
        receipt: { key: 'DEV-2', url: 'https://team.atlassian.net/browse/DEV-2' },
        sprintId: 3,
      },
    });
    await dashboard['refresh']();
    expect(dashboard['states']()[testPr.url].status).toBe('partial');
    expect(dashboard['states']()[testPr.url].receipt?.key).toBe('DEV-2');
    expect(dashboard['states']()[testPr.url].sprintId).toBe(3);
    expect(dashboard['linkedTickets'](testPr).map((entry) => entry.key)).toEqual(['DEV-1']);
  });

  it('migrates legacy single receipts and rejects invalid stored associations', () => {
    const dashboard = fixture.componentInstance;
    localStorage.setItem(
      'pullboard:v1:https://team.atlassian.net:DEV:dev@example.com',
      JSON.stringify({
        [testPr.url]: {
          status: 'synced',
          receipt: { key: ticket.key, url: ticket.url },
          linkedTickets: [
            { key: 'DEV-2', url: 'https://evil.example/browse/DEV-2' },
            { key: 'SIMULATED-1', url: '', simulated: true },
            null,
          ],
        },
      }),
    );
    dashboard['restoreReceipts']('https://team.atlassian.net');
    expect(dashboard['states']()[testPr.url].linkedTickets).toEqual([
      { key: ticket.key, url: ticket.url },
    ]);
  });
  it('prevents closing if any linked PR is open or unknown', () => {
    const dashboard = fixture.componentInstance;
    expect(
      dashboard['canClose']({
        ...ticket,
        pullRequests: [
          { url: 'a', state: 'merged' },
          { url: 'b', state: 'open' },
        ],
      }),
    ).toBe(false);
    expect(
      dashboard['canClose']({ ...ticket, pullRequests: [{ url: 'a', error: 'API failure' }] }),
    ).toBe(false);
    expect(
      dashboard['canClose']({
        ...ticket,
        linkError: 'cannot read links',
        pullRequests: [{ url: 'a', state: 'merged' }],
      }),
    ).toBe(false);
  });
  it('rechecks links and upstream status, and refuses closure of a reopened PR', async () => {
    forges.getPullRequestStatus.mockResolvedValueOnce({ state: 'open', draft: false });
    const dashboard = fixture.componentInstance;
    await dashboard['closeTicket'](dashboard['tickets']()[0]);
    expect(jira.getWebLinks).toHaveBeenCalledWith('DEV-1');
    expect(jira.transition).not.toHaveBeenCalled();
    expect(dashboard['ticketMessages']()['DEV-1']).toContain('not closed');
  });
  it('closes only JIRA after a successful fresh status check and retains an inline result', async () => {
    const dashboard = fixture.componentInstance;
    await dashboard['closeTicket'](dashboard['tickets']()[0]);
    expect(jira.transition).toHaveBeenCalledWith('DEV-1', 'done');
    expect(dashboard['ticketMessages']()['DEV-1']).toBe('Ticket closed in JIRA.');
    expect(dashboard['tickets']()[0].url).toBe(ticket.url);
  });
  it('routes accessible existing-ticket linking through the same synchronization service', async () => {
    const dashboard = fixture.componentInstance;
    dashboard['states'].set({});
    dashboard['tickets'].set([{ ...ticket, links: [] }]);
    await dashboard['link'](testPr, ticket);
    expect(sync.resume).toHaveBeenCalledWith(testPr, { key: ticket.key, url: ticket.url });
    expect(dashboard['states']()[testPr.url].status).toBe('synced');
  });
  it('keeps saved success receipts when their PR leaves the open list', async () => {
    forges.getOpenPullRequests.mockResolvedValue({
      pullRequests: [],
      warnings: [],
      identities: [],
    });
    await fixture.componentInstance['refresh']();
    await fixture.whenStable();
    expect(fixture.nativeElement.textContent).toContain('Saved JIRA links');
    expect(fixture.nativeElement.querySelector('.saved-link a').href).toBe(ticket.url);
  });

  it('handles a CDK drop by linking the dragged PR to that exact ticket', async () => {
    const dashboard = fixture.componentInstance;
    dashboard['states'].set({});
    dashboard['tickets'].set([{ ...ticket, links: [] }]);
    const event = {
      previousContainer: { id: 'pull-requests' },
      container: { id: ticket.key },
      item: { data: testPr },
    } as unknown as CdkDragDrop<JiraTicket, UnifiedPullRequest[], UnifiedPullRequest>;
    await dashboard['drop'](event, ticket);
    expect(sync.resume).toHaveBeenCalledWith(testPr, { key: ticket.key, url: ticket.url });
    expect(sync.create).not.toHaveBeenCalled();
  });

  it('never reopens a retained closed card through an existing-ticket link', async () => {
    const dashboard = fixture.componentInstance;
    dashboard['states'].set({});
    await dashboard['link'](testPr, { ...ticket, closed: true });
    expect(sync.resume).not.toHaveBeenCalled();
  });

  it('restores the exact sprint choice from durable partial state', async () => {
    const dashboard = fixture.componentInstance;
    localStorage.setItem(
      'pullboard:v1:https://team.atlassian.net:DEV:dev@example.com',
      JSON.stringify({
        [testPr.url]: {
          status: 'partial',
          receipt: { key: ticket.key, url: ticket.url },
          sprintId: 3,
        },
      }),
    );
    dashboard['restoreReceipts']('https://team.atlassian.net');
    await dashboard['resume'](testPr);
    expect(sync.resume).toHaveBeenLastCalledWith(testPr, { key: ticket.key, url: ticket.url }, 3);
  });

  it('does not guess a sprint when restoring older partial receipts', async () => {
    const dashboard = fixture.componentInstance;
    localStorage.setItem(
      'pullboard:v1:https://team.atlassian.net:DEV:dev@example.com',
      JSON.stringify({
        [testPr.url]: {
          status: 'partial',
          receipt: { key: ticket.key, url: ticket.url },
          addToSprint: true,
        },
      }),
    );
    dashboard['restoreReceipts']('https://team.atlassian.net');
    await dashboard['resume'](testPr);
    expect(sync.resume).not.toHaveBeenCalled();
    expect(dashboard['states']()[testPr.url].error).toContain('no saved sprint ID');
  });

  it('filters titles, ticket keys and PR numbers locally and independently', async () => {
    const [prFilter, ticketFilter] = fixture.nativeElement.querySelectorAll(
      'input[type=search]',
    ) as NodeListOf<HTMLInputElement>;
    prFilter.value = 'does not match';
    prFilter.dispatchEvent(new Event('input'));
    await fixture.whenStable();
    expect(fixture.nativeElement.querySelectorAll('.pr-card')).toHaveLength(0);
    expect(fixture.nativeElement.querySelectorAll('.jira-card')).toHaveLength(1);
    expect(fixture.nativeElement.textContent).toContain('No matching pull requests');
    prFilter.value = `#${testPr.number}`;
    prFilter.dispatchEvent(new Event('input'));
    ticketFilter.value = 'dev-1';
    ticketFilter.dispatchEvent(new Event('input'));
    await fixture.whenStable();
    expect(fixture.nativeElement.querySelectorAll('.pr-card')).toHaveLength(1);
    expect(fixture.nativeElement.querySelectorAll('.jira-card')).toHaveLength(1);
    expect(jira.getTickets).toHaveBeenCalledTimes(1);
    ticketFilter.value = 'SHUTDOWN';
    ticketFilter.dispatchEvent(new Event('input'));
    await fixture.whenStable();
    expect(fixture.nativeElement.querySelectorAll('.jira-card')).toHaveLength(1);
  });

  it('loads the selected sprint and preserves it on automatic/manual refresh', async () => {
    const result = await jira.getTickets();
    jira.getTickets.mockResolvedValue({ ...result, selection: 3 });
    const select: HTMLSelectElement = fixture.nativeElement.querySelector('.sprint-filter select');
    select.value = '3';
    select.dispatchEvent(new Event('change'));
    await fixture.whenStable();
    expect(jira.getTickets).toHaveBeenLastCalledWith(3);
    await fixture.componentInstance['refresh']();
    expect(jira.getTickets).toHaveBeenLastCalledWith(3);
    jira.getTickets.mockResolvedValue({ ...result, selection: null });
    await fixture.componentInstance['selectSprint']('none');
    expect(jira.getTickets).toHaveBeenLastCalledWith(null);
    expect(fixture.componentInstance['selectedSprintName']()).toBe('No sprint');
  });

  it('uses the active sprint as dialog default independently of the viewed future sprint', async () => {
    const dashboard = fixture.componentInstance;
    dashboard['states'].set({});
    dashboard['tickets'].set([{ ...ticket, links: [] }]);
    dashboard['filters'].update((filters) => ({ ...filters, sprint: '3' }));
    const open = vi
      .spyOn(TestBed.inject(MatDialog), 'open')
      .mockReturnValue({ afterClosed: () => of(undefined) } as never);
    await dashboard['suggest'](testPr, true);
    expect(open).toHaveBeenCalledWith(
      expect.anything(),
      expect.objectContaining({
        data: expect.objectContaining({ defaultSprintId: 2 }),
      }),
    );
    expect(sync.create).not.toHaveBeenCalled();
  });

  it('stores the exact selected sprint with a partial receipt for safe resumption', async () => {
    const dashboard = fixture.componentInstance;
    dashboard['states'].set({});
    dashboard['tickets'].set([{ ...ticket, links: [] }]);
    vi.spyOn(TestBed.inject(MatDialog), 'open').mockReturnValue({
      afterClosed: () => of({ title: 'Track', description: 'Scope', storyPoints: 3, sprintId: 3 }),
    } as never);
    sync.create.mockImplementation(async (_pr, _suggestion, onCreated) => {
      onCreated({ key: ticket.key, url: ticket.url });
      throw new PartialSyncError({ key: ticket.key, url: ticket.url }, new Error('Denied'));
    });
    await dashboard['suggest'](testPr, true);
    expect(dashboard['states']()[testPr.url].sprintId).toBe(3);
    await dashboard['resume'](testPr);
    expect(sync.resume).toHaveBeenLastCalledWith(testPr, { key: ticket.key, url: ticket.url }, 3);
  });
});

describe('Dashboard test-mode isolation', () => {
  let fixture: ComponentFixture<Dashboard>;
  const receipt = { key: 'SIMULATED-1', url: '', simulated: true };
  const ticket: JiraTicket = {
    key: 'DEV-1',
    url: 'https://team.atlassian.net/browse/DEV-1',
    summary: 'Work',
    description: 'Scope',
    status: 'Review',
    issueType: 'Story',
    links: [{ id: 1, url: testPr.url, title: 'PR' }],
    pullRequests: [],
  };
  const jira = {
    getTickets: vi.fn(),
    getWebLinks: vi.fn(),
    transition: vi.fn(),
    getPullRequestLinks: vi.fn(),
  };
  beforeEach(async () => {
    localStorage.clear();
    localStorage.setItem(
      'pullboard:v1:https://team.atlassian.net:DEV:dev@example.com',
      '{"keep":"real receipts"}',
    );
    jira.getPullRequestLinks.mockResolvedValue({
      links: {},
      complete: true,
      checkedTickets: 0,
      failedTickets: 0,
    });
    jira.getTickets.mockResolvedValue({
      context: {
        boardId: 1,
        accountId: 'account-1',
        username: 'Dev',
        projectKey: 'DEV',
        sprints: [],
      },
      selection: null,
      tickets: [ticket],
    });
    jira.getWebLinks.mockResolvedValue(ticket.links);
    await TestBed.configureTestingModule({
      imports: [Dashboard],
      providers: [
        { provide: TEST_MODE, useValue: true },
        { provide: ConfigService, useValue: { load: async () => testConfig } },
        {
          provide: ForgeAggregatorService,
          useValue: {
            getOpenPullRequests: async () => ({
              pullRequests: [testPr],
              identities: [],
              warnings: [],
            }),
            supportsUrl: async () => true,
            getPullRequestStatus: async () => ({ state: 'merged', draft: false }),
          },
        },
        { provide: JiraService, useValue: jira },
        { provide: LlmService, useValue: {} },
        { provide: SyncService, useValue: { create: async () => receipt } },
      ],
    }).compileComponents();
    fixture = TestBed.createComponent(Dashboard);
    await fixture.whenStable();
  });
  afterEach(() => {
    fixture.destroy();
    localStorage.clear();
  });

  it('shows simulated creation inline without fake links or touching real storage', async () => {
    fixture.componentInstance['states'].set({});
    fixture.componentInstance['tickets'].set([{ ...ticket, links: [] }]);
    jira.getTickets.mockResolvedValue({
      ...(await jira.getTickets()),
      tickets: [{ ...ticket, links: [] }],
    });
    vi.spyOn(TestBed.inject(MatDialog), 'open').mockReturnValue({
      afterClosed: () => of({ title: 'Track', description: 'Scope', storyPoints: 3 }),
    } as never);
    await fixture.componentInstance['suggest'](testPr, true);
    await fixture.whenStable();
    expect(fixture.nativeElement.querySelector('.test-banner').textContent).toContain('TEST MODE');
    expect(fixture.nativeElement.querySelector('.simulation-message').textContent).toContain(
      'ticket creation simulated',
    );
    expect(fixture.nativeElement.querySelector('.linked-tickets')).toBeNull();
    expect(
      localStorage.getItem('pullboard:v1:https://team.atlassian.net:DEV:dev@example.com'),
    ).toBe('{"keep":"real receipts"}');
    expect(localStorage.length).toBe(1);
    fixture.componentInstance['pullRequests'].set([]);
    expect(fixture.componentInstance['savedLinks']()).toEqual([]);
  });

  it('reports simulated closure without changing the real card status', async () => {
    await fixture.componentInstance['closeTicket'](fixture.componentInstance['tickets']()[0]);
    expect(fixture.componentInstance['tickets']()[0].status).toBe('Review');
    expect(fixture.componentInstance['tickets']()[0].closed).toBeUndefined();
    expect(fixture.componentInstance['ticketMessages']()['DEV-1']).toContain('closure simulated');
  });

  it('never claims a creation outcome is unknown when a simulation fails', async () => {
    fixture.componentInstance['states'].set({});
    fixture.componentInstance['tickets'].set([{ ...ticket, links: [] }]);
    vi.spyOn(TestBed.inject(MatDialog), 'open').mockReturnValue({
      afterClosed: () => of({ title: 'Track', description: 'Scope', storyPoints: 3 }),
    } as never);
    vi.spyOn(TestBed.inject(SyncService), 'create').mockRejectedValue(
      new HttpErrorResponse({ status: 0 }),
    );
    await fixture.componentInstance['suggest'](testPr, true);
    expect(fixture.componentInstance['states']()[testPr.url].status).toBe('error');
    expect(
      localStorage.getItem('pullboard:v1:https://team.atlassian.net:DEV:dev@example.com'),
    ).toBe('{"keep":"real receipts"}');
  });
});
