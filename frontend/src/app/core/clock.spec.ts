import { TestBed } from '@angular/core/testing';
import { vi } from 'vitest';
import { Clock, elapsed } from './clock';
describe('Upstream elapsed time', () => {
  it('formats minutes, hours, days and never shows negative ages', () => {
    const now = Date.parse('2026-11-06T12:00:00Z');
    expect(elapsed('2026-11-06T11:59:00Z', now)).toBe('1m ago');
    expect(elapsed('2026-11-06T10:00:00Z', now)).toBe('2h ago');
    expect(elapsed('2026-11-04T12:00:00Z', now)).toBe('2d ago');
    expect(elapsed('2026-11-07T12:00:00Z', now)).toBe('Just now');
    expect(elapsed(null, now)).toBe('Not yet');
    expect(elapsed('invalid', now)).toBe('Unknown');
  });
  it('updates time periodically and cleans its interval up', () => {
    vi.useFakeTimers();
    const clock = TestBed.inject(Clock);
    const now = clock.now();
    vi.advanceTimersByTime(30_000);
    expect(clock.now()).toBe(now + 30_000);
    TestBed.resetTestingModule();
    vi.advanceTimersByTime(30_000);
    expect(clock.now()).toBe(now + 30_000);
    vi.useRealTimers();
  });
});
