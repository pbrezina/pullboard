import { Component } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { ExternalLink } from './external-link';
@Component({
  imports: [ExternalLink],
  template: `<pb-link [url]="url" label="Full title">Visible title</pb-link>`,
})
class Host {
  url = 'https://example.com/thread#1';
}
describe('Safe external links', () => {
  it('projects link content and uses safe target and rel', async () => {
    const f = TestBed.createComponent(Host);
    await f.whenStable();
    const link: HTMLAnchorElement = f.nativeElement.querySelector('a');
    expect(link.textContent).toBe('Visible title');
    expect(link.target).toBe('_blank');
    expect(link.rel).toContain('noopener');
    expect(link.title).toBe('Full title');
  });
  it('renders unsafe URLs as text, never a clickable javascript link', async () => {
    const f = TestBed.createComponent(ExternalLink);
    f.componentRef.setInput('url', 'javascript:alert(1)');
    await f.whenStable();
    expect(f.nativeElement.querySelector('a')).toBeNull();
  });
});
