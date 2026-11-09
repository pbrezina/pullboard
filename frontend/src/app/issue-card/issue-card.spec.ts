import { TestBed } from '@angular/core/testing';
import { CdkDropList, type CdkDragDrop } from '@angular/cdk/drag-drop';
import { By } from '@angular/platform-browser';
import { vi } from 'vitest';
import { IssueCard } from './issue-card';
import { Actions } from '../core/actions';
import { BoardStore } from '../core/board-store';
import { RuntimeMode } from '../core/runtime-mode';
import type { JiraIssue } from '../core/api-types';
import {
  attention,
  board,
  context,
  foreignMention,
  issue,
  pr,
  runtimeMode,
} from '../testing/fixtures';
describe('Jira card', () => {
  beforeEach(() => {
    TestBed.inject(BoardStore).context.set(context());
    TestBed.inject(BoardStore).board.set(board());
    TestBed.inject(RuntimeMode).value.set(runtimeMode());
    TestBed.inject(RuntimeMode).dragMode = runtimeMode();
  });
  it('renders normalized text, issue type and key without interpreting upstream HTML', async () => {
    const f = TestBed.createComponent(IssueCard);
    f.componentRef.setInput(
      'issue',
      issue({ description: '<script>alert(1)</script>', assignees: [] }),
    );
    await f.whenStable();
    expect(f.nativeElement.textContent).toContain('<script>alert(1)</script>');
    expect(f.nativeElement.querySelector('script')).toBeNull();
    expect(f.nativeElement.textContent).toContain('DEV-1');
    expect(f.nativeElement.querySelector('.issue-key')?.textContent).toBe('DEV-1');
    expect(f.nativeElement.textContent).toContain('Task');
    expect(f.nativeElement.querySelector('button').textContent).toContain('Link pull request');
    expect(f.nativeElement.querySelector('button').disabled).toBe(false);
  });
  it('uses distinct semantic marks for bugs, stories, epics and tasks', async () => {
    const f = TestBed.createComponent(IssueCard);
    for (const [issue_type, kind] of [
      ['Bug', 'bug'],
      ['Story', 'story'],
      ['Epic', 'epic'],
      ['Task', 'task'],
    ]) {
      f.componentRef.setInput('issue', issue({ issue_type }));
      await f.whenStable();
      expect(f.nativeElement.querySelector('article').dataset.issueKind).toBe(kind);
    }
  });
  it('shows baseline order, actual linked PR titles, URLs and only known state in either tab', async () => {
    const f = TestBed.createComponent(IssueCard);
    f.componentRef.setInput(
      'issue',
      issue({
        linked_pr_ids: [pr().id, 'missing:pr'],
        description: '<b>Literal upstream description</b>',
      }),
    );
    for (const attentionMode of [false, true]) {
      f.componentRef.setInput('attentionMode', attentionMode);
      await f.whenStable();
      const card = f.nativeElement.querySelector('article') as HTMLElement;
      expect([...card.children].map((element) => element.className || element.tagName)).toEqual([
        'card-meta',
        'card-title',
        'card-preview',
        'linked-items linked-pull-requests',
        'APP-ATTENTION-DETAIL',
        'card-footer',
      ]);
      const meta = card.querySelector('.card-meta')!;
      expect(meta.firstElementChild?.tagName).toBe('PB-LINK');
      expect(meta.lastElementChild?.textContent).toBe(issue().status);
      const preview = card.querySelector('.card-preview')!;
      expect(preview.textContent).toContain('<b>Literal upstream description</b>');
      expect(preview.querySelector('b')).toBeNull();
      const linked = card.querySelector('.linked-items')!;
      expect(linked.querySelector('h4')?.textContent).toBe('Linked pull requests');
      expect(linked.querySelector('a')?.getAttribute('href')).toBe(pr().url);
      expect(linked.querySelector('a')?.textContent).toContain(pr().title);
      expect(linked.querySelector('a')?.textContent).toContain(pr().url);
      expect(linked.querySelector('.card-tag')?.textContent).toBe('Ready for review');
      expect(linked.textContent).toContain('missing:pr');
      expect(linked.querySelectorAll('a')).toHaveLength(1);
      expect(card.classList.contains('needs-attention')).toBe(true);
      expect(card.querySelector('details')).toBeNull();
      expect(card.querySelector('.attention-tag a')?.getAttribute('href')).toBe(
        issue().attention!.threads[0].url,
      );
      expect(card.querySelectorAll('time')).toHaveLength(1);
      expect(card.querySelector('time')?.dateTime).toBe(issue().attention!.waiting_since);
      expect(card.textContent).not.toMatch(/merged|Close Jira/i);
    }
    f.componentRef.setInput('issue', issue({ attention: null }));
    await f.whenStable();
    expect(f.nativeElement.querySelector('article').classList.contains('needs-attention')).toBe(
      false,
    );
  });
  it('keeps source status inline, but leaves coverage information in source health', async () => {
    const store = TestBed.inject(BoardStore);
    store.board.set(
      board({
        sources: [
          {
            ...board().sources[0],
            source_id: 'jira',
            status: 'backoff',
            coverage_notes: ['Neutral coverage information.'],
          },
        ],
      }),
    );
    const f = TestBed.createComponent(IssueCard);
    f.componentRef.setInput('issue', issue({ attention: { ...attention(), coverage: 'partial' } }));
    await f.whenStable();
    expect(f.nativeElement.textContent).toContain('Stale snapshot');
    expect(f.nativeElement.textContent).not.toContain('Partial history');
    expect(f.nativeElement.textContent).not.toContain('Neutral coverage information.');
  });
  it('disables both linking paths for foreign mentions while retaining the card', async () => {
    const f = TestBed.createComponent(IssueCard);
    const link = vi.spyOn(TestBed.inject(Actions), 'link').mockImplementation(() => {});
    const requested = vi.fn();
    f.componentInstance.linkRequested.subscribe(requested);
    const drop = { item: { data: pr() } } as CdkDragDrop<JiraIssue>;
    for (const assignedToMe of [false, true]) {
      f.componentRef.setInput('issue', foreignMention(assignedToMe));
      await f.whenStable();
      const button = f.nativeElement.querySelector('button') as HTMLButtonElement;
      expect(button.disabled).toBe(true);
      expect(button.title).toContain('configured Jira projects');
      expect(
        f.debugElement.query(By.directive(CdkDropList)).injector.get(CdkDropList).disabled,
      ).toBe(true);
      button.click();
      f.componentInstance.drop(drop);
      expect(f.nativeElement.textContent).toContain(foreignMention(assignedToMe).key);
    }
    expect(link).not.toHaveBeenCalled();
    expect(requested).not.toHaveBeenCalled();
    f.componentRef.setInput('issue', issue());
    await f.whenStable();
    expect(f.nativeElement.querySelector('button').disabled).toBe(false);
    expect(f.debugElement.query(By.directive(CdkDropList)).injector.get(CdkDropList).disabled).toBe(
      false,
    );
    f.componentInstance.drop(drop);
    expect(link).toHaveBeenCalledWith('DEV-1', pr().id, runtimeMode(), expect.any(Object));
  });
});
