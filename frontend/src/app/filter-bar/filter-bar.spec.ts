import { TestBed } from '@angular/core/testing';
import { FilterBar } from './filter-bar';
import { board, context, orderedSprintScenario, sprint } from '../testing/fixtures';
describe('Board filter controls', () => {
  it('offers active and future sprints, no sprint, forge instances, and disambiguated repositories', async () => {
    const f = TestBed.createComponent(FilterBar);
    f.componentRef.setInput(
      'board',
      board({
        sprints: [
          sprint,
          { ...sprint, id: 'future', state: 'future', name: 'Next', default_eligible: false },
        ],
      }),
    );
    f.componentRef.setInput('context', context());
    await f.whenStable();
    const text = f.nativeElement.textContent;
    expect(text).toContain('No sprint');
    expect(text).toContain('Next');
    expect(text).not.toContain('Default eligible active sprints');
    expect(f.nativeElement.querySelector('optgroup[label="Future"] option').value).toBe('future');
    expect(f.componentInstance.sources().map((s) => s.id)).toEqual(['work', 'personal']);
    expect(f.componentInstance.repositories()).toEqual([
      { id: 'work\u001facme\u001fwidget', label: 'github · acme/widget' },
    ]);
  });
  it('retains every board sprint, including creation-excluded names, and deliberate choices on publication', async () => {
    const scenario = orderedSprintScenario();
    const f = TestBed.createComponent(FilterBar);
    f.componentRef.setInput('board', scenario.board);
    f.componentRef.setInput('context', scenario.context);
    f.componentRef.setInput('filters', {
      ...f.componentInstance.filters(),
      sprint: scenario.excluded.id,
    });
    await f.whenStable();
    const select = f.nativeElement.querySelector('.filter-pair select') as HTMLSelectElement;
    const ids = f.componentInstance.sprints().map((s) => s.id);
    expect(ids).toHaveLength(scenario.board.sprints.length);
    expect(ids).toEqual(expect.arrayContaining(scenario.board.sprints.map((s) => s.id)));
    expect(f.nativeElement.textContent).toContain(scenario.excluded.name);
    expect(f.nativeElement.textContent).toContain(scenario.core.name);
    expect(select.value).toBe(scenario.excluded.id);
    f.componentRef.setInput('board', { ...scenario.board, revision: 2 });
    f.componentRef.setInput('context', { ...scenario.context, current_sprints: [] });
    await f.whenStable();
    expect(select.value).toBe(scenario.excluded.id);
    expect(f.componentInstance.filters().sprint).toBe(scenario.excluded.id);
  });
  it('clears the other PR scope filter when selecting a forge or repository', () => {
    const f = TestBed.createComponent(FilterBar);
    f.componentRef.setInput('board', board());
    f.componentRef.setInput('context', context());
    f.componentRef.setInput('filters', {
      ...f.componentInstance.filters(),
      source: 'work',
      repository: 'work\u001facme\u001fwidget',
    });
    f.componentInstance.selectSource({ target: { value: 'personal' } } as unknown as Event);
    expect(f.componentInstance.filters()).toMatchObject({ source: 'personal', repository: 'all' });
    f.componentInstance.selectRepository({
      target: { value: 'work\u001facme\u001fwidget' },
    } as unknown as Event);
    expect(f.componentInstance.filters()).toMatchObject({
      source: 'all',
      repository: 'work\u001facme\u001fwidget',
    });
  });
  it.each([false, true])(
    'restores a repository selection after its options render (upstream=%s)',
    async (upstreamOnly) => {
      const f = TestBed.createComponent(FilterBar);
      f.componentRef.setInput('board', board());
      f.componentRef.setInput('context', context());
      f.componentRef.setInput('upstreamOnly', upstreamOnly);
      f.componentRef.setInput('filters', {
        ...f.componentInstance.filters(),
        repository: 'work\u001facme\u001fwidget',
      });
      await f.whenStable();
      const repository = f.nativeElement.querySelectorAll(
        '.pr-filter-pair select',
      )[1] as HTMLSelectElement;
      expect(repository.value).toBe('work\u001facme\u001fwidget');
    },
  );
});
