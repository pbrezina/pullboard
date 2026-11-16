import { TestBed } from '@angular/core/testing';
import { IdentityBar } from './identity-bar';
import { board, context, orderedSprintScenario, sprint } from '../testing/fixtures';
describe('Who am I', () => {
  it('joins configured source names by ID while keeping same-forge accounts distinct', async () => {
    const f = TestBed.createComponent(IdentityBar);
    f.componentRef.setInput('context', context());
    const source = board().sources[0];
    f.componentRef.setInput('sources', [
      { ...source, source_id: 'personal', name: 'Personal GitHub' },
      source,
      { ...source, source_id: 'jira', kind: 'jira', name: 'Work Jira' },
      { ...source, source_id: 'unverified', name: 'Unverified forge' },
    ]);
    await f.whenStable();
    const labels = () =>
      Array.from(
        f.nativeElement.querySelectorAll('.identity strong') as NodeListOf<HTMLElement>,
      ).map((label) => label.textContent?.trim());
    expect(labels()).toEqual(['Work Jira', 'Work GitHub', 'Personal GitHub']);
    expect(f.nativeElement.querySelectorAll('.identity')).toHaveLength(3);
    expect(f.nativeElement.querySelector('.identity a').textContent).toBe('alex');

    f.componentRef.setInput('sources', [{ ...source, name: 'Renamed GitHub' }]);
    await f.whenStable();
    expect(labels()).toEqual(['jira', 'Renamed GitHub', 'personal']);
  });
  it('shows distinct identities, current project and linked full sprint names with ambiguity', async () => {
    const f = TestBed.createComponent(IdentityBar);
    f.componentRef.setInput(
      'context',
      context({
        current_sprints: [
          sprint,
          { ...sprint, id: 's2', name: 'Parallel sprint' },
          { ...sprint, id: 's3', name: 'Another matching sprint', team: null },
        ],
      }),
    );
    await f.whenStable();
    expect(f.componentInstance.sprints()).toHaveLength(3);
    expect(f.nativeElement.textContent).toContain('Current project');
    expect(f.nativeElement.textContent).toContain('DEV / Development');
    expect(f.nativeElement.textContent).toContain('Multiple matching active sprints');
    expect(f.nativeElement.textContent).toContain('personal');
    expect(f.nativeElement.textContent).toContain('work');
    expect(f.nativeElement.textContent).toContain('Parallel sprint');
    expect(
      f.nativeElement.querySelector(`a[href="${sprint.board_url}?selectedSprint=sprint-1"]`),
    ).not.toBeNull();
    expect(f.nativeElement.textContent).toContain('Ends');
  });
  it('links the primary-board deduplicated current sprint, never additional-board matches', async () => {
    const scenario = orderedSprintScenario();
    const f = TestBed.createComponent(IdentityBar);
    f.componentRef.setInput('context', scenario.context);
    await f.whenStable();
    expect(f.componentInstance.sprints()).toEqual([scenario.primary]);
    const links = f.nativeElement.querySelectorAll(
      '.sprint-list a',
    ) as NodeListOf<HTMLAnchorElement>;
    expect(links).toHaveLength(1);
    expect(links[0].href).toBe(
      'https://jira.example/secure/RapidBoard.jspa?rapidView=12&selectedSprint=sprint-1&view=planning',
    );
    expect(f.nativeElement.textContent).not.toContain(scenario.secondaryIdentity.name);
    expect(f.nativeElement.textContent).not.toContain(scenario.core.name);
    expect(f.nativeElement.textContent).not.toContain(scenario.excluded.name);
  });
  it('explicitly represents no active sprint and unconfigured accounts', async () => {
    const f = TestBed.createComponent(IdentityBar);
    f.componentRef.setInput('context', context({ identities: [], current_sprints: [] }));
    await f.whenStable();
    expect(f.nativeElement.textContent).toContain('No matching active sprint');
    expect(f.nativeElement.textContent).toContain('No verified identities yet');
  });
  it('excludes nonmatching CORE sprints without claiming ownership or queried board numbers', async () => {
    const f = TestBed.createComponent(IdentityBar);
    const c = context();
    f.componentRef.setInput('context', {
      ...c,
      identities: [{ ...c.identities[0], username: '' }],
      current_sprints: [
        { ...sprint, team: null },
        {
          ...sprint,
          id: 'foreign',
          name: 'CORE foreign origin sprint',
          team: null,
          default_eligible: false,
        },
      ],
    });
    await f.whenStable();
    expect(f.nativeElement.textContent).toContain('Alex Developer');
    expect(f.nativeElement.textContent).not.toContain('Ownership unknown');
    expect(f.nativeElement.textContent).not.toContain('CORE');
    expect(f.nativeElement.textContent).not.toContain('Board 12');
    expect(f.nativeElement.querySelector('a').getAttribute('href')).toBe(
      sprint.board_url + '?selectedSprint=sprint-1',
    );
  });
});
