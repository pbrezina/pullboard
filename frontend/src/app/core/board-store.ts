import { DestroyRef, inject, Service, signal } from '@angular/core';
import { catchError, finalize, forkJoin, of, Subscription, switchMap, map } from 'rxjs';
import { Api, errorMessage } from './api';
import type { Board, Context, Health, SyncStatus } from './api-types';

@Service()
export class BoardStore {
  private readonly api = inject(Api);
  private readonly destroy = inject(DestroyRef);
  readonly context = signal<Context | null>(null);
  readonly board = signal<Board | null>(null);
  readonly sync = signal<SyncStatus | null>(null);
  readonly health = signal<Health | null>(null);
  readonly loading = signal(false);
  readonly refreshing = signal(false);
  readonly error = signal('');
  private started = false;
  private disposed = false;
  private timer?: ReturnType<typeof setTimeout>;
  private read?: Subscription;
  private write?: Subscription;
  private pendingRefresh = false;
  private poll?: Subscription;
  private publication = '';
  private statusVersion = 0;

  constructor() {
    this.destroy.onDestroy(() => {
      this.disposed = true;
      clearTimeout(this.timer);
      this.read?.unsubscribe();
      this.write?.unsubscribe();
      this.poll?.unsubscribe();
    });
  }
  start() {
    if (!this.started) {
      this.started = true;
      this.load();
    }
  }
  /** Complete each cycle before starting another; preserve last good values on error. */
  load() {
    if (this.disposed || this.loading()) return;
    clearTimeout(this.timer);
    this.poll?.unsubscribe();
    const statusVersion = ++this.statusVersion;
    this.loading.set(true);
    const errors: string[] = [];
    const recover = <T>(request: import('rxjs').Observable<T>) =>
      request.pipe(
        catchError((e) => {
          errors.push(errorMessage(e));
          return of(null);
        }),
      );
    this.read = forkJoin({
      context: recover(this.api.context()),
      sync: recover(this.api.sync()),
      health: recover(this.api.health()),
    })
      .pipe(
        // Read the snapshot AFTER status: a concurrent commit must not let a
        // pre-publication board consume a post-publication status fingerprint.
        switchMap((result) =>
          recover(this.api.board()).pipe(map((board) => ({ ...result, board }))),
        ),
        finalize(() => {
          this.loading.set(false);
          if (!this.disposed) {
            if (this.pendingRefresh) {
              this.pendingRefresh = false;
              this.load();
            } else this.schedule();
          }
        }),
      )
      .subscribe((result) => {
        if (result.context) this.context.set(result.context);
        if (result.board) this.board.set(result.board);
        if (result.sync && statusVersion === this.statusVersion) {
          this.sync.set(result.sync);
          // A status observation is not an acknowledgement: only a successful
          // snapshot read ordered after it consumes this publication fingerprint.
          // On failure, unchanged future status polls must retry the full read.
          if (result.board) this.publication = this.publicationKey(result.sync);
        }
        if (result.health) this.health.set(result.health);
        this.error.set([...new Set(errors)].join(' '));
      });
  }
  refresh() {
    if (this.refreshing() || this.disposed) return;
    this.refreshing.set(true);
    this.write = this.api
      .refresh()
      .pipe(finalize(() => this.refreshing.set(false)))
      .subscribe({
        next: (status) => {
          // An older full read must not overwrite current health returned by POST.
          ++this.statusVersion;
          this.acceptStatus(status);
          clearTimeout(this.timer);
          this.schedule();
        },
        error: (e) => this.error.set(errorMessage(e)),
      });
  }
  reload() {
    if (this.loading()) this.pendingRefresh = true;
    else this.load();
  }
  private schedule() {
    clearTimeout(this.timer);
    if (this.disposed || this.loading()) return;
    const delay =
      this.sync()?.state === 'syncing'
        ? 2000
        : Math.min(
            30_000,
            Math.max(10_000, (this.context()?.refresh_interval_seconds ?? 30) * 1000),
          );
    this.timer = setTimeout(() => this.pollStatus(), delay);
  }
  private publicationKey(status: SyncStatus) {
    // A follow-up can begin between polls: a new job also signals publication of
    // its predecessor. Include per-source completion during worker finalization.
    return JSON.stringify([
      status.started_at,
      status.finished_at,
      status.last_success_at,
      status.progress
        .filter((p) => p.phase === 'completed')
        .map((p) => [p.source_id, p.finished_at]),
    ]);
  }
  private acceptStatus(status: SyncStatus) {
    const key = this.publicationKey(status);
    this.sync.set(status);
    if (key !== this.publication) {
      this.reload();
    }
  }
  private pollStatus() {
    if (this.disposed || this.loading()) return;
    const statusVersion = ++this.statusVersion;
    this.poll = this.api.sync().subscribe({
      next: (status) => {
        if (statusVersion === this.statusVersion) this.acceptStatus(status);
      },
      error: (e) => {
        this.error.set(errorMessage(e));
        this.schedule();
      },
      complete: () => {
        if (!this.loading()) this.schedule();
      },
    });
  }
}
