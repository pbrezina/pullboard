import { HttpClient } from '@angular/common/http';
import { inject, Service } from '@angular/core';
import { firstValueFrom, fromEvent, takeUntil } from 'rxjs';
import { ConfigService } from './config';
import { mapConcurrent } from './concurrency';
import { errorMessage } from './errors';
import {
  JiraSprint,
  JiraTicket,
  JiraWebLink,
  SprintContext,
  SyncReceipt,
  TicketSuggestion,
} from './models';
import { compileSprintNamePattern } from './sprint-name-pattern';
import { TEST_MODE } from './test-mode';

interface User {
  accountId: string;
  emailAddress?: string;
  displayName?: string;
  active: boolean;
}
interface Sprint {
  id: number;
  name: string;
  state: string;
}

/** Extract readable text from JIRA Cloud's ADF without rendering untrusted HTML. */
export function descriptionText(value: unknown): string {
  if (typeof value === 'string') return value;
  if (!value || typeof value !== 'object') return '';
  const node = value as {
    type?: string;
    text?: string;
    attrs?: { text?: string; shortName?: string };
    content?: unknown[];
  };
  if (node.type === 'text') return node.text ?? '';
  if (node.type === 'hardBreak') return '\n';
  if (node.type === 'mention') return node.attrs?.text ?? '';
  if (node.type === 'emoji') return node.attrs?.text ?? node.attrs?.shortName ?? '';
  const text = Array.isArray(node.content) ? node.content.map(descriptionText).join('') : '';
  return ['paragraph', 'heading', 'listItem', 'codeBlock'].includes(node.type ?? '')
    ? `${text}\n`
    : text;
}
interface Transition {
  id: string;
  name: string;
  to: { name: string };
  fields?: Record<string, { required?: boolean; hasDefaultValue?: boolean }>;
}
interface CreateField {
  fieldId: string;
  name: string;
  required: boolean;
  hasDefaultValue?: boolean;
  schema?: { type?: string };
}
interface CreateMetadata {
  issueTypeId: string;
  pointsFieldId: string;
}

export type TicketPhase = 'inProgress' | 'review' | 'done';
export const DEFAULT_STATUSES: Record<TicketPhase, string[]> = {
  inProgress: ['In Progress'],
  review: ['Review', 'In Review', 'Code Review'],
  done: ['Done', 'Closed'],
};

export function selectTransition(transitions: Transition[], targets: string[]): Transition {
  for (const target of targets) {
    const matches = transitions.filter(
      (transition) => transition.to.name.toLowerCase() === target.toLowerCase(),
    );
    const usable = matches.find(
      (transition) =>
        !Object.values(transition.fields ?? {}).some(
          (field) => field.required && !field.hasDefaultValue,
        ),
    );
    if (usable) return usable;
    if (matches.length)
      throw new Error(
        `Transition to ${target} requires extra fields. Complete it in JIRA or adjust the workflow.`,
      );
  }
  throw new Error(
    `No available transition to ${targets.join(' / ')}. Configure jira.statuses or update the workflow.`,
  );
}

@Service()
export class JiraService {
  private readonly http = inject(HttpClient);
  private readonly config = inject(ConfigService);
  private readonly testMode = inject(TEST_MODE);
  private context?: Promise<SprintContext>;
  private account?: Promise<User>;
  private metadata?: Promise<CreateMetadata>;
  private simulatedCount = 0;

  private async headers(): Promise<Record<string, string>> {
    const { jira } = await this.config.load();
    const bytes = new TextEncoder().encode(`${jira.email}:${jira.apiToken}`);
    return {
      Authorization: `Basic ${btoa(Array.from(bytes, (byte) => String.fromCharCode(byte)).join(''))}`,
      Accept: 'application/json',
    };
  }

  private async get<T>(
    path: string,
    params: Record<string, string | number> = {},
    signal?: AbortSignal,
  ): Promise<T> {
    const headers = await this.headers();
    signal?.throwIfAborted();
    const request = this.http.get<T>(`/api/jira${path}`, { headers, params });
    return firstValueFrom(signal ? request.pipe(takeUntil(fromEvent(signal, 'abort'))) : request);
  }
  private async post<T>(path: string, body: unknown): Promise<T> {
    return firstValueFrom(
      this.http.post<T>(`/api/jira${path}`, body, { headers: await this.headers() }),
    );
  }

  private async values<T>(
    path: string,
    params: Record<string, string | number> = {},
  ): Promise<T[]> {
    const all: T[] = [];
    for (let startAt = 0; ;) {
      const page = await this.get<{ values: T[]; isLast?: boolean; total?: number }>(path, {
        ...params,
        startAt,
        maxResults: 50,
      });
      all.push(...page.values);
      startAt += page.values.length;
      if (page.isLast || !page.values.length || (page.total !== undefined && startAt >= page.total))
        return all;
    }
  }

