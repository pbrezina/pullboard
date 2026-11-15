import { NgTemplateOutlet } from '@angular/common';
import { Component, computed, input } from '@angular/core';
import type {
  ChoiceProgress,
  JiraOptionsStatus,
  SourceHealth as Source,
  SourcePhase,
  SourceProgress,
} from '../core/api-types';
import { Timestamp } from '../ui/timestamp/timestamp';

interface WorkRow extends ChoiceProgress {
  label: string;
  elapsed_ms: number | null;
}

@Component({
  imports: [NgTemplateOutlet, Timestamp],
  selector: 'app-source-health',
  host: { role: 'region', 'aria-label': 'Source health' },
  styleUrl: './source-health.css',
  templateUrl: './source-health.html',
})
export class SourceHealth {
  readonly sources = input.required<Source[]>();
  readonly progress = input<SourceProgress[]>([]);
  readonly syncing = input(false);
  readonly jiraOptions = input<JiraOptionsStatus | null>(null);
  readonly optionsFailed = computed(() => {
    const options = this.jiraOptions();
    return (
      !!options &&
      (['error', 'backoff'].includes(options.health.status) ||
        ['failed', 'backoff'].includes(options.progress.phase))
    );
  });
  readonly refreshing = computed(
    () => this.syncing() || this.progress().some((p) => this.active(p)),
  );
  readonly rows = computed(() =>
    this.sources().map((source) => {
      const progress = this.progress().find((p) => p.source_id === source.source_id);
      const options = source.kind === 'jira' ? this.jiraOptions() : null;
      const ticket: WorkRow | null = progress
        ? {
            ...progress,
            name: source.kind === 'jira' ? 'Tickets' : 'Items',
            label: source.name + ' evaluated items',
          }
        : null;
      const choices: WorkRow[] = options
        ? [
            ...options.progress.groups.map((p) => ({
              ...p,
              name: 'Group: ' + p.name,
              label: p.name + ' group membership',
              elapsed_ms: null,
            })),
            ...options.progress.projects.map((p) => ({
              ...p,
              name: p.name + ' checks',
              label: p.name + ' project eligibility checks',
              elapsed_ms: null,
            })),
          ]
        : [];
      const failed =
        source.status === 'error' ||
        source.status === 'backoff' ||
        progress?.phase === 'failed' ||
        progress?.phase === 'backoff';
      const phase =
        progress && (this.active(progress) || ['failed', 'backoff'].includes(progress.phase))
          ? progress.phase
          : source.status;
      return { source, progress, ticket, options, choices, failed, phase };
    }),
  );
  readonly degraded = computed(
    () => this.rows().filter((r) => r.source.status !== 'ok' || r.failed).length,
  );
  active(p: { phase: SourcePhase } | undefined) {
    return (
      !!p && ['queued', 'preparing', 'discovering', 'fetching', 'publishing'].includes(p.phase)
    );
  }
  duration(ms: number) {
    const seconds = Math.floor(ms / 1000);
    return seconds >= 60 ? `${Math.floor(seconds / 60)}m ${seconds % 60}s` : `${seconds}s`;
  }
  fraction(p: Pick<ChoiceProgress, 'completed_items' | 'total_items'>) {
    return p.total_items !== null && p.total_items > 0 ? p.completed_items : null;
  }
  count(p: Pick<ChoiceProgress, 'completed_items' | 'total_items'>) {
    return p.completed_items + ' / ' + (p.total_items ?? '?');
  }
  optionsPhase(options: JiraOptionsStatus) {
    if (this.active(options.progress)) return options.progress.phase;
    if (this.optionsFailed()) return options.ready ? 'stale' : 'unavailable';
    return options.ready ? 'ready' : 'unavailable';
  }
  formatTime(value: string) {
    return new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' }).format(
      new Date(value),
    );
  }
}
