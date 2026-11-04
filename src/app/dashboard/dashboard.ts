import { HttpErrorResponse } from '@angular/common/http';
import { Component, computed, inject, OnInit, signal } from '@angular/core';
import { CdkDrag, CdkDragDrop, CdkDropList, DragDropModule } from '@angular/cdk/drag-drop';
import { MatButtonModule } from '@angular/material/button';
import { MatCardModule } from '@angular/material/card';
import { MatDialog } from '@angular/material/dialog';
import { MatMenuModule } from '@angular/material/menu';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { disabled, form, FormField } from '@angular/forms/signals';
import { firstValueFrom } from 'rxjs';
import { ConfigService } from '../core/config';
import { mapConcurrent } from '../core/concurrency';
import { errorMessage } from '../core/errors';
import { ForgeAggregatorService } from '../core/forge-aggregator';
import { JiraService } from '../core/jira';
import { LlmService } from '../core/llm';
import {
  JiraTicket,
  ForgeIdentity,
  LinkedPullRequest,
  SprintContext,
  SyncReceipt,
  TicketCreation,
  UnifiedPullRequest,
} from '../core/models';
import { PartialSyncError, SyncService } from '../core/sync';
import { SyncDialog, SyncDialogData } from '../sync-dialog/sync-dialog';
import { TEST_MODE } from '../core/test-mode';
import { PrBadges } from '../pr-badges/pr-badges';
import { isAccepted } from '../core/pull-request-order';
import { PrJiraLinks } from '../core/pr-jira-links';
import { PrActivity } from '../pr-activity/pr-activity';
import { ReviewTickets } from '../core/review-tickets';

interface CardState {
  status: 'suggesting' | 'syncing' | 'synced' | 'partial' | 'uncertain' | 'error';
  receipt?: SyncReceipt;
  /** Completed associations, independent of the current operation's receipt. */
  linkedTickets?: SyncReceipt[];
  /** Legacy receipts did not save the destination ID; never guess on resume. */
  addToSprint?: boolean;
  sprintId?: number;
  error?: string;
}

@Component({
  imports: [
    DragDropModule,
    MatButtonModule,
    MatCardModule,
    MatMenuModule,
    MatProgressBarModule,
    FormField,
    PrBadges,
    PrActivity,
  ],
  selector: 'app-dashboard',
  styleUrl: './dashboard.scss',
  templateUrl: './dashboard.html',
})
export class Dashboard implements OnInit {
  private readonly config = inject(ConfigService);
  private readonly forges = inject(ForgeAggregatorService);
  private readonly jira = inject(JiraService);
  private readonly llm = inject(LlmService);
  private readonly sync = inject(SyncService);
  private readonly dialogs = inject(MatDialog);
  protected readonly jiraLinks = inject(PrJiraLinks);
  protected readonly reviews = inject(ReviewTickets);
  protected readonly isAccepted = isAccepted;
  protected readonly refreshedAt = signal(Date.now());
  protected readonly testMode = inject(TEST_MODE);
  protected readonly pullRequests = signal<UnifiedPullRequest[]>([]);
  protected readonly tickets = signal<JiraTicket[]>([]);
  protected readonly context = signal<SprintContext | undefined>(undefined);
  protected readonly identities = signal<ForgeIdentity[]>([]);
  protected readonly jiraUsername = signal('');
  protected readonly projectKey = signal('');
  protected readonly loading = signal(true);
  protected readonly configured = signal(false);
  protected readonly configError = signal('');
  protected readonly warnings = signal<string[]>([]);
  protected readonly storageWarning = signal('');
  protected readonly states = signal<Record<string, CardState>>({});
  protected readonly ticketBusy = signal<Record<string, boolean>>({});
  protected readonly ticketMessages = signal<Record<string, string>>({});
  protected readonly updatedAt = signal('');
  protected readonly jiraHome = signal('');
  protected readonly savedLinks = computed(() => {
    const visible = new Set(this.pullRequests().map((pr) => pr.url));
    return Object.entries(this.states())
      .filter(([url]) => !visible.has(url))
      .flatMap(([url, state]) => {
        const receipts = new Map(
          (state.linkedTickets ?? []).map((receipt) => [receipt.key, receipt]),
        );
        if (state.receipt && !state.receipt.simulated)
          receipts.set(state.receipt.key, state.receipt);
        return [...receipts.values()].map((receipt) => ({
          url,
          receipt,
          partial: state.receipt?.key === receipt.key && state.status !== 'synced',
        }));
      });
  });
  protected readonly unlinkedCount = computed(
    () => this.pullRequests().filter((pr) => !this.linkedTickets(pr).length).length,
  );
  protected readonly closedCount = computed(
    () => this.tickets().filter((ticket) => this.hasClosedPr(ticket)).length,
  );
  protected readonly operationBusy = computed(
    () =>
      Object.values(this.states()).some(
        (state) => state.status === 'syncing' || state.status === 'suggesting',
      ) || Object.values(this.ticketBusy()).some(Boolean),
  );
  protected readonly filters = signal({ pullRequests: '', tickets: '', sprint: 'default' });
  protected readonly filterFields = form(this.filters, (path) => {
    disabled(path.sprint, () => this.loading() || this.operationBusy() || !this.context());
  });
  protected readonly filteredPullRequests = computed(() => {
    const query = this.filters().pullRequests.trim().toLowerCase();
    return this.pullRequests().filter((pr) =>
      `${pr.title} #${pr.number}`.toLowerCase().includes(query),
    );
  });
  protected readonly filteredTickets = computed(() => {
    const query = this.filters().tickets.trim().toLowerCase();
    return this.tickets().filter((ticket) =>
      `${ticket.summary} ${ticket.key}`.toLowerCase().includes(query),
    );
  });
  protected readonly selectedSprintName = computed(
    () =>
      this.context()?.sprints.find((sprint) => String(sprint.id) === this.filters().sprint)?.name ??
      'No sprint',
  );
  protected readonly canEnterTicket = (
    drag: CdkDrag<UnifiedPullRequest>,
    drop: CdkDropList<JiraTicket>,
  ): boolean => this.canLink(drag.data, drop.data);
  private storageKey = '';

