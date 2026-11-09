import { TestBed } from '@angular/core/testing';
import { Timestamp } from './timestamp';
describe('Timestamp', () => {
  it('exposes an upstream machine timestamp and a localized exact tooltip', async () => {
    const f = TestBed.createComponent(Timestamp);
    f.componentRef.setInput('value', '2026-11-01T00:00:00Z');
    await f.whenStable();
    const time: HTMLTimeElement = f.nativeElement.querySelector('time');
    expect(time.dateTime).toBe('2026-11-01T00:00:00Z');
    expect(time.title).toContain('2026');
    expect(time.textContent).toContain('ago');
  });
});