  getSprintContext(refresh = false): Promise<SprintContext> {
    if (refresh) this.context = undefined;
    return (this.context ??= this.discoverContext().catch((error: unknown) => {
      this.context = undefined;
      throw error;
    }));
  }

  private getAccount(): Promise<User> {
    return (this.account ??= this.discoverAccount().catch((error: unknown) => {
      this.account = undefined;
      throw error;
    }));
  }

  private async discoverAccount(): Promise<User> {
    const { jira } = await this.config.load();
    const [myself, users] = await Promise.all([
      this.get<User>('/rest/api/3/myself'),
      this.get<User[]>('/rest/api/3/user/search', { query: jira.email, maxResults: 1000 }),
    ]);
    const account =
      users.find(
        (user) => user.active && user.emailAddress?.toLowerCase() === jira.email.toLowerCase(),
      ) ??
      (myself.active &&
      (myself.emailAddress?.toLowerCase() === jira.email.toLowerCase() ||
        users.some((user) => user.accountId === myself.accountId))
        ? myself
        : undefined);
    if (!account || account.accountId !== myself.accountId)
      throw new Error(
        'JIRA email must identify the authenticated current user. Allow user lookup/email visibility.',
      );
    return { ...account, displayName: myself.displayName ?? account.displayName };
  }

  private async discoverContext(): Promise<SprintContext> {
    const [{ jira }, account] = await Promise.all([this.config.load(), this.getAccount()]);
    let boardId = jira.boardId;
    if (!boardId) {
      const boards = await this.values<{ id: number }>('/rest/agile/1.0/board', {
        projectKeyOrId: jira.projectKey,
        type: 'scrum',
      });
      if (boards.length !== 1)
        throw new Error('Expected one Scrum board. Set jira.boardId to select the project board.');
      boardId = boards[0].id;
    }
    const sprints = (
      await this.values<Sprint>(`/rest/agile/1.0/board/${boardId}/sprint`, {
        state: 'active,future',
      })
    ).filter((entry): entry is JiraSprint => entry.state === 'active' || entry.state === 'future');
    const namePattern = jira.sprintNamePattern
      ? compileSprintNamePattern(jira.sprintNamePattern)
      : undefined;
    const available = sprints
      .filter((entry) => entry.id === jira.sprintId || !namePattern || namePattern.test(entry.name))
      .sort((a, b) => Number(b.state === 'active') - Number(a.state === 'active'));
    const candidates = jira.sprintId
      ? available.filter((entry) => entry.id === jira.sprintId && entry.state === 'active')
      : available.filter((entry) => entry.state === 'active');
    let sprintWarning: string | undefined;
    if (candidates.length !== 1) {
      if (jira.sprintId)
        sprintWarning = 'jira.sprintId must identify an active sprint on the selected board.';
      else if (namePattern)
        sprintWarning = candidates.length
          ? 'Multiple active sprints match jira.sprintNamePattern. Select a sprint or set jira.sprintId.'
          : 'No active sprint matches jira.sprintNamePattern. Select a future sprint or use No sprint assigned.';
      else
        sprintWarning = candidates.length
          ? 'Multiple active sprints. Select a sprint, configure jira.sprintNamePattern, or set jira.sprintId.'
          : 'No active sprint. Select a future sprint or use No sprint assigned.';
    }
    return {
      boardId,
      accountId: account.accountId,
      username: account.displayName
        ? `${account.displayName} (${account.emailAddress ?? jira.email})`
        : (account.emailAddress ?? jira.email),
      projectKey: jira.projectKey,
      sprints: available,
      defaultSprintId: candidates.length === 1 ? candidates[0].id : undefined,
      sprintWarning,
    };
  }

  async assertSelectableSprint(sprintId: number): Promise<void> {
    const context = await this.getSprintContext();
    if (!context.sprints.some((entry) => entry.id === sprintId))
      throw new Error('Select an available sprint on the configured board.');
    const sprint = await this.get<Sprint>(`/rest/agile/1.0/sprint/${sprintId}`);
    if (sprint.state !== 'active' && sprint.state !== 'future') {
      this.context = undefined;
      throw new Error('The sprint is no longer active or future. Refresh before syncing.');
    }
  }

