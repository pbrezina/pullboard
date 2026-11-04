import { TestBed } from '@angular/core/testing';
import { By } from '@angular/platform-browser';
import { provideRouter, Router } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { vi } from 'vitest';
import { routes } from './app.routes';
import { ConfigService } from './core/config';
import { ForgeAggregatorService } from './core/forge-aggregator';
import { JiraService } from './core/jira';
import { LlmService } from './core/llm';
import { SyncService } from './core/sync';
import { testConfig, testPr } from './core/testing';
import { Dashboard } from './dashboard/dashboard';
import { PullRequests } from './pull-requests/pull-requests';
import { App } from './app';
import { Workspace } from './workspace/workspace';

describe('Workspace tabs and routes', () => {
  const forges = { getPullRequestInbox: vi.fn(), getOpenPullRequests: vi.fn() };
  const jira = { getTickets: vi.fn() };
  beforeEach(() => {
    localStorage.clear();
    vi.resetAllMocks();
    forges.getPullRequestInbox.mockResolvedValue({
      opened: [],
      requested: [],
      identities: [],
      warnings: [],
    });
    forges.getOpenPullRequests.mockResolvedValue({
      pullRequests: [],
      identities: [],
      warnings: [],
    });
    jira.getTickets.mockResolvedValue({
      context: { boardId: 1, accountId: 'dev', username: 'dev', projectKey: 'DEV', sprints: [] },
      selection: null,
      tickets: [],
    });
    TestBed.configureTestingModule({
      providers: [
        provideRouter(routes),
        { provide: ConfigService, useValue: { load: async () => testConfig } },
        { provide: ForgeAggregatorService, useValue: forges },
        { provide: JiraService, useValue: jira },
        { provide: LlmService, useValue: {} },
        { provide: SyncService, useValue: {} },
      ],
    });
  });
  afterEach(() => localStorage.clear());

  it('loads both tabs on page open and preserves their components and filters on navigation', async () => {
    const harness = await RouterTestingHarness.create();
    const workspace = await harness.navigateByUrl('/', Workspace);
    await harness.fixture.whenStable();
    expect(harness.routeNativeElement?.textContent).toContain('Requested reviews from me');
    expect(jira.getTickets).toHaveBeenCalledTimes(1);
    expect(forges.getPullRequestInbox).toHaveBeenCalledTimes(1);
    expect(forges.getOpenPullRequests).toHaveBeenCalledTimes(1);
    const inbox = harness.fixture.debugElement.query(By.directive(PullRequests))
      .componentInstance as PullRequests;
    inbox['filters'].set({ opened: 'keep', requested: 'also keep' });
    expect(harness.routeNativeElement?.querySelector('app-dashboard')?.parentElement?.hidden).toBe(
      true,
    );
    expect(await harness.navigateByUrl('/synchronize-to-jira', Workspace)).toBe(workspace);
    await harness.fixture.whenStable();
    expect(
      harness.routeNativeElement?.querySelector('app-pull-requests')?.parentElement?.hidden,
    ).toBe(true);
    expect(harness.routeNativeElement?.querySelector('app-dashboard')?.parentElement?.hidden).toBe(
      false,
    );
    await harness.navigateByUrl('/', Workspace);
    await harness.fixture.whenStable();
    expect(harness.fixture.debugElement.query(By.directive(PullRequests)).componentInstance).toBe(
      inbox,
    );
    expect(inbox['filters']().opened).toBe('keep');
    expect(jira.getTickets).toHaveBeenCalledTimes(1);
    expect(forges.getPullRequestInbox).toHaveBeenCalledTimes(1);
    expect(forges.getOpenPullRequests).toHaveBeenCalledTimes(1);
    const ids = [...harness.routeNativeElement!.querySelectorAll('[id]')].map(
      (element) => element.id,
    );
    expect(new Set(ids).size).toBe(ids.length);
  });

  it('preserves in-flight operations while switching tabs and supports direct sync entry', async () => {
    const harness = await RouterTestingHarness.create();
    await harness.navigateByUrl('/synchronize-to-jira', Workspace);
    await harness.fixture.whenStable();
    expect(harness.routeNativeElement?.querySelector('app-dashboard')?.parentElement?.hidden).toBe(
      false,
    );
    const dashboard = harness.fixture.debugElement.query(By.directive(Dashboard))
      .componentInstance as Dashboard;
    dashboard['states'].set({ [testPr.url]: { status: 'syncing' } });
    await harness.navigateByUrl('/');
    expect(TestBed.inject(Router).url).toBe('/');
    expect(dashboard['states']()[testPr.url].status).toBe('syncing');
    expect(harness.fixture.debugElement.query(By.directive(Dashboard)).componentInstance).toBe(
      dashboard,
    );
    expect(forges.getPullRequestInbox).toHaveBeenCalledTimes(1);
  });

  it('refreshes only the manually requested tab', async () => {
    const harness = await RouterTestingHarness.create('/');
    await harness.fixture.whenStable();
    const inbox = harness.fixture.debugElement.query(By.directive(PullRequests))
      .componentInstance as PullRequests;
    const dashboard = harness.fixture.debugElement.query(By.directive(Dashboard))
      .componentInstance as Dashboard;
    await inbox['refresh']();
    expect(forges.getPullRequestInbox).toHaveBeenCalledTimes(2);
    expect(jira.getTickets).toHaveBeenCalledTimes(1);
    await dashboard['refresh']();
    expect(forges.getPullRequestInbox).toHaveBeenCalledTimes(2);
    expect(jira.getTickets).toHaveBeenCalledTimes(2);
  });

  it('starts the hidden tab immediately without blocking the active tab on its response', async () => {
    let finish!: (result: Awaited<ReturnType<JiraService['getTickets']>>) => void;
    jira.getTickets.mockReturnValueOnce(new Promise((resolve) => (finish = resolve)));
    const harness = await RouterTestingHarness.create('/');
    await harness.fixture.whenStable();
    const inbox = harness.fixture.debugElement.query(By.directive(PullRequests))
      .componentInstance as PullRequests;
    const dashboard = harness.fixture.debugElement.query(By.directive(Dashboard))
      .componentInstance as Dashboard;
    expect(inbox['loading']()).toBe(false);
    expect(dashboard['loading']()).toBe(true);
    expect(forges.getOpenPullRequests).toHaveBeenCalledTimes(1);
    expect(jira.getTickets).toHaveBeenCalledTimes(1);
    await harness.navigateByUrl('/synchronize-to-jira', Workspace);
    expect(jira.getTickets).toHaveBeenCalledTimes(1);
    finish({
      context: { boardId: 1, accountId: 'dev', username: 'dev', projectKey: 'DEV', sprints: [] },
      selection: null,
      tickets: [],
    });
    await vi.waitFor(() => expect(dashboard['loading']()).toBe(false));
    await harness.fixture.whenStable();
    expect(dashboard['loading']()).toBe(false);
  });

  it('renders accessible navigation tabs with the selected route marked active', async () => {
    const fixture = TestBed.createComponent(App);
    await TestBed.inject(Router).navigateByUrl('/');
    await fixture.whenStable();
    const tabs = fixture.nativeElement.querySelectorAll('[role="tab"]');
    expect(tabs).toHaveLength(2);
    expect(tabs[0].textContent).toContain('Pull requests');
    expect(tabs[0].getAttribute('aria-selected')).toBe('true');
    tabs[1].click();
    await fixture.whenStable();
    expect(TestBed.inject(Router).url).toBe('/synchronize-to-jira');
    expect(tabs[1].getAttribute('aria-selected')).toBe('true');
  });
});
