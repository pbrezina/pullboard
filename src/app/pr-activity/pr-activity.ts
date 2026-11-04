import { Component, input } from '@angular/core';
import { DatePipe } from '@angular/common';
import { UnifiedPullRequest } from '../core/models';
import { relativeAge } from '../core/pull-request-order';

@Component({
  imports: [DatePipe],
  selector: 'app-pr-activity',
  styleUrl: './pr-activity.scss',
  templateUrl: './pr-activity.html',
})
export class PrActivity {
  readonly pr = input.required<UnifiedPullRequest>();
  readonly asOf = input.required<number>();
  readonly waitingSince = input<string>();
  readonly requestTimeUnknown = input(false);
  protected readonly age = (date: string) => relativeAge(date, this.asOf());
  protected readonly timestamp = (date: string) => new Date(date).toLocaleString();
}