  async getTickets(selection?: number | null): Promise<{
    tickets: JiraTicket[];
    context: SprintContext;
    selection: number | null;
  }> {
    const { jira } = await this.config.load();
    const context = await this.getSprintContext(true);
    const requested = selection === undefined ? (context.defaultSprintId ?? null) : selection;
    const selected =
      requested !== null && !context.sprints.some((entry) => entry.id === requested)
        ? null
        : requested;
    const sprintWarning =
      requested !== selected
        ? 'The selected sprint is no longer available. Showing tickets without a sprint.'
        : context.sprintWarning;
    const tickets: JiraTicket[] = [];
    let nextPageToken: string | undefined;
    const seen = new Set<string>();
    do {
      const page = await this.get<{
        issues: {
          key: string;
          fields: {
            summary: string;
            description?: unknown;
            status: { name: string };
            issuetype?: { name: string };
          };
        }[];
        nextPageToken?: string;
        isLast?: boolean;
      }>('/rest/api/3/search/jql', {
        jql: `project = ${jira.projectKey} AND ${selected === null ? 'sprint IS EMPTY' : `sprint = ${selected}`} AND assignee = ${JSON.stringify(context.accountId)} AND statusCategory != Done ORDER BY updated DESC`,
        fields: 'summary,status,description,issuetype',
        maxResults: 100,
        ...(nextPageToken ? { nextPageToken } : {}),
      });
      tickets.push(
        ...page.issues.map((issue) => ({
          key: issue.key,
          summary: issue.fields.summary,
          description: descriptionText(issue.fields.description).trim(),
          status: issue.fields.status.name,
          issueType: issue.fields.issuetype?.name ?? 'Unknown type',
          url: `${jira.baseUrl}/browse/${issue.key}`,
          links: [],
          pullRequests: [],
        })),
      );
      nextPageToken = page.isLast ? undefined : page.nextPageToken;
      if (nextPageToken && seen.has(nextPageToken))
        throw new Error('JIRA returned a repeated search page. Refresh to retry.');
      if (nextPageToken) seen.add(nextPageToken);
    } while (nextPageToken);
    return {
      context: { ...context, sprintWarning },
      selection: selected,
      tickets: await mapConcurrent(tickets, async (ticket) => {
        try {
          return { ...ticket, links: await this.getWebLinks(ticket.key) };
        } catch (error) {
          return { ...ticket, linkError: errorMessage(error) };
        }
      }),
    };
  }

  private issuePath(key: string): string {
    if (!/^[A-Z][A-Z0-9_]*-\d+$/.test(key)) throw new Error('Invalid JIRA issue key.');
    return `/rest/api/3/issue/${key}`;
  }

  async getWebLinks(key: string, signal?: AbortSignal): Promise<JiraWebLink[]> {
    const links = await this.get<{ id: number; object?: { url?: string; title?: string } }[]>(
      `${this.issuePath(key)}/remotelink`,
      {},
      signal,
    );
    return links.flatMap((link) => {
      try {
        const url = new URL(link.object?.url ?? '');
        return url.protocol === 'https:' && !url.username && !url.password
          ? [{ id: link.id, url: url.href, title: link.object?.title ?? url.href }]
          : [];
      } catch {
        return [];
      }
    });
  }

  async addWebLink(key: string, prUrl: string): Promise<void> {
    const url = new URL(prUrl);
    if (url.protocol !== 'https:' || url.username || url.password)
      throw new Error('Only HTTPS PR links are allowed.');
    if (this.testMode) return;
    // A stable globalId makes retries update the same JIRA remote link instead of duplicating it.
    url.hash = '';
    url.search = '';
    url.pathname = url.pathname.replace(/\/$/, '');
    await this.post(`${this.issuePath(key)}/remotelink`, {
      globalId: `pullboard:${url.href}`,
      object: { url: url.href, title: 'Upstream pull request' },
    });
  }

  async transition(key: string, phase: TicketPhase): Promise<void> {
    if (this.testMode) return;
    const { jira } = await this.config.load();
    const targets = jira.statuses?.[phase] ?? DEFAULT_STATUSES[phase];
    const current = await this.get<{ fields: { status: { name: string } } }>(this.issuePath(key), {
      fields: 'status',
    });
    if (targets.some((target) => target.toLowerCase() === current.fields.status.name.toLowerCase()))
      return;
    // Discover transitions available NOW for this issue; never hard-code workflow IDs.
    const { transitions } = await this.get<{ transitions: Transition[] }>(
      `${this.issuePath(key)}/transitions`,
      { expand: 'transitions.fields' },
    );
    const transition = selectTransition(transitions, targets);
    await this.post(`${this.issuePath(key)}/transitions`, { transition: { id: transition.id } });
  }

