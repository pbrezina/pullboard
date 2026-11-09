import { TestBed } from '@angular/core/testing';
import { AttentionDetail } from './attention-detail';
import { attention } from '../testing/fixtures';
describe('Attention badges', () => {
  it('shows linked reasons and upstream waiting without coverage warnings, inline histories or disclosures', async () => {
    const f = TestBed.createComponent(AttentionDetail);
    f.componentRef.setInput('attention', { ...attention(), coverage: 'partial' });
    await f.whenStable();
    const text = f.nativeElement.textContent;
    expect(text).toContain('You were mentioned');
    expect(text).not.toContain('Could you check the cache behavior?');
    expect(text).toContain('Waiting since');
    expect(text).not.toContain('Partial history');
    expect(f.nativeElement.querySelector('.source-warning')).toBeNull();
    expect(f.nativeElement.querySelector('a').href).toContain('#comment-1');
    expect(f.nativeElement.querySelector('details, summary')).toBeNull();
    expect(f.nativeElement.querySelector('.attention-threads')).toBeNull();
    expect(f.nativeElement.querySelectorAll('time')).toHaveLength(1);
    expect(f.nativeElement.querySelector('time').dateTime).toBe(attention().waiting_since);
    expect(f.nativeElement.querySelector('.attention-tag').textContent).toContain(
      'You were mentioned',
    );
  });
  it('links each reason to its oldest supplied thread without inventing missing discussion URLs', async () => {
    const f = TestBed.createComponent(AttentionDetail);
    const current = attention();
    f.componentRef.setInput('attention', {
      ...current,
      reasons: ['pr_mention', 'review_request'],
      threads: [
        current.threads[0],
        {
          ...current.threads[0],
          id: 'older',
          url: 'https://forge.example/acme/widget/pull/42#older-comment',
          excerpt: 'Older unanswered feedback',
          waiting_since: '2026-10-01T00:00:00Z',
        },
      ],
    });
    await f.whenStable();
    const badges = f.nativeElement.querySelectorAll('.attention-tag');
    expect(badges).toHaveLength(2);
    expect(badges[0].textContent).toContain('You were mentioned');
    expect(badges[0].querySelector('a').href).toContain('#older-comment');
    expect(badges[1].textContent).toContain('Review requested');
    expect(badges[1].querySelector('a')).toBeNull();
    expect(f.nativeElement.textContent).not.toContain('Older unanswered feedback');
    expect(f.nativeElement.querySelector('time').dateTime).toBe(current.waiting_since);
    f.componentRef.setInput('showWaiting', false);
    await f.whenStable();
    expect(f.nativeElement.querySelector('time')).toBeNull();
  });
});
