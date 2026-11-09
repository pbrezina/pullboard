import { Component, computed, input } from '@angular/core';
import type { Context } from '../core/api-types';
import { sprintViewUrl } from '../core/board-logic';
import { ExternalLink } from '../ui/external-link/external-link';

@Component({
  imports: [ExternalLink],
  selector: 'app-identity-bar',
  styleUrl: './identity-bar.css',
  templateUrl: './identity-bar.html',
})
export class IdentityBar {
  readonly context = input.required<Context>();
  readonly sprints = computed(() =>
    this.context().current_sprints.filter((s) => s.state === 'active' && s.default_eligible),
  );
  readonly sprintUrl = sprintViewUrl;
  endDate(value: string | null) {
    if (!value || !Number.isFinite(Date.parse(value))) return '';
    return new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' }).format(new Date(value));
  }
}
