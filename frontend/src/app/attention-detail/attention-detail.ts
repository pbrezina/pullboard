import { Component, computed, input } from '@angular/core';
import type { Attention, AttentionReason } from '../core/api-types';
import { ExternalLink } from '../ui/external-link/external-link';
import { Timestamp } from '../ui/timestamp/timestamp';
export const reasonLabels: Record<AttentionReason, string> = {
  jira_mention: 'You were mentioned',
  review_request: 'Review requested',
  pr_mention: 'You were mentioned',
  author_feedback: 'Feedback on your PR',
};

@Component({
  imports: [ExternalLink, Timestamp],
  selector: 'app-attention-detail',
  styleUrl: './attention-detail.css',
  templateUrl: './attention-detail.html',
})
export class AttentionDetail {
  readonly attention = input.required<Attention>();
  readonly showWaiting = input(true);
  readonly reasonLinks = computed(() => {
    const threads = [...this.attention().threads].sort(
      (a, b) => Date.parse(a.waiting_since) - Date.parse(b.waiting_since),
    );
    return this.attention().reasons.map((reason) => ({
      reason,
      url: threads.find((thread) => thread.reason === reason)?.url,
    }));
  });
  readonly labels = reasonLabels;
}
