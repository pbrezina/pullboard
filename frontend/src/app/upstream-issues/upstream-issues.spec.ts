import { TestBed } from '@angular/core/testing';
import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { Dialog } from '@angular/cdk/dialog';
import { vi } from 'vitest';
import { UpstreamIssues } from './upstream-issues';
import { BoardStore } from '../core/board-store';
import { Preferences, preferenceKey } from '../core/preferences';
import { RuntimeMode } from '../core/runtime-mode';
import { defaultFilters, repositoryFilterValue } from '../core/board-logic';
import { board, context, runtimeMode, upstreamIssue } from '../testing/fixtures';

describe('Upstream issue work list', () => {
  let http: HttpTestingController;
  let store: BoardStore;
  const open = vi.fn();
  beforeEach(() => {
    localStorage.clear();
    history.replaceState(null, '', '/');
    open.mockReset();
    TestBed.configureTestingModule({
      providers: [
        provideHttpClient(),
        provideHttpClientTesting(),
        { provide: Dialog, useValue: { open, openDialogs: [] } },
      ],
    });
    http = TestBed.inject(HttpTestingController);
    store = TestBed.inject(BoardStore);
    store.board.set(
      board({
        upstream_issues: [
          upstreamIssue({ id: 'z', title: 'zulu' }),
          upstreamIssue({ id: 'b', title: 'Alpha' }),
          upstreamIssue({ id: 'a', title: 'alpha', reporter: null }),
          upstreamIssue({
            id: 'p',
            title: 'Personal issue',
            source_id: 'personal',
            repository: 'other',
          }),
        ],
      }),
    );
    store.context.set(context());
    TestBed.inject(RuntimeMode).apply(runtimeMode());
  });
  afterEach(() => http.verify());
  it('renders title-ordered full-width rows with metadata, labels, snippets and nullable reporter, not PR cards', async () => {
    const f = TestBed.createComponent(UpstreamIssues);
    await f.whenStable();
    expect(f.componentInstance.issues().map((i) => i.id)).toEqual(['a', 'b', 'p', 'z']);
    expect(f.nativeElement.querySelectorAll('.upstream-row')).toHaveLength(4);
    expect(f.nativeElement.querySelector('.work-card')).toBeNull();
    expect(f.nativeElement.textContent).toContain('Reporter:  unavailable');
    expect(f.nativeElement.textContent).toContain('Keep the first line.');
    expect(f.nativeElement.textContent).toContain('acme/widget');
    expect(f.nativeElement.querySelector('input[type="search"]').placeholder).toContain(
      'issue titles',
    );
    expect(f.nativeElement.querySelectorAll('input[type="search"]')).toHaveLength(1);
    expect(f.nativeElement.textContent).not.toContain('Sprint');
    expect(f.nativeElement.textContent).toContain('bug');
  });
  it('keeps independent case-insensitive forge/repository/title preferences without content or lookup selections', async () => {
    const f = TestBed.createComponent(UpstreamIssues);
    const p = TestBed.inject(Preferences);
    p.setFilters('attention', { ...defaultFilters(), prTitle: 'unrelated' });
    p.setFilters('upstream', { ...defaultFilters(), source: 'work', prTitle: ' ALPHA ' });
    f.componentInstance.model.set({
      url: 'https://forge.example/private/secret/issues/7',
      source: 'personal',
    });
    await f.whenStable();
    expect(f.componentInstance.issues().map((i) => i.id)).toEqual(['a', 'b']);
    expect(f.nativeElement.textContent).toContain('2 hidden by filters');
    p.setFilters('upstream', {
      ...defaultFilters(),
      repository: repositoryFilterValue(
        upstreamIssue({ source_id: 'personal', repository: 'other' }),
      ),
    });
    await f.whenStable();
    expect(f.componentInstance.issues().map((i) => i.id)).toEqual(['p']);
    expect(p.value().attention.prTitle).toBe('unrelated');
    const saved = localStorage.getItem(preferenceKey)!;
    expect(saved).not.toContain('secret');
    expect(saved).not.toContain('Keep the first line');
    expect(saved).not.toContain('mode-test-1');
    expect(saved).not.toContain('lookup');
    expect(location.search).not.toContain('secret');
  });
  it('opens the shared implementation editor directly from a row without resolving or creating', async () => {
    const f = TestBed.createComponent(UpstreamIssues);
    await f.whenStable();
    f.nativeElement.querySelector('.upstream-row button').click();
    await vi.waitFor(() => expect(open).toHaveBeenCalledOnce());
    expect(open.mock.calls[0][1].data).toMatchObject({
      kind: 'upstream',
      issue: { id: 'a' },
      purpose: 'implementation',
      mode: runtimeMode(),
    });
    http.expectNone('/api/upstream-issues/resolve');
    http.expectNone('/api/upstream-issues/jira-issues');
  });
  it('validates malformed URLs without a request and focuses the input', async () => {
    const f = TestBed.createComponent(UpstreamIssues);
    await f.whenStable();
    f.componentInstance.model.set({ url: 'not a URL', source: '' });
    f.componentInstance.resolve(new Event('submit'));
    await f.whenStable();
    expect(f.componentInstance.error()).toContain('HTTP(S)');
    expect(document.activeElement).toBe(f.nativeElement.querySelector('input[type="url"]'));
    http.expectNone('/api/upstream-issues/resolve');
    expect(open).not.toHaveBeenCalled();
  });
  it('shows authoritative URL/ambiguity errors and supports explicit instance selection on retry', async () => {
    const f = TestBed.createComponent(UpstreamIssues);
    const c = f.componentInstance;
    c.model.set({ url: upstreamIssue().url, source: '' });
    c.resolve(new Event('submit'));
    const r = await vi.waitFor(() => http.expectOne('/api/upstream-issues/resolve'));
    expect(r.request.body).toEqual({ source_id: null, url: upstreamIssue().url });
    expect(r.request.headers.get('X-PullBoard-Request')).toBe('1');
    r.flush(
      {
        error: { code: 'ambiguous_upstream_source', message: 'Select a matching forge instance.' },
      },
      { status: 422, statusText: 'Invalid' },
    );
    await f.whenStable();
    expect(f.nativeElement.textContent).toContain('Select a matching forge instance');
    expect(open).not.toHaveBeenCalled();
    c.model.update((m) => ({ ...m, source: 'work' }));
    c.resolve(new Event('submit'));
    const retry = await vi.waitFor(() => http.expectOne('/api/upstream-issues/resolve'));
    expect(retry.request.body.source_id).toBe('work');
    retry.flush({ issue: upstreamIssue(), warning: null });
    await f.whenStable();
    await vi.waitFor(() => expect(open).toHaveBeenCalledOnce());
  });
  it('retains the gesture mode through asynchronous closed/unassigned resolution and leaves Board untouched', async () => {
    const f = TestBed.createComponent(UpstreamIssues);
    const c = f.componentInstance;
    const snapshot = store.board();
    c.model.set({ url: upstreamIssue().url, source: '' });
    c.resolve(new Event('submit'));
    const r = await vi.waitFor(() => http.expectOne('/api/upstream-issues/resolve'));
    c.resolve(new Event('submit'));
    http.expectNone('/api/upstream-issues/resolve');
    TestBed.inject(RuntimeMode).apply(
      runtimeMode({ read_only_test_mode: false, mode_revision: 'live-2' }),
    );
    const closed = upstreamIssue({
      id: 'manual',
      state: 'closed',
      is_assigned_to_me: false,
      assignees: [],
    });
    r.flush({ issue: closed, warning: 'This upstream issue is closed.' });
    await f.whenStable();
    await vi.waitFor(() => expect(open).toHaveBeenCalledOnce());
    expect(c.warning()).toBe('This upstream issue is closed.');
    expect(open.mock.calls[0][1].data).toEqual({
      kind: 'upstream',
      issue: closed,
      warning: c.warning(),
      purpose: 'implementation',
      mode: runtimeMode(),
    });
    expect(store.board()).toBe(snapshot);
    expect(store.board()?.upstream_issues.some((i) => i.id === 'manual')).toBe(false);
    http.expectNone('/api/upstream-issues/jira-issues');
  });
  it('cancels pending lookup on destroy and never opens a late editor', async () => {
    const f = TestBed.createComponent(UpstreamIssues);
    f.componentInstance.model.set({ url: upstreamIssue().url, source: '' });
    f.componentInstance.resolve(new Event('submit'));
    const r = await vi.waitFor(() => http.expectOne('/api/upstream-issues/resolve'));
    f.destroy();
    expect(r.cancelled).toBe(true);
    expect(open).not.toHaveBeenCalled();
  });
  it('preserves the original mode/focus during lazy loading and ignores duplicate opening gestures', async () => {
    const f = TestBed.createComponent(UpstreamIssues);
    await f.whenStable();
    const origin = f.nativeElement.querySelector('.upstream-row button') as HTMLButtonElement;
    origin.focus();
    const opening = f.componentInstance.clone(upstreamIssue());
    void f.componentInstance.clone(upstreamIssue());
    f.componentInstance.model.set({ url: upstreamIssue().url, source: '' });
    f.componentInstance.resolve(new Event('submit'));
    http.expectNone('/api/upstream-issues/resolve');
    TestBed.inject(RuntimeMode).apply(
      runtimeMode({ read_only_test_mode: false, mode_revision: 'changed-during-import' }),
    );
    await opening;
    expect(open).toHaveBeenCalledOnce();
    expect(open.mock.calls[0][1]).toMatchObject({
      data: { mode: runtimeMode() },
      restoreFocus: origin,
    });
  });
  it('does not open a late editor after its upstream view is destroyed during lazy loading', async () => {
    const f = TestBed.createComponent(UpstreamIssues);
    const opening = f.componentInstance.clone(upstreamIssue());
    f.destroy();
    await opening;
    expect(open).not.toHaveBeenCalled();
  });
});
