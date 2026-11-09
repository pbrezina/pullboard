import { Component, computed, inject } from '@angular/core';
import { BoardStore } from './core/board-store';
import { Preferences } from './core/preferences';
import { Notifications } from './core/notifications';
import { BoardView } from './board-view/board-view';
import { IdentityBar } from './identity-bar/identity-bar';
import { SourceHealth } from './source-health/source-health';
import { Timestamp } from './ui/timestamp/timestamp';
import { ExternalLink } from './ui/external-link/external-link';
import { RuntimeMode } from './core/runtime-mode';
import { Dialog } from '@angular/cdk/dialog';
import { ModeConfirm } from './core/mode-confirm';

@Component({
  imports: [BoardView, IdentityBar, SourceHealth, Timestamp, ExternalLink],
  selector: 'app-root',
  styleUrl: './app.css',
  templateUrl: './app.html',
})
export class App {
  readonly store = inject(BoardStore);
  readonly preferences = inject(Preferences);
  readonly notices = inject(Notifications);
  readonly mode = inject(RuntimeMode);
  private readonly dialog = inject(Dialog);
  readonly sources = computed(
    () => this.store.sync()?.sources ?? this.store.board()?.sources ?? [],
  );
  readonly sourceFailure = computed(
    () =>
      this.sources().some((s) => s.status === 'error' || s.status === 'backoff') ||
      this.store.sync()?.progress.some((p) => p.phase === 'failed' || p.phase === 'backoff'),
  );
  constructor() {
    this.store.start();
    this.mode.start();
  }
  changeMode() {
    const mode = this.mode.value();
    if (!mode) return;
    this.dialog.open(ModeConfirm, {
      data: { mode, target: !mode.read_only_test_mode },
      ariaLabelledBy: 'mode-heading',
      restoreFocus: true,
    });
  }
}
