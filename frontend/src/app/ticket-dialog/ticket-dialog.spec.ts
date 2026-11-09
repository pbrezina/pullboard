import { TestBed } from '@angular/core/testing';
import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { DIALOG_DATA, DialogRef } from '@angular/cdk/dialog';
import { Subject } from 'rxjs';
import { vi } from 'vitest';
import { createPayload, TicketDialog, type TicketDialogData } from './ticket-dialog';
import { BoardStore } from '../core/board-store';
import { RuntimeMode } from '../core/runtime-mode';
import { Notifications } from '../core/notifications';
import {
  board,
  context,
  draft,
  foreignMention,
  options,
  person,
  pr,
  runtimeMode,
  sync,
} from '../testing/fixtures';

describe('Editable ticket dialog', () => {
  let http: HttpTestingController;
  let ref: {
    disableClose: boolean;
    close: ReturnType<typeof vi.fn>;
    backdropClick: Subject<MouseEvent>;
    keydownEvents: Subject<KeyboardEvent>;
  };
  beforeEach(() => {
    ref = {
      disableClose: false,
      close: vi.fn(),
      backdropClick: new Subject(),
      keydownEvents: new Subject(),
    };
    TestBed.configureTestingModule({
      providers: [
        provideHttpClient(),
        provideHttpClientTesting(),
        {
          provide: DIALOG_DATA,
          useValue: { kind: 'pr', pr: pr(), purpose: 'review', mode: runtimeMode() },
        },
        { provide: DialogRef, useValue: ref },
      ],
    });
    TestBed.inject(BoardStore).context.set(context());
    TestBed.inject(RuntimeMode).value.set(runtimeMode());
    http = TestBed.inject(HttpTestingController);
  });
  afterEach(() => http.verify());
  async function ready() {
    const f = TestBed.createComponent(TicketDialog);
    const r = http.expectOne('/api/ticket-drafts');
    expect(r.request.body.project_key).toBeNull();
    r.flush(draft());
    http.expectOne((r) => r.url === '/api/jira/options').flush(options());
    await f.whenStable();
    return f;
  }
  it('presents editable title, description and all explicit write fields with template warning', async () => {
    const f = await ready();
    expect(f.nativeElement.textContent).toContain('Template draft');
    expect(f.nativeElement.textContent).toContain('AI is not configured');
    expect(f.nativeElement.querySelector('input').value).toBe('Review snapshot cache');
    expect(f.nativeElement.querySelector('textarea').value).toBe('Review cache consistency.');
    expect(f.nativeElement.textContent).toContain('No sprint');
    expect(f.nativeElement.textContent).toContain('Read-only test mode');
    expect(f.componentInstance.model()).toMatchObject({ assignee: '', sprint: '', status: '' });
  });
  it('keeps creation projects configured-only when foreign mentions are cached', async () => {
    TestBed.inject(BoardStore).board.set(
      board({ issues: [foreignMention(), foreignMention(true)] }),
    );
    const f = await ready();
    const projects = Array.from(
      f.nativeElement.querySelectorAll('#ticket-project option') as NodeListOf<HTMLOptionElement>,
      (option) => option.value,
    );
    expect(projects).toEqual(['DEV']);
    expect(f.componentInstance.model().project).toBe('DEV');
    http.expectNone('/api/jira/issues');
  });
  it('submits edited values, preserves nulls, prevents double submits and closes simulated success', async () => {
    const f = await ready();
    const c = f.componentInstance;
    c.model.update((m) => ({
      ...m,
      title: 'Edited title',
      description: 'Edited description',
      points: '3',
      status: '',
      team: '',
    }));
    await f.whenStable();
    c.save(new Event('submit'));
    c.save(new Event('submit'));
    const r = await vi.waitFor(() => http.expectOne('/api/jira/issues'));
    expect(r.request.body).toEqual({
      expected_mode_revision: 'mode-test-1',
      pull_request_id: pr().id,
      purpose: 'review',
      project_key: 'DEV',
      issue_type: 'Task',
      title: 'Edited title',
      description: 'Edited description',
      story_points: 3,
      assignee_id: null,
      sprint_id: null,
      team: null,
      status: null,
    });
    expect(c.busy()).toBe(true);
    r.flush({ outcome: 'simulated', issue: null, message: 'Created in test mode.', warnings: [] });
    await f.whenStable();
    expect(ref.close).toHaveBeenCalledOnce();
    expect(TestBed.inject(Notifications).items()[0].message).toContain('Simulated:');
  });
  it('reloads project options, resolves disallowed type and preserves deliberate nulls and text', async () => {
    const f = await ready();
    f.componentInstance.changeProject('OPS');
    const r = http.expectOne(
      (r) => r.url === '/api/jira/options' && r.params.get('project_key') === 'OPS',
    );
    const o = options();
    r.flush({
      ...o,
      project_key: 'OPS',
      issue_type: 'Review',
      issue_types: ['Review'],
      sprints: o.sprints.map((s) => ({ ...s, team: 'Operations' })),
      defaults: {
        ...o.defaults,
        issue_type: 'Review',
        team: 'team-platform',
        assignee_id: null,
        workflow: { draft: 'Queued', ready: 'Doing' },
      },
    });
    await f.whenStable();
    expect(f.componentInstance.model()).toMatchObject({
      project: 'OPS',
      issueType: 'Review',
      team: '',
      assignee: '',
      status: '',
      sprint: '',
      title: draft().title,
    });
  });
  it('never sends assignee search to Jira and retains current edits', async () => {
    const f = await ready();
    f.componentInstance.model.update((m) => ({
      ...m,
      title: 'Keep my title',
    }));
    const input = f.nativeElement.querySelector('#ticket-assignee') as HTMLInputElement;
    input.value = 'bob';
    input.dispatchEvent(new Event('input'));
    http.expectNone((r) => r.url === '/api/jira/options');
    await f.whenStable();
    expect(f.componentInstance.model().title).toBe('Keep my title');
  });
  it('never infers a sprint from team or replaces intentional nulls with refreshed defaults', async () => {
    const f = await ready();
    f.componentInstance.changeProject('OPS');
    const o = options();
    http
      .expectOne((r) => r.url === '/api/jira/options')
      .flush({
        ...o,
        project_key: 'OPS',
        defaults: { ...o.defaults, team: 'Operations' },
        sprints: [...o.sprints, { ...o.sprints[0], id: 'ops-sprint', team: 'Operations' }],
      });
    await f.whenStable();
    expect(f.componentInstance.model().sprint).toBe('');
    expect(f.componentInstance.model().team).toBe('');
  });
  it('surfaces errors without closing or swallowing edited data', async () => {
    const f = await ready();
    f.componentInstance.model.update((m) => ({ ...m, title: 'Keep this edit' }));
    f.componentInstance.save(new Event('submit'));
    const r = await vi.waitFor(() => http.expectOne('/api/jira/issues'));
    r.flush(
      { error: { code: 'upstream_failure', message: 'Jira is unavailable. Retry later.' } },
      { status: 502, statusText: 'Unavailable' },
    );
    await f.whenStable();
    expect(ref.close).not.toHaveBeenCalled();
    expect(f.componentInstance.busy()).toBe(false);
    expect(f.nativeElement.textContent).toContain('Jira is unavailable');
    expect(f.componentInstance.model().title).toBe('Keep this edit');
  });
  it('rejects blank titles and invalid points before any write', async () => {
    const f = await ready();
    f.componentInstance.model.update((m) => ({ ...m, title: ' ', points: '-1' }));
    f.componentInstance.save(new Event('submit'));
    await f.whenStable();
    http.expectNone('/api/jira/issues');
    expect(f.componentInstance.fields().invalid()).toBe(true);
  });
  it('surfaces draft loading failure and supports an explicit retry', async () => {
    const f = TestBed.createComponent(TicketDialog);
    expect(f.componentInstance.loading()).toBe(true);
    http
      .expectOne('/api/ticket-drafts')
      .flush(
        { error: { code: 'not_ready', message: 'PR source is not ready.' } },
        { status: 503, statusText: 'Not ready' },
      );
    await f.whenStable();
    expect(f.componentInstance.loading()).toBe(false);
    expect(f.nativeElement.textContent).toContain('Retry draft');
    f.componentInstance.generate();
    http.expectOne('/api/ticket-drafts').flush(draft());
    http.expectOne((r) => r.url === '/api/jira/options').flush(options());
    await f.whenStable();
    expect(f.componentInstance.model().title).toBe(draft().title);
  });
  it('retains deliberate nulls and resolved project type across option failure and retry', async () => {
    const f = await ready();
    f.componentInstance.changeProject('OPS');
    http
      .expectOne((r) => r.url === '/api/jira/options')
      .flush(
        { error: { code: 'upstream_failure', message: 'Options unavailable.' } },
        { status: 502, statusText: 'Unavailable' },
      );
    expect(f.componentInstance.optionsError()).toBe('Options unavailable.');
    f.componentInstance.loadOptions();
    const o = options();
    http
      .expectOne((r) => r.url === '/api/jira/options')
      .flush({ ...o, project_key: 'OPS', defaults: { ...o.defaults, team: 'Operations' } });
    await f.whenStable();
    expect(f.componentInstance.model()).toMatchObject({
      project: 'OPS',
      issueType: 'Task',
      team: '',
    });
  });
  it('cancels superseded option refresh and pending work on destroy', async () => {
    const f = await ready();
    f.componentInstance.loadOptions();
    const old = http.expectOne((r) => r.url === '/api/jira/options');
    f.componentInstance.loadOptions();
    expect(old.cancelled).toBe(true);
    const pending = http.expectOne((r) => r.url === '/api/jira/options');
    expect(pending.request.params.get('query')).toBe('');
    f.destroy();
    expect(pending.cancelled).toBe(true);
  });
  it('starts implementation Task unassigned/no sprint/no transition while retaining eligible self', async () => {
    TestBed.inject<TicketDialogData>(DIALOG_DATA).purpose = 'implementation';
    const f = TestBed.createComponent(TicketDialog);
    http.expectOne('/api/ticket-drafts').flush(draft('implementation'));
    const r = http.expectOne((r) => r.url === '/api/jira/options');
    expect(r.request.params.get('purpose')).toBe('implementation');
    expect(r.request.params.get('issue_type')).toBe('Task');
    r.flush(options('implementation'));
    await f.whenStable();
    expect(f.componentInstance.model()).toMatchObject({
      issueType: 'Task',
      assignee: '',
      sprint: '',
      status: '',
    });
    expect(f.componentInstance.assignees()).toContainEqual({ ...person, email: null });
  });
  it('keeps searches inside the returned eligible pool, preserving a selected user absent from search matches', async () => {
    const f = await ready();
    const c = f.componentInstance;
    expect(c.model().assignee).toBe('');
    expect(c.assignees().some((p) => p.id === person.id)).toBe(false);
    c.model.update((m) => ({
      ...m,
      assignee: 'reviewer',
      team: 'team-platform',
      sprint: 'future-1',
    }));
    const input = f.nativeElement.querySelector('#ticket-assignee') as HTMLInputElement;
    input.value = 'no match';
    input.dispatchEvent(new Event('input'));
    http.expectNone((r) => r.url === '/api/jira/options');
    await f.whenStable();
    expect(c.model()).toMatchObject({
      assignee: 'reviewer',
      team: 'team-platform',
      sprint: 'future-1',
    });
    expect(c.selectionErrors()).toEqual([]);
    expect(c.assignees()).toEqual(options().assignees);
  });
  it('preserves independent manual team/sprint and explicit blanks through type capability refresh', async () => {
    const f = await ready();
    const c = f.componentInstance;
    c.model.update((m) => ({
      ...m,
      team: 'team-platform',
      sprint: 'future-1',
      title: 'Manual',
      status: '',
      points: '2',
    }));
    c.typeChanged({ target: { value: 'Bug' } } as unknown as Event);
    const r = http.expectOne((r) => r.url === '/api/jira/options');
    expect(r.request.params.get('issue_type')).toBe('Bug');
    r.flush({
      ...options(),
      issue_type: 'Bug',
      defaults: { ...options().defaults, team: null, sprint_id: null },
    });
    await f.whenStable();
    expect(c.model()).toMatchObject({
      team: 'team-platform',
      sprint: 'future-1',
      title: 'Manual',
      status: '',
      points: '2',
      assignee: '',
    });
    expect(f.nativeElement.textContent).toContain('Sprint will be applied after creation');
    expect(c.fields.sprint().disabled()).toBe(false);
    c.model.update((m) => ({ ...m, team: '', sprint: '' }));
    c.loadOptions();
    http.expectOne((r) => r.url === '/api/jira/options').flush({ ...options(), issue_type: 'Bug' });
    await f.whenStable();
    expect(c.model()).toMatchObject({ team: '', sprint: '' });
  });
  it('visibly clears only disallowed values, disables unknown fields and respects required blockers', async () => {
    const f = await ready();
    const c = f.componentInstance;
    c.model.update((m) => ({
      ...m,
      team: 'team-platform',
      sprint: 'future-1',
      points: '3',
      assignee: 'reviewer',
    }));
    c.typeChanged({ target: { value: 'Bug' } } as unknown as Event);
    const o = options();
    http
      .expectOne((r) => r.url === '/api/jira/options')
      .flush({
        ...o,
        issue_type: 'Bug',
        teams: [],
        capabilities: {
          ...o.capabilities,
          can_create: false,
          unassigned_allowed: false,
          team: { mode: 'unknown', required: true, diagnostic: null },
          story_points: { mode: 'unsupported', required: false, diagnostic: null },
          blockers: [
            {
              code: 'required_field_unsupported',
              field: 'team',
              message: 'Required team cannot be set.',
            },
          ],
        },
      });
    await f.whenStable();
    expect(c.model()).toMatchObject({
      team: '',
      points: '',
      sprint: 'future-1',
      assignee: 'reviewer',
    });
    expect(c.fields.team().disabled()).toBe(true);
    expect(c.fields.points().disabled()).toBe(true);
    expect(f.nativeElement.textContent).toContain('Team was cleared');
    expect(f.nativeElement.textContent).toContain('Required team cannot be set');
    c.save(new Event('submit'));
    http.expectNone('/api/jira/issues');
  });
  it('shows all foreign/future choices with null defaults and no optional-empty warning wall', async () => {
    const f = TestBed.createComponent(TicketDialog);
    http.expectOne('/api/ticket-drafts').flush({
      ...draft(),
      diagnostics: [],
    });
    const o = options();
    http
      .expectOne((r) => r.url === '/api/jira/options')
      .flush({
        ...o,
        sprints: [
          ...o.sprints,
          { ...o.sprints[0], id: 'foreign', name: 'CORE', team: null, default_eligible: false },
        ],
        defaults: { ...o.defaults, sprint_id: null },
        diagnostics: [],
      });
    await f.whenStable();
    expect(f.componentInstance.model().sprint).toBe('');
    expect(f.nativeElement.textContent).not.toContain('Multiple default sprints');
    expect(f.nativeElement.querySelector('option[value="foreign"]')).not.toBeNull();
    expect(f.nativeElement.querySelector('option[value="future-1"]')).not.toBeNull();
  });
  it('keeps populated controls during local search/refresh and cancels superseded scopes', async () => {
    const f = await ready();
    const c = f.componentInstance;
    c.loadOptions();
    const old = http.expectOne((r) => r.url === '/api/jira/options');
    const input = f.nativeElement.querySelector('#ticket-assignee') as HTMLInputElement;
    input.value = 'pat';
    input.dispatchEvent(new Event('input'));
    expect(old.cancelled).toBe(false);
    expect(c.optionsLoading()).toBe(true);
    expect(c.options()).not.toBeNull();
    c.typeChanged({ target: { value: 'Bug' } } as unknown as Event);
    const typed = http.expectOne((r) => r.url === '/api/jira/options');
    c.changeProject('OPS');
    expect(typed.cancelled).toBe(true);
    http
      .expectOne((r) => r.url === '/api/jira/options')
      .flush({ ...options(), project_key: 'OPS' });
    await f.whenStable();
    expect(c.model()).toMatchObject({ project: 'OPS', issueType: 'Task' });
  });
  it('shows sanitized discovery errors instead of treating failed groups as an empty eligible pool', async () => {
    const f = await ready();
    const c = f.componentInstance;
    c.model.update((m) => ({ ...m, assignee: 'reviewer' }));
    c.loadOptions();
    http
      .expectOne((r) => r.url === '/api/jira/options')
      .flush(
        {
          error: {
            code: 'jira_discovery_failed',
            message: 'Eligible assignee discovery failed. Check source health.',
          },
        },
        { status: 502, statusText: 'Unavailable' },
      );
    await f.whenStable();
    expect(c.model().assignee).toBe('reviewer');
    expect(c.createBlocked()).toBe(true);
    expect(f.nativeElement.textContent).toContain('Eligible assignee discovery failed');
  });
  it('blocks a duplicate retry when creation outcome is unknown', async () => {
    const f = await ready();
    const c = f.componentInstance;
    c.save(new Event('submit'));
    const r = await vi.waitFor(() => http.expectOne('/api/jira/issues'));
    r.flush(
      { error: { code: 'write_outcome_unknown', message: 'Check Jira before retrying.' } },
      { status: 502, statusText: 'Unknown' },
    );
    await f.whenStable();
    expect(c.outcomeUnknown()).toBe(true);
    c.save(new Event('submit'));
    http.expectNone('/api/jira/issues');
    expect(f.nativeElement.textContent).toContain('avoid a duplicate');
  });
  it('does not invite a duplicate creation after the browser loses the create response', async () => {
    const f = await ready();
    const c = f.componentInstance;
    c.save(new Event('submit'));
    const r = await vi.waitFor(() => http.expectOne('/api/jira/issues'));
    r.error(new ProgressEvent('error'));
    await f.whenStable();
    expect(c.outcomeUnknown()).toBe(true);
    expect(c.error()).toContain('Check Jira');
    c.save(new Event('submit'));
    http.expectNone('/api/jira/issues');
  });
  it('builds exact nullable requests without leaking editor-only search or AI fields', () => {
    expect(
      createPayload(
        {
          project: 'DEV',
          issueType: 'Task',
          title: 'Title',
          description: '',
          points: '',
          assignee: '',
          sprint: '',
          team: '',
          status: '',
        },
        { kind: 'pr', pr: pr(), purpose: 'implementation', mode: runtimeMode() },
      ),
    ).toEqual({
      expected_mode_revision: 'mode-test-1',
      pull_request_id: pr().id,
      purpose: 'implementation',
      project_key: 'DEV',
      issue_type: 'Task',
      title: 'Title',
      description: '',
      story_points: null,
      assignee_id: null,
      sprint_id: null,
      team: null,
      status: null,
    });
  });
  it('blocks cross-tab mode changes and round trips without relabeling or discarding edits', async () => {
    const f = await ready();
    const c = f.componentInstance;
    c.model.update((m) => ({
      ...m,
      title: 'Keep these edits',
      description: 'Deliberate description',
    }));
    TestBed.inject(RuntimeMode).apply(
      runtimeMode({ read_only_test_mode: false, mode_revision: 'live-2' }),
    );
    await f.whenStable();
    expect(c.review.stale()).toBe(true);
    expect(c.review.captured()).toEqual(runtimeMode());
    expect(f.nativeElement.textContent).toContain('Simulate creation');
    expect(f.nativeElement.textContent).toContain('Review current mode');
    c.save(new Event('submit'));
    http.expectNone('/api/jira/issues');
    TestBed.inject(RuntimeMode).apply(runtimeMode({ mode_revision: 'test-3' }));
    await f.whenStable();
    expect(c.review.stale()).toBe(true);
    expect(c.model().title).toBe('Keep these edits');
  });
  it('preserves edits and freezes token on server conflict; refresh is not a retry', async () => {
    const f = await ready();
    const c = f.componentInstance;
    c.model.update((m) => ({ ...m, title: 'Keep conflict edits' }));
    c.save(new Event('submit'));
    const r = await vi.waitFor(() => http.expectOne('/api/jira/issues'));
    expect(r.request.body.expected_mode_revision).toBe('mode-test-1');
    r.flush(
      { error: { code: 'mode_conflict', message: 'Mode changed. Review again.' } },
      { status: 409, statusText: 'Conflict' },
    );
    await f.whenStable();
    http.expectOne('/api/runtime-mode').flush(runtimeMode({ mode_revision: 'restart-test' }));
    expect(c.review.stale()).toBe(true);
    expect(c.review.captured().mode_revision).toBe('mode-test-1');
    expect(c.model().title).toBe('Keep conflict edits');
    expect(c.outcomeUnknown()).toBe(false);
    c.save(new Event('submit'));
    http.expectNone('/api/jira/issues');
    expect(ref.close).not.toHaveBeenCalled();
  });
  it('keeps backend-resolved team but starts with no sprint rather than selecting by order', async () => {
    const f = TestBed.createComponent(TicketDialog);
    http.expectOne('/api/ticket-drafts').flush({ ...draft(), team: 'team-platform' });
    const o = options();
    http
      .expectOne((r) => r.url === '/api/jira/options')
      .flush({
        ...o,
        sprints: [
          { ...o.sprints[0], id: 'foreign', team: null, name: 'CORE', default_eligible: false },
          ...o.sprints,
        ],
      });
    await f.whenStable();
    expect(f.componentInstance.model()).toMatchObject({
      team: 'team-platform',
      sprint: '',
    });
  });
  it('does not invent an issue type when a complete inventory has no supported purpose default', async () => {
    const f = TestBed.createComponent(TicketDialog);
    http.expectOne('/api/ticket-drafts').flush({ ...draft(), issue_type: null });
    const r = http.expectOne((r) => r.url === '/api/jira/options');
    expect(r.request.params.has('issue_type')).toBe(false);
    const o = options();
    r.flush({
      ...o,
      issue_type: null,
      teams: [],
      statuses: [],
      defaults: { ...o.defaults, issue_type: null },
      capabilities: {
        ...o.capabilities,
        can_create: false,
        blockers: [
          {
            code: 'issue_type_required',
            field: 'issue_type',
            message: 'Choose a supported issue type.',
          },
        ],
      },
    });
    await f.whenStable();
    expect(f.componentInstance.model().issueType).toBe('');
    expect(f.componentInstance.createBlocked()).toBe(true);
    expect(f.nativeElement.textContent).toContain('Choose a supported issue type.');
  });
  it('retains review null when unassigned is disallowed and requires an eligible human choice', async () => {
    const f = await ready();
    const c = f.componentInstance;
    c.loadOptions();
    const o = options();
    http
      .expectOne((r) => r.url === '/api/jira/options')
      .flush({
        ...o,
        assignees: [...o.assignees, { ...person, email: null }], // defensive self filtering uses verified account ID, not login
        capabilities: { ...o.capabilities, unassigned_allowed: false },
      });
    await f.whenStable();
    expect(c.model().assignee).toBe('');
    expect(c.assignees().map((p) => p.id)).toEqual(['reviewer']);
    expect(c.selectionErrors()).toContain(
      'Select an eligible assignee; this issue type cannot be unassigned.',
    );
    c.model.update((m) => ({ ...m, assignee: 'reviewer' }));
    expect(c.createBlocked()).toBe(false);
  });
  it('keeps optional unsupported sprint empty without blocking creation or hiding board sprint options', async () => {
    const f = await ready();
    const c = f.componentInstance;
    c.loadOptions();
    const o = options();
    http
      .expectOne((r) => r.url === '/api/jira/options')
      .flush({
        ...o,
        capabilities: {
          ...o.capabilities,
          sprint: {
            mode: 'unsupported',
            required: false,
            diagnostic: {
              code: 'sprint_unsupported',
              field: 'sprint_id',
              message: 'Sprint assignment is unavailable.',
            },
          },
        },
      });
    await f.whenStable();
    expect(c.fields.sprint().disabled()).toBe(true);
    expect(c.model().sprint).toBe('');
    expect(c.createBlocked()).toBe(false);
    expect(f.nativeElement.querySelector('option[value="future-1"]')).not.toBeNull();
    expect(f.nativeElement.textContent).toContain('Sprint assignment is unavailable.');
    expect(f.nativeElement.querySelectorAll('.warning-banner').length).toBe(1); // template warning only
  });
  it('uses cached native status names, disables ambiguous names and submits name rather than ID', async () => {
    const f = await ready();
    const c = f.componentInstance;
    expect(c.model().status).toBe('');
    c.loadOptions();
    http
      .expectOne((r) => r.url === '/api/jira/options')
      .flush({
        ...options(),
        statuses: [
          { id: '100', name: 'Review' },
          { id: '101', name: 'Duplicate' },
          { id: '102', name: 'Duplicate' },
        ],
      });
    await f.whenStable();
    const select = f.nativeElement.querySelector('#ticket-status') as HTMLSelectElement;
    expect(select.options[0].textContent).toContain('Jira default — no transition');
    expect(
      Array.from(select.options)
        .filter((o) => o.value === 'Duplicate')
        .every((o) => o.disabled),
    ).toBe(true);
    c.model.update((m) => ({ ...m, status: 'Review' }));
    c.save(new Event('submit'));
    const r = await vi.waitFor(() => http.expectOne('/api/jira/issues'));
    expect(r.request.body.status).toBe('Review');
    r.flush({ outcome: 'simulated', issue: null, message: 'Simulated.', warnings: [] });
    await f.whenStable();
  });
  it('preserves deliberate status on refresh and visibly reconciles incompatible project/type changes', async () => {
    const f = await ready();
    const c = f.componentInstance;
    c.model.update((m) => ({ ...m, status: 'Review' }));
    c.loadOptions();
    http.expectOne((r) => r.url === '/api/jira/options').flush({ ...options(), statuses: [] });
    await f.whenStable();
    expect(c.model().status).toBe('Review');
    expect(c.fieldErrors().status).toContain('unambiguous');
    c.typeChanged({ target: { value: 'Bug' } } as unknown as Event);
    http
      .expectOne((r) => r.url === '/api/jira/options')
      .flush({ ...options(), issue_type: 'Bug', statuses: [{ id: 'new', name: 'Open' }] });
    await f.whenStable();
    expect(c.model().status).toBe('');
    expect(f.nativeElement.textContent).toContain('Workflow status was cleared');
  });
  it('uses live Jira failure health for retained cached choices, and keeps populated controls stable during explicit refresh', async () => {
    const f = await ready();
    const c = f.componentInstance;
    const health = {
      source_id: 'jira',
      name: 'Jira',
      kind: 'jira' as const,
      status: 'error' as const,
      last_success_at: '2026-11-01T00:00:00Z',
      last_attempt_at: '2026-11-02T00:00:00Z',
      error: 'Refresh failed.',
      retry_at: null,
      coverage_notes: [],
    };
    const store = TestBed.inject(BoardStore);
    store.board.set(board({ sources: [{ ...health, status: 'ok', error: null }] }));
    store.sync.set({ ...sync(), sources: [health] });
    c.model.update((m) => ({
      ...m,
      title: 'Keep title',
      assignee: 'reviewer',
      sprint: 'future-1',
      status: 'Review',
    }));
    await f.whenStable();
    expect(f.nativeElement.textContent).toContain('Using cached Jira choices');
    expect(c.createBlocked()).toBe(false);
    const refresh = vi.spyOn(store, 'refresh').mockImplementation(() => {});
    c.refreshChoices();
    expect(refresh).toHaveBeenCalledOnce();
    c.loadOptions();
    const optionsRequest = http.expectOne((r) => r.url === '/api/jira/options');
    await f.whenStable();
    expect(f.nativeElement.textContent).not.toContain('Loading Jira options');
    expect(f.nativeElement.querySelector('#ticket-status').value).toBe('Review');
    optionsRequest.flush(options());
    expect(c.model()).toMatchObject({
      title: 'Keep title',
      assignee: 'reviewer',
      sprint: 'future-1',
      status: 'Review',
    });
    store.board.set(
      board({
        sources: [
          { ...health, status: 'ok', error: null, last_success_at: '2026-11-03T00:00:00Z' },
        ],
      }),
    );
    await f.whenStable();
    http.expectOne((r) => r.url === '/api/jira/options').flush(options());
    await f.whenStable();
    expect(c.model()).toMatchObject({
      title: 'Keep title',
      assignee: 'reviewer',
      sprint: 'future-1',
      status: 'Review',
    });
  });
  it('places actual required errors next to controls and focuses the first actionable error', async () => {
    const f = await ready();
    const c = f.componentInstance;
    c.loadOptions();
    const o = options();
    http
      .expectOne((r) => r.url === '/api/jira/options')
      .flush({ ...o, capabilities: { ...o.capabilities, unassigned_allowed: false } });
    await f.whenStable();
    document.body.appendChild(f.nativeElement);
    c.save(new Event('submit'));
    await f.whenStable();
    expect(document.activeElement?.id).toBe('ticket-assignee');
    expect(f.nativeElement.querySelector('#assignee-error').textContent).toContain(
      'cannot be unassigned',
    );
    http.expectNone('/api/jira/issues');
    f.nativeElement.remove();
  });
  it('enables the assignee selector only after the complete scoped pool is loaded', async () => {
    const f = TestBed.createComponent(TicketDialog);
    http.expectOne('/api/ticket-drafts').flush(draft());
    const request = http.expectOne((r) => r.url === '/api/jira/options');
    await f.whenStable();
    expect(f.nativeElement.querySelector('#ticket-assignee').disabled).toBe(true);
    request.flush(options());
    await f.whenStable();
    expect(f.nativeElement.querySelector('#ticket-assignee').disabled).toBe(false);
  });
});
