import { TestBed } from '@angular/core/testing';
import {
  Preferences,
  parsePreferences,
  preferenceKey,
  preferencesFromUrl,
  validFilters,
} from './preferences';
import { defaultFilters } from './board-logic';
import { board, context, sprint, upstreamIssue } from '../testing/fixtures';

describe('Display preferences', () => {
  beforeEach(() => {
    localStorage.clear();
    history.replaceState(null, '', '/');
  });
  it('loads shareable, bounded URL display selections without form or identity data', () => {
    const p = preferencesFromUrl(
      parsePreferences(null),
      new URL(
        'https://localhost/?view=overview&attention.source=personal&overview.sprint=none&title=not-a-form',
      ),
    );
    expect(p.tab).toBe('overview');
    expect(p.attention.source).toBe('personal');
    expect(p.overview.sprint).toBe('none');
    expect(Object.keys(p)).toHaveLength(5);
  });
  it('recovers from malformed JSON, wrong types and oversized values', () => {
    expect(parsePreferences('{').tab).toBe('attention');
    expect(parsePreferences('null').theme).toBe('system');
    const result = parsePreferences(
      JSON.stringify({
        tab: 'bad',
        theme: 'evil',
        attention: { sprint: 5, jiraTitle: 'x'.repeat(501) },
      }),
    );
    expect(result.attention).toEqual(defaultFilters('attention'));
  });
  it('persists independent filters per tab, not upstream content', () => {
    const p = TestBed.inject(Preferences);
    p.setFilters('attention', { ...defaultFilters(), prTitle: 'review', source: 'personal' });
    p.setFilters('overview', { ...defaultFilters(), jiraTitle: 'assigned', sprint: 'none' });
    p.setTab('overview');
    p.setTheme('dark');
    TestBed.tick();
    const saved = parsePreferences(localStorage.getItem(preferenceKey));
    expect(saved.attention.prTitle).toBe('review');
    expect(saved.overview.jiraTitle).toBe('assigned');
    expect(saved.overview.sprint).toBe('none');
    expect(saved.theme).toBe('dark');
    expect(document.documentElement.dataset['theme']).toBe('dark');
    expect(Object.keys(saved).sort()).toEqual([
      'attention',
      'overview',
      'tab',
      'theme',
      'upstream',
    ]);
  });
  it('preserves unavailable actual sprint/source selections and text rather than broadening', () => {
    expect(
      validFilters(
        { ...defaultFilters(), source: 'deleted', sprint: 'deleted', prTitle: 'cache' },
        board(),
        context(),
      ),
    ).toEqual({ ...defaultFilters(), source: 'deleted', sprint: 'deleted', prTitle: 'cache' });
    expect(
      validFilters({ ...defaultFilters(), source: 'personal', sprint: 'none' }, board(), context())
        .source,
    ).toBe('personal');
  });
  it('validates repository selections against the current PR snapshot', () => {
    expect(
      validFilters(
        { ...defaultFilters(), repository: 'deleted\u001facme\u001fwidget' },
        board(),
        context(),
      ).repository,
    ).toBe('deleted\u001facme\u001fwidget');
    expect(
      validFilters(
        { ...defaultFilters(), repository: 'work\u001facme\u001fwidget' },
        board(),
        context(),
      ).repository,
    ).toBe('work\u001facme\u001fwidget');
  });
  it('resets only the requested tab', () => {
    const p = TestBed.inject(Preferences);
    p.setFilters('attention', { ...defaultFilters(), prTitle: 'wait' });
    p.setFilters('overview', { ...defaultFilters(), prTitle: 'own' });
    p.reset('attention');
    expect(p.value().attention).toEqual(defaultFilters('attention'));
    expect(p.value().overview.prTitle).toBe('own');
  });
  it('restores bounded upstream display filters independently from URL/storage and ignores action data', () => {
    const stored = parsePreferences(
      JSON.stringify({
        tab: 'upstream',
        upstream: {
          prTitle: 'copy',
          source: 'work',
          url: 'private-url',
          mode_revision: 'private-mode',
          issue: { title: 'private-content' },
        },
      }),
    );
    const p = preferencesFromUrl(
      stored,
      new URL(
        'https://localhost/?view=upstream&upstream.prTitle=bug&overview.prTitle=PR&upstream.url=secret&upstream.mode_revision=secret',
      ),
    );
    expect(p.tab).toBe('upstream');
    expect(p.upstream.prTitle).toBe('bug');
    expect(p.overview.prTitle).toBe('PR');
    expect(JSON.stringify(p)).not.toContain('secret');
    expect(JSON.stringify(p)).not.toContain('private-');
  });
  it('validates upstream repositories against issues, not PRs, and resets only upstream preferences', () => {
    const snapshot = board({ upstream_issues: [upstreamIssue({ repository: 'issue-only' })] });
    const filters = { ...defaultFilters(), repository: 'work\u001facme\u001fissue-only' };
    expect(validFilters(filters, snapshot, context(), 'upstream').repository).toBe(
      filters.repository,
    );
    expect(validFilters(filters, snapshot, context(), 'overview').repository).toBe(
      filters.repository,
    );
    const p = TestBed.inject(Preferences);
    p.setFilters('overview', { ...defaultFilters(), prTitle: 'keep PR filter' });
    p.setFilters('upstream', { ...filters, prTitle: 'issue' });
    p.reset('upstream');
    expect(p.value().upstream).toEqual(defaultFilters('upstream'));
    expect(p.value().overview.prTitle).toBe('keep PR filter');
  });
  it('handles unavailable browser storage without breaking in-memory preferences', () => {
    const spy = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('blocked');
    });
    const p = TestBed.inject(Preferences);
    p.setTheme('light');
    expect(() => TestBed.tick()).not.toThrow();
    expect(p.value().theme).toBe('light');
    spy.mockRestore();
  });
  it('migrates active sentinel and initializes Overview once to the sole actual current ID', () => {
    localStorage.setItem(
      preferenceKey,
      JSON.stringify({ overview: { sprint: 'active', jiraTitle: 'cache' } }),
    );
    const p = TestBed.inject(Preferences);
    p.validate(board({ sprints: [] }), context({ current_project: null, current_sprints: [] }));
    expect(p.value().overview.sprint).toBe('');
    p.validate(board(), context());
    expect(p.value().overview).toMatchObject({ sprint: sprint.id, jiraTitle: 'cache' });
    p.validate(board(), context({ current_sprints: [{ ...sprint, id: 'rollover' }] }));
    expect(p.value().overview.sprint).toBe(sprint.id);
    p.setFilters('overview', { ...p.value().overview, sprint: 'none' });
    p.validate(board(), context());
    expect(p.value().overview.sprint).toBe('none');
  });
  it.each([0, 2])(
    'leaves %s matching current sprints explicitly unselected until deliberate choice/reset',
    (count) => {
      const p = TestBed.inject(Preferences);
      const c = context({
        current_sprints: Array.from({ length: count }, (_, index) => ({
          ...sprint,
          id: String(index),
        })),
      });
      p.validate(board(), c);
      expect(p.value().overview.sprint).toBe('');
      p.setFilters('overview', {
        jiraTitle: 'cache',
        prTitle: 'work',
        sprint: 'all',
        source: 'work',
        repository: 'saved',
      });
      p.validate(board(), c);
      expect(p.value().overview.sprint).toBe('all');
      p.reset('overview');
      expect(p.value().overview).toEqual(defaultFilters('overview'));
      p.validate(board(), context());
      p.reset('overview');
      expect(p.value().overview.sprint).toBe(sprint.id);
    },
  );
  it('preserves saved actual IDs, forge/repository and text while discovery is pending and on refresh', () => {
    const filters = {
      jiraTitle: 'Saved title',
      prTitle: 'PR',
      sprint: 'saved-sprint',
      source: 'saved-forge',
      repository: 'saved-repo',
    };
    localStorage.setItem(preferenceKey, JSON.stringify({ overview: filters }));
    const p = TestBed.inject(Preferences);
    p.validate(
      board({ sprints: [], sources: [], pull_requests: [] }),
      context({ current_project: null, current_sprints: [], identities: [] }),
    );
    p.validate(board(), context());
    expect(p.value().overview).toEqual(filters);
    p.reset('overview');
    expect(p.value().overview).toEqual({ ...defaultFilters('overview'), sprint: sprint.id });
  });
});
