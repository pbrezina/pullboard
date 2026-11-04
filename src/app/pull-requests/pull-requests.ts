import { Component, computed, inject, OnInit, signal } from '@angular/core';
import { form, FormField } from '@angular/forms/signals';
import { MatButtonModule } from '@angular/material/button';
import { MatCardModule } from '@angular/material/card';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { ForgeAggregatorService } from '../core/forge-aggregator';
import { errorMessage } from '../core/errors';
import { ForgeIdentity, PullRequestWorkItem } from '../core/models';
import {
  isAccepted,
  actionFirst,
  isReviewExcluded,
  isAgedReview,
  waitingFirst,
  waitingSince,
} from '../core/pull-request-order';
import { PrBadges } from '../pr-badges/pr-badges';
import { PrJiraLinks } from '../core/pr-jira-links';
import { PrActivity } from '../pr-activity/pr-activity';
import { ReviewTicketActions } from '../review-ticket-actions/review-ticket-actions';
import { TEST_MODE } from '../core/test-mode';

@Component({
  imports: [
    MatButtonModule,
    MatCardModule,
    MatProgressBarModule,
    FormField,
    PrBadges,
    PrActivity,
    ReviewTicketActions,
  ],
  selector: 'app-pull-requests',
  styleUrls: ['../dashboard/dashboard.scss', './pull-requests.scss'],
  templateUrl: './pull-requests.html',
})
export class PullRequests implements OnInit {
  private readonly forges = inject(ForgeAggregatorService);
  protected readonly jiraLinks = inject(PrJiraLinks);
  protected readonly testMode = inject(TEST_MODE);
  protected readonly loading = signal(false);
  protected readonly error = signal('');
  protected readonly warnings = signal<string[]>([]);
  protected readonly identities = signal<ForgeIdentity[]>([]);
  protected readonly opened = signal<PullRequestWorkItem[]>([]);
  protected readonly requested = signal<PullRequestWorkItem[]>([]);
  protected readonly refreshedAt = signal<number | undefined>(undefined);
  protected readonly filters = signal({ requested: '', opened: '' });
  protected readonly filterFields = form(this.filters);
  protected readonly isAccepted = isAccepted;
  protected readonly waitingSince = waitingSince;
  protected readonly isAged = (pr: PullRequestWorkItem) =>
    isAgedReview(pr, this.refreshedAt() ?? Date.now());
  protected readonly visibleRequested = computed(() =>
    this.requested().filter((pr) => !isReviewExcluded(pr)),
  );
  protected readonly timestamp = (date: string | number) => new Date(date).toLocaleString();
  protected readonly columns = computed(() => [
    {
      id: 'requested' as const,
      title: 'Requested reviews from me',
      description:
        'Outstanding reviews and mentions first, then past reviews. Blocked and Deferred hidden.',
      items: this.filter(this.visibleRequested()).sort(waitingFirst),
      total: this.visibleRequested().length,
      empty: 'No reviews or mentions to show',
    },
    {
      id: 'opened' as const,
      title: 'My opened pull requests',
      description:
        'Your open PRs, including drafts. Needs your action first; oldest activity within each group.',
      items: this.filter(this.opened(), 'opened').sort(actionFirst),
      total: this.opened().length,
      empty: 'No open pull requests',
    },
  ]);

  ngOnInit(): void {
    void this.refresh();
  }

  private filter(
    items: PullRequestWorkItem[],
    column: 'requested' | 'opened' = 'requested',
  ): PullRequestWorkItem[] {
    const query = this.filters()[column].trim().toLowerCase();
    return items.filter((pr) =>
      `${pr.title} ${pr.repository} #${pr.number} ${pr.labels.join(' ')}`
        .toLowerCase()
        .includes(query),
    );
  }

  protected async refresh(): Promise<void> {
    if (this.loading()) return;
    this.loading.set(true);
    this.error.set('');
    this.warnings.set([]);
    try {
      const result = await this.forges.getPullRequestInbox();
      this.opened.set(result.opened);
      this.requested.set(result.requested);
      this.identities.set(result.identities);
      this.warnings.set(result.warnings);
      this.refreshedAt.set(Date.now());
    } catch (error) {
      this.error.set(
        `Could not load pull requests. ${errorMessage(error)} Previously loaded cards, if any, are stale. Check your local configuration and use Refresh board to retry.`,
      );
    } finally {
      this.loading.set(false);
    }
  }
}
