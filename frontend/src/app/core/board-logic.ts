import type {
  Attention,
  Board,
  Identity,
  JiraIssue,
  Person,
  Project,
  PullRequest,
  Sprint,
  UpstreamIssue,
} from './api-types';

export type Tab = 'attention' | 'overview' | 'upstream';
export interface Filters {
  jiraTitle: string;
  prTitle: string;
  sprint: string;
  source: string;
  repository: string;
}
export const defaultFilters = (tab: Tab = 'overview'): Filters => ({
  jiraTitle: '',
  prTitle: '',
  sprint: tab === 'overview' ? '' : 'all',
  source: 'all',
  repository: 'all',
});
const collator = new Intl.Collator(undefined, { sensitivity: 'accent' });
export const byTitle = <T extends { title: string; id: string }>(a: T, b: T) =>
  collator.compare(a.title, b.title) || a.id.localeCompare(b.id);
export const byWaiting = <T extends { attention: Attention | null; id: string }>(a: T, b: T) =>
  Date.parse(a.attention?.waiting_since ?? '') - Date.parse(b.attention?.waiting_since ?? '') ||
  a.id.localeCompare(b.id);
export function matchesSprint(issue: JiraIssue, selected: string, sprints: Sprint[]): boolean {
  if (selected === 'all') return true;
  if (selected === 'none') return issue.sprint_ids.length === 0;
  if (!selected || selected === 'active') return false;
  return issue.sprint_ids.includes(selected);
}
export function sprintViewUrl(sprint: Sprint): string | null {
  const trusted = safeUrl(sprint.board_url);
  if (!trusted) return null;
  const url = new URL(trusted);
  url.searchParams.set('selectedSprint', sprint.id);
  return url.href;
}
export function isConfiguredIssue(issue: JiraIssue, projects: Project[]): boolean {
  return projects.some((project) => project.key === issue.project_key);
}
export function repositoryFilterValue(
  pr: Pick<PullRequest, 'source_id' | 'owner' | 'repository'>,
): string {
  return `${pr.source_id}\u001f${pr.owner}\u001f${pr.repository}`;
}
export function upstreamQueue(issues: UpstreamIssue[], filters: Filters): UpstreamIssue[] {
  return issues
    .filter(
      (issue) =>
        (filters.source === 'all' || issue.source_id === filters.source) &&
        (filters.repository === 'all' || repositoryFilterValue(issue) === filters.repository) &&
        issue.title.toLocaleLowerCase().includes(filters.prTitle.trim().toLocaleLowerCase()),
    )
    .sort(byTitle);
}
export function queues(
  board: Board,
  tab: Exclude<Tab, 'upstream'>,
  filters: Filters,
  projects: Project[],
) {
  const sort = tab === 'attention' ? byWaiting : byTitle;
  const jira = board.issues
    .filter((i) =>
      tab === 'attention'
        ? i.attention !== null
        : i.is_assigned_to_me && isConfiguredIssue(i, projects),
    )
    .sort(sort);
  const prs = board.pull_requests
    .filter((p) => (tab === 'attention' ? p.attention !== null : p.is_mine))
    .sort(sort);
  const issues = jira.filter(
    (i) =>
      matchesSprint(i, filters.sprint, board.sprints) &&
      i.title.toLocaleLowerCase().includes(filters.jiraTitle.trim().toLocaleLowerCase()),
  );
  const pullRequests = prs.filter(
    (p) =>
      (filters.source === 'all' || p.source_id === filters.source) &&
      (filters.repository === 'all' || repositoryFilterValue(p) === filters.repository) &&
      p.title.toLocaleLowerCase().includes(filters.prTitle.trim().toLocaleLowerCase()),
  );
  return {
    jira,
    prs,
    issues,
    reviews: pullRequests.filter((p) => !p.is_mine),
    own: pullRequests.filter((p) => p.is_mine),
    total: jira.length + prs.length,
    shown: issues.length + pullRequests.length,
  };
}
export function otherPeople(people: Person[], source: string, identities: Identity[]): Person[] {
  const self = identities.find((i) => i.source_id === source);
  return people
    .filter((p) => !self || p.id !== self.account_id)
    .sort((a, b) => collator.compare(a.username, b.username));
}
export function safeUrl(value: string | null | undefined): string | null {
  if (!value) return null;
  try {
    const url = new URL(value);
    return ['https:', 'http:'].includes(url.protocol) && !url.username && !url.password
      ? url.href
      : null;
  } catch {
    return null;
  }
}
export function repositoryUrl(pr: PullRequest): string | null {
  const url = safeUrl(pr.url);
  if (!url) return null;
  const parsed = new URL(url);
  const cut =
    pr.forge === 'gitlab'
      ? parsed.pathname.lastIndexOf('/-/merge_requests/')
      : parsed.pathname.lastIndexOf(pr.forge === 'github' ? '/pull/' : '/pulls/');
  if (cut < 0) return null;
  parsed.pathname = parsed.pathname.slice(0, cut);
  parsed.search = '';
  parsed.hash = '';
  return parsed.href;
}
