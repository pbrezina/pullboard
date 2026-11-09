import { DestroyRef, inject, Service, signal } from '@angular/core';
import { finalize, Subscription } from 'rxjs';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { Api, errorMessage } from './api';
import type { RuntimeMode as Mode } from './api-types';

/** Authoritative pair only; never merge mode/revision from separate responses. */
@Service()
export class RuntimeMode {
  private readonly api = inject(Api);
  private readonly destroy = inject(DestroyRef);
  readonly value = signal<Mode | null>(null);
  readonly changing = signal(false);
  readonly error = signal('');
  private epoch = 0;
  private started = false;
  private read?: Subscription;
  private timer?: ReturnType<typeof setTimeout>;
  private disposed = false;
  dragMode: Mode | null = null;

  constructor() {
    this.destroy.onDestroy(() => {
      this.disposed = true;
      clearTimeout(this.timer);
      this.read?.unsubscribe();
    });
  }
  start() {
    if (this.started) return;
    this.started = true;
    this.refresh();
  }
  matches(mode: Mode | null) {
    return !!mode && mode.mode_revision === this.value()?.mode_revision && !this.error();
  }
  refresh() {
    if (this.disposed || this.changing()) return;
    clearTimeout(this.timer);
    this.read?.unsubscribe();
    const epoch = ++this.epoch;
    this.read = this.api.runtimeMode().subscribe({
      next: (mode) => {
        if (epoch !== this.epoch) return;
        this.value.set(mode);
        this.error.set('');
      },
      error: (e) => {
        if (epoch === this.epoch) this.error.set(errorMessage(e));
        this.schedule();
      },
      complete: () => this.schedule(),
    });
  }
  change(readOnly: boolean, captured: Mode) {
    // Invalidate all reads that predate our own toggle (including a failed toggle).
    ++this.epoch;
    this.read?.unsubscribe();
    clearTimeout(this.timer);
    this.changing.set(true);
    return this.api
      .changeRuntimeMode({
        read_only_test_mode: readOnly,
        expected_mode_revision: captured.mode_revision,
      })
      .pipe(
        takeUntilDestroyed(this.destroy),
        finalize(() => {
          this.changing.set(false);
          this.refresh();
        }),
      );
  }
  apply(mode: Mode) {
    ++this.epoch;
    this.value.set(mode);
    this.error.set('');
  }
  private schedule() {
    if (this.started && !this.disposed) this.timer = setTimeout(() => this.refresh(), 2000);
  }
}
