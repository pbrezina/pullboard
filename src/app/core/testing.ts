import { AppConfig, UnifiedPullRequest } from './models';

/** Credential-free fixtures for the HTTP and workflow tests. */
export const testConfig: AppConfig = {
  forges: [{ type: 'github', token: 'test-read-only', apiUrl: 'https://api.github.com' }],
  jira: {
    email: 'dev@example.com',
    apiToken: 'test-only',
    baseUrl: 'https://team.atlassian.net',
    projectKey: 'DEV',
    boardId: 1,
  },
  llm: { provider: 'openai', apiKey: 'test-only', model: 'gpt-4o-mini' },
};

export const testPr: UnifiedPullRequest = {
  id: 'forge-0:org/repo#42',
  providerId: 'forge-0',
  forge: 'GitHub',
  number: 42,
  title: 'Fix worker shutdown race',
  description: 'Wait for pending work before stopping.',
  url: 'https://github.com/org/repo/pull/42',
  repository: 'org/repo',
  author: 'dev',
  reviewers: [],
  assignees: [],
  labels: [],
  state: 'open',
  draft: false,
  updatedAt: '2026-11-01T12:00:00Z',
};
