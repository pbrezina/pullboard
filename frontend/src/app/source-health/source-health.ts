import { Component, computed, input } from '@angular/core';
import type { SourceHealth as Source, SourceProgress } from '../core/api-types';
import { Timestamp } from '../ui/timestamp/timestamp';

@Component({
  imports: [Timestamp],
  selector: 'app-source-health',
  host: { role: 'region', 'aria-label': 'Source health' },
  styleUrl: './source-health.css',
  templateUrl: './source-health.html',
})
export class SourceHealth {
  readonly sources = input.required<Source[]>();
  readonly progress = input<SourceProgress[]>([]);
  readonly syncing = input(false);
  readonly refreshing = computed(
    () => this.syncing() || this.progress().some((p) => this.active(p)),
  );
  readonly rows = computed(() =>
    this.sources().map((source) => ({
      source,
      progress: this.progress().find((p) => p.source_id === source.source_id),
    })),
  );
  readonly degraded = computed(
    () =>
      this.rows().filter(
        (r) =>
          r.source.status !== 'ok' ||
          r.progress?.phase === 'failed' ||
          r.progress?.phase === 'backoff',
      ).length,
  );
  readonly liveRows = computed(() =>
    this.rows().filter(
      (r) =>
        this.refreshing() ||
        r.source.status === 'error' ||
        r.source.status === 'backoff' ||
        r.progress?.phase === 'failed' ||
        r.progress?.phase === 'backoff',
    ),
  );
  active(p: SourceProgress) {
    return ['queued', 'preparing', 'discovering', 'fetching', 'publishing'].includes(p.phase);
  }
  duration(ms: number) {
    const seconds = Math.floor(ms / 1000);
    return seconds >= 60 ? `${Math.floor(seconds / 60)}m ${seconds % 60}s` : `${seconds}s`;
  }
  fraction(p: SourceProgress) {
    return p.total_items !== null && p.total_items > 0 ? p.completed_items : null;
  }
  formatTime(value: string) {
    return new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' }).format(
      new Date(value),
    );
  }
}
