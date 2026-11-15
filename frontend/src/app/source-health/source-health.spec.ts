import { TestBed } from '@angular/core/testing';
import { SourceHealth } from './source-health';
import { board, sync, jiraOptionsStatus } from '../testing/fixtures';
import type { SourceProgress } from '../core/api-types';

describe('Source health', () => {
  it('keeps every progress row inside the original closed disclosure, including during refresh', async () => {
    const f = TestBed.createComponent(SourceHealth);
    f.componentRef.setInput('sources', board().sources);
    f.componentRef.setInput('progress', sync().progress);
    f.componentRef.setInput('jiraOptions', jiraOptionsStatus());
    f.componentRef.setInput('syncing', true);
    await f.whenStable();
    const panel = f.nativeElement.querySelector('.health-panel') as HTMLDetailsElement;
    expect(panel.open).toBe(false);
    expect(f.nativeElement.children).toHaveLength(1);
    expect(f.nativeElement.querySelectorAll('progress')).toHaveLength(3);
    expect(panel.querySelectorAll('progress')).toHaveLength(3);
    expect(panel.querySelector('.jira-options details')).toBeNull();
    expect(panel.querySelectorAll('.source-diagnostics')).toHaveLength(2);
    panel.open = true;
    f.componentRef.setInput('syncing', false);
    await f.whenStable();
    expect(panel.open).toBe(true);
    expect(panel.querySelectorAll('progress')).toHaveLength(3);
    expect(panel.querySelector('.progress-row')?.textContent).toContain('1 / 1');
    expect(panel.querySelector('.progress-time')?.textContent).toBe('1s');
  });

  it('retains errors and retry deadlines but hides routine health information behind diagnostics', async () => {
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
    const panel = f.nativeElement.querySelector('.health-panel') as HTMLDetailsElement;
    expect(panel.querySelector(':scope > summary')?.textContent).toContain(
      '1 source(s) need attention',
    );
    expect(panel.querySelector('.error-text')?.textContent).toContain('Stale snapshot retained.');
    expect(panel.querySelector('time[datetime="2026-11-06T15:00:00Z"]')).not.toBeNull();
    const diagnostics = panel.querySelector('.source-diagnostics') as HTMLDetailsElement;
    expect(diagnostics.open).toBe(false);
    expect(diagnostics.textContent).toContain('Private review history unavailable.');
  });

  it('keeps neutral coverage green for every source kind', async () => {
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
    expect(f.nativeElement.querySelector('.health-panel > summary').textContent).toContain(
      'All sources up to date',
    );
  });

  it('joins ticket progress by source ID and gives unknown, empty and failed work honest counts', async () => {
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
      { ...p, source_id: 'second', phase: 'fetching', completed_items: 7, total_items: null },
    ]);
    await f.whenStable();
    const bars = f.nativeElement.querySelectorAll('progress');
    expect(bars[0].hasAttribute('value')).toBe(false);
    expect(bars[1].getAttribute('value')).toBe('25');
    expect(bars[1].getAttribute('aria-valuetext')).toBe('publishing: 25 / 25');
    expect(f.nativeElement.textContent).toContain('1m 5s');
    expect(f.nativeElement.textContent).toContain('7 / ?');
    expect(f.nativeElement.querySelector('.source-completed')).toBeNull();
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
          ...p,
          source_id: 'work',
          phase,
          completed_items: 0,
          total_items: phase === 'completed' ? 0 : null,
        },
      ]);
      await f.whenStable();
      expect(f.nativeElement.querySelector('.progress-row').getAttribute('title')).toBe(phase);
      if (phase === 'completed') {
        expect(f.nativeElement.textContent).toContain('0 / 0');
        expect(f.nativeElement.querySelector('progress')).toBeNull();
      }
      if (phase === 'failed' || phase === 'backoff') {
        expect(f.nativeElement.querySelector('progress')).toBeNull();
        expect(f.nativeElement.querySelector('.source-failed')).not.toBeNull();
        expect(f.componentInstance.degraded()).toBe(1);
        expect(f.nativeElement.querySelectorAll('.chip.ready')).toHaveLength(1);
      }
    }
  });

  it('adds only compact group/project rows under Jira with one shared options time and no metadata bar', async () => {
    const f = TestBed.createComponent(SourceHealth);
    f.componentRef.setInput('sources', board().sources);
    f.componentRef.setInput('progress', sync().progress);
    const options = jiraOptionsStatus('syncing');
    options.ready = false;
    options.health = { ...options.health, status: 'pending', last_success_at: null };
    options.progress = {
      ...options.progress,
      phase: 'fetching',
      elapsed_ms: 65000,
      groups: [{ name: 'Reviewers', phase: 'completed', completed_items: 75, total_items: 75 }],
      projects: [
        { name: 'DEV', phase: 'completed', completed_items: 75, total_items: 75 },
        { name: 'OPS', phase: 'fetching', completed_items: 28, total_items: 75 },
      ],
    };
    f.componentRef.setInput('jiraOptions', options);
    await f.whenStable();
    const panel = f.nativeElement.querySelector('[aria-label="Jira health"]');
    expect(panel.querySelectorAll('.progress-row')).toHaveLength(4);
    expect(panel.querySelector('.options-heading').textContent).toContain('Choices: fetching');
    expect(panel.querySelector('.options-heading').textContent).toContain('1m 5s');
    expect(panel.querySelector('.jira-options .progress-time')).toBeNull();
    const checks = panel.querySelector('progress[aria-label="OPS project eligibility checks"]');
    expect(checks.getAttribute('value')).toBe('28');
    expect(checks.getAttribute('max')).toBe('75');
    expect(checks.closest('.progress-row').textContent).toContain('28 / 75');
    expect(panel.querySelector('[aria-label="Jira creation metadata and statuses"]')).toBeNull();
    expect(panel.textContent).not.toContain('eligible and ineligible');
    expect(panel.querySelector('[role="status"], [aria-live]')).toBeNull();
    expect(f.componentInstance.degraded()).toBe(0);
  });

  it('keeps group discovery indeterminate until complete, including empty memberships', async () => {
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
    const panel = f.nativeElement.querySelector('.jira-options');
    expect(
      panel
        .querySelector('progress[aria-label="Reviewers group membership"]')
        .hasAttribute('value'),
    ).toBe(false);
    expect(panel.querySelector('progress[aria-label="Empty group group membership"]')).toBeNull();
    expect(panel.textContent).toContain('0 / 0');
    expect(panel.textContent).toContain('0 / ?');
    expect(panel.querySelector('.options-heading').textContent).toContain('discovering');
  });

  it.each([true, false])(
    'shows a brief independent options failure with ready=%s',
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
      const panel = f.nativeElement.querySelector('.jira-options');
      expect(f.componentInstance.degraded()).toBe(0);
      expect(panel.querySelector('.error-text').textContent).toContain(
        ready ? 'Cached choices retained.' : 'Creation unavailable.',
      );
      expect(panel.querySelector('.options-heading').textContent).toContain(
        ready ? 'stale' : 'unavailable',
      );
      expect(f.nativeElement.querySelector('.health-panel > summary').textContent).toContain(
        'Creation options need attention',
      );
    },
  );

  it('preserves options retry and stale health even with idle process-local progress', async () => {
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
    const panel = f.nativeElement.querySelector('.jira-options');
    expect(panel.textContent).toContain('Choices: stale');
    expect(panel.querySelector('time[datetime="2026-11-06T15:00:00Z"]')).not.toBeNull();
    expect(panel.querySelector('progress')).toBeNull();
  });

  it('omits creation choices with Jira disabled', async () => {
    const f = TestBed.createComponent(SourceHealth);
    f.componentRef.setInput('sources', []);
    await f.whenStable();
    expect(f.nativeElement.querySelector('.jira-options')).toBeNull();
    expect(f.nativeElement.textContent).toContain('No sources configured');
  });
});
