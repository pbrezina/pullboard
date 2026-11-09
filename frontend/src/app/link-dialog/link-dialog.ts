import { Component, computed, inject, signal } from '@angular/core';
import { DIALOG_DATA, DialogRef } from '@angular/cdk/dialog';
import { disabled, form, FormField } from '@angular/forms/signals';
import { Actions } from '../core/actions';
import { BoardStore } from '../core/board-store';
import { byTitle, isConfiguredIssue } from '../core/board-logic';
import type { RuntimeMode } from '../core/api-types';
import { ActionModeReview } from '../core/mode-confirm';
import { errorMessage, isModeConflict } from '../core/api';
export interface LinkDialogData {
  mode: RuntimeMode;
  issueKey?: string;
  prId?: string;
}

@Component({
  imports: [FormField],
  selector: 'app-link-dialog',
  styleUrl: './link-dialog.css',
  templateUrl: './link-dialog.html',
})
export class LinkDialog {
  readonly data = inject<LinkDialogData>(DIALOG_DATA);
  readonly review = new ActionModeReview(this.data.mode);
  readonly busy = signal(false);
  readonly error = signal('');
  readonly ref = inject<DialogRef>(DialogRef);
  readonly store = inject(BoardStore);
  private readonly actions = inject(Actions);
  readonly model = signal({ issue: this.data.issueKey ?? '', pr: this.data.prId ?? '', query: '' });
  readonly fields = form(this.model, (p) => disabled(p, { when: () => this.busy() }));
  readonly issues = computed(() =>
    [...(this.store.board()?.issues ?? [])]
      .filter((i) => isConfiguredIssue(i, this.store.context()?.jira_projects ?? []))
      .sort(byTitle)
      .filter(
        (i) =>
          i.title.toLocaleLowerCase().includes(this.model().query.toLocaleLowerCase()) ||
          i.key.toLocaleLowerCase().includes(this.model().query.toLocaleLowerCase()),
      ),
  );
  readonly prs = computed(() =>
    [...(this.store.board()?.pull_requests ?? [])]
      .sort(byTitle)
      .filter((p) => p.title.toLocaleLowerCase().includes(this.model().query.toLocaleLowerCase())),
  );
  readonly selectedTitle = computed(
    () =>
      this.store.board()?.pull_requests.find((p) => p.id === this.data.prId)?.title ??
      this.data.prId,
  );
  readonly canLink = computed(() => {
    const { issue, pr } = this.model();
    const board = this.store.board();
    return (
      !this.review.stale() &&
      !this.busy() &&
      !this.actions.linking().has(JSON.stringify([issue, pr])) &&
      !!board?.issues.some(
        (i) => i.key === issue && isConfiguredIssue(i, this.store.context()?.jira_projects ?? []),
      ) &&
      !!board?.pull_requests.some((p) => p.id === pr)
    );
  });
  link(event: Event) {
    event.preventDefault();
    const m = this.model();
    if (this.canLink()) {
      this.busy.set(true);
      this.ref.disableClose = true;
      this.actions.link(m.issue, m.pr, this.review.captured(), {
        success: () => this.ref.close(),
        error: (e) => {
          this.busy.set(false);
          this.ref.disableClose = false;
          this.error.set(errorMessage(e));
          if (isModeConflict(e)) this.review.invalidate();
        },
      });
    }
  }
}
