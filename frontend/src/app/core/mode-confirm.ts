import { Component, computed, DestroyRef, inject, signal } from '@angular/core';
import { DIALOG_DATA, Dialog, DialogRef } from '@angular/cdk/dialog';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import type { RuntimeMode as Mode } from './api-types';
import { RuntimeMode } from './runtime-mode';
import { errorMessage } from './api';

interface ConfirmData {
  mode: Mode;
  target?: boolean;
}

@Component({
  selector: 'app-mode-confirm',
  template: ` <section class="dialog-shell link-dialog">
    <header class="dialog-header">
      <h2 id="mode-heading">
        {{
          data.target === undefined
            ? 'Review action mode'
            : data.target
              ? 'Switch to Test mode?'
              : 'Enable Live Jira writes?'
        }}
      </h2>
    </header>
    <div class="dialog-body">
      <p>
        {{
          readOnly
            ? 'Test mode: Jira submissions are simulated with zero upstream writes.'
            : 'Live mode: explicit submissions can create tickets and links in Jira.'
        }}
      </p>
      <p>
        This mode is application-wide, including other tabs. Admitted live actions may finish after
        switching to Test. Forge adapters are always read-only.
      </p>
      @if (stale()) {
        <p class="warning-banner" role="status">
          Mode changed or cannot be verified. Cancel and review the current mode again. Nothing was
          retried.
        </p>
      }
      @if (error()) {
        <p class="error-banner" role="alert">{{ error() }}</p>
      }
    </div>
    <footer class="dialog-footer">
      <button (click)="ref.close()" [disabled]="busy()">Cancel</button
      ><button class="primary-button" [disabled]="stale() || busy()" (click)="confirm()">
        {{
          busy()
            ? 'Changing…'
            : data.target === undefined
              ? 'Use reviewed mode'
              : data.target
                ? 'Switch to Test'
                : 'Enable Live writes'
        }}
      </button>
    </footer>
  </section>`,
})
export class ModeConfirm {
  readonly data = inject<ConfirmData>(DIALOG_DATA);
  readonly ref = inject<DialogRef<Mode>>(DialogRef);
  readonly mode = inject(RuntimeMode);
  readonly readOnly = this.data.target ?? this.data.mode.read_only_test_mode;
  readonly stale = computed(() => !this.mode.matches(this.data.mode) || this.conflict());
  readonly busy = signal(false);
  readonly error = signal('');
  readonly conflict = signal(false);
  confirm() {
    if (this.stale() || this.busy()) return;
    if (this.data.target === undefined) {
      this.ref.close(this.data.mode);
      return;
    }
    this.busy.set(true);
    this.ref.disableClose = true;
    this.mode.change(this.data.target, this.data.mode).subscribe({
      next: (mode) => {
        this.mode.apply(mode);
        this.ref.close(mode);
      },
      error: (e) => {
        this.error.set(errorMessage(e));
        this.conflict.set(true);
        this.busy.set(false);
        this.ref.disableClose = false;
      },
    });
  }
}

/** Per-dialog frozen pair. Only explicit nested review may replace it; edits stay intact. */
export class ActionModeReview {
  readonly runtime = inject(RuntimeMode);
  private readonly dialog = inject(Dialog);
  private readonly destroy = inject(DestroyRef);
  readonly captured;
  readonly conflict = signal(false);
  readonly stale = computed(() => this.conflict() || !this.runtime.matches(this.captured()));
  constructor(mode: Mode) {
    this.captured = signal(mode);
  }
  invalidate() {
    this.conflict.set(true);
    this.runtime.refresh();
  }
  review() {
    const mode = this.runtime.value();
    if (!mode) {
      this.runtime.refresh();
      return;
    }
    this.dialog
      .open<Mode>(ModeConfirm, {
        data: { mode },
        ariaLabelledBy: 'mode-heading',
        restoreFocus: true,
      })
      .closed.pipe(takeUntilDestroyed(this.destroy))
      .subscribe((confirmed) => {
        if (confirmed && this.runtime.matches(confirmed)) {
          this.captured.set(confirmed);
          this.conflict.set(false);
        }
      });
  }
}
