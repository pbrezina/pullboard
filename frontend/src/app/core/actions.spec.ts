import { TestBed } from '@angular/core/testing';
import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { vi } from 'vitest';
import { Actions } from './actions';
import { BoardStore } from './board-store';
import { Notifications } from './notifications';
import { board, createRequest, pr, runtimeMode } from '../testing/fixtures';

describe('Explicit Jira actions', () => {
  let actions: Actions;
  let http: HttpTestingController;
  let store: BoardStore;
  let notices: Notifications;
  beforeEach(() => {
    TestBed.configureTestingModule({
      providers: [provideHttpClient(), provideHttpClientTesting()],
    });
    actions = TestBed.inject(Actions);
    http = TestBed.inject(HttpTestingController);
    store = TestBed.inject(BoardStore);
    store.board.set(board());
    notices = TestBed.inject(Notifications);
  });
  afterEach(() => http.verify());
  it('treats simulated create as normal success without IDs, writes to cache or refresh', () => {
    const before = store.board();
    const refresh = vi.spyOn(store, 'refresh');
    actions.create(createRequest()).subscribe((result) => expect(result.issue).toBeNull());
    http
      .expectOne('/api/jira/issues')
      .flush({ outcome: 'simulated', issue: null, message: 'Creation validated.', warnings: [] });
    expect(notices.items()[0].message).toBe('Simulated: Creation validated.');
    expect(notices.items()[0].url).toBeNull();
    expect(store.board()).toBe(before);
    expect(refresh).not.toHaveBeenCalled();
  });
  it('does not fabricate a link or refresh on simulated linking', () => {
    const before = store.board();
    const refresh = vi.spyOn(store, 'refresh');
    actions.link('DEV-1', pr().id, runtimeMode());
    http.expectOne('/api/jira/issues/DEV-1/links').flush({
      outcome: 'simulated',
      issue_key: 'DEV-1',
      pull_request_id: pr().id,
      message: 'Link validated.',
    });
    expect(notices.items()[0].message).toContain('Simulated:');
    expect(store.board()).toBe(before);
    expect(store.board()?.issues[0].linked_pr_ids).toEqual([]);
    expect(refresh).not.toHaveBeenCalled();
  });
  it('coalesces duplicate in-flight links and reports already linked', () => {
    vi.spyOn(store, 'refresh').mockImplementation(() => {});
    actions.link('DEV-1', pr().id, runtimeMode());
    actions.link('DEV-1', pr().id, runtimeMode());
    const r = http.expectOne('/api/jira/issues/DEV-1/links');
    expect(actions.linking().size).toBe(1);
    r.flush({
      outcome: 'already_linked',
      issue_key: 'DEV-1',
      pull_request_id: pr().id,
      message: 'The PR is already linked.',
    });
    expect(actions.linking().size).toBe(0);
    expect(notices.items()[0].message).toContain('Already linked');
  });
  it('refreshes only after real success, and preserves post-creation warnings', () => {
    const refresh = vi.spyOn(store, 'refresh').mockImplementation(() => {});
    actions.create(createRequest()).subscribe();
    http.expectOne('/api/jira/issues').flush({
      outcome: 'created',
      issue: { key: 'DEV-23', url: 'https://jira.example/DEV-23', title: 'Review snapshot' },
      message: 'Ticket created.',
      warnings: ['Transition was not applied.'],
    });
    expect(refresh).toHaveBeenCalledOnce();
    expect(notices.items()[0].label).toBe('DEV-23');
    expect(notices.items()).toHaveLength(1);
    expect(notices.items()[0].message).toContain('Ticket created.');
    expect(notices.items()[0].message).toContain('Transition');
    expect(notices.items()[0].kind).toBe('warning');
    expect(store.board()?.issues).toHaveLength(1);
  });
  it('surfaces sanitized link failure without changing the board', () => {
    const before = store.board();
    actions.link('DEV-1', pr().id, runtimeMode());
    http
      .expectOne('/api/jira/issues/DEV-1/links')
      .flush(
        { error: { code: 'upstream_failure', message: 'Jira could not link this PR.' } },
        { status: 502, statusText: 'Upstream' },
      );
    expect(notices.items()[0].kind).toBe('error');
    expect(actions.linking().size).toBe(0);
    expect(store.board()).toBe(before);
  });
  it('accepts created/null as success plus warning, without fake ID or duplicate retry', () => {
    const refresh = vi.spyOn(store, 'refresh').mockImplementation(() => {});
    actions.create(createRequest()).subscribe();
    http.expectOne('/api/jira/issues').flush({
      outcome: 'created',
      issue: null,
      message: 'Jira accepted creation. Inspect Jira; do not resubmit.',
      warnings: ['No confirmed issue key.'],
    });
    expect(notices.items()[0].kind).toBe('warning');
    expect(notices.items()[0].url).toBeNull();
    expect(notices.items()[0].label).toBeNull();
    expect(notices.items()).toHaveLength(1);
    expect(refresh).toHaveBeenCalledOnce();
    http.expectNone('/api/jira/issues');
  });
});
