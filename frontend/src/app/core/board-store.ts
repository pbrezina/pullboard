import { computed, DestroyRef, inject, Service, signal } from '@angular/core';
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
  readonly jiraOptionsReady = computed(() => this.sync()?.jira_options?.ready === true);
  private started = false;
  private disposed = false;
  private timer?: ReturnType<typeof setTimeout>;
  private read?: Subscription;
  private write?: Subscription;
  private pendingRefresh = false;
  private poll?: Subscription;
  // Acknowledged status revision, not the possibly newer board response's revision:
  // Context may have been sampled before that newer board commit.
  private publication: number | null = null;
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
    this.read = recover(this.api.sync())
      .pipe(
        // BOTH dependent reads follow status; Context contains published identities,
        // projects and sprints too. Never acknowledge a commit using an older read.
        switchMap((sync) =>
          forkJoin({
            context: recover(this.api.context()),
            health: recover(this.api.health()),
          }).pipe(map((result) => ({ ...result, sync }))),
        ),
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
        // POST may have observed a quarantine/new commit while this read was pending.
        // Discard the whole older cycle, not just its status, and reconcile again.
        if (statusVersion !== this.statusVersion) {
          this.pendingRefresh = true;
          return;
        }
        if (result.sync) this.sync.set(result.sync);
        const current = this.board();
        // Revisions order one cache lifetime only. A fresh, serialized status read
        // below our acknowledged revision signals a reinitialized database; this
        // is not inferred from runtime-mode tokens or a late board response.
        const reinitialized =
          result.sync && this.publication !== null && result.sync.board_revision < this.publication;
        const usable =
          result.board &&
          (!current || reinitialized || result.board.revision >= current.revision) &&
          (!result.sync || result.board.revision >= result.sync.board_revision);
        if (usable && result.context && result.board) {
          this.context.set(result.context);
          this.board.set(result.board);
          // A status observation is not an acknowledgement: only a successful
          // snapshot AND context read ordered after it consumes this revision.
          // On failure, unchanged future status polls must retry the full read.
          if (result.sync && result.board.revision >= result.sync.board_revision)
            this.publication = result.sync.board_revision;
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
  private acceptStatus(status: SyncStatus) {
    this.sync.set(status);
    if (status.board_revision !== this.publication) {
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
