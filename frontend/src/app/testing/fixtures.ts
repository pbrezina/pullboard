/** Contract-shaped test fixtures only. Never imported by application code. */
import type {
  Attention,
  Board,
  Context,
  CreateIssueRequest,
  JiraIssue,
  JiraOptions,
  Person,
  PullRequest,
  Sprint,
  SyncStatus,
  TicketDraft,
  RuntimeMode,
  UpstreamIssue,
  UpstreamIssueDraft,
} from '../core/api-types';
export const person: Person = {
  id: 'person-1',
  username: 'alex',
  display_name: 'Alex Developer',
  url: 'https://forge.example/alex',
};
export const sprint: Sprint = {
  id: 'sprint-1',
  board_id: 12,
  team: 'Platform',
  name: 'Platform 42',
  state: 'active',
  default_eligible: true,
  board_url: 'https://jira.example/board/12',
  start_at: '2026-11-01T00:00:00Z',
  end_at: '2026-11-15T00:00:00Z',
};
export function attention(time = '2026-11-01T12:00:00Z'): Attention {
  return {
    reasons: ['pr_mention'],
    waiting_since: time,
    threads: [
      {
        id: 'thread-1',
        url: 'https://forge.example/acme/widget/pull/42#comment-1',
        reason: 'pr_mention',
        waiting_since: time,
        excerpt: 'Could you check the cache behavior?',
      },
    ],
    coverage: 'complete',
  };
}
export function issue(overrides: Partial<JiraIssue> = {}): JiraIssue {
  return {
    id: 'jira:DEV-1',
    source_id: 'jira',
    key: 'DEV-1',
    url: 'https://jira.example/browse/DEV-1',
    title: 'Improve cache invalidation',
    description: 'Keep cache entries consistent.',
    project_key: 'DEV',
    issue_type: 'Task',
    status: 'In progress',
    reporter: person,
    assignees: [person],
    is_assigned_to_me: true,
    created_at: '2026-10-20T12:00:00Z',
    updated_at: '2026-11-05T12:00:00Z',
    sprint_ids: ['sprint-1'],
    linked_pr_ids: [],
    attention: {
      ...attention(),
      reasons: ['jira_mention'],
      threads: [{ ...attention().threads[0], reason: 'jira_mention' }],
    },
    ...overrides,
  };
}
export function foreignMention(assignedToMe = false): JiraIssue {
  const key = assignedToMe ? 'FOREIGN-2' : 'FOREIGN-1';
  const url = `https://jira.example/browse/${key}`;
  const mention = attention(assignedToMe ? '2026-10-31T12:00:00Z' : '2026-10-30T12:00:00Z');
  return issue({
    id: `jira:${key}`,
    key,
    url,
    project_key: 'FOREIGN',
    title: assignedToMe ? 'Foreign mention assigned to you' : 'Foreign mention assigned to Pat',
    assignees: assignedToMe
      ? [person]
      : [{ ...person, id: 'other-person', username: 'pat', display_name: 'Pat Developer' }],
    is_assigned_to_me: assignedToMe,
    sprint_ids: assignedToMe ? ['sprint-1'] : [],
    attention: {
      ...mention,
      reasons: ['jira_mention'],
      threads: [
        {
          ...mention.threads[0],
          id: `${key}:comment-1`,
          url: `${url}?focusedCommentId=1`,
          reason: 'jira_mention',
        },
      ],
    },
  });
}
export function pr(overrides: Partial<PullRequest> = {}): PullRequest {
  return {
    id: 'work:acme/widget:42',
    source_id: 'work',
    forge: 'github',
    number: 42,
    owner: 'acme',
    repository: 'widget',
    title: 'Add snapshot cache',
    url: 'https://forge.example/acme/widget/pull/42',
    description: 'Replace snapshots atomically and retain last good data.',
    state: 'ready',
    author: person,
    assignees: [],
    reviewers: [],
    labels: ['backend', 'cache'],
    created_at: '2026-10-22T12:00:00Z',
    updated_at: '2026-11-05T12:00:00Z',
    last_commit_at: '2026-11-05T12:00:00Z',
    linked_issue_keys: [],
    is_mine: true,
    attention: attention(),
    ...overrides,
  };
}
export function board(overrides: Partial<Board> = {}): Board {
  return {
    revision: 1,
    generated_at: '2026-11-06T12:00:00Z',
    issues: [issue()],
    pull_requests: [pr()],
    upstream_issues: [upstreamIssue()],
    sprints: [sprint],
    sources: [
      {
        source_id: 'work',
        name: 'Work GitHub',
        kind: 'github',
        status: 'ok',
        last_success_at: '2026-11-06T12:00:00Z',
        last_attempt_at: '2026-11-06T12:00:00Z',
        error: null,
        retry_at: null,
        coverage_notes: [],
      },
    ],
    ...overrides,
  };
}
export function upstreamIssue(overrides: Partial<UpstreamIssue> = {}): UpstreamIssue {
  return {
    id: 'work:acme/widget:issue:7',
    source_id: 'work',
    forge: 'github',
    number: 7,
    owner: 'acme',
    repository: 'widget',
    title: 'Preserve issue description newlines',
    url: 'https://forge.example/acme/widget/issues/7',
    description: 'Keep the first line.\nKeep the second line.',
    state: 'open',
    reporter: person,
    assignees: [person],
    labels: ['bug', 'text'],
    is_assigned_to_me: true,
    created_at: '2026-10-20T12:00:00Z',
    updated_at: '2026-11-05T12:00:00Z',
    ...overrides,
  };
}
export function upstreamDraft(issue = upstreamIssue()): UpstreamIssueDraft {
  return {
    upstream_issue_id: issue.id,
    purpose: 'implementation',
    project_key: 'DEV',
    title: issue.title,
    description: `${issue.description ? issue.description + '\n\n' : ''}Upstream issue: ${issue.url}${issue.labels.length ? '\nLabels: ' + issue.labels.join(', ') : ''}`,
    story_points: null,
    assignee_id: null,
    sprint_id: null,
    issue_type: 'Task',
    team: null,
    suggested_status: null,
    generated_by: 'template',
    warning:
      issue.state === 'closed'
        ? 'This upstream issue is closed. Review it before creating a Jira ticket.'
        : null,
    diagnostics: [],
  };
}
export function context(overrides: Partial<Context> = {}): Context {
  return {
    mode_revision: 'mode-test-1',
    read_only_test_mode: true,
    refresh_interval_seconds: 3600,
    identities: [
      {
        source_id: 'jira',
        kind: 'jira',
        account_id: person.id,
        display_name: person.display_name,
        username: 'alex@work.example',
        profile_url: null,
      },
      {
        source_id: 'work',
        kind: 'github',
        account_id: person.id,
        display_name: person.display_name,
        username: 'alex',
        profile_url: person.url,
      },
      {
        source_id: 'personal',
        kind: 'github',
        account_id: 'personal-user',
        display_name: 'Alex',
        username: 'alex-home',
        profile_url: null,
      },
    ],
    current_sprints: [sprint],
    current_project: { key: 'DEV', name: 'Development', issue_types: ['Task', 'Bug'] },
    jira_projects: [
      {
        key: 'DEV',
        name: 'Development',
        issue_types: ['Task', 'Bug'],
      },
    ],
    ai: { enabled: false, provider: null, model: null },
    ...overrides,
  };
}
export function sync(state: SyncStatus['state'] = 'idle'): SyncStatus {
  return {
    state,
    started_at: '2026-11-06T12:00:00Z',
    finished_at: state === 'idle' ? '2026-11-06T12:00:01Z' : null,
    last_success_at: state === 'idle' ? '2026-11-06T12:00:01Z' : null,
    sources: board().sources,
    progress: board().sources.map((s) => ({
      source_id: s.source_id,
      phase: state === 'idle' ? 'completed' : 'queued',
      completed_items: state === 'idle' ? 1 : 0,
      total_items: state === 'idle' ? 1 : null,
      started_at: state === 'idle' ? '2026-11-06T12:00:00Z' : null,
      finished_at: state === 'idle' ? '2026-11-06T12:00:01Z' : null,
      elapsed_ms: state === 'idle' ? 1000 : 0,
    })),
  };
}
export function options(purpose: JiraOptions['purpose'] = 'review'): JiraOptions {
  return {
    project_key: 'DEV',
    purpose,
    issue_type: 'Task',
    assignees:
      purpose === 'review'
        ? [
            {
              ...person,
              id: 'reviewer',
              username: 'pat',
              display_name: 'Pat Reviewer',
              email: 'pat@example.test',
            },
          ]
        : [{ ...person, email: null }],
    sprints: [
      sprint,
      { ...sprint, id: 'future-1', state: 'future', default_eligible: false, name: 'Platform 43' },
    ],
    issue_types: ['Task', 'Bug'],
    teams: [{ id: 'team-platform', name: 'Platform' }],
    statuses: [
      { id: 'status-1', name: 'Backlog' },
      { id: 'status-2', name: 'Review' },
    ],
    capabilities: {
      can_create: true,
      unassigned_allowed: true,
      story_points: { mode: 'create', required: false, diagnostic: null },
      team: { mode: 'create', required: false, diagnostic: null },
      sprint: { mode: 'follow_up', required: false, diagnostic: null },
      blockers: [],
    },
    diagnostics: [],
    defaults: {
      issue_type: 'Task',
      assignee_id: null,
      team: 'team-platform',
      sprint_id: null,
      workflow: { draft: 'Backlog', ready: 'Review' },
    },
  };
}
export function draft(purpose: TicketDraft['purpose'] = 'review'): TicketDraft {
  return {
    pull_request_id: pr().id,
    purpose,
    project_key: 'DEV',
    title: 'Review snapshot cache',
    description: 'Review cache consistency.',
    story_points: null,
    assignee_id: null,
    sprint_id: null,
    issue_type: 'Task',
    team: null,
    suggested_status: null,
    generated_by: 'template',
    warning: 'AI is not configured. An editable template is provided.',
    diagnostics: [],
  };
}
export function createRequest(): CreateIssueRequest {
  return {
    expected_mode_revision: 'mode-test-1',
    pull_request_id: pr().id,
    purpose: 'review',
    project_key: 'DEV',
    issue_type: 'Task',
    title: 'Review snapshot cache',
    description: 'Review cache consistency.',
    story_points: null,
    assignee_id: null,
    sprint_id: null,
    team: null,
    status: null,
  };
}
export function runtimeMode(overrides: Partial<RuntimeMode> = {}): RuntimeMode {
  return { read_only_test_mode: true, mode_revision: 'mode-test-1', ...overrides };
}
