import { TestBed, ComponentFixture } from '@angular/core/testing';
import { signal } from '@angular/core';
import { vi } from 'vitest';
import { ForgeAggregatorService } from '../core/forge-aggregator';
import { PullRequestWorkItem } from '../core/models';
import { testPr } from '../core/testing';
import { PullRequests } from './pull-requests';
import { JiraService } from '../core/jira';
import { PrJiraLinks } from '../core/pr-jira-links';
import { ReviewTickets } from '../core/review-tickets';

describe('PullRequests', () => {
  let fixture: ComponentFixture<PullRequests>;
  const forges = { getPullRequestInbox: vi.fn() };
  const jira = { getPullRequestLinks: vi.fn() };
  const reviews = {
    states: signal({}),
    receipts: signal({}),
    storageWarning: signal(''),
    busy: signal(false),
    initialize: async () => {},
    create: vi.fn(),
  };
  const requested: PullRequestWorkItem = {
    ...testPr,
    author: 'review-author',
    needsAction: false,
    reviewStatus: 'outstanding',
    createdAt: '2026-10-01T10:00:00Z',
    reviewRequestedAt: '2026-11-01T10:00:00Z',
    updatedAt: '2026-11-04T10:00:00Z',
    labels: ['Zulu', 'accepted', 'Alpha'],
  };
  const mentioned: PullRequestWorkItem = {
    ...testPr,
    id: 'mention',
    number: 43,
    url: 'https://github.com/org/repo/pull/43',
    title: 'Check release notes',
    author: 'colleague',
    needsAction: false,
    mentionedAt: '2026-11-02T10:00:00Z',
    updatedAt: '2026-11-02T10:00:00Z',
    draft: true,
  };
  beforeEach(async () => {
    vi.resetAllMocks();
    forges.getPullRequestInbox.mockResolvedValue({
      requested: [mentioned, requested],
      opened: [{ ...testPr, needsAction: true }],
      identities: [{ providerId: 'forge-0', label: 'GitHub', host: 'github.com', username: 'dev' }],
      warnings: [],
    });
    await TestBed.configureTestingModule({
      imports: [PullRequests],
      providers: [
        { provide: ForgeAggregatorService, useValue: forges },
        { provide: JiraService, useValue: jira },
        { provide: ReviewTickets, useValue: reviews },
      ],
    }).compileComponents();
    fixture = TestBed.createComponent(PullRequests);
    await fixture.whenStable();
  });
  afterEach(() => {
    vi.useRealTimers();
    fixture.destroy();
  });

  it('loads both columns and orders by waiting time without waiting for JIRA links', () => {
    const element: HTMLElement = fixture.nativeElement;
    expect(element.textContent).toContain('Requested reviews from me');
    expect(element.textContent).toContain('My opened pull requests');
    const cards = element.querySelectorAll<HTMLElement>('.board-column:first-child .pr-card');
    expect(cards[0].textContent).toContain('Review outstanding');
    expect(cards[0].classList.contains('review-card')).toBe(true);
    expect(cards[1].textContent).toContain('Mentioned');
    expect(cards[1].classList.contains('mention-card')).toBe(true);
    expect(element.querySelector('.action-card')?.textContent).toContain('Needs your action');
  });

  it('shows sorted labels, accepted highlighting, draft and ready states, and exact activity times', () => {
    const element: HTMLElement = fixture.nativeElement;
    const accepted = element.querySelector('.accepted-card')!;
    expect(
      [...accepted.querySelectorAll('.label-badge')].map((badge) => badge.textContent?.trim()),
    ).toEqual(['accepted', 'Alpha', 'Zulu']);
    expect(accepted.querySelector('.accepted-badge')).toBeTruthy();
    expect(accepted.textContent).toContain('Ready for review');
    expect(element.querySelector('.mention-card')?.textContent).toContain('Draft');
    expect(
      accepted.querySelector('.last-activity time[title]')?.getAttribute('title'),
    ).toBeTruthy();
    const activity = accepted.querySelector('.card-meta app-pr-activity')!;
    expect(activity.querySelector('.created strong')?.textContent?.trim()).toBe('2026-10-01');
    expect(activity.querySelector('.last-activity')?.textContent).toContain('Last activity:');
    expect(activity.querySelector('.last-activity strong')?.textContent).not.toContain(
      'Last activity',
    );
    expect(activity.querySelector('.waiting-since')?.textContent).toContain('Waiting since:');
    expect(activity.querySelector('.waiting-since strong')?.textContent).not.toContain(
      'Waiting since',
    );
    const metadataSize = getComputedStyle(accepted.querySelector('.pr-people')!).fontSize;
    expect(getComputedStyle(accepted.querySelector('.last-activity')!).fontSize).toBe(metadataSize);
    expect(getComputedStyle(accepted.querySelector('.waiting-since')!).fontSize).toBe(metadataSize);
    expect(accepted.querySelector('p.last-activity')).toBeNull();
    expect(accepted.querySelector('p.waiting-since')).toBeNull();
    expect(getComputedStyle(accepted.querySelector('.last-activity')!).borderLeftStyle).not.toBe(
      'solid',
    );
    expect(element.textContent).toContain('Last refreshed');
    expect(element.textContent).toContain('Manual refresh only');
  });

  it('shows each PR title only once at the top of the card in both columns', () => {
    const element: HTMLElement = fixture.nativeElement;
    const cards = element.querySelectorAll('.pr-card');
    expect(cards).toHaveLength(3);
    for (const card of cards) {
      expect(card.querySelectorAll('h3')).toHaveLength(1);
      const title = card.querySelector('mat-card-content > h3:first-child');
      expect(title?.textContent?.trim()).toBeTruthy();
      expect(title?.nextElementSibling?.classList.contains('card-meta')).toBe(true);
      expect(card.querySelector('.card-meta + app-pr-badges')).toBeTruthy();
    }
  });

  it('filters each column locally by title, repository, number or label', async () => {
    const dashboard = fixture.componentInstance;
    dashboard['filters'].set({ requested: '#43', opened: 'no match' });
    await fixture.whenStable();
    expect(dashboard['columns']()[0].items.map((pr) => pr.id)).toEqual(['mention']);
    expect(dashboard['columns']()[1].items).toEqual([]);
    dashboard['filters'].set({ requested: 'ACCEPTED', opened: 'org/repo' });
    await fixture.whenStable();
    expect(dashboard['columns']()[0].items.map((pr) => pr.id)).toEqual([testPr.id]);
    expect(dashboard['columns']()[1].items).toHaveLength(1);
    expect(forges.getPullRequestInbox).toHaveBeenCalledTimes(1);
  });

  it('keeps past reviews visible and distinguishes them from outstanding reviews', async () => {
    fixture.componentInstance['requested'].set([{ ...requested, reviewStatus: 'given' }]);
    await fixture.whenStable();
    expect(
      fixture.nativeElement.querySelector('.pr-card .review-given-badge')?.textContent,
    ).toContain('Review given');
    expect(fixture.nativeElement.querySelector('.pr-card .review-badge')).toBeNull();
  });

  it('shows activity errors without hiding usable cards', async () => {
    fixture.componentInstance['requested'].set([
      { ...requested, attentionError: 'Activity is incomplete' },
    ]);
    await fixture.whenStable();
    expect(fixture.nativeElement.querySelector('.pr-card [role="alert"]').textContent).toContain(
      'Activity is incomplete',
    );
  });

  it('never polls, but reloads through the manual refresh button', async () => {
    vi.useFakeTimers();
    await vi.advanceTimersByTimeAsync(5 * 60000);
    expect(forges.getPullRequestInbox).toHaveBeenCalledTimes(1);
    vi.useRealTimers();
    fixture.nativeElement.querySelector('button').click();
    await fixture.whenStable();
    expect(forges.getPullRequestInbox).toHaveBeenCalledTimes(2);
  });

  it('preserves stale data on refresh failure and supports retry', async () => {
    forges.getPullRequestInbox.mockRejectedValueOnce(new Error('Network offline'));
    await fixture.componentInstance['refresh']();
    await fixture.whenStable();
    expect(fixture.nativeElement.textContent).toContain('Network offline');
    expect(fixture.nativeElement.textContent).toContain('stale');
    expect(fixture.componentInstance['requested']()).toHaveLength(2);
    await fixture.componentInstance['refresh']();
    expect(fixture.componentInstance['error']()).toBe('');
  });

  it('puts opened PRs needing action first, preserving activity order inside each group', async () => {
    fixture.componentInstance['opened'].set([
      { ...requested, author: 'dev', needsAction: true },
      { ...mentioned, author: 'dev', needsAction: false },
    ]);
    await fixture.whenStable();
    expect(fixture.componentInstance['columns']()[1].items.map((pr) => pr.id)).toEqual([
      testPr.id,
      'mention',
    ]);
  });

  it('hides Blocked/Deferred reviews and mentions case-insensitively, but not opened PRs', async () => {
    const blocked = { ...requested, labels: ['bLoCkEd'] };
    const deferred = { ...mentioned, labels: ['DEFERRED'] };
    fixture.componentInstance['requested'].set([blocked, deferred]);
    fixture.componentInstance['opened'].set([blocked, deferred]);
    await fixture.whenStable();
    expect(fixture.componentInstance['columns']()[0].items).toEqual([]);
    expect(fixture.componentInstance['columns']()[0].total).toBe(0);
    expect(fixture.componentInstance['columns']()[1].items).toHaveLength(2);
  });

  it('puts pending requests and mentions before older completed reviews', async () => {
    fixture.componentInstance['requested'].set([
      { ...requested, id: 'given', reviewStatus: 'given', updatedAt: '2020-01-01T00:00:00Z' },
      mentioned,
      requested,
    ]);
    await fixture.whenStable();
    expect(fixture.componentInstance['columns']()[0].items.map((pr) => pr.id)).toEqual([
      testPr.id,
      'mention',
      'given',
    ]);
  });

  it('highlights reviews older than 30 days using waiting age or inactive age', async () => {
    fixture.componentInstance['refreshedAt'].set(Date.parse('2026-11-04T10:00:00Z'));
    fixture.componentInstance['requested'].set([
      { ...requested, reviewRequestedAt: '2026-10-01T10:00:00Z' },
      { ...mentioned, mentionedAt: '2026-11-03T10:00:00Z', updatedAt: '2020-01-01T00:00:00Z' },
      { ...requested, id: 'given', reviewStatus: 'given', updatedAt: '2026-10-01T10:00:00Z' },
    ]);
    await fixture.whenStable();
    const cards = fixture.nativeElement.querySelectorAll('.aged-review-card');
    expect(cards).toHaveLength(2);
    expect(cards[0].textContent).toContain('Waiting over 30 days');
    expect(cards[1].textContent).toContain('Inactive over 30 days');
  });

  it('shows locally known links without making a project-wide JIRA query', async () => {
    TestBed.inject(PrJiraLinks).remember(testPr.url, [
      { key: 'DEV-9', url: 'https://team.atlassian.net/browse/DEV-9' },
    ]);
    await fixture.whenStable();
    expect(fixture.nativeElement.querySelector('.linked-tickets a')?.textContent).toBe('DEV-9');
    await fixture.componentInstance['refresh']();
    await fixture.whenStable();
    expect(jira.getPullRequestLinks).not.toHaveBeenCalled();
    expect(fixture.nativeElement.textContent).not.toContain('Loading JIRA links');
    expect(fixture.nativeElement.textContent).not.toContain('No known JIRA links');
  });

  it('offers review ticket creation on every card in both columns', async () => {
    const actions = fixture.nativeElement.querySelectorAll('app-review-ticket-actions');
    expect(actions).toHaveLength(3);
    actions[0].querySelector('button').click();
    await fixture.whenStable();
    expect(reviews.create).toHaveBeenCalledWith(requested);
    actions[2].querySelector('button').click();
    await fixture.whenStable();
    expect(reviews.create).toHaveBeenLastCalledWith(
      expect.objectContaining({ author: testPr.author }),
    );
  });
});
