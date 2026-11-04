import { Component, computed, inject, input, OnInit } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { ReviewTickets, ReviewTicketState } from '../core/review-tickets';
import { UnifiedPullRequest } from '../core/models';

@Component({
  imports: [MatButtonModule],
  selector: 'app-review-ticket-actions',
  styles: `
    :host {
      display: block;
      margin-top: 12px;
    }
    .actions {
      display: flex;
      flex-wrap: wrap;
      gap: 8px;
      align-items: center;
    }
    p {
      font-size: 0.85rem;
      line-height: 1.5;
    }
    a {
      color: var(--mat-sys-primary);
    }
  `,
  template: `
    @if (reviews.storageWarning()) {
      <p role="alert">{{ reviews.storageWarning() }}</p>
    }
    @if (state()?.receipt; as receipt) {
      <p role="status">
        @if (receipt.simulated) {
          Test mode: review ticket creation simulated. No changes saved.
        } @else {
          Review ticket:
          <a [href]="receipt.url" target="_blank" rel="noopener noreferrer">{{ receipt.key }}</a>
        }
      </p>
    }
    @if (state()?.error) {
      <p role="alert">{{ state()?.error }}</p>
    }
    @if (state()?.status === 'partial') {
      <p>The ticket exists, but its Web Link or sprint assignment is incomplete.</p>
      <button mat-stroked-button (click)="reviews.resume(pr())" [disabled]="reviews.busy()">
        Finish review ticket
      </button>
    } @else if (state()?.status === 'uncertain') {
      <p role="alert">
        Creation outcome unknown. Check JIRA for an existing review ticket before retrying.
      </p>
      <button mat-stroked-button (click)="reviews.allowRetry(pr())" [disabled]="reviews.busy()">
        I checked; allow retry
      </button>
    } @else {
      <div class="actions">
        <button
          mat-stroked-button
          (click)="reviews.create(pr())"
          [disabled]="disabled() || reviews.busy()"
        >
          {{ working() ? 'Preparing review ticket…' : 'Create review ticket' }}
        </button>
        @if (state()?.status === 'error') {
          <button
            mat-button
            (click)="reviews.create(pr(), true)"
            [disabled]="disabled() || reviews.busy()"
          >
            Write review ticket manually
          </button>
        }
      </div>
      @if (working()) {
        <p role="status">
          {{
            state()?.status === 'suggesting'
              ? 'Preparing the confirmation dialog…'
              : 'Creating the ticket and adding its Web Link…'
          }}
        </p>
      }
    }
  `,
})
export class ReviewTicketActions implements OnInit {
  readonly pr = input.required<UnifiedPullRequest>();
  readonly disabled = input(false);
  protected readonly reviews = inject(ReviewTickets);
  protected readonly state = computed<ReviewTicketState | undefined>(
    () => this.reviews.states()[this.pr().url],
  );
  protected readonly working = computed(() =>
    ['suggesting', 'creating', 'linking'].includes(this.state()?.status ?? ''),
  );
  ngOnInit(): void {
    void this.reviews.initialize();
  }
}
