import { TestBed } from '@angular/core/testing';
import { SourceHealth } from './source-health';
import { board, sync, jiraOptionsStatus } from '../testing/fixtures';
import type { SourceProgress } from '../core/api-types';
describe('Source health', () => {
  it('exposes stale cache, backoff, exact retry, and coverage notes', async () => {
    const f = TestBed.createComponent(SourceHealth);
    f.componentRef.setInput('sources', [
      {
        ...board().sources[0],
        status: 'backoff',
        error: 'Rate limited.',
        retry_at: '2026-11-06T15:00:00Z',
        coverage_notes: ['Private review history unavailable.'],
      },
    ]);
    await f.whenStable();
    const text = f.nativeElement.textContent;
    expect(text).toContain('1 source(s) need attention');
    expect(text).toContain('Last good snapshot is retained');
    expect(text).toContain('Private review history unavailable');
    expect(text).toContain('Retry at');
  });
  it('renders actual phase names, empty 0/0, and no false fraction for unknown terminal work', async () => {
    const f = TestBed.createComponent(SourceHealth);
    f.componentRef.setInput('sources', board().sources);
    for (const phase of [
      'idle',
      'queued',
      'preparing',
      'discovering',
      'fetching',
      'publishing',
      'completed',
      'failed',
      'backoff',
    ] as SourceProgress['phase'][]) {
      f.componentRef.setInput('progress', [
        {
          ...sync().progress[0],
          phase,
          completed_items: 0,
          total_items: phase === 'completed' ? 0 : null,
        },
      ]);
      await f.whenStable();
      expect(f.nativeElement.querySelector('.progress-history').textContent).toContain(phase + ':');
      if (f.componentInstance.active({ ...sync().progress[0], phase })) {
        expect(f.nativeElement.querySelector('.source-progress header span').textContent).toBe(
          phase,
        );
      }
      if (phase === 'completed') {
        expect(f.nativeElement.textContent).toContain('0 / 0 evaluated');
        expect(f.nativeElement.querySelector('progress')).toBeNull();
        expect(f.nativeElement.querySelector('.source-progress-grid')).toBeNull();
      }
      if (phase === 'failed' || phase === 'backoff')
        expect(f.nativeElement.querySelector('progress')).toBeNull();
    }
  });
  it('collapses healthy idle counters into existing details, but retains all progress during an ongoing job', async () => {
    const f = TestBed.createComponent(SourceHealth);
    f.componentRef.setInput('sources', board().sources);
    f.componentRef.setInput('progress', sync().progress);
    await f.whenStable();
    const details = f.nativeElement.querySelector('.health-panel') as HTMLDetailsElement;
    expect(details.open).toBe(false);
    expect(f.nativeElement.querySelector('.source-progress-grid')).toBeNull();
    expect(details.textContent).toContain('All sources up to date');
    expect(details.querySelector('.progress-history')?.textContent).toContain(
      'completed: 1 / 1 evaluated / 1s',
    );
    expect(f.componentInstance.degraded()).toBe(0);
    details.open = true;
    expect(details.querySelector('.progress-history')).not.toBeNull();
    f.componentRef.setInput('syncing', true);
    await f.whenStable();
    expect(f.nativeElement.querySelector('.source-progress-grid')).not.toBeNull();
    expect(f.nativeElement.querySelector('.source-completed')).not.toBeNull();
    f.componentRef.setInput('syncing', false);
    f.componentRef.setInput('sources', [
      {
        ...board().sources[0],
        status: 'backoff',
        error: 'Actual rate limit.',
        retry_at: '2026-11-06T15:00:00Z',
      },
    ]);
    await f.whenStable();
    expect(f.nativeElement.querySelector('.source-failed').textContent).toContain(
      'Actual rate limit.',
    );
  });
  it('keeps neutral coverage green for every source kind independently of attention history', async () => {
    const f = TestBed.createComponent(SourceHealth);
    f.componentRef.setInput(
      'sources',
      ['jira', 'github', 'gitlab', 'forgejo', 'codeberg'].map((kind) => ({
        ...board().sources[0],
        source_id: kind,
        kind,
        coverage_notes: ['History scope information.'],
      })),
    );
    await f.whenStable();
    expect(f.componentInstance.degraded()).toBe(0);
    expect(f.nativeElement.querySelectorAll('.chip.ready')).toHaveLength(5);
    expect(f.nativeElement.textContent).toContain('All sources up to date');
  });
  it('keeps completed ticket progress independent of unavailable options and exposes per-project finished checks', async () => {
    const f = TestBed.createComponent(SourceHealth);
    f.componentRef.setInput('sources', board().sources);
    f.componentRef.setInput('progress', sync().progress);
    f.componentRef.setInput('syncing', true);
    const options = jiraOptionsStatus();
    options.ready = false;
    options.health = { ...options.health, status: 'pending', last_success_at: null };
    options.progress = {
      ...options.progress,
      phase: 'fetching',
      finished_at: null,
      elapsed_ms: 65000,
      groups: [{ name: 'Reviewers', phase: 'completed', completed_items: 75, total_items: 75 }],
      projects: [
        { name: 'DEV', phase: 'completed', completed_items: 75, total_items: 75 },
        { name: 'OPS', phase: 'fetching', completed_items: 28, total_items: 75 },
      ],
    };
    f.componentRef.setInput('jiraOptions', options);
    await f.whenStable();
    const panel = f.nativeElement.querySelector('.jira-options') as HTMLElement;
    expect(panel.textContent).toContain('Creation unavailable');
    expect(panel.textContent).toContain('Discovery complete');
    expect(panel.textContent).toContain('28 / 75 checks finished');
    expect(panel.textContent).toContain('eligible and ineligible');
    const checks = panel.querySelector('progress[aria-label="OPS project eligibility checks"]')!;
    expect(checks.getAttribute('value')).toBe('28');
    expect(checks.getAttribute('max')).toBe('75');
    expect(f.nativeElement.querySelector('[aria-label="Jira refresh"]').textContent).toContain(
      'Jira tickets',
    );
    expect(
      f.nativeElement.querySelectorAll('.source-progress-grid .source-completed'),
    ).toHaveLength(2);
    expect(panel.querySelector('[role="status"], [aria-live]')).toBeNull();
  });
  it('keeps groups indeterminate until full pagination, including empty completed memberships', async () => {
    const f = TestBed.createComponent(SourceHealth);
    f.componentRef.setInput('sources', board().sources);
    const options = jiraOptionsStatus('syncing');
    options.progress = {
      ...options.progress,
      phase: 'discovering',
      groups: [
        { name: 'Reviewers', phase: 'discovering', completed_items: 0, total_items: null },
        { name: 'Empty group', phase: 'completed', completed_items: 0, total_items: 0 },
      ],
    };
    f.componentRef.setInput('jiraOptions', options);
    await f.whenStable();
    const panel = f.nativeElement.querySelector('.jira-options') as HTMLElement;
    expect(
      panel
        .querySelector('progress[aria-label="Reviewers group membership"]')!
        .hasAttribute('value'),
    ).toBe(false);
    expect(
      panel
        .querySelector('progress[aria-label="Jira creation metadata and statuses"]')!
        .hasAttribute('value'),
    ).toBe(false);
    expect(panel.querySelector('progress[aria-label="Empty group group membership"]')).toBeNull();
    expect(panel.textContent).toContain('0 / 0 members discovered');
    expect(panel.textContent).toContain('Total unknown');
  });
  it.each([true, false])(
    'reports options failure with ready=%s without failing ticket health',
    async (ready) => {
      const f = TestBed.createComponent(SourceHealth);
      f.componentRef.setInput('sources', board().sources);
      f.componentRef.setInput('progress', sync().progress);
      const options = jiraOptionsStatus();
      options.ready = ready;
      options.health = {
        ...options.health,
        status: 'error',
        last_success_at: ready ? options.health.last_success_at : null,
        error: 'Eligibility check failed.',
      };
      options.progress.phase = 'failed';
      f.componentRef.setInput('jiraOptions', options);
      await f.whenStable();
      expect(f.componentInstance.degraded()).toBe(0);
      expect(f.nativeElement.querySelectorAll('.source-progress-grid .source-failed')).toHaveLength(
        0,
      );
      expect(f.nativeElement.querySelector('.jira-options.source-failed').textContent).toContain(
        'tickets are unaffected',
      );
      expect(f.nativeElement.textContent).toContain(
        ready ? 'Stale cached choices available' : 'Creation unavailable',
      );
    },
  );
  it('omits the options panel when Jira is disabled', async () => {
    const f = TestBed.createComponent(SourceHealth);
    f.componentRef.setInput('sources', []);
    f.componentRef.setInput('jiraOptions', null);
    await f.whenStable();
    expect(f.nativeElement.querySelector('.jira-options')).toBeNull();
    expect(f.nativeElement.textContent).toContain('No sources configured');
  });
  it('shows independent options retry and stale health with idle process-local progress', async () => {
    const f = TestBed.createComponent(SourceHealth);
    f.componentRef.setInput('sources', board().sources);
    const options = jiraOptionsStatus('syncing');
    options.progress.phase = 'idle';
    options.health = {
      ...options.health,
      status: 'backoff',
      error: 'Options rate limited.',
      retry_at: '2026-11-06T15:00:00Z',
    };
    f.componentRef.setInput('jiraOptions', options);
    await f.whenStable();
    const panel = f.nativeElement.querySelector('.jira-options') as HTMLElement;
    expect(panel.textContent).toContain('Stale cached choices available');
    expect(panel.textContent).toContain('Options rate limited.');
    expect(panel.querySelector('time[datetime="2026-11-06T15:00:00Z"]')).not.toBeNull();
    expect(f.componentInstance.degraded()).toBe(0);
    expect(panel.querySelector('progress')).toBeNull();
  });
  it('joins progress by source ID, keeps unknown totals indeterminate, and does not call publishing complete', async () => {
    const f = TestBed.createComponent(SourceHealth);
    f.componentRef.setInput('sources', [
      { ...board().sources[0], source_id: 'second' },
      board().sources[0],
    ]);
    const p = sync().progress[0];
    f.componentRef.setInput('progress', [
      {
        ...p,
        source_id: 'work',
        phase: 'publishing',
        completed_items: 25,
        total_items: 25,
        elapsed_ms: 65123,
      },
      {
        ...p,
        source_id: 'second',
        phase: 'fetching',
        completed_items: 7,
        total_items: null,
        elapsed_ms: 2500,
      },
    ]);
    await f.whenStable();
    const bars = f.nativeElement.querySelectorAll('progress');
    expect(bars[0].hasAttribute('value')).toBe(false);
    expect(bars[1].getAttribute('value')).toBe('25');
    expect(f.nativeElement.textContent).toContain('1m 5s');
    expect(f.nativeElement.textContent).toContain('7 evaluated / total unknown');
    expect(f.nativeElement.textContent).toContain('waiting for atomic publication');
    expect(f.nativeElement.querySelector('.source-completed')).toBeNull();
    f.componentRef.setInput('progress', [{ ...p, source_id: 'work', phase: 'failed' }]);
    await f.whenStable();
    expect(f.componentInstance.degraded()).toBe(1);
    expect(f.nativeElement.querySelector('.source-failed')).not.toBeNull();
  });
});
