import { TestBed } from '@angular/core/testing';
import { SourceHealth } from './source-health';
import { board, sync } from '../testing/fixtures';
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
