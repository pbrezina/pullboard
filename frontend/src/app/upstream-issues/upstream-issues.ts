import {
  Component,
  computed,
  DestroyRef,
  ElementRef,
  inject,
  signal,
  viewChild,
} from '@angular/core';
import { DOCUMENT } from '@angular/common';
import { Dialog } from '@angular/cdk/dialog';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { disabled, form, FormField, maxLength, submit, validate } from '@angular/forms/signals';
import { firstValueFrom } from 'rxjs';
import { Api, errorMessage } from '../core/api';
import type { RuntimeMode as Mode, UpstreamIssue } from '../core/api-types';
import { BoardStore } from '../core/board-store';
import { upstreamQueue, safeUrl } from '../core/board-logic';
import { Preferences } from '../core/preferences';
import { RuntimeMode } from '../core/runtime-mode';
import { FilterBar } from '../filter-bar/filter-bar';
import type { UpstreamTicketDialogData } from '../ticket-dialog/ticket-dialog';
import { Notifications } from '../core/notifications';
import { ExternalLink } from '../ui/external-link/external-link';

@Component({
  imports: [FormField, FilterBar, ExternalLink],
  selector: 'app-upstream-issues',
  styleUrl: './upstream-issues.css',
  templateUrl: './upstream-issues.html',
})
export class UpstreamIssues {
  readonly store = inject(BoardStore);
  readonly preferences = inject(Preferences);
  private readonly api = inject(Api);
  private readonly dialog = inject(Dialog);
  private readonly mode = inject(RuntimeMode);
  private readonly destroy = inject(DestroyRef);
  private readonly document = inject(DOCUMENT);
  private readonly notices = inject(Notifications);
  private openingTicket = false;
  private readonly urlInput = viewChild<ElementRef<HTMLInputElement>>('issueUrl');
  readonly filters = computed(() => this.preferences.value().upstream);
  readonly issues = computed(() =>
    upstreamQueue(this.store.board()?.upstream_issues ?? [], this.filters()),
  );
  readonly unavailableMode = computed(() => !this.mode.value() || !!this.mode.error());
  readonly resolving = signal(false);
  readonly error = signal('');
  readonly warning = signal<string | null>(null);
  // Transient form only: never put URLs, resolved content or action tokens in preferences.
  readonly model = signal({ url: '', source: '' });
  readonly fields = form(this.model, (p) => {
    validate(p.url, ({ value }) =>
      safeUrl(value().trim())
        ? undefined
        : {
            kind: 'url',
            message: 'Enter a complete HTTP(S) issue URL from a configured forge instance.',
          },
    );
    maxLength(p.url, 4096);
    disabled(p, { when: () => this.resolving() });
  });
  readonly sources = computed(() => {
    const sources = new Map(
      (this.store.context()?.identities ?? [])
        .filter((s) => s.kind !== 'jira')
        .map((s) => [s.source_id, { id: s.source_id, name: s.source_id }]),
    );
    for (const s of this.store.board()?.sources ?? [])
      if (s.kind !== 'jira') sources.set(s.source_id, { id: s.source_id, name: s.name });
    return [...sources.values()];
  });
  stale(source: string) {
    return (this.store.sync()?.sources ?? this.store.board()?.sources ?? []).some(
      (s) => s.source_id === source && (s.status === 'error' || s.status === 'backoff'),
    );
  }
  async clone(
    issue: UpstreamIssue,
    warning: string | null = null,
    captured?: Mode,
    restoreFocus: boolean | HTMLElement = true,
  ) {
    const mode = captured ?? this.mode.value();
    if (!mode || (!captured && this.mode.error()) || this.openingTicket) return;
    const origin = this.document.activeElement;
    const data: UpstreamTicketDialogData = {
      kind: 'upstream',
      issue,
      warning,
      mode,
      purpose: 'implementation',
    };
    this.openingTicket = true;
    try {
      const { TicketDialog } = await import('../ticket-dialog/ticket-dialog');
      if (this.destroy.destroyed || this.dialog.openDialogs.length) return;
      this.dialog.open(TicketDialog, {
        data,
        ariaLabelledBy: 'ticket-heading',
        autoFocus: 'first-tabbable',
        restoreFocus:
          restoreFocus === true && origin instanceof HTMLElement ? origin : restoreFocus,
      });
    } catch {
      if (!this.destroy.destroyed)
        this.notices.show(
          'The ticket editor could not load. Check the local server connection and try opening it again.',
          'error',
        );
    } finally {
      this.openingTicket = false;
    }
  }
  resolve(event: Event) {
    event.preventDefault();
    // Capture the pair at the gesture, BEFORE the asynchronous read. Never rebase on resolution.
    const captured = this.mode.value();
    if (!captured || this.unavailableMode() || this.resolving() || this.openingTicket) return;
    const origin = this.document.activeElement;
    this.error.set('');
    this.warning.set(null);
    void submit(this.fields, async () => {
      this.resolving.set(true);
      try {
        const { url, source } = this.model();
        const result = await firstValueFrom(
          this.api
            .resolveUpstreamIssue({
              source_id: source || null,
              url: url.trim(),
            })
            .pipe(takeUntilDestroyed(this.destroy)),
        );
        this.warning.set(result.warning);
        this.resolving.set(false);
        void this.clone(
          result.issue,
          result.warning,
          captured,
          origin instanceof HTMLElement ? origin : true,
        );
      } catch (e) {
        this.error.set(errorMessage(e));
      } finally {
        this.resolving.set(false);
      }
    });
    if (this.fields().invalid()) {
      this.error.set(
        'Enter a complete HTTP(S) issue URL, at most 4,096 characters, from a configured forge instance.',
      );
      this.urlInput()?.nativeElement.focus();
    }
  }
}
