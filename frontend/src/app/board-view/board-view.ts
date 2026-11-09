import { Component, computed, DestroyRef, effect, inject, untracked } from '@angular/core';
import { DOCUMENT } from '@angular/common';
import { CdkDropList, CdkDropListGroup } from '@angular/cdk/drag-drop';
import { Dialog } from '@angular/cdk/dialog';
import { BoardStore } from '../core/board-store';
import { Preferences } from '../core/preferences';
import { defaultFilters, queues } from '../core/board-logic';
import type { Filters } from '../core/board-logic';
import type { JiraIssue, PullRequest, Purpose } from '../core/api-types';
import { IssueCard } from '../issue-card/issue-card';
import { PrCard } from '../pr-card/pr-card';
import { FilterBar } from '../filter-bar/filter-bar';
import type { PrTicketDialogData } from '../ticket-dialog/ticket-dialog';
import { Notifications } from '../core/notifications';
import { LinkDialog } from '../link-dialog/link-dialog';
import { RuntimeMode } from '../core/runtime-mode';
import { UpstreamIssues } from '../upstream-issues/upstream-issues';

@Component({
  imports: [CdkDropList, CdkDropListGroup, IssueCard, PrCard, FilterBar, UpstreamIssues],
  selector: 'app-board-view',
  styleUrl: './board-view.css',
  templateUrl: './board-view.html',
})
export class BoardView {
  readonly store = inject(BoardStore);
  readonly preferences = inject(Preferences);
  private readonly dialog = inject(Dialog);
  private readonly mode = inject(RuntimeMode);
  private readonly destroy = inject(DestroyRef);
  private readonly document = inject(DOCUMENT);
  private readonly notices = inject(Notifications);
  private openingTicket = false;
  readonly tab = computed(() => this.preferences.value().tab);
  readonly filters = computed(() => this.preferences.value()[this.tab()]);
  readonly data = computed(() => {
    const b = this.store.board();
    const tab = this.tab();
    return b && tab !== 'upstream'
      ? queues(b, tab, this.filters(), this.store.context()?.jira_projects ?? [])
      : null;
  });
  readonly unfilteredAttention = computed(() => {
    const b = this.store.board();
    return b
      ? queues(b, 'attention', defaultFilters(), this.store.context()?.jira_projects ?? []).total
      : 0;
  });
  readonly reviewTotal = computed(() => this.data()?.prs.filter((p) => !p.is_mine).length ?? 0);
  readonly ownTotal = computed(() => this.data()?.prs.filter((p) => p.is_mine).length ?? 0);
  readonly noEnter = () => false;
  constructor() {
    effect(() => {
      const board = this.store.board(),
        context = this.store.context();
      if (board && context) untracked(() => this.preferences.validate(board, context));
    });
  }
  changeFilters(filters: Filters) {
    this.preferences.setFilters(this.tab(), filters);
  }
  showEverything() {
    this.changeFilters({ ...defaultFilters(), sprint: 'all' });
  }
  async ticket(pr: PullRequest, purpose: Purpose) {
    const mode = this.mode.value();
    if (!mode || this.mode.error() || this.openingTicket) return;
    // Freeze gesture intent and focus before loading code; never rebase a delayed action.
    const data: PrTicketDialogData = {
      kind: 'pr',
      pr,
      mode,
      purpose,
    };
    const origin = this.document.activeElement;
    this.openingTicket = true;
    try {
      const { TicketDialog } = await import('../ticket-dialog/ticket-dialog');
      if (this.destroy.destroyed || this.dialog.openDialogs.length) return;
      this.dialog.open(TicketDialog, {
        data,
        ariaLabelledBy: 'ticket-heading',
        autoFocus: 'first-tabbable',
        restoreFocus: origin instanceof HTMLElement ? origin : true,
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
  linkIssue(issue: JiraIssue) {
    const mode = this.mode.value();
    if (!mode || this.mode.error()) return;
    this.dialog.open(LinkDialog, {
      data: { issueKey: issue.key, mode },
      ariaLabelledBy: 'link-heading',
      restoreFocus: true,
    });
  }
  linkPr(pr: PullRequest) {
    const mode = this.mode.value();
    if (!mode || this.mode.error()) return;
    this.dialog.open(LinkDialog, {
      data: { prId: pr.id, mode },
      ariaLabelledBy: 'link-heading',
      restoreFocus: true,
    });
  }
}
