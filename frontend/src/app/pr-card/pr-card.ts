import { Component, computed, inject, input, output, signal } from '@angular/core';
import { CdkDrag, CdkDragHandle } from '@angular/cdk/drag-drop';
import type { PullRequest, Purpose } from '../core/api-types';
import { BoardStore } from '../core/board-store';
import { otherPeople, repositoryUrl } from '../core/board-logic';
import { AttentionDetail } from '../attention-detail/attention-detail';
import { ExternalLink } from '../ui/external-link/external-link';
import { Timestamp } from '../ui/timestamp/timestamp';
import { RuntimeMode } from '../core/runtime-mode';

@Component({
  imports: [CdkDrag, CdkDragHandle, AttentionDetail, ExternalLink, Timestamp],
  selector: 'app-pr-card',
  styleUrl: './pr-card.css',
  templateUrl: './pr-card.html',
})
export class PrCard {
  readonly mode = inject(RuntimeMode);
  prepareDrag() {
    this.mode.dragMode = this.mode.value();
  }
  beginDrag() {
    this.dragging.set(true);
  }
  endDrag() {
    this.dragging.set(false);
    // CDK emits ended before the target's dropped event. Keep the original
    // admission pair until all synchronous drop handlers have consumed it.
    const captured = this.mode.dragMode;
    queueMicrotask(() => {
      if (this.mode.dragMode === captured) this.mode.dragMode = null;
    });
  }
  readonly dragging = signal(false);
  readonly pr = input.required<PullRequest>();
  readonly attentionMode = input(false);
  readonly createRequested = output<{ pr: PullRequest; purpose: Purpose }>();
  readonly linkRequested = output<PullRequest>();
  readonly store = inject(BoardStore);
  readonly source = computed(
    () =>
      this.store.sync()?.sources.find((s) => s.source_id === this.pr().source_id) ??
      this.store.board()?.sources.find((s) => s.source_id === this.pr().source_id),
  );
  readonly reviewers = computed(() =>
    otherPeople(this.pr().reviewers, this.pr().source_id, this.store.context()?.identities ?? []),
  );
  readonly assignees = computed(() =>
    otherPeople(this.pr().assignees, this.pr().source_id, this.store.context()?.identities ?? []),
  );
  readonly repoUrl = computed(() => repositoryUrl(this.pr()));
  readonly linked = computed(() =>
    this.pr().linked_issue_keys.map((key) => ({
      key,
      issue: this.store.board()?.issues.find((i) => i.key === key),
    })),
  );
}
