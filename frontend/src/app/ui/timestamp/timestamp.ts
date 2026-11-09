import { Component, computed, inject, input } from '@angular/core';
import { Clock, elapsed } from '../../core/clock';

@Component({
  selector: 'pb-time',
  template: `<time [attr.datetime]="value()" [title]="exact()">{{ relative() }}</time>`,
})
export class Timestamp {
  readonly value = input<string | null>(null);
  private readonly clock = inject(Clock);
  readonly relative = computed(() => elapsed(this.value(), this.clock.now()));
  readonly exact = computed(() => {
    const value = this.value();
    return value && Number.isFinite(Date.parse(value))
      ? new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' }).format(
          new Date(value),
        )
      : 'No upstream timestamp available';
  });
}
