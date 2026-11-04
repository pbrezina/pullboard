import { HttpErrorResponse } from '@angular/common/http';
import { computed, inject, Service, signal } from '@angular/core';
import { MatDialog } from '@angular/material/dialog';
import { firstValueFrom } from 'rxjs';
import { ConfigService } from './config';
import { errorMessage } from './errors';
import { JiraService } from './jira';
import { LlmService } from './llm';
import { SyncReceipt, TicketCreation, UnifiedPullRequest } from './models';
import { PrJiraLinks } from './pr-jira-links';
import { TEST_MODE } from './test-mode';
import { SyncDialog, SyncDialogData } from '../sync-dialog/sync-dialog';

export interface ReviewTicketState {
  status: 'suggesting' | 'creating' | 'linking' | 'complete' | 'partial' | 'uncertain' | 'error';
  receipt?: SyncReceipt;
  sprintId?: number;
  error?: string;
}

export const reviewTitlePrefix = (pr: UnifiedPullRequest) =>
  `Review PR ${pr.repository}#${pr.number}`;

@Service()
export class ReviewTickets {
  private readonly jira = inject(JiraService);
  private readonly llm = inject(LlmService);
  private readonly config = inject(ConfigService);
  private readonly dialogs = inject(MatDialog);
  private readonly links = inject(PrJiraLinks);
  private readonly testMode = inject(TEST_MODE);
  private initialized?: Promise<void>;
  private storageKey = '';
  readonly states = signal<Record<string, ReviewTicketState>>({});
  readonly receipts = signal<Record<string, SyncReceipt[]>>({});
  readonly storageWarning = signal('');
  readonly busy = computed(() =>
    Object.values(this.states()).some((state) =>
      ['suggesting', 'creating', 'linking'].includes(state.status),
    ),
  );

  initialize(): Promise<void> {
    return (this.initialized ??= this.restore());
  }

  private async restore(): Promise<void> {
    if (this.testMode) return;
    try {
      const { jira } = await this.config.load();
      this.storageKey = `pullboard:reviews:v1:${jira.baseUrl}:${jira.projectKey}:${jira.email}`;
      const raw: unknown = JSON.parse(localStorage.getItem(this.storageKey) ?? '{}');
      if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return;
      const saved = raw as {
        receipts?: Record<string, unknown>;
        pending?: Record<string, unknown>;
      };
      const validUrl = (value: string) => {
        try {
          const url = new URL(value);
          return url.protocol === 'https:' && !url.username && !url.password;
        } catch {
          return false;
        }
      };
      const validReceipt = (value: unknown): value is SyncReceipt => {
        if (!value || typeof value !== 'object') return false;
        const receipt = value as SyncReceipt;
        return (
          !receipt.simulated &&
          typeof receipt.key === 'string' &&
          /^[A-Z][A-Z0-9_]*-\d+$/.test(receipt.key) &&
          receipt.url === `${jira.baseUrl}/browse/${receipt.key}`
        );
      };
      const receipts: Record<string, SyncReceipt[]> = {};
      for (const [url, entries] of Object.entries(saved.receipts ?? {})) {
        if (!validUrl(url) || !Array.isArray(entries)) continue;
        receipts[url] = entries.filter(validReceipt);
        this.links.remember(url, receipts[url]);
      }
      this.receipts.set(receipts);
      const states: Record<string, ReviewTicketState> = {};
      for (const [url, value] of Object.entries(saved.pending ?? {})) {
        if (!validUrl(url) || !value || typeof value !== 'object') continue;
        const state = value as ReviewTicketState;
        if (state.status === 'uncertain') states[url] = { status: 'uncertain' };
        else if (state.status === 'partial' && validReceipt(state.receipt)) {
          states[url] = {
            status: 'partial',
            receipt: state.receipt,
            sprintId:
              Number.isSafeInteger(state.sprintId) && state.sprintId! > 0
                ? state.sprintId
                : undefined,
          };
        }
      }
      this.states.set(states);
    } catch {
      this.storageWarning.set(
        'Review ticket storage is unavailable. Keep this page open and check JIRA before creating another ticket after a reload.',
      );
    }
  }

  private save(): void {
    if (this.testMode || !this.storageKey) return;
    try {
      const pending = Object.fromEntries(
        Object.entries(this.states()).flatMap(([url, state]) => {
          if (state.status === 'creating' || state.status === 'uncertain')
            return [[url, { status: 'uncertain' }]];
          if (state.receipt && ['linking', 'partial'].includes(state.status))
            return [[url, { status: 'partial', receipt: state.receipt, sprintId: state.sprintId }]];
          return [];
        }),
      );
      localStorage.setItem(this.storageKey, JSON.stringify({ receipts: this.receipts(), pending }));
    } catch {
      this.storageWarning.set(
        'Could not save the review ticket receipt. Keep this page open and check JIRA before retrying after a reload.',
      );
    }
  }

