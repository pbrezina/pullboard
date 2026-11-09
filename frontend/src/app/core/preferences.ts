import { DestroyRef, effect, inject, Service, signal } from '@angular/core';
import { DOCUMENT } from '@angular/common';
import { defaultFilters, type Filters, type Tab } from './board-logic';
import type { Board, Context } from './api-types';

export type Theme = 'light' | 'dark' | 'system';
export interface DisplayPreferences {
  tab: Tab;
  theme: Theme;
  attention: Filters;
  overview: Filters;
  upstream: Filters;
}
export const preferenceKey = 'pullboard.display.v1';
const blank = (): DisplayPreferences => ({
  tab: 'attention',
  theme: 'system',
  attention: defaultFilters('attention'),
  overview: defaultFilters('overview'),
  upstream: defaultFilters('upstream'),
});
export function parsePreferences(raw: string | null): DisplayPreferences {
  const result = blank();
  try {
    const data: unknown = JSON.parse(raw ?? '{}');
    if (!data || typeof data !== 'object') return result;
    const item = data as Record<string, unknown>;
    if (item['tab'] === 'overview' || item['tab'] === 'upstream') result.tab = item['tab'];
    if (item['theme'] === 'light' || item['theme'] === 'dark') result.theme = item['theme'];
    for (const tab of ['attention', 'overview', 'upstream'] as const) {
      const f = item[tab];
      if (f && typeof f === 'object')
        for (const key of ['sprint', 'source', 'repository', 'jiraTitle', 'prTitle'] as const) {
          const value = (f as Record<string, unknown>)[key];
          if (typeof value === 'string' && value.length <= 500)
            result[tab][key] =
              key === 'sprint' && value === 'active' ? defaultFilters(tab).sprint : value;
        }
    }
  } catch {
    /* Invalid preferences revert to safe defaults. */
  }
  return result;
}
export function validFilters(
  filters: Filters,
  board: Board,
  context: Context,
  tab: Tab = 'overview',
): Filters {
  // Missing snapshot entries may be pending discovery, stale, or a sprint rollover.
  // Never silently broaden a deliberate selection to all work.
  return {
    ...filters,
    sprint:
      filters.sprint === 'active'
        ? tab === 'overview' && context.current_sprints.length === 1
          ? context.current_sprints[0].id
          : defaultFilters(tab).sprint
        : filters.sprint,
  };
}
/** Shareable display state; no identity, content or write-form data enters the URL. */
export function preferencesFromUrl(value: DisplayPreferences, url: URL): DisplayPreferences {
  const next = structuredClone(value);
  const tab = url.searchParams.get('view');
  if (tab === 'attention' || tab === 'overview' || tab === 'upstream') next.tab = tab;
  for (const view of ['attention', 'overview', 'upstream'] as const)
    for (const key of ['sprint', 'source', 'repository', 'jiraTitle', 'prTitle'] as const) {
      const item = url.searchParams.get(`${view}.${key}`);
      if (item !== null && item.length <= 500)
        next[view][key] =
          key === 'sprint' && item === 'active' ? defaultFilters(view).sprint : item;
    }
  return next;
}

@Service()
export class Preferences {
  private readonly document = inject(DOCUMENT);
  readonly value = signal<DisplayPreferences>(this.read());
  private overviewInitialized = !!this.value().overview.sprint;
  private currentContext: Context | null = null;
  private readonly media = this.document.defaultView?.matchMedia?.('(prefers-color-scheme: dark)');
  readonly systemDark = signal(this.media?.matches ?? false);
  constructor() {
    const listener = (event: MediaQueryListEvent) => this.systemDark.set(event.matches);
    this.media?.addEventListener('change', listener);
    inject(DestroyRef).onDestroy(() => this.media?.removeEventListener('change', listener));
    effect(() => {
      const p = this.value();
      try {
        this.document.defaultView?.localStorage.setItem(preferenceKey, JSON.stringify(p));
      } catch {
        /* Display preferences still work in memory. */
      }
      const dark = p.theme === 'dark' || (p.theme === 'system' && this.systemDark());
      this.document.documentElement.dataset['theme'] = dark ? 'dark' : 'light';
      this.document
        .querySelector('meta[name="theme-color"]')
        ?.setAttribute('content', dark ? '#151e2c' : '#f2f5f9');
      const window = this.document.defaultView;
      if (window) {
        const url = new URL(window.location.href);
        if (p.tab === 'attention') url.searchParams.delete('view');
        else url.searchParams.set('view', p.tab);
        for (const view of ['attention', 'overview', 'upstream'] as const)
          for (const key of ['sprint', 'source', 'repository', 'jiraTitle', 'prTitle'] as const) {
            const name = `${view}.${key}`;
            if (p[view][key] === defaultFilters(view)[key]) url.searchParams.delete(name);
            else url.searchParams.set(name, p[view][key]);
          }
        if (url.href !== window.location.href) window.history.replaceState(null, '', url);
      }
    });
  }
  setTab(tab: Tab) {
    this.value.update((p) => ({ ...p, tab }));
  }
  setTheme(theme: Theme) {
    this.value.update((p) => ({ ...p, theme }));
  }
  setFilters(tab: Tab, filters: Filters) {
    if (tab === 'overview' && filters.sprint !== this.value().overview.sprint)
      this.overviewInitialized = true;
    this.value.update((p) => ({ ...p, [tab]: filters }));
  }
  reset(tab: Tab) {
    const filters = defaultFilters(tab);
    if (tab === 'overview') {
      const sprints = this.currentContext?.current_sprints ?? [];
      filters.sprint = sprints.length === 1 ? sprints[0].id : '';
      this.overviewInitialized = !!this.currentContext?.current_project;
    }
    this.value.update((p) => ({ ...p, [tab]: filters }));
  }
  validate(board: Board, context: Context) {
    this.currentContext = context;
    const p = this.value();
    const next = {
      ...p,
      attention: validFilters(p.attention, board, context, 'attention'),
      overview: validFilters(p.overview, board, context, 'overview'),
      upstream: validFilters(p.upstream, board, context, 'upstream'),
    };
    const jira = board.sources.find((s) => s.source_id === 'jira');
    if (!this.overviewInitialized && context.current_project && (!jira || jira.last_success_at)) {
      next.overview.sprint =
        context.current_sprints.length === 1 ? context.current_sprints[0].id : '';
      this.overviewInitialized = true;
    }
    if (JSON.stringify(p) !== JSON.stringify(next)) this.value.set(next);
  }
  private read() {
    let value = blank();
    const window = this.document.defaultView;
    try {
      value = parsePreferences(window?.localStorage.getItem(preferenceKey) ?? null);
    } catch {
      /* Blocked storage uses defaults. */
    }
    return window ? preferencesFromUrl(value, new URL(window.location.href)) : value;
  }
}