  ngOnInit(): void {
    void this.initialize();
  }

  private async initialize(): Promise<void> {
    try {
      const { jira } = await this.config.load();
      this.jiraUsername.set(jira.email);
      this.projectKey.set(jira.projectKey);
      this.jiraHome.set(
        `${jira.baseUrl}/issues/?jql=${encodeURIComponent(`project = ${jira.projectKey} ORDER BY created DESC`)}`,
      );
      this.storageKey = `pullboard:v1:${jira.baseUrl}:${jira.projectKey}:${jira.email}`;
      if (!this.testMode) this.restoreReceipts(jira.baseUrl);
      this.configured.set(true);
      await this.refresh();
    } catch (error) {
      this.configError.set(`Cannot load local configuration. ${errorMessage(error)}`);
      this.loading.set(false);
    }
  }

  private restoreReceipts(jiraBaseUrl: string): void {
    try {
      const raw: unknown = JSON.parse(localStorage.getItem(this.storageKey) ?? '{}');
      if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return;
      const states: Record<string, CardState> = {};
      for (const [url, value] of Object.entries(raw)) {
        if (!value || typeof value !== 'object') continue;
        const state = value as Partial<CardState>;
        const validReceipt = (receipt: SyncReceipt | undefined): receipt is SyncReceipt =>
          !!receipt &&
          !receipt.simulated &&
          typeof receipt.key === 'string' &&
          /^[A-Z][A-Z0-9_]*-\d+$/.test(receipt.key) &&
          receipt.url === `${jiraBaseUrl}/browse/${receipt.key}`;
        const linkedTickets = Array.isArray(state.linkedTickets)
          ? state.linkedTickets.filter(validReceipt)
          : [];
        if (state.status === 'uncertain') {
          states[url] = { status: 'uncertain', linkedTickets };
          continue;
        }
        const receipt = state.receipt;
        if ((state.status === 'synced' || state.status === 'partial') && validReceipt(receipt)) {
          states[url] = {
            status: state.status,
            receipt,
            linkedTickets:
              state.status === 'synced'
                ? [
                    ...new Map(
                      [...linkedTickets, receipt].map((entry) => [entry.key, entry]),
                    ).values(),
                  ]
                : linkedTickets,
            addToSprint: state.addToSprint === true,
            sprintId:
              Number.isSafeInteger(state.sprintId) && state.sprintId! > 0
                ? state.sprintId
                : undefined,
          };
        } else if (linkedTickets.length) {
          states[url] = { status: 'synced', linkedTickets };
        }
      }
      this.states.set(states);
      for (const [url, state] of Object.entries(states))
        this.jiraLinks.remember(url, state.linkedTickets ?? []);
    } catch {
      this.storageWarning.set(
        'Local receipt storage is unavailable. Keep this tab open after creating tickets; check JIRA before retrying after a reload.',
      );
    }
  }