  private set(url: string, state?: ReviewTicketState): void {
    this.states.update((states) => {
      const next = { ...states };
      if (state) next[url] = state;
      else delete next[url];
      return next;
    });
    this.save();
  }

  async create(pr: UnifiedPullRequest, manual = false): Promise<void> {
    await this.initialize();
    if (this.busy() || ['partial', 'uncertain'].includes(this.states()[pr.url]?.status)) return;
    this.set(pr.url, { status: 'suggesting' });
    try {
      const prefix = reviewTitlePrefix(pr);
      const [suggestion, context] = await Promise.all([
        manual
          ? {
              title: pr.title,
              description: `Review ${pr.url}\n\n${pr.description}`,
              storyPoints: 1,
            }
          : this.llm.suggest(pr, true),
        this.jira.getSprintContext(),
      ]);
      const result = await firstValueFrom(
        this.dialogs
          .open<SyncDialog, SyncDialogData, TicketCreation>(SyncDialog, {
            width: '640px',
            maxWidth: 'calc(100vw - 32px)',
            closeOnNavigation: false,
            data: {
              suggestion: {
                ...suggestion,
                title: suggestion.title.slice(0, 255 - prefix.length - 2),
              },
              draft: pr.draft,
              generated: !manual,
              titlePrefix: prefix,
              previousTickets: this.receipts()[pr.url] ?? [],
              sprints: context.sprints,
              defaultSprintId: context.defaultSprintId,
            },
          })
          .afterClosed(),
      );
      if (!result) {
        this.set(pr.url);
        return;
      }
      if (result.sprintId !== undefined) await this.jira.assertSelectableSprint(result.sprintId);
      // Enforce the prefix independently of the model and the confirmation dialog.
      const suffix = result.title.startsWith(`${prefix}: `)
        ? result.title.slice(prefix.length + 2)
        : result.title;
      const creation = { ...result, title: `${prefix}: ${suffix.trim()}` };
      this.set(pr.url, { status: 'creating' });
      const receipt = await this.jira.createTicket(creation, true);
      // Persist the key before any subsequent non-transactional request.
      this.set(pr.url, { status: 'linking', receipt, sprintId: result.sprintId });
      await this.finish(pr.url);
    } catch (error) {
      const state = this.states()[pr.url];
      if (state?.receipt)
        this.set(pr.url, { ...state, status: 'partial', error: errorMessage(error) });
      else if (
        !this.testMode &&
        state?.status === 'creating' &&
        error instanceof HttpErrorResponse &&
        (error.status === 0 || error.status >= 500)
      ) {
        this.set(pr.url, {
          status: 'uncertain',
          error: 'Creation outcome is unknown. Check JIRA before trying again.',
        });
      } else this.set(pr.url, { status: 'error', error: errorMessage(error) });
    }
  }

  async resume(pr: UnifiedPullRequest): Promise<void> {
    await this.initialize();
    const state = this.states()[pr.url];
    if (this.busy() || state?.status !== 'partial' || !state.receipt) return;
    this.set(pr.url, { ...state, status: 'linking', error: undefined });
    try {
      await this.finish(pr.url);
    } catch (error) {
      this.set(pr.url, { ...state, status: 'partial', error: errorMessage(error) });
    }
  }

  private async finish(url: string): Promise<void> {
    const state = this.states()[url];
    const receipt = state.receipt!;
    if (!this.testMode) {
      await this.jira.addWebLink(receipt.key, url);
      if (state.sprintId !== undefined) await this.jira.addToSprint(receipt.key, state.sprintId);
      this.receipts.update((receipts) => ({
        ...receipts,
        [url]: [
          ...new Map(
            [...(receipts[url] ?? []), receipt].map((entry) => [entry.key, entry]),
          ).values(),
        ],
      }));
      this.links.remember(url, [receipt]);
    }
    // Deliberately do not transition: review tickets keep JIRA's initial status.
    this.set(url, { status: 'complete', receipt });
  }

  allowRetry(pr: UnifiedPullRequest): void {
    if (this.busy() || this.states()[pr.url]?.status !== 'uncertain') return;
    if (
      window.confirm('Check JIRA first. Only allow a new attempt if no review ticket was created.')
    )
      this.set(pr.url);
  }
}
