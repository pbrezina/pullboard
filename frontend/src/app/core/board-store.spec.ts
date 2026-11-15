import { TestBed } from '@angular/core/testing';
import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { vi } from 'vitest';
import { BoardStore } from './board-store';
import { board, context, sync, runtimeMode } from '../testing/fixtures';
import type { SyncStatus } from './api-types';

describe('BoardStore revision-aware polling and publication races', () => {
  let store: BoardStore;
  let http: HttpTestingController;
  beforeEach(() => {
    vi.useFakeTimers();
    TestBed.configureTestingModule({
      providers: [provideHttpClient(), provideHttpClientTesting()],
    });
    store = TestBed.inject(BoardStore);
    http = TestBed.inject(HttpTestingController);
  });
  afterEach(() => {
    try {
      http.verify();
    } finally {
      TestBed.resetTestingModule();
      vi.useRealTimers();
    }
  });
  const status = (revision = 1, state: SyncStatus['state'] = 'syncing'): SyncStatus => ({
    ...sync(state),
    board_revision: revision,
  });
  function dependencies(revision = 1) {
    http.expectOne('/api/context').flush(context());
    http.expectOne('/api/health').flush({ status: 'ok', version: '1.5', ...runtimeMode() });
    http.expectOne('/api/board').flush(board({ revision }));
  }
  function flush(s = status(), revision = s.board_revision) {
    http.expectOne('/api/sync').flush(s);
    dependencies(revision);
  }
  function poll(s: SyncStatus, delay = 2000) {
    vi.advanceTimersByTime(delay);
    http.expectOne('/api/sync').flush(s);
  }
  it('loads once, ordering both context and board after sync', () => {
    store.start();
    store.start();
    http.expectNone('/api/context');
    http.expectNone('/api/board');
    flush();
    expect(store.loading()).toBe(false);
    expect(store.board()?.revision).toBe(1);
  });
  it('polls progress only without reloading unchanged publications, including publishing', () => {
    store.start();
    flush();
    const old = store.board();
    for (const phase of ['fetching', 'publishing', 'completed'] as const) {
      const s = status();
      s.progress[0].phase = phase;
      poll(s);
      expect(store.board()).toBe(old);
      http.expectNone('/api/board');
      http.expectNone('/api/context');
    }
  });
  it('loads every observed mid-job commit and options-only/quarantine commits without phase changes', () => {
    store.start();
    flush();
    for (const revision of [2, 4, 5]) {
      const s = status(revision);
      poll(s);
      flush(s);
      expect(store.board()?.revision).toBe(revision);
      expect(store.sync()?.state).toBe('syncing');
    }
  });
  it('detects missed completed/idle phases and follow-ups solely by revision', () => {
    store.start();
    flush();
    const followup = { ...status(3), started_at: '2026-11-06T13:00:00Z' };
    poll(followup);
    flush(followup);
    expect(store.board()?.revision).toBe(3);
    const idle = status(4, 'idle');
    poll(idle);
    flush(idle);
    expect(store.board()?.revision).toBe(4);
    poll(status(5, 'idle'), 30_000);
    flush(status(5, 'idle'));
    expect(store.board()?.revision).toBe(5);
  });
  it.each(['context', 'board'] as const)(
    'does not consume a revision after failed %s reads; unchanged status retries',
    (failed) => {
      store.start();
      flush();
      const old = store.board(),
        oldContext = store.context();
      poll(status(2));
      http.expectOne('/api/sync').flush(status(2));
      const ctx = http.expectOne('/api/context');
      if (failed === 'context')
        ctx.flush(
          { error: { code: 'read_failed', message: 'Read failed.' } },
          { status: 503, statusText: 'Unavailable' },
        );
      else ctx.flush(context());
      http.expectOne('/api/health').flush({ status: 'ok', version: '1.5', ...runtimeMode() });
      const snapshot = http.expectOne('/api/board');
      if (failed === 'board')
        snapshot.flush(
          { error: { code: 'read_failed', message: 'Read failed.' } },
          { status: 503, statusText: 'Unavailable' },
        );
      else snapshot.flush(board({ revision: 2 }));
      expect(store.board()).toBe(old);
      expect(store.context()).toBe(oldContext);
      expect(store.error()).toBe('Read failed.');
      poll(status(2));
      flush(status(2));
      expect(store.board()?.revision).toBe(2);
      expect(store.error()).toBe('');
      poll(status(2));
      http.expectNone('/api/context');
    },
  );
  it('does not acknowledge a newer board as a newer context/status read', () => {
    store.start();
    flush(status(1), 3);
    poll(status(3));
    flush(status(3));
    expect(store.board()?.revision).toBe(3);
    poll(status(3));
    http.expectNone('/api/context');
  });
  it('retries the last observed target when the full-cycle status read fails', () => {
    store.start();
    flush();
    poll(status(2));
    http
      .expectOne('/api/sync')
      .flush(
        { error: { code: 'read_failed', message: 'Status temporarily unavailable.' } },
        { status: 503, statusText: 'Unavailable' },
      );
    dependencies(2);
    expect(store.sync()?.board_revision).toBe(2);
    expect(store.board()?.revision).toBe(2);
    poll(status(2));
    flush(status(2));
    poll(status(2));
    http.expectNone('/api/context');
  });
  it('discards regressing board/context responses without swallowing their target revision', () => {
    store.start();
    flush(status(1), 3);
    const old = store.board(),
      oldContext = store.context();
    poll(status(4));
    flush(status(4), 2);
    expect(store.board()).toBe(old);
    expect(store.context()).toBe(oldContext);
    poll(status(4));
    flush(status(4));
    expect(store.board()?.revision).toBe(4);
  });
  it('allows a reinitialized cache after a fresh lower status revision, without ordering mode tokens', () => {
    store.start();
    flush(status(5));
    poll(status(0));
    flush(status(0));
    expect(store.board()?.revision).toBe(0);
    poll(status(0));
    http.expectNone('/api/context');
  });
  it('preserves latest POST health and discards an older pending board/context cycle', () => {
    store.start();
    http.expectOne('/api/sync').flush(status());
    http.expectOne('/api/context').flush(context());
    http.expectOne('/api/health').flush({ status: 'ok', version: '1.5', ...runtimeMode() });
    const lateBoard = http.expectOne('/api/board');
    store.refresh();
    store.refresh();
    const current = status(2);
    current.sources[0] = { ...current.sources[0], status: 'error', error: 'Current failure.' };
    http.expectOne((r) => r.method === 'POST').flush(current);
    lateBoard.flush(board());
    expect(store.board()).toBeNull();
    expect(store.sync()?.sources[0].error).toBe('Current failure.');
    flush(current);
    expect(store.board()?.revision).toBe(2);
    expect(store.sync()?.sources[0].error).toBe('Current failure.');
  });
  it('ignores a late poll started before a newer POST', () => {
    store.start();
    flush();
    vi.advanceTimersByTime(2000);
    const late = http.expectOne((r) => r.method === 'GET' && r.url === '/api/sync');
    store.refresh();
    http.expectOne((r) => r.method === 'POST').flush(status(1, 'idle'));
    late.flush(status());
    expect(store.sync()?.state).toBe('idle');
  });
  it('retains current cards on live source failure without inventing a publication', () => {
    store.start();
    flush();
    const s = status();
    s.sources[0] = { ...s.sources[0], status: 'error', error: 'Read failed.' };
    s.progress[0].phase = 'failed';
    poll(s);
    expect(store.sync()?.sources[0].status).toBe('error');
    expect(store.board()?.sources[0].status).toBe('ok');
    http.expectNone('/api/context');
  });
  it('retries status failures without overlapping reads and allows explicit queued reload', () => {
    store.start();
    flush();
    vi.advanceTimersByTime(2000);
    http
      .expectOne('/api/sync')
      .flush(
        { error: { code: 'read_failed', message: 'Retry later.' } },
        { status: 503, statusText: 'Unavailable' },
      );
    expect(store.error()).toBe('Retry later.');
    store.reload();
    store.load();
    store.reload();
    flush();
    flush();
    expect(store.error()).toBe('');
  });
  it('gates options readiness independently and handles no Jira', () => {
    for (const ready of [false, true]) {
      const s = status();
      s.jira_options!.ready = ready;
      store.sync.set(s);
      expect(store.jiraOptionsReady()).toBe(ready);
    }
    store.sync.set({ ...status(), jira_options: null });
    expect(store.jiraOptionsReady()).toBe(false);
  });
  it('requires a committed options generation, then accepts it before any cards complete', () => {
    const s = status();
    s.progress = s.progress.map((p) => ({
      ...p,
      phase: 'fetching',
      started_at: s.started_at,
      completed_items: 8,
      total_items: 55,
      elapsed_ms: 15000,
    }));
    s.jira_options = {
      ...s.jira_options!,
      ready: false,
      health: { ...s.jira_options!.health, status: 'pending', last_success_at: null },
      progress: {
        ...s.jira_options!.progress,
        phase: 'discovering',
        started_at: s.started_at,
        elapsed_ms: 14000,
        groups: [{ name: 'Reviewers', phase: 'completed', completed_items: 75, total_items: 75 }],
        projects: [{ name: 'DEV', phase: 'fetching', completed_items: 28, total_items: 75 }],
      },
    };
    store.start();
    flush(s);
    expect(store.jiraOptionsReady()).toBe(false);
    const allChecked = {
      ...s,
      jira_options: {
        ...s.jira_options,
        progress: {
          ...s.jira_options.progress,
          projects: [
            { name: 'DEV', phase: 'completed' as const, completed_items: 75, total_items: 75 },
          ],
        },
      },
    };
    poll(allChecked);
    expect(store.jiraOptionsReady()).toBe(false);
    http.expectNone('/api/context');
    http.expectNone('/api/board');
    const committed: SyncStatus = {
      ...s,
      board_revision: 2,
      jira_options: {
        ...allChecked.jira_options,
        ready: true,
        health: sync().jira_options!.health,
        progress: {
          ...allChecked.jira_options.progress,
          phase: 'completed',
          finished_at: '2026-11-06T12:00:14Z',
        },
      },
    };
    poll(committed);
    flush(committed);
    expect(store.jiraOptionsReady()).toBe(true);
    expect(store.sync()?.state).toBe('syncing');
    expect(store.sync()?.progress.every((p) => p.phase === 'fetching')).toBe(true);
    expect(store.board()?.revision).toBe(2);
    http.expectNone('/api/jira/options');
  });
  it('surfaces refresh failures and cancels timers and pending requests on destruction', () => {
    store.refresh();
    http
      .expectOne('/api/sync')
      .flush(
        { error: { code: 'backoff', message: 'Retry later.' } },
        { status: 429, statusText: 'Backoff' },
      );
    expect(store.error()).toBe('Retry later.');
    expect(store.refreshing()).toBe(false);
    store.start();
    const requests = http.match(() => true);
    TestBed.resetTestingModule();
    expect(requests.every((r) => r.cancelled)).toBe(true);
    vi.advanceTimersByTime(60_000);
  });
});
