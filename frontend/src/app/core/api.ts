import { inject, Service } from '@angular/core';
import { HttpClient, HttpErrorResponse } from '@angular/common/http';
import type {
  Board,
  Context,
  CreateIssueRequest,
  CreateIssueResult,
  DraftRequest,
  Health,
  JiraOptions,
  LinkResult,
  Purpose,
  SyncStatus,
  TicketDraft,
  RuntimeMode,
  ChangeRuntimeModeRequest,
  ResolveUpstreamIssueRequest,
  ResolvedUpstreamIssue,
  UpstreamIssueDraftRequest,
  UpstreamIssueDraft,
  CreateUpstreamIssueRequest,
} from './api-types';

const writeOptions = {
  headers: { 'X-PullBoard-Request': '1', 'Content-Type': 'application/json' },
  timeout: 90_000,
};

@Service()
export class Api {
  private readonly http = inject(HttpClient);
  health() {
    return this.http.get<Health>('/api/health', { timeout: 20_000 });
  }
  context() {
    return this.http.get<Context>('/api/context', { timeout: 20_000 });
  }
  runtimeMode() {
    return this.http.get<RuntimeMode>('/api/runtime-mode', { timeout: 20_000 });
  }
  changeRuntimeMode(body: ChangeRuntimeModeRequest) {
    return this.http.post<RuntimeMode>('/api/runtime-mode', body, writeOptions);
  }
  board() {
    return this.http.get<Board>('/api/board', { timeout: 20_000 });
  }
  sync() {
    return this.http.get<SyncStatus>('/api/sync', { timeout: 20_000 });
  }
  refresh() {
    return this.http.post<SyncStatus>('/api/sync', {}, writeOptions);
  }
  options(project_key: string, purpose: Purpose, issue_type: string | null = null, query = '') {
    return this.http.get<JiraOptions>('/api/jira/options', {
      params: { project_key, purpose, ...(issue_type ? { issue_type } : {}), query },
      timeout: 20_000,
    });
  }
  draft(body: DraftRequest) {
    return this.http.post<TicketDraft>('/api/ticket-drafts', body, writeOptions);
  }
  create(body: CreateIssueRequest) {
    return this.http.post<CreateIssueResult>('/api/jira/issues', body, writeOptions);
  }
  resolveUpstreamIssue(body: ResolveUpstreamIssueRequest) {
    return this.http.post<ResolvedUpstreamIssue>(
      '/api/upstream-issues/resolve',
      body,
      writeOptions,
    );
  }
  draftUpstreamIssue(body: UpstreamIssueDraftRequest) {
    return this.http.post<UpstreamIssueDraft>(
      '/api/upstream-issues/ticket-drafts',
      body,
      writeOptions,
    );
  }
  createUpstreamIssue(body: CreateUpstreamIssueRequest) {
    return this.http.post<CreateIssueResult>(
      '/api/upstream-issues/jira-issues',
      body,
      writeOptions,
    );
  }
  link(issueKey: string, pull_request_id: string, expected_mode_revision: string) {
    return this.http.post<LinkResult>(
      `/api/jira/issues/${encodeURIComponent(issueKey)}/links`,
      { pull_request_id, expected_mode_revision },
      writeOptions,
    );
  }
}

export function isModeConflict(error: unknown): boolean {
  return (
    error instanceof HttpErrorResponse &&
    error.status === 409 &&
    error.error?.error?.code === 'mode_conflict'
  );
}

export function errorMessage(error: unknown): string {
  if (error instanceof HttpErrorResponse) {
    const body: unknown = error.error;
    if (typeof body === 'object' && body !== null && 'error' in body) {
      const detail = body.error;
      if (
        typeof detail === 'object' &&
        detail !== null &&
        'message' in detail &&
        typeof detail.message === 'string'
      )
        return detail.message;
    }
    return error.status === 0
      ? 'Cannot reach PullBoard. Check that the local server is running, then retry.'
      : `Request failed (${error.status}). Retry or inspect source health.`;
  }
  return 'The request could not complete. Please retry.';
}