  private getCreateMetadata(): Promise<CreateMetadata> {
    return (this.metadata ??= this.discoverCreateMetadata().catch((error: unknown) => {
      this.metadata = undefined;
      throw error;
    }));
  }

  private async discoverCreateMetadata(): Promise<CreateMetadata> {
    const { jira } = await this.config.load();
    const types: { id: string; name: string; subtask?: boolean }[] = [];
    for (let startAt = 0; ;) {
      const page = await this.get<{ issueTypes: typeof types; total: number }>(
        `/rest/api/3/issue/createmeta/${jira.projectKey}/issuetypes`,
        { startAt, maxResults: 50 },
      );
      types.push(...page.issueTypes);
      startAt += page.issueTypes.length;
      if (!page.issueTypes.length || startAt >= page.total) break;
    }
    const issueType = jira.issueTypeId
      ? types.find((type) => type.id === jira.issueTypeId)
      : types.find((type) => type.name.toLowerCase() === 'story' && !type.subtask);
    if (!issueType)
      throw new Error(
        'No Story issue type is available. Set jira.issueTypeId to a creatable non-subtask type.',
      );
    if (issueType.subtask)
      throw new Error('Subtasks are not supported. Select a standard issue type.');
    const fields: CreateField[] = [];
    for (let startAt = 0; ;) {
      const page = await this.get<{ fields: CreateField[]; total: number }>(
        `/rest/api/3/issue/createmeta/${jira.projectKey}/issuetypes/${issueType.id}`,
        { startAt, maxResults: 50 },
      );
      fields.push(...page.fields);
      startAt += page.fields.length;
      if (!page.fields.length || startAt >= page.total) break;
    }
    const points = jira.storyPointsFieldId
      ? fields.find((field) => field.fieldId === jira.storyPointsFieldId)
      : fields.find((field) =>
          ['story points', 'story point estimate'].includes(field.name.toLowerCase()),
        );
    if (!points || points.schema?.type !== 'number')
      throw new Error(
        'Expose a numeric Story Points field on the create screen, or set jira.storyPointsFieldId.',
      );
    if (!fields.some((field) => field.fieldId === 'assignee'))
      throw new Error('Expose Assignee on the JIRA create screen to enable automatic assignment.');
    const supplied = new Set([
      'summary',
      'description',
      'project',
      'issuetype',
      'assignee',
      points.fieldId,
    ]);
    const missing = fields.filter(
      (field) => field.required && !field.hasDefaultValue && !supplied.has(field.fieldId),
    );
    if (missing.length)
      throw new Error(
        `JIRA requires additional fields: ${missing.map((field) => field.name).join(', ')}. Configure defaults in JIRA.`,
      );
    return { issueTypeId: issueType.id, pointsFieldId: points.fieldId };
  }

  /** No automatic retry: creation is not idempotent. Explicit null overrides project assignment defaults. */
  async createTicket(suggestion: TicketSuggestion, unassigned = false): Promise<SyncReceipt> {
    if (
      !suggestion.title.trim() ||
      suggestion.title.trim().length > 255 ||
      !suggestion.description.trim() ||
      !Number.isFinite(suggestion.storyPoints) ||
      suggestion.storyPoints < 0 ||
      suggestion.storyPoints > 100
    )
      throw new Error('Review the ticket title, description, and story points (0–100).');
    if (this.testMode)
      return { key: `SIMULATED-${++this.simulatedCount}`, url: '', simulated: true };
    const [{ jira }, account, metadata] = await Promise.all([
      this.config.load(),
      unassigned ? undefined : this.getAccount(),
      this.getCreateMetadata(),
    ]);
    const paragraphs = suggestion.description.split(/\r?\n/).map((line) => ({
      type: 'paragraph',
      ...(line ? { content: [{ type: 'text', text: line }] } : {}),
    }));
    const created = await this.post<{ key: string }>('/rest/api/3/issue', {
      fields: {
        project: { key: jira.projectKey },
        issuetype: { id: metadata.issueTypeId },
        summary: suggestion.title.trim(),
        description: { type: 'doc', version: 1, content: paragraphs },
        // Explicit null prevents the project's default assignee from taking ownership.
        assignee: account ? { accountId: account.accountId } : null,
        [metadata.pointsFieldId]: suggestion.storyPoints,
      },
    });
    return { key: created.key, url: `${jira.baseUrl}/browse/${created.key}` };
  }

  async addToSprint(key: string, sprintId: number): Promise<void> {
    await this.assertSelectableSprint(sprintId);
    if (this.testMode) return;
    await this.post(`/rest/agile/1.0/sprint/${sprintId}/issue`, { issues: [key] });
  }
}
