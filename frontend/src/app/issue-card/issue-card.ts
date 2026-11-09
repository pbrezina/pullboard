import { Component, computed, inject, input, output } from '@angular/core';
import { CdkDropList, type CdkDragDrop } from '@angular/cdk/drag-drop';
import type { JiraIssue, PullRequest } from '../core/api-types';
import { BoardStore } from '../core/board-store';
import { Actions } from '../core/actions';
import { isConfiguredIssue } from '../core/board-logic';
import { AttentionDetail } from '../attention-detail/attention-detail';
import { ExternalLink } from '../ui/external-link/external-link';
import { RuntimeMode } from '../core/runtime-mode';
import { Notifications } from '../core/notifications';
import { isModeConflict } from '../core/api';

@Component({
  imports: [CdkDropList, AttentionDetail, ExternalLink],
  selector: 'app-issue-card',
  styleUrl: './issue-card.css',
  templateUrl: './issue-card.html',
})
export class IssueCard {
  readonly issue = input.required<JiraIssue>();
  readonly kind = computed(() => {
    const type = this.issue().issue_type.toLocaleLowerCase();
    if (type.includes('bug') || type.includes('defect')) return 'bug';
    if (type.includes('story')) return 'story';
    if (type.includes('epic')) return 'epic';
    return 'task';
  });
  readonly attentionMode = input(false);
  readonly linkRequested = output<JiraIssue>();
  readonly store = inject(BoardStore);
  readonly actions = inject(Actions);
  readonly mode = inject(RuntimeMode);
  private readonly notices = inject(Notifications);
  readonly canLink = computed(() =>
    isConfiguredIssue(this.issue(), this.store.context()?.jira_projects ?? []),
  );
  readonly source = computed(
    () =>
      this.store.sync()?.sources.find((s) => s.source_id === this.issue().source_id) ??
      this.store.board()?.sources.find((s) => s.source_id === this.issue().source_id),
  );
  readonly linked = computed(() =>
    this.issue().linked_pr_ids.map((id) => ({
      id,
      pr: this.store.board()?.pull_requests.find((p) => p.id === id),
    })),
  );
  drop(event: CdkDragDrop<JiraIssue>) {
    const mode = this.mode.dragMode;
    if (!this.mode.matches(mode)) {
      this.notices.show(
        'Runtime mode changed. Use Link PR to review the current mode and submit explicitly.',
        'error',
      );
      this.mode.refresh();
      return;
    }
    const pr = event.item.data as PullRequest;
    if (this.canLink() && pr && this.store.board()?.pull_requests.some((p) => p.id === pr.id))
      this.actions.link(this.issue().key, pr.id, mode!, {
        success: () => {},
        error: (e) => {
          if (isModeConflict(e)) this.mode.refresh();
        },
      });
  }
}
