import { Component, computed, input } from '@angular/core';
import { UnifiedPullRequest } from '../core/models';
import { sortedLabels } from '../core/pull-request-order';

@Component({
  imports: [],
  selector: 'app-pr-badges',
  styleUrl: './pr-badges.scss',
  templateUrl: './pr-badges.html',
})
export class PrBadges {
  readonly pr = input.required<UnifiedPullRequest>();
  protected readonly labels = computed(() => sortedLabels(this.pr().labels));
}
