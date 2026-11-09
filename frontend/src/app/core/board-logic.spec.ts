import {
  board,
  issue,
  foreignMention,
  pr,
  sprint,
  attention,
  context,
  person,
} from '../testing/fixtures';
import {
  byTitle,
  defaultFilters,
  matchesSprint,
  otherPeople,
  queues,
  repositoryUrl,
  repositoryFilterValue,
  safeUrl,
  sprintViewUrl,
} from './board-logic';

describe('Board selection and ordering', () => {
  it('sorts attention by oldest upstream waiting timestamp, not commit or update', () => {
    const b = board({
      pull_requests: [
        pr({
          id: 'new',
          attention: attention('2026-11-04T00:00:00Z'),
          last_commit_at: '2020-01-01T00:00:00Z',
        }),
        pr({ id: 'old', attention: attention('2026-10-01T00:00:00Z') }),
      ],
    });
    expect(
      queues(b, 'attention', defaultFilters(), context().jira_projects).own.map((p) => p.id),
    ).toEqual(['old', 'new']);
  });
  it('excludes mention-only unassigned tickets from Overview but not attention', () => {
    const b = board({ issues: [issue({ is_assigned_to_me: false })] });
    expect(queues(b, 'overview', defaultFilters(), context().jira_projects).issues).toHaveLength(0);
    expect(
      queues(b, 'attention', defaultFilters('attention'), context().jira_projects).issues,
    ).toHaveLength(1);
  });
  it('includes foreign mentions for other assignees and self in attention only, with accurate counts', () => {
    const b = board({
      issues: [issue(), foreignMention(), foreignMention(true)],
    });
    const projects = context().jira_projects;
    const attentionQueue = queues(b, 'attention', defaultFilters('attention'), projects);
    expect(attentionQueue.issues.map((i) => i.key)).toEqual(['FOREIGN-1', 'FOREIGN-2', 'DEV-1']);
    expect(attentionQueue.total).toBe(4);
    expect(attentionQueue.shown).toBe(4);
    const filtered = queues(
      b,
      'attention',
      { ...defaultFilters('attention'), jiraTitle: 'FOREIGN MENTION ASSIGNED TO PAT' },
      projects,
    );
    expect(filtered.issues.map((i) => i.key)).toEqual(['FOREIGN-1']);
    expect(filtered.total).toBe(4);
    expect(filtered.shown).toBe(2);
    for (const sprint of ['active', 'all', 'none']) {
      const overview = queues(b, 'overview', { ...defaultFilters(), sprint }, projects);
      expect(overview.jira.map((i) => i.key)).toEqual(['DEV-1']);
      expect(overview.total).toBe(2);
      expect(overview.issues.every((i) => i.project_key === 'DEV')).toBe(true);
    }
  });
  it('does not turn handled foreign mention candidates into attention or assigned Overview work', () => {
    const b = board({ issues: [{ ...foreignMention(true), attention: null }], pull_requests: [] });
    expect(queues(b, 'attention', defaultFilters('attention'), context().jira_projects).total).toBe(
      0,
    );
    expect(queues(b, 'overview', defaultFilters(), context().jira_projects).total).toBe(0);
  });
  it.each(['Done', 'Resolved'])('retains configured assigned issues with %s status', (status) => {
    const b = board({ issues: [issue({ status })], pull_requests: [] });
    for (const tab of ['attention', 'overview'] as const) {
      const result = queues(
        b,
        tab,
        { ...defaultFilters(tab), sprint: 'all' },
        context().jira_projects,
      );
      expect(result.issues.map((i) => i.key)).toEqual(['DEV-1']);
      expect(result.total).toBe(1);
    }
  });
  it('requires configured project inventory for Overview but not attention', () => {
    expect(queues(board(), 'overview', defaultFilters(), []).jira).toEqual([]);
    expect(queues(board(), 'attention', defaultFilters('attention'), []).issues).toHaveLength(1);
  });
  it('only includes authored open PRs in Overview, and splits attention by ownership', () => {
    const b = board({
      pull_requests: [
        pr(),
        pr({ id: 'other', is_mine: false }),
        pr({ id: 'quiet', attention: null }),
      ],
    });
    expect(queues(b, 'overview', defaultFilters(), context().jira_projects).own).toHaveLength(2);
    expect(queues(b, 'attention', defaultFilters(), context().jira_projects).reviews).toHaveLength(
      1,
    );
    expect(queues(b, 'attention', defaultFilters(), context().jira_projects).own).toHaveLength(1);
  });
  it('keeps forge instances separate even for the same forge kind', () => {
    const b = board({ pull_requests: [pr(), pr({ id: 'personal:42', source_id: 'personal' })] });
    const result = queues(
      b,
      'attention',
      { ...defaultFilters('attention'), source: 'personal' },
      context().jira_projects,
    );
    expect(result.own.map((p) => p.source_id)).toEqual(['personal']);
    expect(result.total).toBe(3);
    expect(result.shown).toBe(2);
  });
  it('filters PRs by an unambiguous forge repository identity', () => {
    const github = pr({ id: 'github', source_id: 'github', owner: 'acme', repository: 'widget' });
    const gitlab = pr({
      id: 'gitlab',
      source_id: 'gitlab',
      forge: 'gitlab',
      owner: 'acme',
      repository: 'widget',
    });
    const result = queues(
      board({ pull_requests: [github, gitlab] }),
      'attention',
      { ...defaultFilters('attention'), repository: repositoryFilterValue(gitlab) },
      context().jira_projects,
    );
    expect(result.own.map((p) => p.id)).toEqual(['gitlab']);
  });
  it('matches titles case insensitively with independent Jira and PR queries', () => {
    const result = queues(
      board(),
      'attention',
      { ...defaultFilters('attention'), jiraTitle: 'CACHE INVALID', prTitle: ' SNAPSHOT ' },
      context().jira_projects,
    );
    expect(result.shown).toBe(2);
    expect(
      queues(
        board(),
        'attention',
        { ...defaultFilters(), prTitle: 'missing' },
        context().jira_projects,
      ).total,
    ).toBe(2);
  });
  it('requires an actual sprint selection and never unions eligible active sprints', () => {
    const s2 = { ...sprint, id: 's2', team: 'Second team' };
    expect(matchesSprint(issue({ sprint_ids: ['s2'] }), '', [sprint, s2])).toBe(false);
    expect(matchesSprint(issue(), 'active', [])).toBe(false);
    const foreign = { ...sprint, id: 'foreign', team: null, default_eligible: false };
    expect(matchesSprint(issue({ sprint_ids: ['foreign'] }), 'active', [foreign])).toBe(false);
    expect(matchesSprint(issue({ sprint_ids: ['foreign'] }), 'foreign', [foreign])).toBe(true);
    const b = board({
      sprints: [foreign],
      issues: [issue({ sprint_ids: ['foreign'] }), issue({ id: 'no-sprint', sprint_ids: [] })],
    });
    expect(
      queues(b, 'overview', defaultFilters('overview'), context().jira_projects).issues,
    ).toHaveLength(0);
    expect(
      queues(b, 'attention', defaultFilters('attention'), context().jira_projects).issues,
    ).toHaveLength(2);
  });
  it('allows future, no sprint, and all selectors without inventing IDs', () => {
    const future = { ...sprint, id: 'future', state: 'future' as const };
    expect(matchesSprint(issue({ sprint_ids: ['future'] }), 'active', [future])).toBe(false);
    expect(matchesSprint(issue({ sprint_ids: ['future'] }), 'future', [future])).toBe(true);
    expect(matchesSprint(issue({ sprint_ids: [] }), 'none', [])).toBe(true);
    expect(matchesSprint(issue(), 'none', [])).toBe(false);
    expect(matchesSprint(issue(), 'all', [])).toBe(true);
  });
  it('sorts overview titles case-insensitively with stable ID tiebreakers', () => {
    const values = [
      pr({ id: 'z', title: 'alpha' }),
      pr({ id: 'a', title: 'Alpha' }),
      pr({ id: 'b', title: 'Beta' }),
    ];
    expect(values.sort(byTitle).map((v) => v.id)).toEqual(['a', 'z', 'b']);
  });
  it('sorts people and excludes only the current source identity', () => {
    const people = [
      person,
      { ...person, id: 'z', username: 'Zed' },
      { ...person, id: 'a', username: 'Alice' },
    ];
    expect(otherPeople(people, 'work', context().identities).map((p) => p.username)).toEqual([
      'Alice',
      'Zed',
    ]);
    expect(otherPeople(people, 'personal', context().identities)).toHaveLength(3);
    expect(
      otherPeople(people, 'work', [{ ...context().identities[1], username: '' }]).map((p) => p.id),
    ).toEqual(['a', 'z']);
    expect(people[0]).toEqual(person);
  });
  it('accepts only absolute HTTP(S) links and preserves nested GitLab repo paths', () => {
    for (const url of [
      'javascript:alert(1)',
      'data:text/html,x',
      '/relative',
      'not a url',
      'https://secret@example.com/',
    ])
      expect(safeUrl(url)).toBeNull();
    expect(safeUrl('https://example.com')).toBe('https://example.com/');
    expect(
      repositoryUrl(
        pr({
          forge: 'gitlab',
          url: 'https://git.example/base/team/sub/widget/-/merge_requests/42#note',
        }),
      ),
    ).toBe('https://git.example/base/team/sub/widget');
  });
  it('links Codeberg and Forgejo repositories using their plural pulls URL', () => {
    for (const forge of ['codeberg', 'forgejo'] as const) {
      expect(repositoryUrl(pr({ forge, url: 'https://forge.example/team/widget/pulls/42' }))).toBe(
        'https://forge.example/team/widget',
      );
    }
  });
  it('derives encoded sprint links from the trusted board URL preserving rapidView and path', () => {
    const url = new URL(
      sprintViewUrl({
        ...sprint,
        id: 'sprint /&?',
        board_url: 'https://jira.example/secure/RapidBoard.jspa?rapidView=12&selectedSprint=old',
      })!,
    );
    expect(url.pathname).toBe('/secure/RapidBoard.jspa');
    expect(url.searchParams.get('rapidView')).toBe('12');
    expect(url.searchParams.get('selectedSprint')).toBe('sprint /&?');
    expect(sprintViewUrl({ ...sprint, board_url: 'javascript:alert(1)' })).toBeNull();
  });
  it('does not hide no-sprint or future Jira mentions in the default attention view', () => {
    const b = board({
      issues: [
        issue({ id: 'backlog', sprint_ids: [] }),
        issue({ id: 'future', sprint_ids: ['future'] }),
      ],
    });
    expect(
      queues(b, 'attention', defaultFilters('attention'), context().jira_projects).issues,
    ).toHaveLength(2);
    expect(defaultFilters('overview').sprint).toBe('');
  });
});
