import { TestBed } from '@angular/core/testing';
import { AssigneePicker } from './assignee-picker';
import { options, person } from '../../testing/fixtures';

describe('Cached assignee combobox', () => {
  async function ready() {
    const f = TestBed.createComponent(AssigneePicker);
    f.componentRef.setInput('people', [...options().assignees, { ...person, email: null }]);
    await f.whenStable();
    return f;
  }
  it('filters case-insensitively by real name, username and exposed email without changing selection', async () => {
    const f = await ready();
    const c = f.componentInstance;
    c.value.set('reviewer');
    for (const query of ['PAT REVIEWER', 'pAt', 'EXAMPLE.TEST']) {
      c.query.set(query);
      expect(c.matches().map((p) => p.id)).toEqual(['reviewer']);
    }
    c.query.set('no match');
    expect(c.matches()).toEqual([]);
    expect(c.value()).toBe('reviewer');
    expect(c.selectedLabel()).toBe('Pat Reviewer');
    c.query.set('alex');
    expect(c.matches()).toEqual([{ ...person, email: null }]);
  });
  it('supports arrow/Enter selection, Escape without closing the editor and easy Unassigned', async () => {
    const f = await ready();
    const c = f.componentInstance;
    c.keydown(new KeyboardEvent('keydown', { key: 'ArrowDown', cancelable: true }));
    c.keydown(new KeyboardEvent('keydown', { key: 'Enter', cancelable: true }));
    expect(c.value()).toBe('reviewer');
    expect(c.open()).toBe(false);
    c.show();
    const escape = new KeyboardEvent('keydown', { key: 'Escape', cancelable: true });
    const stop = vi.spyOn(escape, 'stopPropagation');
    c.keydown(escape);
    expect(stop).toHaveBeenCalledOnce();
    expect(c.value()).toBe('reviewer');
    c.show();
    c.keydown(new KeyboardEvent('keydown', { key: 'Home', cancelable: true }));
    c.keydown(new KeyboardEvent('keydown', { key: 'Enter', cancelable: true }));
    expect(c.value()).toBe('');
  });
  it('shows a clear no-match state, selected person and no duplicated name for hidden email', async () => {
    const f = await ready();
    const c = f.componentInstance;
    c.value.set('reviewer');
    c.show();
    c.query.set('missing');
    await f.whenStable();
    expect(f.nativeElement.textContent).toContain('No matching assignees');
    expect(f.nativeElement.textContent).toContain('Selected: Pat Reviewer');
    expect(f.nativeElement.querySelector('#ticket-assignee').getAttribute('role')).toBe('combobox');
    expect(c.choices()).toEqual([null]);
  });
  it('bounds rendering for a complete large eligible pool', async () => {
    const f = await ready();
    f.componentRef.setInput(
      'people',
      Array.from({ length: 2000 }, (_, index) => ({
        ...person,
        id: String(index),
        display_name: `Person ${index}`,
        email: null,
      })),
    );
    f.componentInstance.show();
    await f.whenStable();
    expect(f.componentInstance.matches()).toHaveLength(2000);
    expect(f.nativeElement.querySelectorAll('[role="option"]').length).toBeLessThan(50);
  });
});
