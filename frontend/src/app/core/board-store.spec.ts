import { TestBed } from '@angular/core/testing';
import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { vi } from 'vitest';
import { BoardStore } from './board-store';
import { board, context, sync, runtimeMode } from '../testing/fixtures';
import type { SyncStatus } from './api-types';

describe('BoardStore lightweight polling and publication races', () => {
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
    http.verify();
    TestBed.resetTestingModule();
    vi.useRealTimers();
  });
  function flush(status: SyncStatus = sync(), revision = 1) {
    http.expectOne('/api/context').flush(context());
    http.expectOne('/api/sync').flush(status);
    http.expectOne('/api/health').flush({ status: 'ok', version: '1.2', ...runtimeMode() });
    http.expectOne('/api/board').flush(board({ revision }));
  }
  it('loads once and reads board after sync, never before publication observation', () => {
    store.start();
    store.start();
    expect(store.loading()).toBe(true);
    http.expectNone('/api/board');
    flush();
    expect(store.loading()).toBe(false);
    expect(store.board()?.revision).toBe(1);
  });
  it('polls only sync when unchanged and retains cards through fetching/publication', () => {
    store.start();
    flush(sync('syncing'));
    const old = store.board();
    vi.advanceTimersByTime(2000);
    const status = sync('syncing');
    status.progress[0] = {
      ...status.progress[0],
      phase: 'fetching',
      completed_items: 7,
      total_items: null,
      started_at: status.started_at,
      elapsed_ms: 3000,
    };
    http.expectOne('/api/sync').flush(status);
    http.expectNone('/api/board');
    http.expectNone('/api/context');
    expect(store.board()).toBe(old);
    vi.advanceTimersByTime(2000);
    http.expectOne('/api/sync').flush({
      ...status,
      progress: [{ ...status.progress[0], phase: 'publishing', total_items: 7 }],
    });
    http.expectNone('/api/board');
    expect(store.board()).toBe(old);
  });
  it('fetches publication when a follow-up started between polls, even without seeing idle', () => {
    store.start();
    flush(sync('syncing'));
    vi.advanceTimersByTime(2000);
    const followup = { ...sync('syncing'), started_at: '2026-11-06T12:01:00Z' };
    http.expectOne('/api/sync').flush(followup);
    flush(followup, 2);
    expect(store.board()?.revision).toBe(2);
    vi.advanceTimersByTime(2000);
    http.expectOne('/api/sync').flush(sync());
    flush(sync(), 3);
    expect(store.board()?.revision).toBe(3);
  });
  it('does not mask live failures with the old board successful health', () => {
    store.start();
    flush(sync('syncing'));
    vi.advanceTimersByTime(2000);
    const status = sync('syncing');
    status.sources[0] = { ...status.sources[0], status: 'error', error: 'Read failed.' };
    status.progress[0] = {
      ...status.progress[0],
      phase: 'failed',
      finished_at: '2026-11-06T12:00:02Z',
    };
    http.expectOne('/api/sync').flush(status);
    expect(store.sync()?.sources[0].status).toBe('error');
    expect(store.board()?.sources[0].status).toBe('ok');
    http.expectNone('/api/board');
  });
  it('allows write-triggered refresh coalescing during sync and does not miss finished POST', () => {
    store.start();
    flush(sync('syncing'));
    store.refresh();
    store.refresh();
    const r = http.expectOne((r) => r.url === '/api/sync' && r.method === 'POST');
    expect(r.request.body).toEqual({});
    expect(r.request.headers.get('X-PullBoard-Request')).toBe('1');
    r.flush(sync());
    flush(sync(), 2);
    expect(store.board()?.revision).toBe(2);
    expect(store.refreshing()).toBe(false);
  });
  it('retains last good board after failure and retries without overlapping reads', () => {
    store.start();
    flush();
    const old = store.board();
    store.load();
    store.load();
    http.expectOne('/api/context').flush(context());
    http.expectOne('/api/sync').flush(sync());
    http.expectOne('/api/health').flush({ status: 'ok', version: '1.2', ...runtimeMode() });
    http
      .expectOne('/api/board')
      .flush(
        { error: { code: 'cache_failed', message: 'Cache unavailable.' } },
        { status: 503, statusText: 'Unavailable' },
      );
    expect(store.board()).toBe(old);
    expect(store.error()).toBe('Cache unavailable.');
    store.reload();
    flush();
    expect(store.error()).toBe('');
  });
  it('automatically retries an unchanged finished publication after a transient board read failure', () => {
    store.start();
    flush(sync(), 1);
    const old = store.board();
    const published = {
      ...sync(),
      started_at: '2026-11-06T13:00:00Z',
      finished_at: '2026-11-06T13:00:01Z',
      last_success_at: '2026-11-06T13:00:01Z',
    };
    vi.advanceTimersByTime(30_000);
    http.expectOne('/api/sync').flush(published);
    http.expectOne('/api/context').flush(context());
    http.expectOne('/api/sync').flush(published);
    http.expectOne('/api/health').flush({ status: 'ok', version: '1.2', ...runtimeMode() });
    http
      .expectOne('/api/board')
      .flush(
        { error: { code: 'cache_failed', message: 'Transient cache read failed.' } },
        { status: 503, statusText: 'Unavailable' },
      );
    expect(store.board()).toBe(old);
    expect(store.error()).toBe('Transient cache read failed.');
    vi.advanceTimersByTime(29_999);
    http.expectNone('/api/sync');
    vi.advanceTimersByTime(1);
    http.expectOne('/api/sync').flush(published);
    // Same status, no new upstream job or explicit retry: fetch again.
    http.expectNone('/api/board');
    flush(published, 2);
    expect(store.board()?.revision).toBe(2);
    expect(store.error()).toBe('');
    vi.advanceTimersByTime(30_000);
    http.expectOne('/api/sync').flush(published);
    http.expectNone('/api/board');
    http.expectNone('/api/context');
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
  it('does not let an older full read mask live POST health while its board response is pending', () => {
    store.start();
    http.expectOne('/api/context').flush(context());
    http.expectOne('/api/sync').flush(sync('syncing'));
    http.expectOne('/api/health').flush({ status: 'ok', version: '1.2', ...runtimeMode() });
    const oldBoard = http.expectOne('/api/board');
    store.refresh();
    const current = sync('syncing');
    current.sources[0] = { ...current.sources[0], status: 'error', error: 'Current failure.' };
    current.progress[0] = {
      ...current.progress[0],
      phase: 'failed',
      finished_at: '2026-11-06T12:00:02Z',
    };
    http.expectOne((r) => r.method === 'POST').flush(current);
    oldBoard.flush(board());
    expect(store.sync()?.sources[0].error).toBe('Current failure.');
    // Initial publication fingerprint differs; one queued reload reconciles cards.
    flush(current);
    expect(store.sync()?.sources[0].error).toBe('Current failure.');
  });
});
