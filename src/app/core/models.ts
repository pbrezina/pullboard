export interface ForgeConfig {
  /** Registry key; new provider types require no UI changes. */
  type: string;
  token: string;
  apiUrl: string;
  /** GitHub defaults to github.com; enterprise defaults to the API origin. */
  webUrl?: string;
}

export interface JiraConfig {
  email: string;
  apiToken: string;
  baseUrl: string;
  projectKey: string;
  /** Select a board when discovery finds multiple Scrum boards. */
  boardId?: number;
  /** Explicit active sprint override; takes precedence over sprintNamePattern. */
  sprintId?: number;
  /** Full sprint name template: %YEAR matches four digits, %NUMBER matches one or more digits. */
  sprintNamePattern?: string;
  issueTypeId?: string;
  storyPointsFieldId?: string;
  /** Destination STATUS names, not transition action names or IDs. */
  statuses?: { inProgress?: string[]; review?: string[]; done?: string[] };
}

export interface AppConfig {
  forges: ForgeConfig[];
  jira: JiraConfig;
  llm: { provider: 'gemini' | 'openai'; apiKey: string; model: string };
}

export type PullRequestState = 'open' | 'closed' | 'merged';
export interface PullRequestStatus {
  state: PullRequestState;
  draft: boolean;
}

export interface UnifiedPullRequest extends PullRequestStatus {
  id: string;
  providerId: string;
  forge: string;
  number: number;
  title: string;
  description: string;
  url: string;
  repository: string;
  author: string;
  reviewers: string[];
  assignees: string[];
  labels: string[];
  participantsError?: string;
  createdAt?: string;
  updatedAt: string;
}

export interface PullRequestWorkItem extends UnifiedPullRequest {
  reviewStatus?: 'outstanding' | 'given';
  reviewRequestedAt?: string;
  mentionedAt?: string;
  needsAction: boolean;
  attentionError?: string;
}

export interface PullRequestInbox {
  opened: PullRequestWorkItem[];
  requested: PullRequestWorkItem[];
  warnings: string[];
}

/** Deliberately no mutation methods. Implementations must only send upstream GETs. */
export interface IForgeProvider {
  readonly id: string;
  readonly label: string;
  getUsername(): Promise<string>;
  supportsUrl(url: string): boolean;
  getOpenPullRequests(): Promise<UnifiedPullRequest[]>;
  getPullRequestInbox(): Promise<PullRequestInbox>;
  getPullRequestStatus(url: string): Promise<PullRequestStatus>;
}

export interface TicketSuggestion {
  title: string;
  description: string;
  storyPoints: number;
}

export interface TicketCreation extends TicketSuggestion {
  /** Omitted when the ticket should not belong to a sprint. */
  sprintId?: number;
}

export interface ForgeIdentity {
  providerId: string;
  label: string;
  host: string;
  username: string;
}

export interface JiraSprint {
  id: number;
  name: string;
  state: 'active' | 'future';
}

export interface JiraWebLink {
  id: number;
  url: string;
  title: string;
}
export interface LinkedPullRequest {
  url: string;
  title?: string;
  state?: PullRequestState;
  error?: string;
}
export interface JiraTicket {
  key: string;
  summary: string;
  description: string;
  status: string;
  issueType: string;
  /** Retain a locally closed card without allowing it to become a link target. */
  closed?: boolean;
  url: string;
  links: JiraWebLink[];
  pullRequests: LinkedPullRequest[];
  linkError?: string;
}
export interface SprintContext {
  boardId: number;
  accountId: string;
  username: string;
  projectKey: string;
  sprints: JiraSprint[];
  defaultSprintId?: number;
  sprintWarning?: string;
}
export interface SyncReceipt {
  key: string;
  url: string;
  simulated?: boolean;
}
