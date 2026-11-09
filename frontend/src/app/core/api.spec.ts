import { TestBed } from '@angular/core/testing';
import { provideHttpClient, HttpErrorResponse } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { Api, errorMessage } from './api';
import {
  board,
  context,
  createRequest,
  draft,
  options,
  pr,
  sync,
  runtimeMode,
  upstreamIssue,
  upstreamDraft,
} from '../testing/fixtures';
import type { Observable } from 'rxjs';

describe('Api wire contract', () => {
  let api: Api;
  let http: HttpTestingController;
  beforeEach(() => {
    TestBed.configureTestingModule({
      providers: [provideHttpClient(), provideHttpClientTesting()],
    });
    api = TestBed.inject(Api);
    http = TestBed.inject(HttpTestingController);
  });
  afterEach(() => http.verify());
  it('reads health, context, snapshot and sync without auth', () => {
    for (const [call, url, result] of [
      [() => api.health(), '/api/health', { status: 'ok', version: '1.2.0', ...runtimeMode() }],
      [() => api.context(), '/api/context', context()],
      [() => api.board(), '/api/board', board()],
      [() => api.sync(), '/api/sync', sync()],
      [() => api.runtimeMode(), '/api/runtime-mode', runtimeMode()],
    ] as const) {
      (call() as Observable<unknown>).subscribe((value) => expect(value).toEqual(result));
      const req = http.expectOne(url);
      expect(req.request.method).toBe('GET');
      expect(req.request.headers.has('Authorization')).toBe(false);
      req.flush(result);
    }
  });
  it('posts exact mode precondition and requested value, without persisting preferences', () => {
    api
      .changeRuntimeMode({ read_only_test_mode: false, expected_mode_revision: 'mode-test-1' })
      .subscribe();
    const r = http.expectOne('/api/runtime-mode');
    expect(r.request.body).toEqual({
      read_only_test_mode: false,
      expected_mode_revision: 'mode-test-1',
    });
    expectWrite(r.request);
    r.flush(runtimeMode({ read_only_test_mode: false, mode_revision: 'live-2' }));
  });
  it('includes project and encoded search query for options', () => {
    api.options('DEV', 'review', 'Task', 'Alex & Pat').subscribe();
    const req = http.expectOne((r) => r.url === '/api/jira/options');
    expect(req.request.params.get('project_key')).toBe('DEV');
    expect(req.request.params.get('query')).toBe('Alex & Pat');
    expect(req.request.params.get('purpose')).toBe('review');
    expect(req.request.params.get('issue_type')).toBe('Task');
    req.flush(options());
  });
  it('posts an empty refresh object with required write headers', () => {
    api.refresh().subscribe();
    const r = http.expectOne('/api/sync');
    expect(r.request.body).toEqual({});
    expectWrite(r.request);
    r.flush(sync('syncing'));
  });
  it('omits an unavailable type rather than sending an empty or invented type', () => {
    api.options('DEV', 'implementation').subscribe();
    const r = http.expectOne((r) => r.url === '/api/jira/options');
    expect(r.request.params.keys().sort()).toEqual(['project_key', 'purpose', 'query']);
    r.flush(options('implementation'));
  });
  it('sends explicit null project for a draft, without alternate fields', () => {
    const body = { pull_request_id: pr().id, purpose: 'review' as const, project_key: null };
    api.draft(body).subscribe();
    const r = http.expectOne('/api/ticket-drafts');
    expect(r.request.body).toEqual(body);
    expectWrite(r.request);
    r.flush(draft());
  });
  it('posts every nullable create field as null', () => {
    api.create(createRequest()).subscribe();
    const r = http.expectOne('/api/jira/issues');
    expect(r.request.body).toEqual(createRequest());
    expect(Object.values(r.request.body).some((v) => v === undefined)).toBe(false);
    expectWrite(r.request);
    r.flush({ outcome: 'simulated', issue: null, message: 'Simulated', warnings: [] });
  });
  it('uses dedicated upstream read endpoints with exact selectors/nulls and required headers', () => {
    api.resolveUpstreamIssue({ source_id: null, url: upstreamIssue().url }).subscribe();
    const resolve = http.expectOne('/api/upstream-issues/resolve');
    expect(resolve.request.body).toEqual({ source_id: null, url: upstreamIssue().url });
    expectWrite(resolve.request);
    resolve.flush({ issue: upstreamIssue(), warning: null });
    api
      .draftUpstreamIssue({ upstream_issue_id: upstreamIssue().id, project_key: null })
      .subscribe();
    const draft = http.expectOne('/api/upstream-issues/ticket-drafts');
    expect(draft.request.body).toEqual({
      upstream_issue_id: upstreamIssue().id,
      project_key: null,
    });
    expectWrite(draft.request);
    draft.flush(upstreamDraft());
  });
  it('links only cached IDs, encoding the issue key', () => {
    api.link('DEV/1', pr().id, 'mode-test-1').subscribe();
    const r = http.expectOne('/api/jira/issues/DEV%2F1/links');
    expect(r.request.body).toEqual({
      pull_request_id: pr().id,
      expected_mode_revision: 'mode-test-1',
    });
    expectWrite(r.request);
    r.flush({
      outcome: 'simulated',
      issue_key: 'DEV/1',
      pull_request_id: pr().id,
      message: 'Simulated',
    });
  });
  it('uses safe uniform error messages and network fallback', () => {
    expect(
      errorMessage(
        new HttpErrorResponse({
          status: 502,
          error: { error: { code: 'upstream_failure', message: 'Source unavailable.' } },
        }),
      ),
    ).toBe('Source unavailable.');
    expect(errorMessage(new HttpErrorResponse({ status: 0 }))).toContain('local server');
    expect(
      errorMessage(new HttpErrorResponse({ status: 500, error: 'raw upstream HTML' })),
    ).not.toContain('raw');
  });
});
function expectWrite(request: { method: string; headers: { get(name: string): string | null } }) {
  expect(request.method).toBe('POST');
  expect(request.headers.get('X-PullBoard-Request')).toBe('1');
  expect(request.headers.get('Content-Type')).toBe('application/json');
}
