import { TestBed } from '@angular/core/testing';
import { DIALOG_DATA, DialogRef } from '@angular/cdk/dialog';
import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { vi } from 'vitest';
import { ModeConfirm } from './mode-confirm';
import { RuntimeMode } from './runtime-mode';
import { runtimeMode } from '../testing/fixtures';

describe('Live confirmation', () => {
  const close = vi.fn();
  beforeEach(() => {
    close.mockClear();
    TestBed.configureTestingModule({
      providers: [
        provideHttpClient(),
        provideHttpClientTesting(),
        { provide: DIALOG_DATA, useValue: { mode: runtimeMode(), target: false } },
        { provide: DialogRef, useValue: { close } },
      ],
    });
    TestBed.inject(RuntimeMode).value.set(runtimeMode());
  });
  it('explains shared admission and writes only after explicit confirmation', async () => {
    const f = TestBed.createComponent(ModeConfirm);
    const http = TestBed.inject(HttpTestingController);
    await f.whenStable();
    expect(f.nativeElement.textContent).toContain('Admitted live actions may finish');
    http.expectNone((r) => r.method === 'POST');
    f.componentInstance.confirm();
    http
      .expectOne((r) => r.method === 'POST')
      .flush(runtimeMode({ read_only_test_mode: false, mode_revision: 'live-2' }));
    http
      .expectOne('/api/runtime-mode')
      .flush(runtimeMode({ read_only_test_mode: false, mode_revision: 'live-2' }));
    expect(close).toHaveBeenCalledOnce();
    http.verify();
  });
  it('invalidates the confirmation itself on a cross-tab round trip without replacing its token', async () => {
    const f = TestBed.createComponent(ModeConfirm);
    TestBed.inject(RuntimeMode).apply(runtimeMode({ mode_revision: 'test-3' }));
    await f.whenStable();
    expect(f.componentInstance.stale()).toBe(true);
    expect(f.componentInstance.data.mode.mode_revision).toBe('mode-test-1');
    f.componentInstance.confirm();
    TestBed.inject(HttpTestingController).expectNone((r) => r.method === 'POST');
    expect(close).not.toHaveBeenCalled();
  });
});
