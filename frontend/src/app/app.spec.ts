import { TestBed } from '@angular/core/testing';
import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { App } from './app';
import { board, context, sync, runtimeMode } from './testing/fixtures';
describe('PullBoard app', () => {
  it('loads the same-origin API and renders a complete local dashboard without authentication UI', async () => {
    localStorage.clear();
    TestBed.configureTestingModule({
      providers: [provideHttpClient(), provideHttpClientTesting()],
    });
    const f = TestBed.createComponent(App);
    const http = TestBed.inject(HttpTestingController);
    http.expectOne('/api/context').flush(context());
    http.expectOne('/api/sync').flush(sync());
    http
      .expectOne('/api/runtime-mode')
      .flush({ read_only_test_mode: true, mode_revision: 'mode-test-1' });
    http.expectOne('/api/health').flush({ status: 'ok', version: '1.2.0', ...runtimeMode() });
    http.expectOne('/api/board').flush(board());
    await f.whenStable();
    const text = f.nativeElement.textContent;
    expect(f.nativeElement.querySelector('h1').textContent).toBe('PullBoard');
    expect(text).toContain('Needs Attention');
    expect(text).toContain('Upstream Pull Requests');
    expect(text).toContain('Current active sprints');
    expect(text).toContain('Source health');
    expect(text).toContain('Read-only test mode');
    expect(f.nativeElement.querySelector('input[type="password"]')).toBeNull();
    http.verify();
  });
});
