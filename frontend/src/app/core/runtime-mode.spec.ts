import { TestBed } from '@angular/core/testing';
import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { vi } from 'vitest';
import { RuntimeMode } from './runtime-mode';
import { runtimeMode } from '../testing/fixtures';

describe('Atomic runtime mode and stale reads', () => {
  let mode: RuntimeMode;
  let http: HttpTestingController;
  beforeEach(() => {
    vi.useFakeTimers();
    TestBed.configureTestingModule({
      providers: [provideHttpClient(), provideHttpClientTesting()],
    });
    mode = TestBed.inject(RuntimeMode);
    http = TestBed.inject(HttpTestingController);
  });
  afterEach(() => {
    http.verify();
    TestBed.resetTestingModule();
    vi.useRealTimers();
  });
  it('polls lightweight mode, replaces the entire pair, and recognizes same-boolean restart', () => {
    mode.start();
    http.expectOne('/api/runtime-mode').flush(runtimeMode());
    vi.advanceTimersByTime(2000);
    http.expectOne('/api/runtime-mode').flush(runtimeMode({ mode_revision: 'new-boot' }));
    expect(mode.matches(runtimeMode())).toBe(false);
    expect(mode.value()).toEqual(runtimeMode({ mode_revision: 'new-boot' }));
    http.expectNone('/api/context');
    http.expectNone('/api/board');
  });
  it('drops pre-toggle in-flight reads, sends original token, and polls afresh after own success', () => {
    mode.value.set(runtimeMode());
    mode.start();
    const oldRead = http.expectOne('/api/runtime-mode');
    mode.change(false, runtimeMode()).subscribe((pair) => mode.apply(pair));
    expect(oldRead.cancelled).toBe(true);
    const change = http.expectOne((r) => r.method === 'POST');
    expect(change.request.body).toEqual({
      read_only_test_mode: false,
      expected_mode_revision: 'mode-test-1',
    });
    expect(change.request.headers.get('X-PullBoard-Request')).toBe('1');
    change.flush(runtimeMode({ read_only_test_mode: false, mode_revision: 'live-2' }));
    expect(mode.value()).toEqual(
      runtimeMode({ read_only_test_mode: false, mode_revision: 'live-2' }),
    );
    http
      .expectOne('/api/runtime-mode')
      .flush(runtimeMode({ read_only_test_mode: false, mode_revision: 'live-2' }));
  });
  it('does not retry mode conflicts and fails closed when mode cannot be verified', () => {
    mode.value.set(runtimeMode());
    const error = vi.fn();
    mode.change(false, runtimeMode()).subscribe({ error });
    http
      .expectOne((r) => r.method === 'POST')
      .flush(
        { error: { code: 'mode_conflict', message: 'Changed.' } },
        { status: 409, statusText: 'Conflict' },
      );
    http
      .expectOne('/api/runtime-mode')
      .flush(
        { error: { code: 'unavailable', message: 'Unavailable.' } },
        { status: 503, statusText: 'Unavailable' },
      );
    expect(mode.matches(runtimeMode())).toBe(false);
    expect(error).toHaveBeenCalledOnce();
    http.expectNone((r) => r.method === 'POST');
  });
});
