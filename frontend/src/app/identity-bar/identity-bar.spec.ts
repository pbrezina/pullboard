import { TestBed } from '@angular/core/testing';
import { IdentityBar } from './identity-bar';
import { context, sprint } from '../testing/fixtures';
describe('Who am I', () => {
  it('shows distinct identities, current project and linked full sprint names with ambiguity', async () => {
    const f = TestBed.createComponent(IdentityBar);
    f.componentRef.setInput(
      'context',
      context({
        current_sprints: [
          sprint,
          { ...sprint, id: 's2', name: 'Parallel sprint' },
          { ...sprint, id: 's3', team: 'Web', board_id: 13 },
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
