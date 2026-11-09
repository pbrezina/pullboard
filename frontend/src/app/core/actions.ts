import { DestroyRef, inject, Service, signal } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { finalize, tap } from 'rxjs';
import { Api, errorMessage } from './api';
import type {
  CreateIssueRequest,
  CreateUpstreamIssueRequest,
  CreateIssueResult,
  RuntimeMode,
} from './api-types';
import { BoardStore } from './board-store';
import { Notifications } from './notifications';

@Service()
export class Actions {
  private readonly api = inject(Api);
  private readonly store = inject(BoardStore);
  private readonly notices = inject(Notifications);
  private readonly destroy = inject(DestroyRef);
  readonly linking = signal<ReadonlySet<string>>(new Set());
  link(
    issueKey: string,
    prId: string,
    mode: RuntimeMode,
    callbacks?: { success(): void; error(e: unknown): void },
  ) {
    const key = JSON.stringify([issueKey, prId]);
    if (this.linking().has(key)) return;
    this.linking.update((s) => new Set([...s, key]));
    this.api
      .link(issueKey, prId, mode.mode_revision)
      .pipe(
        takeUntilDestroyed(this.destroy),
        finalize(() => this.linking.update((s) => new Set([...s].filter((k) => k !== key)))),
      )
      .subscribe({
        next: (result) => {
          const prefix =
            result.outcome === 'simulated'
              ? 'Simulated: '
              : result.outcome === 'already_linked'
                ? 'Already linked: '
                : 'Linked: ';
          this.notices.show(prefix + result.message);
          if (result.outcome !== 'simulated') this.store.refresh();
          callbacks?.success();
        },
        error: (e) => {
          this.notices.show(errorMessage(e), 'error');
          callbacks?.error(e);
        },
      });
  }
  create(request: CreateIssueRequest) {
    return this.api.create(request).pipe(tap((result) => this.created(result)));
  }
  createUpstreamIssue(request: CreateUpstreamIssueRequest) {
    return this.api.createUpstreamIssue(request).pipe(tap((result) => this.created(result)));
  }
  private created(result: CreateIssueResult) {
    this.notices.show(
      (result.outcome === 'simulated' ? 'Simulated: ' : 'Ticket created. ') +
        result.message +
        (result.warnings.length ? ' Warning: ' + result.warnings.join(' ') : ''),
      result.warnings.length ? 'warning' : 'success',
      result.issue?.url ?? null,
      result.issue?.key ?? null,
    );
    // No optimistic or synthetic cache entities, including test mode.
    if (result.outcome === 'created') this.store.refresh();
  }
}