  private setState(url: string, state?: CardState): void {
    this.states.update((current) => {
      const next = { ...current };
      const linkedTickets = new Map(
        [...(current[url]?.linkedTickets ?? []), ...(state?.linkedTickets ?? [])].map((receipt) => [
          receipt.key,
          receipt,
        ]),
      );
      if (state?.status === 'synced' && state.receipt && !state.receipt.simulated)
        linkedTickets.set(state.receipt.key, state.receipt);
      if (state) next[url] = { ...state, linkedTickets: [...linkedTickets.values()] };
      else if (linkedTickets.size)
        next[url] = { status: 'synced', linkedTickets: [...linkedTickets.values()] };
      else delete next[url];
      return next;
    });
    this.jiraLinks.remember(url, this.states()[url]?.linkedTickets ?? []);
    if (this.testMode) return;
    try {
      const durable = Object.fromEntries(
        Object.entries(this.states()).flatMap(([key, value]) => {
          if ((value.receipt && !value.receipt.simulated) || value.linkedTickets?.length)
            return [
              [
                key,
                {
                  ...value,
                  status:
                    value.status === 'uncertain'
                      ? 'uncertain'
                      : value.status === 'synced' || !value.receipt
                        ? 'synced'
                        : 'partial',
                  error: undefined,
                },
              ],
            ];
          return value.status === 'uncertain' ? [[key, { status: 'uncertain' }]] : [];
        }),
      );
      localStorage.setItem(this.storageKey, JSON.stringify(durable));
    } catch {
      this.storageWarning.set(
        'Could not save the local receipt. Keep this tab open and check JIRA before retrying after a reload.',
      );
    }
  }

  protected async refresh(): Promise<void> {
    this.loading.set(true);
    this.warnings.set([]);
    const [forgeResult, jiraResult] = await Promise.allSettled([
      this.forges.getOpenPullRequests(),
      this.jira.getTickets(
        this.filters().sprint === 'default'
          ? undefined
          : this.filters().sprint === 'none'
            ? null
            : Number(this.filters().sprint),
      ),
    ]);
    const warnings: string[] = [];
    if (forgeResult.status === 'fulfilled') {
      this.pullRequests.set(forgeResult.value.pullRequests);
      this.identities.set(forgeResult.value.identities);
      warnings.push(...forgeResult.value.warnings);
    } else warnings.push(`Forges: ${errorMessage(forgeResult.reason)}`);
    if (jiraResult.status === 'fulfilled') {
      this.context.set(jiraResult.value.context);
      this.jiraUsername.set(jiraResult.value.context.username);
      this.filters.update((filters) => ({
        ...filters,
        sprint: jiraResult.value.selection?.toString() ?? 'none',
      }));
      if (jiraResult.value.context.sprintWarning)
        warnings.push(jiraResult.value.context.sprintWarning);
      try {
        const checked = await mapConcurrent(jiraResult.value.tickets, (ticket) =>
          this.checkTicket(ticket),
        );
        this.tickets.set(checked);
        // Keep every association, including receipts from other sprint views. Never mask a partial sync.
        for (const pr of this.pullRequests()) {
          const linked = checked.filter((entry) =>
            entry.links.some((link) => this.sameUrl(link.url, pr.url)),
          );
          if (linked.length)
            this.setState(pr.url, {
              ...(this.states()[pr.url] ?? { status: 'synced' }),
              linkedTickets: linked.map((ticket) => ({ key: ticket.key, url: ticket.url })),
            });
        }
      } catch (error) {
        warnings.push(`PR status checks: ${errorMessage(error)}`);
      }
    } else {
      this.context.set(undefined);
      warnings.push(
        `JIRA: ${errorMessage(jiraResult.reason)}. Previously loaded cards, if any, are stale.`,
      );
    }
    this.warnings.set(warnings);
    this.refreshedAt.set(Date.now());
    this.updatedAt.set(new Date().toLocaleString());
    this.loading.set(false);
  }

  protected async selectSprint(value: string): Promise<void> {
    if (this.loading() || this.operationBusy()) return;
    this.filters.update((filters) => ({ ...filters, sprint: value }));
    await this.refresh();
  }

