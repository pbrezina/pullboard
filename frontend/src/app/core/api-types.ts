/** PullBoard v1.4 wire types. Keep in sync with api/openapi.json. */
export type ForgeKind = 'github' | 'gitlab' | 'codeberg' | 'forgejo';
export type SourceKind = ForgeKind | 'jira';
export type Purpose = 'implementation' | 'review';
export interface Person {
  id: string;
  username: string;
  display_name: string;
  url: string | null;
}
export interface Identity {
  source_id: string;
  kind: SourceKind;
  account_id: string;
  display_name: string;
  username: string;
  profile_url: string | null;
}
export interface JiraAssignee extends Person {
  email: string | null;
}
export interface Sprint {
  id: string;
  board_id: number;
  team: string | null;
  name: string;
  state: 'active' | 'future';
  default_eligible: boolean;
  board_url: string;
  start_at: string | null;
  end_at: string | null;
}
export type AttentionReason = 'jira_mention' | 'review_request' | 'pr_mention' | 'author_feedback';
export interface AttentionThread {
  id: string;
  url: string;
  reason: AttentionReason;
  waiting_since: string;
  excerpt: string;
}
export interface Attention {
  reasons: AttentionReason[];
  waiting_since: string;
  threads: AttentionThread[];
  coverage: 'complete' | 'partial';
}
export interface JiraIssue {
  id: string;
  source_id: 'jira';
  key: string;
  url: string;
  title: string;
  description: string;
  project_key: string;
  issue_type: string;
  status: string;
  reporter: Person | null;
  assignees: Person[];
  is_assigned_to_me: boolean;
  created_at: string;
  updated_at: string;
  sprint_ids: string[];
  linked_pr_ids: string[];
  attention: Attention | null;
}
export interface PullRequest {
  id: string;
  source_id: string;
  forge: ForgeKind;
  number: number;
  owner: string;
  repository: string;
  title: string;
  url: string;
  description: string;
  state: 'draft' | 'ready';
  author: Person;
  assignees: Person[];
  reviewers: Person[];
  labels: string[];
  created_at: string;
  updated_at: string;
  last_commit_at: string | null;
  linked_issue_keys: string[];
  is_mine: boolean;
  attention: Attention | null;
}
export interface SourceHealth {
  source_id: string;
  name: string;
  kind: SourceKind;
  status: 'pending' | 'ok' | 'error' | 'backoff';
  last_success_at: string | null;
  last_attempt_at: string | null;
  error: string | null;
  retry_at: string | null;
  coverage_notes: string[];
}
export interface UpstreamIssue {
  id: string;
  source_id: string;
  forge: ForgeKind;
  number: number;
  owner: string;
  repository: string;
  title: string;
  url: string;
  description: string;
  state: 'open' | 'closed';
  reporter: Person | null;
  assignees: Person[];
  labels: string[];
  is_assigned_to_me: boolean;
  created_at: string;
  updated_at: string;
}
export interface ResolveUpstreamIssueRequest {
  source_id: string | null;
  url: string;
}
export interface ResolvedUpstreamIssue {
  issue: UpstreamIssue;
  warning: string | null;
}
export interface UpstreamIssueDraftRequest {
  upstream_issue_id: string;
  project_key: string | null;
}
export interface Board {
  revision: number;
  generated_at: string | null;
  issues: JiraIssue[];
  pull_requests: PullRequest[];
  upstream_issues: UpstreamIssue[];
  sprints: Sprint[];
  sources: SourceHealth[];
}
export interface RuntimeMode {
  read_only_test_mode: boolean;
  mode_revision: string;
}
export interface ChangeRuntimeModeRequest {
  read_only_test_mode: boolean;
  expected_mode_revision: string;
}
export interface Health extends RuntimeMode {
  status: 'ok';
  version: string;
}
export interface Project {
  key: string;
  name: string;
  issue_types: string[];
}
export interface Context extends RuntimeMode {
  refresh_interval_seconds: number;
  identities: Identity[];
  current_project: Project | null;
  current_sprints: Sprint[];
  jira_projects: Project[];
  ai: { enabled: boolean; provider: string | null; model: string | null };
}
export interface SyncStatus {
  state: 'idle' | 'syncing';
  started_at: string | null;
  finished_at: string | null;
  last_success_at: string | null;
  sources: SourceHealth[];
  progress: SourceProgress[];
}
export interface SourceProgress {
  source_id: string;
  phase:
    | 'idle'
    | 'queued'
    | 'preparing'
    | 'discovering'
    | 'fetching'
    | 'publishing'
    | 'completed'
    | 'failed'
    | 'backoff';
  completed_items: number;
  total_items: number | null;
  started_at: string | null;
  finished_at: string | null;
  elapsed_ms: number;
}
export interface Workflow {
  draft: string | null;
  ready: string | null;
}
export interface TeamChoice {
  id: string;
  name: string;
}
export interface StatusChoice {
  id: string;
  name: string;
}
export interface JiraDiagnostic {
  code: string;
  field: 'issue_type' | 'assignee_id' | 'team' | 'sprint_id' | 'story_points' | 'status' | null;
  message: string;
}
export interface FieldCapability {
  mode: 'create' | 'follow_up' | 'unsupported' | 'unknown';
  required: boolean;
  diagnostic: JiraDiagnostic | null;
}
export interface CreateCapabilities {
  can_create: boolean;
  unassigned_allowed: boolean;
  story_points: FieldCapability;
  team: FieldCapability;
  sprint: FieldCapability;
  blockers: JiraDiagnostic[];
}
export interface JiraDefaults {
  issue_type: string | null;
  assignee_id: null;
  team: string | null;
  sprint_id: null;
  workflow: Workflow;
}
export interface JiraOptions {
  project_key: string;
  purpose: Purpose;
  issue_type: string | null;
  assignees: JiraAssignee[];
  sprints: Sprint[];
  issue_types: string[];
  teams: TeamChoice[];
  statuses: StatusChoice[];
  defaults: JiraDefaults;
  capabilities: CreateCapabilities;
  diagnostics: JiraDiagnostic[];
}
export interface DraftRequest {
  pull_request_id: string;
  purpose: Purpose;
  project_key: string | null;
}
export interface TicketDraft {
  pull_request_id: string;
  purpose: Purpose;
  project_key: string;
  title: string;
  description: string;
  story_points: number | null;
  assignee_id: null;
  sprint_id: null;
  issue_type: string | null;
  team: string | null;
  suggested_status: null;
  generated_by: 'ai' | 'template';
  warning: string | null;
  diagnostics: JiraDiagnostic[];
}
export interface CreateIssueRequest {
  pull_request_id: string;
  expected_mode_revision: string;
  purpose: Purpose;
  project_key: string;
  issue_type: string;
  title: string;
  description: string;
  story_points: number | null;
  assignee_id: string | null;
  sprint_id: string | null;
  team: string | null;
  status: string | null;
}
export interface UpstreamIssueDraft extends Omit<
  TicketDraft,
  'pull_request_id' | 'purpose' | 'generated_by' | 'story_points' | 'suggested_status'
> {
  upstream_issue_id: string;
  purpose: 'implementation';
  generated_by: 'template';
  story_points: null;
  suggested_status: null;
}
export interface CreateUpstreamIssueRequest extends Omit<
  CreateIssueRequest,
  'pull_request_id' | 'purpose'
> {
  upstream_issue_id: string;
  purpose: 'implementation';
}
export interface IssueReference {
  key: string;
  url: string;
  title: string;
}
export interface CreateIssueResult {
  outcome: 'created' | 'simulated';
  issue: IssueReference | null;
  message: string;
  warnings: string[];
}
export interface LinkResult {
  outcome: 'linked' | 'already_linked' | 'simulated';
  issue_key: string;
  pull_request_id: string;
  message: string;
}
export interface LinkRequest {
  pull_request_id: string;
  expected_mode_revision: string;
}
export interface ErrorResponse {
  error: { code: string; message: string };
}
