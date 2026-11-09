import { Component, computed, input, model } from '@angular/core';
import { form, FormField } from '@angular/forms/signals';
import type { Board, Context } from '../core/api-types';
import { defaultFilters, repositoryFilterValue, type Filters } from '../core/board-logic';

@Component({
  imports: [FormField],
  selector: 'app-filter-bar',
  styleUrl: './filter-bar.css',
  templateUrl: './filter-bar.html',
})
export class FilterBar {
  readonly filters = model<Filters>(defaultFilters());
  readonly board = input.required<Board>();
  readonly context = input.required<Context>();
  readonly upstreamOnly = input(false);
  readonly fields = form(this.filters);
  readonly sourceMissing = computed(
    () =>
      this.filters().source !== 'all' &&
      !this.sources().some((s) => s.id === this.filters().source),
  );
  readonly repositoryMissing = computed(
    () =>
      this.filters().repository !== 'all' &&
      !this.repositories().some((r) => r.id === this.filters().repository),
  );
  readonly sprintGroups = computed(() =>
    ['active', 'future'].map((state) => ({
      label: state === 'active' ? 'Active' : 'Future',
      sprints: this.sprints().filter((s) => s.state === state),
    })),
  );
  readonly sprintMissing = computed(
    () =>
      !['', 'all', 'none'].includes(this.filters().sprint) &&
      !this.sprints().some((s) => s.id === this.filters().sprint),
  );
  readonly sprints = computed(() =>
    [
      ...new Map(
        [...this.board().sprints, ...this.context().current_sprints].map((s) => [s.id, s]),
      ).values(),
    ].sort((a, b) => a.name.localeCompare(b.name)),
  );
  readonly sources = computed(() => {
    const sources = new Map(
      this.context()
        .identities.filter((i) => i.kind !== 'jira')
        .map((i) => [i.source_id, { id: i.source_id, label: `${i.source_id} (${i.kind})` }]),
    );
    for (const s of this.board().sources.filter((s) => s.kind !== 'jira'))
      sources.set(s.source_id, { id: s.source_id, label: `${s.name} (${s.source_id})` });
    return [...sources.values()];
  });
  readonly repositories = computed(() =>
    [
      ...new Map(
        (this.upstreamOnly() ? this.board().upstream_issues : this.board().pull_requests).map(
          (pr) => [
            repositoryFilterValue(pr),
            { id: repositoryFilterValue(pr), label: `${pr.forge} · ${pr.owner}/${pr.repository}` },
          ],
        ),
      ).values(),
    ].sort((a, b) => a.label.localeCompare(b.label)),
  );
  selectSource(event: Event) {
    const source = (event.target as HTMLSelectElement).value;
    this.filters.update((filters) => ({ ...filters, source, repository: 'all' }));
  }
  selectRepository(event: Event) {
    const repository = (event.target as HTMLSelectElement).value;
    this.filters.update((filters) => ({ ...filters, source: 'all', repository }));
  }
}