  private sameUrl(a: string, b: string): boolean {
    const canonical = (value: string) => {
      const url = new URL(value);
      return `${url.origin}${url.pathname.replace(/\/$/, '')}`;
    };
    try {
      return canonical(a) === canonical(b);
    } catch {
      return false;
    }
  }

  private async checkTicket(ticket: JiraTicket, freshLinks = false): Promise<JiraTicket> {
    let links = ticket.links;
    if (freshLinks) links = await this.jira.getWebLinks(ticket.key);
    const recognized: string[] = [];
    for (const link of links) {
      if (
        (await this.forges.supportsUrl(link.url)) &&
        !recognized.some((url) => this.sameUrl(url, link.url))
      )
        recognized.push(link.url);
    }
    const pullRequests = await mapConcurrent(
      recognized,
      async (url): Promise<LinkedPullRequest> => {
        const title =
          this.pullRequests().find((pr) => this.sameUrl(pr.url, url))?.title ??
          links.find((link) => link.url === url)?.title;
        try {
          return { url, title, state: (await this.forges.getPullRequestStatus(url)).state };
        } catch (error) {
          return { url, title, error: errorMessage(error) };
        }
      },
      2,
    );
    return { ...ticket, links, pullRequests, linkError: freshLinks ? undefined : ticket.linkError };
  }

  protected hasClosedPr(ticket: JiraTicket): boolean {
    return ticket.pullRequests.some((pr) => pr.state === 'closed' || pr.state === 'merged');
  }
  protected canClose(ticket: JiraTicket): boolean {
    return (
      !ticket.linkError &&
      ticket.pullRequests.length > 0 &&
      ticket.pullRequests.every((pr) => pr.state === 'closed' || pr.state === 'merged')
    );
  }
  protected prBusy(pr: UnifiedPullRequest): boolean {
    const status = this.states()[pr.url]?.status;
    return status === 'syncing' || status === 'suggesting';
  }

  protected linkedTickets(pr: UnifiedPullRequest): SyncReceipt[] {
    const state = this.states()[pr.url];
    const links = new Map((state?.linkedTickets ?? []).map((receipt) => [receipt.key, receipt]));
    for (const ticket of this.tickets()) {
      if (ticket.links.some((link) => this.sameUrl(link.url, pr.url)))
        links.set(ticket.key, { key: ticket.key, url: ticket.url });
    }
    return [...links.values()];
  }

  protected canLink(pr: UnifiedPullRequest, ticket?: JiraTicket): boolean {
    const state = this.states()[pr.url];
    return (
      !this.loading() &&
      !this.operationBusy() &&
      !!this.context() &&
      (!state || state.status === 'synced' || state.status === 'error') &&
      !ticket?.closed &&
      (!ticket || !this.linkedTickets(pr).some((receipt) => receipt.key === ticket.key))
    );
  }

  protected async suggest(pr: UnifiedPullRequest, manual = false): Promise<void> {
    if (
      this.operationBusy() ||
      this.loading() ||
      !this.context() ||
      this.linkedTickets(pr).length > 0 ||
      (this.states()[pr.url]?.receipt && !this.states()[pr.url]?.receipt?.simulated) ||
      this.states()[pr.url]?.status === 'uncertain'
    )
      return;
    this.setState(pr.url, { status: 'suggesting' });
    let sprintId: number | undefined;
    try {
      const suggestion = manual
        ? {
            title: pr.title,
            description: pr.description || `Track work from ${pr.url}`,
            storyPoints: 1,
          }
        : await this.llm.suggest(pr);
      const result = await firstValueFrom(
        this.dialogs
          .open<SyncDialog, SyncDialogData, TicketCreation>(SyncDialog, {
            width: '640px',
            maxWidth: 'calc(100vw - 32px)',
            closeOnNavigation: false,
            data: {
              suggestion,
              draft: pr.draft,
              generated: !manual,
              sprints: this.context()!.sprints,
              defaultSprintId: this.context()!.defaultSprintId,
            },
          })
          .afterClosed(),
      );
      if (!result) {
        this.setState(pr.url);
        return;
      }
      sprintId = result.sprintId;
      // If the tab closes during non-idempotent creation, a later session requires manual reconciliation.
      this.setState(pr.url, { status: 'uncertain' });
      this.states.update((states) => ({ ...states, [pr.url]: { status: 'syncing' } }));
      const receipt = await this.sync.create(
        pr,
        result,
        (created) => this.setState(pr.url, { status: 'syncing', receipt: created, sprintId }),
        sprintId,
      );
      this.setState(pr.url, { status: 'synced', receipt });
    } catch (error) {
      if (error instanceof PartialSyncError)
        this.setState(pr.url, {
          status: 'partial',
          receipt: error.receipt,
          sprintId,
          error: errorMessage(error.causeError),
        });
      else if (
        !this.testMode &&
        error instanceof HttpErrorResponse &&
        (error.status === 0 || error.status >= 500) &&
        this.states()[pr.url]?.status === 'syncing'
      ) {
        this.setState(pr.url, {
          status: 'uncertain',
          error: 'Creation outcome is unknown. Check JIRA for the ticket before retrying.',
        });
      } else this.setState(pr.url, { status: 'error', error: errorMessage(error) });
    }
  }

