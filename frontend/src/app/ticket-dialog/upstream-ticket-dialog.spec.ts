import { TestBed } from '@angular/core/testing';
import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { DIALOG_DATA, DialogRef } from '@angular/cdk/dialog';
import { Subject } from 'rxjs';
import { vi } from 'vitest';
import { TicketDialog, type UpstreamTicketDialogData } from './ticket-dialog';
import { BoardStore } from '../core/board-store';
import { RuntimeMode } from '../core/runtime-mode';
import { Notifications } from '../core/notifications';
import {
  board,
  context,
  options,
  person,
  runtimeMode,
  upstreamDraft,
  upstreamIssue,
} from '../testing/fixtures';

describe('Upstream issue in the shared ticket editor', () => {
  let http: HttpTestingController;
  let close: ReturnType<typeof vi.fn>;
  const issue = upstreamIssue({
    id: 'manual-closed',
    state: 'closed',
    is_assigned_to_me: false,
    assignees: [],
  });
  const data = (): UpstreamTicketDialogData => ({
    kind: 'upstream',
    issue,
    warning: 'The resolved issue is closed.',
    mode: runtimeMode(),
    purpose: 'implementation',
  });
  beforeEach(() => {
    close = vi.fn();
    TestBed.configureTestingModule({
      providers: [
        provideHttpClient(),
        provideHttpClientTesting(),
        { provide: DIALOG_DATA, useValue: data() },
        {
          provide: DialogRef,
          useValue: { close, backdropClick: new Subject(), keydownEvents: new Subject() },
        },
      ],
    });
    http = TestBed.inject(HttpTestingController);
    TestBed.inject(BoardStore).board.set(board());
    TestBed.inject(BoardStore).context.set(
      context({
        jira_projects: [
          ...context().jira_projects,
          { key: 'OPS', name: 'Operations', issue_types: ['Task', 'Bug'] },
        ],
      }),
    );
    TestBed.inject(RuntimeMode).apply(runtimeMode());
  });
  afterEach(() => http.verify());
  async function ready() {
    const f = TestBed.createComponent(TicketDialog);
    const r = http.expectOne('/api/upstream-issues/ticket-drafts');
    expect(r.request.body).toEqual({ upstream_issue_id: issue.id, project_key: null });
    r.flush({ ...upstreamDraft(issue), project_key: 'OPS' });
    const o = http.expectOne((r) => r.url === '/api/jira/options');
    expect(o.request.params.get('project_key')).toBe('OPS');
    expect(o.request.params.get('purpose')).toBe('implementation');
    o.flush({ ...options('implementation'), project_key: 'OPS' });
    await f.whenStable();
    return f;
  }
  it('uses the draft-supplied default project/implementation choices and deterministic copy without PR APIs or workflow inference', async () => {
    const f = await ready();
    expect(f.componentInstance.model()).toMatchObject({
      project: 'OPS',
      title: issue.title,
      description: upstreamDraft(issue).description,
      assignee: '',
      points: '',
      sprint: '',
      status: '',
    });
    expect(f.nativeElement.textContent).toContain('Deterministic copy');
    expect(f.nativeElement.textContent).toContain('no AI');
    expect(f.nativeElement.textContent).toContain('The resolved issue is closed');
    http.expectNone('/api/ticket-drafts');
    http.expectNone('/api/jira/issues');
  });
  it('submits only dedicated trusted selector plus edited fields and original revision; simulation does not change the board', async () => {
    const f = await ready();
    const c = f.componentInstance;
    const store = TestBed.inject(BoardStore);
    const snapshot = store.board();
    c.model.update((m) => ({
      ...m,
      title: '  Exact edited title  ',
      description: 'My text',
      assignee: '',
      team: '',
      sprint: '',
      status: '',
    }));
    c.save(new Event('submit'));
    const r = await vi.waitFor(() => http.expectOne('/api/upstream-issues/jira-issues'));
    expect(r.request.body).toEqual({
      upstream_issue_id: issue.id,
      expected_mode_revision: 'mode-test-1',
      purpose: 'implementation',
      project_key: 'OPS',
      issue_type: 'Task',
      title: '  Exact edited title  ',
      description: 'My text',
      story_points: null,
      assignee_id: null,
      team: null,
      sprint_id: null,
      status: null,
    });
    expect(r.request.headers.get('X-PullBoard-Request')).toBe('1');
    r.flush({
      outcome: 'simulated',
      issue: null,
      message: 'Issue creation simulated.',
      warnings: [],
    });
    await f.whenStable();
    expect(close).toHaveBeenCalledOnce();
    expect(store.board()).toBe(snapshot);
    expect(TestBed.inject(Notifications).items()[0]).toMatchObject({
      message: 'Simulated: Issue creation simulated.',
      url: null,
      label: null,
    });
    http.expectNone('/api/sync');
  });
  it('preserves edits and deliberate nulls through project/type option refreshes', async () => {
    const f = await ready();
    const c = f.componentInstance;
    c.model.update((m) => ({
      ...m,
      title: 'Keep edit',
      description: 'Keep description',
      assignee: '',
      team: '',
      sprint: '',
      status: '',
    }));
    c.changeProject('DEV');
    http.expectOne((r) => r.url === '/api/jira/options').flush(options('implementation'));
    c.typeChanged({ target: { value: 'Bug' } } as unknown as Event);
    http
      .expectOne((r) => r.url === '/api/jira/options')
      .flush({ ...options('implementation'), issue_type: 'Bug' });
    await f.whenStable();
    expect(c.model()).toMatchObject({
      title: 'Keep edit',
      description: 'Keep description',
      assignee: '',
      team: '',
      sprint: '',
      status: '',
    });
    expect(c.createBlocked()).toBe(false);
  });
  it('preserves an overlong upstream title for explicit editing rather than silently truncating it', async () => {
    const f = TestBed.createComponent(TicketDialog);
    const longTitle = 'x'.repeat(256);
    http
      .expectOne('/api/upstream-issues/ticket-drafts')
      .flush({ ...upstreamDraft(issue), title: longTitle });
    http.expectOne((r) => r.url === '/api/jira/options').flush(options('implementation'));
    await f.whenStable();
    expect(f.componentInstance.model().title).toBe(longTitle);
    f.componentInstance.save(new Event('submit'));
    await f.whenStable();
    expect(f.componentInstance.fields.title().invalid()).toBe(true);
    http.expectNone('/api/upstream-issues/jira-issues');
  });
  it('retains edits after selector eviction and requires explicit re-resolution, not background replay', async () => {
    const f = await ready();
    const c = f.componentInstance;
    c.model.update((m) => ({ ...m, title: 'Keep expired-selection edit' }));
    c.save(new Event('submit'));
    const r = await vi.waitFor(() => http.expectOne('/api/upstream-issues/jira-issues'));
    r.flush(
      {
        error: {
          code: 'upstream_issue_not_found',
          message: 'The upstream issue selection is unavailable; resolve the issue again.',
        },
      },
      { status: 404, statusText: 'Not found' },
    );
    await f.whenStable();
    expect(c.model().title).toBe('Keep expired-selection edit');
    expect(c.error()).toContain('resolve the issue again');
    expect(close).not.toHaveBeenCalled();
    http.expectNone('/api/upstream-issues/resolve');
    http.expectNone('/api/upstream-issues/jira-issues');
  });
  it('blocks a mode changed before asynchronous resolution/editor opening without rebasing', async () => {
    TestBed.inject(RuntimeMode).apply(
      runtimeMode({ read_only_test_mode: false, mode_revision: 'live-new' }),
    );
    const f = await ready();
    const c = f.componentInstance;
    expect(c.review.stale()).toBe(true);
    expect(c.review.captured()).toEqual(runtimeMode());
    expect(f.nativeElement.textContent).toContain('Simulate creation');
    c.save(new Event('submit'));
    http.expectNone('/api/upstream-issues/jira-issues');
  });
  it('preserves edits on mode conflict, refreshes mode only and never retries creation', async () => {
    const f = await ready();
    const c = f.componentInstance;
    c.model.update((m) => ({ ...m, title: 'Keep conflict edit' }));
    c.save(new Event('submit'));
    const r = await vi.waitFor(() => http.expectOne('/api/upstream-issues/jira-issues'));
    r.flush(
      { error: { code: 'mode_conflict', message: 'Review current mode.' } },
      { status: 409, statusText: 'Conflict' },
    );
    await f.whenStable();
    http.expectOne('/api/runtime-mode').flush(runtimeMode({ mode_revision: 'restart' }));
    expect(c.model().title).toBe('Keep conflict edit');
    expect(c.review.captured().mode_revision).toBe('mode-test-1');
    expect(c.review.stale()).toBe(true);
    expect(c.outcomeUnknown()).toBe(false);
    c.save(new Event('submit'));
    http.expectNone('/api/upstream-issues/jira-issues');
  });
  it.each(['network', 'write_outcome_unknown'])(
    'never replays an uncertain upstream create (%s)',
    async (failure) => {
      const f = await ready();
      const c = f.componentInstance;
      c.model.update((m) => ({ ...m, title: 'Keep uncertain edit' }));
      c.save(new Event('submit'));
      const r = await vi.waitFor(() => http.expectOne('/api/upstream-issues/jira-issues'));
      if (failure === 'network') r.error(new ProgressEvent('error'));
      else
        r.flush(
          { error: { code: failure, message: 'Inspect Jira.' } },
          { status: 502, statusText: 'Unknown' },
        );
      await f.whenStable();
      expect(c.outcomeUnknown()).toBe(true);
      expect(c.model().title).toBe('Keep uncertain edit');
      c.save(new Event('submit'));
      http.expectNone('/api/upstream-issues/jira-issues');
      expect(close).not.toHaveBeenCalled();
    },
  );
  it.each([true, false])(
    'shares confirmed/unknown-key accepted-create notifications without synthetic state (confirmed=%s)',
    async (confirmed) => {
      const f = await ready();
      const snapshot = TestBed.inject(BoardStore).board();
      const refresh = vi.spyOn(TestBed.inject(BoardStore), 'refresh').mockImplementation(() => {});
      f.componentInstance.save(new Event('submit'));
      const r = await vi.waitFor(() => http.expectOne('/api/upstream-issues/jira-issues'));
      r.flush({
        outcome: 'created',
        issue: confirmed
          ? { key: 'OPS-23', url: 'https://jira.example/browse/OPS-23', title: 'Confirmed fixture' }
          : null,
        message: 'Jira accepted creation.',
        warnings: confirmed
          ? ['Web link failed; keep the created ticket.']
          : ['No confirmed key; inspect Jira, do not resubmit.'],
      });
      await f.whenStable();
      expect(close).toHaveBeenCalledOnce();
      const notice = TestBed.inject(Notifications).items()[0];
      expect(notice.label).toBe(confirmed ? 'OPS-23' : null);
      expect(notice.url).toBe(confirmed ? 'https://jira.example/browse/OPS-23' : null);
      expect(TestBed.inject(BoardStore).board()).toBe(snapshot);
      expect(refresh).toHaveBeenCalledOnce();
    },
  );
});
