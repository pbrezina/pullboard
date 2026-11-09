import { TestBed } from '@angular/core/testing';
import { Dialog } from '@angular/cdk/dialog';
import { vi } from 'vitest';
import { BoardView } from './board-view';
import { BoardStore } from '../core/board-store';
import { Preferences } from '../core/preferences';
import { RuntimeMode } from '../core/runtime-mode';
import type { Purpose } from '../core/api-types';
import { board, context, foreignMention, issue, pr, runtimeMode } from '../testing/fixtures';
describe('Board views', () => {
  const open = vi.fn();
  const openDialogs: unknown[] = [];
  beforeEach(() => {
    open.mockReset();
    openDialogs.length = 0;
    TestBed.configureTestingModule({
      providers: [{ provide: Dialog, useValue: { open, openDialogs } }],
    });
    localStorage.clear();
    window.history.replaceState(null, '', window.location.pathname);
    const store = TestBed.inject(BoardStore);
    store.board.set(board({ issues: [issue({ sprint_ids: [] })] }));
    store.context.set(context());
    TestBed.inject(RuntimeMode).apply(runtimeMode());
  });
  it('renders renamed view buttons with the attention count and existing tab identifiers', async () => {
    const f = TestBed.createComponent(BoardView);
    await f.whenStable();
    const buttons = Array.from(
      f.nativeElement.querySelectorAll('.view-switch button'),
    ) as HTMLButtonElement[];
    expect(buttons.map((button) => button.textContent?.trim())).toEqual([
      'Needs Attention 2',
      'Upstream Pull Requests',
      'Upstream issues',
    ]);
    expect(buttons[0].getAttribute('aria-pressed')).toBe('true');
    buttons[1].click();
    await f.whenStable();
    expect(f.componentInstance.tab()).toBe('overview');
    expect(buttons[1].getAttribute('aria-pressed')).toBe('true');
    expect(buttons[0].getAttribute('aria-pressed')).toBe('false');
    buttons[0].click();
    await f.whenStable();
    expect(f.componentInstance.tab()).toBe('attention');
    expect(buttons[0].getAttribute('aria-pressed')).toBe('true');
    expect(buttons[1].getAttribute('aria-pressed')).toBe('false');
  });
  it('shows unfiltered attention badges, hidden counts and a direct show-all action', async () => {
    const f = TestBed.createComponent(BoardView);
    f.componentInstance.changeFilters({ ...f.componentInstance.filters(), sprint: 'sprint-1' });
    await f.whenStable();
    const text = f.nativeElement.textContent;
    expect(text).toContain('1 hidden by filters');
    expect(text).toContain('Showing 1 of 2');
    expect(f.componentInstance.unfilteredAttention()).toBe(2);
    f.componentInstance.showEverything();
    await f.whenStable();
    expect(f.componentInstance.data()?.shown).toBe(2);
  });
  it('keeps title and sprint selections independent across tab switches', async () => {
    const f = TestBed.createComponent(BoardView);
    const p = TestBed.inject(Preferences);
    f.componentInstance.changeFilters({
      jiraTitle: 'cache',
      prTitle: 'snapshot',
      sprint: 'none',
      source: 'work',
      repository: 'all',
    });
    p.setTab('overview');
    await f.whenStable();
    expect(f.componentInstance.filters().prTitle).toBe('');
    f.componentInstance.changeFilters({
      jiraTitle: 'other',
      prTitle: '',
      sprint: 'all',
      source: 'personal',
      repository: 'all',
    });
    p.setTab('attention');
    await f.whenStable();
    expect(f.componentInstance.filters().prTitle).toBe('snapshot');
    expect(f.componentInstance.filters().sprint).toBe('none');
  });
  it('counts and renders foreign mentions without expanding assigned Overview work', async () => {
    const store = TestBed.inject(BoardStore);
    store.board.set(board({ issues: [issue(), foreignMention(), foreignMention(true)] }));
    const f = TestBed.createComponent(BoardView);
    await f.whenStable();
    expect(f.nativeElement.querySelector('.count-badge').textContent).toBe('4');
    expect(f.nativeElement.textContent).toContain('Showing 4 of 4');
    expect(f.nativeElement.querySelectorAll('app-issue-card')).toHaveLength(3);
    f.componentInstance.changeFilters({ ...f.componentInstance.filters(), jiraTitle: 'FOREIGN' });
    await f.whenStable();
    expect(f.nativeElement.textContent).toContain('Showing 3 of 4');
    expect(f.componentInstance.unfilteredAttention()).toBe(4);
    TestBed.inject(Preferences).setTab('overview');
    await f.whenStable();
    expect(f.nativeElement.textContent).toContain('Showing 2 of 2');
    expect(f.nativeElement.querySelectorAll('app-issue-card')).toHaveLength(1);
    expect(f.nativeElement.textContent).not.toContain('FOREIGN-');
    f.componentInstance.showEverything();
    await f.whenStable();
    expect(f.nativeElement.querySelectorAll('app-issue-card')).toHaveLength(1);
    store.context.set(context({ jira_projects: [] }));
    await f.whenStable();
    expect(f.nativeElement.querySelectorAll('app-issue-card')).toHaveLength(0);
    expect(f.componentInstance.unfilteredAttention()).toBe(4);
  });
  it.each<Purpose>(['implementation', 'review'])(
    'opens the explicit %s action from an own PR in Upstream Pull Requests',
    async (purpose) => {
      const selectedPr = pr();
      TestBed.inject(BoardStore).board.set(board({ pull_requests: [selectedPr] }));
      TestBed.inject(Preferences).setTab('overview');
      const f = TestBed.createComponent(BoardView);
      await f.whenStable();
      const ticket = vi.spyOn(f.componentInstance, 'ticket');
      const buttons = Array.from(
        f.nativeElement.querySelectorAll('app-pr-card button'),
      ) as HTMLButtonElement[];
      const label = purpose === 'review' ? 'Create review ticket' : 'Create Jira ticket';
      const action = buttons.find((button) => button.textContent?.trim() === label);
      expect(action).toBeDefined();
      action!.click();
      expect(ticket).toHaveBeenCalledExactlyOnceWith(selectedPr, purpose);
      await ticket.mock.results[0].value;
      expect(open).toHaveBeenCalledOnce();
      expect(open.mock.calls[0][1]).toMatchObject({
        data: { kind: 'pr', pr: selectedPr, purpose, mode: runtimeMode() },
      });
    },
  );
  it.each([true, false])(
    'keeps only the review-ticket action in Required Attention for is_mine=%s',
    async (isMine) => {
      const selectedPr = pr({ is_mine: isMine });
      TestBed.inject(BoardStore).board.set(board({ pull_requests: [selectedPr] }));
      const f = TestBed.createComponent(BoardView);
      await f.whenStable();
      const ticket = vi.spyOn(f.componentInstance, 'ticket');
      const buttons = Array.from(
        f.nativeElement.querySelectorAll('app-pr-card button'),
      ) as HTMLButtonElement[];
      expect(buttons.map((button) => button.textContent?.trim())).toEqual([
        'Link to Jira',
        'Create review ticket',
      ]);
      buttons[1].click();
      expect(ticket).toHaveBeenCalledExactlyOnceWith(selectedPr, 'review');
      await ticket.mock.results[0].value;
      expect(open).toHaveBeenCalledOnce();
      expect(open.mock.calls[0][1]).toMatchObject({
        data: { kind: 'pr', pr: selectedPr, purpose: 'review', mode: runtimeMode() },
      });
    },
  );
  it.each([
    { tab: 'overview', purpose: 'review' },
    { tab: 'overview', purpose: 'implementation' },
    { tab: 'attention', purpose: 'review' },
  ] as const)(
    'captures $purpose purpose, mode and focus in $tab before lazy loading, without duplicate editor opens',
    async ({ tab, purpose }) => {
      TestBed.inject(Preferences).setTab(tab);
      const f = TestBed.createComponent(BoardView);
      await f.whenStable();
      const label = purpose === 'review' ? 'Create review ticket' : 'Create Jira ticket';
      const origin = (
        Array.from(f.nativeElement.querySelectorAll('app-pr-card button')) as HTMLButtonElement[]
      ).find((button) => button.textContent?.trim() === label)!;
      origin.focus();
      const selectedPr = pr();
      const opening = f.componentInstance.ticket(selectedPr, purpose);
      void f.componentInstance.ticket(pr({ id: 'another-pr' }), 'implementation');
      TestBed.inject(RuntimeMode).apply(
        runtimeMode({ read_only_test_mode: false, mode_revision: 'changed-during-import' }),
      );
      TestBed.inject(Preferences).setTab(tab === 'overview' ? 'attention' : 'overview');
      await opening;
      expect(open).toHaveBeenCalledOnce();
      expect(open.mock.calls[0][1]).toMatchObject({
        data: { kind: 'pr', pr: selectedPr, purpose, mode: runtimeMode() },
        restoreFocus: origin,
      });
    },
  );
  it('does not open a lazy editor after its board view is destroyed', async () => {
    const f = TestBed.createComponent(BoardView);
    const opening = f.componentInstance.ticket(pr(), 'review');
    f.destroy();
    await opening;
    expect(open).not.toHaveBeenCalled();
  });
  it('does not stack a delayed ticket editor over another dialog', async () => {
    const f = TestBed.createComponent(BoardView);
    const opening = f.componentInstance.ticket(pr(), 'review');
    openDialogs.push({});
    await opening;
    expect(open).not.toHaveBeenCalled();
  });
});