  protected async drop(
    event: CdkDragDrop<JiraTicket, UnifiedPullRequest[], UnifiedPullRequest>,
    ticket: JiraTicket,
  ): Promise<void> {
    if (event.previousContainer.id === event.container.id) return;
    await this.link(event.item.data, ticket);
  }

  protected async link(pr: UnifiedPullRequest, ticket: JiraTicket): Promise<void> {
    if (!this.canLink(pr, ticket)) return;
    const receipt = { key: ticket.key, url: ticket.url };
    this.setState(pr.url, { status: 'syncing', receipt });
    try {
      const result = await this.sync.resume(pr, receipt);
      this.setState(pr.url, { status: 'synced', receipt: result });
    } catch (error) {
      this.setState(pr.url, {
        status: 'partial',
        receipt,
        error: errorMessage(error instanceof PartialSyncError ? error.causeError : error),
      });
    }
  }

  protected async resume(pr: UnifiedPullRequest): Promise<void> {
    const state = this.states()[pr.url];
    if (!state?.receipt || this.operationBusy() || this.loading()) return;
    if (state.addToSprint && !state.sprintId) {
      this.setState(pr.url, {
        ...state,
        error:
          'This older receipt has no saved sprint ID. Complete its sprint assignment and remaining sync in JIRA to avoid using a different sprint.',
      });
      return;
    }
    this.setState(pr.url, { ...state, status: 'syncing', error: undefined });
    try {
      const receipt = await this.sync.resume(pr, state.receipt, state.sprintId);
      this.setState(pr.url, { status: 'synced', receipt });
    } catch (error) {
      this.setState(pr.url, {
        ...state,
        status: 'partial',
        error: errorMessage(error instanceof PartialSyncError ? error.causeError : error),
      });
    }
  }

  protected allowRetry(pr: UnifiedPullRequest): void {
    if (
      window.confirm(
        'Check JIRA for an existing ticket first. If one exists, cancel and complete its sprint, link and status in JIRA. Allow a new creation attempt only if no ticket was created.',
      )
    )
      this.setState(pr.url);
  }

  protected async closeTicket(ticket: JiraTicket): Promise<void> {
    if (this.operationBusy() || this.loading() || !this.context() || !this.canClose(ticket)) return;
    this.ticketBusy.update((busy) => ({ ...busy, [ticket.key]: true }));
    this.ticketMessages.update((messages) => ({ ...messages, [ticket.key]: '' }));
    try {
      // Re-read both remote links and upstream states immediately before writing JIRA.
      const fresh = await this.checkTicket(ticket, true);
      this.tickets.update((tickets) =>
        tickets.map((entry) => (entry.key === ticket.key ? fresh : entry)),
      );
      if (!this.canClose(fresh))
        throw new Error('Not all linked PRs are closed or merged. The ticket was not closed.');
      await this.jira.transition(ticket.key, 'done');
      this.ticketMessages.update((messages) => ({
        ...messages,
        [ticket.key]: this.testMode
          ? 'Test mode: ticket closure simulated. No changes saved.'
          : 'Ticket closed in JIRA.',
      }));
      if (this.testMode) return;
      // Retain the card and its permanent JIRA link until the next refresh.
      this.tickets.update((tickets) =>
        tickets.map((entry) =>
          entry.key === ticket.key
            ? { ...entry, status: 'Closed', closed: true, pullRequests: [] }
            : entry,
        ),
      );
    } catch (error) {
      this.ticketMessages.update((messages) => ({
        ...messages,
        [ticket.key]: errorMessage(error),
      }));
    } finally {
      this.ticketBusy.update((busy) => ({ ...busy, [ticket.key]: false }));
    }
  }
}
