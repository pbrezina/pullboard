import { TestBed } from '@angular/core/testing';
import { CdkDrag, CdkDragHandle } from '@angular/cdk/drag-drop';
import { By } from '@angular/platform-browser';
import { PrCard } from './pr-card';
import type { PullRequest, Purpose } from '../core/api-types';
import { BoardStore } from '../core/board-store';
import { RuntimeMode } from '../core/runtime-mode';
import { attention, board, context, issue, person, pr, runtimeMode } from '../testing/fixtures';
describe('PR card', () => {
  it('uses source identity to exclude self, shows draft state and supports non-drag linking', async () => {
    TestBed.inject(BoardStore).context.set(context());
    const f = TestBed.createComponent(PrCard);
    f.componentRef.setInput(
      'pr',
      pr({ state: 'draft', reviewers: [person, { ...person, id: 'b', username: 'bob' }] }),
    );
    await f.whenStable();
    expect(f.componentInstance.reviewers().map((p) => p.username)).toEqual(['bob']);
    expect(f.nativeElement.textContent).toContain('Draft');
    expect(f.nativeElement.textContent).toContain('github');
    expect(f.nativeElement.textContent).toContain('Link to Jira');
    expect(
      f.nativeElement.querySelector('a[href="https://forge.example/acme/widget"]'),
    ).not.toBeNull();
    expect(f.nativeElement.querySelector('.source-id')).toBeNull();
    expect(f.nativeElement.querySelector('.repository-tag')?.textContent).toContain('acme/widget');
    expect(f.nativeElement.querySelector('.pr-number')?.textContent).toContain('#42');
    expect(f.nativeElement.querySelector('.card-people dt')).not.toBeNull();
  });
  it('captures mode at gesture initiation and retains it through CDK ended-before-drop ordering', async () => {
    const f = TestBed.createComponent(PrCard);
    f.componentRef.setInput('pr', pr());
    const mode = TestBed.inject(RuntimeMode);
    mode.apply(runtimeMode());
    f.componentInstance.prepareDrag();
    mode.apply(
      runtimeMode({ read_only_test_mode: false, mode_revision: 'live-after-pointer-down' }),
    );
    f.componentInstance.beginDrag();
    expect(mode.dragMode).toEqual(runtimeMode());
    f.componentInstance.endDrag();
    expect(mode.dragMode).toEqual(runtimeMode());
    await f.whenStable();
    expect(mode.dragMode).toBeNull();
  });
  it.each([
    {
      attentionMode: false,
      isMine: true,
      labels: ['Link to Jira', 'Create review ticket', 'Create Jira ticket'],
    },
    { attentionMode: false, isMine: false, labels: ['Link to Jira', 'Create Jira ticket'] },
    { attentionMode: true, isMine: true, labels: ['Link to Jira', 'Create review ticket'] },
    { attentionMode: true, isMine: false, labels: ['Link to Jira', 'Create review ticket'] },
  ])(
    'shows the intended actions for attention=$attentionMode, own PR=$isMine',
    async ({ attentionMode, isMine, labels }) => {
      const f = TestBed.createComponent(PrCard);
      f.componentRef.setInput('attentionMode', attentionMode);
      for (const upstreamAttention of [null, attention()]) {
        f.componentRef.setInput('pr', pr({ is_mine: isMine, attention: upstreamAttention }));
        await f.whenStable();
        const buttons = [...f.nativeElement.querySelectorAll('button')] as HTMLButtonElement[];
        expect(buttons.map((button) => button.textContent?.trim())).toEqual(labels);
      }
    },
  );
  it.each([true, false])(
    'emits explicit creation purposes with attention=%s',
    async (attentionMode) => {
      TestBed.inject(BoardStore).context.set(context());
      TestBed.inject(RuntimeMode).apply(runtimeMode());
      const f = TestBed.createComponent(PrCard);
      const selected = pr();
      f.componentRef.setInput('pr', selected);
      f.componentRef.setInput('attentionMode', attentionMode);
      const requests: { pr: PullRequest; purpose: Purpose }[] = [];
      f.componentInstance.createRequested.subscribe((request) => requests.push(request));
      await f.whenStable();
      const buttons = [...f.nativeElement.querySelectorAll('button')] as HTMLButtonElement[];
      buttons.find((button) => button.textContent?.trim() === 'Create review ticket')!.click();
      await f.whenStable();
      expect(requests).toEqual([{ pr: selected, purpose: 'review' }]);
      if (!attentionMode) {
        buttons.find((button) => button.textContent?.trim() === 'Create Jira ticket')!.click();
        await f.whenStable();
        expect(requests).toEqual([
          { pr: selected, purpose: 'review' },
          { pr: selected, purpose: 'implementation' },
        ]);
      }
    },
  );
  it.each([false, true])(
    'applies creation and runtime disabled gates with attention=%s',
    async (attentionMode) => {
      const store = TestBed.inject(BoardStore);
      const mode = TestBed.inject(RuntimeMode);
      const f = TestBed.createComponent(PrCard);
      f.componentRef.setInput('pr', pr());
      f.componentRef.setInput('attentionMode', attentionMode);
      const requests: { pr: PullRequest; purpose: Purpose }[] = [];
      f.componentInstance.createRequested.subscribe((request) => requests.push(request));
      const cases = [
        {
          hasContext: false,
          hasProjects: false,
          hasMode: true,
          error: '',
          disabled: true,
          linkDisabled: false,
        },
        {
          hasContext: true,
          hasProjects: false,
          hasMode: true,
          error: '',
          disabled: true,
          linkDisabled: false,
        },
        {
          hasContext: true,
          hasProjects: true,
          hasMode: false,
          error: '',
          disabled: true,
          linkDisabled: true,
        },
        {
          hasContext: true,
          hasProjects: true,
          hasMode: true,
          error: 'Mode unavailable',
          disabled: true,
          linkDisabled: true,
        },
        {
          hasContext: true,
          hasProjects: true,
          hasMode: true,
          error: '',
          disabled: false,
          linkDisabled: false,
        },
      ];
      for (const state of cases) {
        store.context.set(
          state.hasContext
            ? context({ jira_projects: state.hasProjects ? context().jira_projects : [] })
            : null,
        );
        mode.value.set(state.hasMode ? runtimeMode() : null);
        mode.error.set(state.error);
        await f.whenStable();
        const buttons = [...f.nativeElement.querySelectorAll('button')] as HTMLButtonElement[];
        const creation = buttons.filter((button) =>
          button.textContent?.trim().startsWith('Create'),
        );
        expect(creation).toHaveLength(attentionMode ? 1 : 2);
        for (const button of creation) {
          expect(button.disabled).toBe(state.disabled);
          if (state.disabled) button.click();
        }
        expect(buttons[0].disabled).toBe(state.linkDisabled);
        expect(f.debugElement.query(By.directive(CdkDrag)).injector.get(CdkDrag).disabled).toBe(
          state.linkDisabled,
        );
        expect(requests).toEqual([]);
      }
      mode.apply(runtimeMode({ read_only_test_mode: false, mode_revision: 'live-mode' }));
      await f.whenStable();
      const buttons = [...f.nativeElement.querySelectorAll('button')] as HTMLButtonElement[];
      expect(buttons.every((button) => !button.disabled)).toBe(true);
    },
  );
  it.each([false, true])(
    'places one CDK handle left of the title and preserves keyboard linking with attention=%s',
    async (attentionMode) => {
      const mode = TestBed.inject(RuntimeMode);
      mode.apply(runtimeMode());
      const f = TestBed.createComponent(PrCard);
      const selected = pr();
      f.componentRef.setInput('pr', selected);
      f.componentRef.setInput('attentionMode', attentionMode);
      const links: PullRequest[] = [];
      f.componentInstance.linkRequested.subscribe((request) => links.push(request));
      await f.whenStable();
      const card = f.nativeElement.querySelector('article') as HTMLElement;
      const title = card.querySelector('h3')!;
      const handles = f.debugElement.queryAll(By.directive(CdkDragHandle));
      expect(card.firstElementChild).toBe(title);
      expect(handles).toHaveLength(1);
      const handle = handles[0].nativeElement as HTMLElement;
      expect(title.firstElementChild).toBe(handle);
      expect(handle.nextElementSibling?.tagName).toBe('PB-LINK');
      expect(handle.nextElementSibling?.textContent).toBe(selected.title);
      expect(handle.getAttribute('aria-hidden')).toBe('true');
      expect(card.querySelector('.card-meta .drag-handle')).toBeNull();
      handle.dispatchEvent(new Event('pointerdown', { bubbles: true }));
      expect(mode.dragMode).toEqual(runtimeMode());
      const link = card.querySelector('button')!;
      expect(link.textContent?.trim()).toBe('Link to Jira');
      expect(link.tabIndex).toBe(0);
      link.focus();
      expect(document.activeElement).toBe(link);
      link.click();
      await f.whenStable();
      expect(links).toEqual([selected]);
    },
  );
  it('keeps the inbox information order, people and real Jira links visible in both tabs', async () => {
    TestBed.inject(BoardStore).context.set(context());
    TestBed.inject(BoardStore).board.set(board());
    const f = TestBed.createComponent(PrCard);
    f.componentRef.setInput(
      'pr',
      pr({
        reviewers: [
          { ...person, id: 'zoe', username: 'zoe' },
          person,
          { ...person, id: 'bob', username: 'bob' },
        ],
        assignees: [person, { ...person, id: 'assigned', username: 'pat' }],
        linked_issue_keys: ['DEV-1', 'DEV-2'],
      }),
    );
    for (const attentionMode of [false, true]) {
      f.componentRef.setInput('attentionMode', attentionMode);
      await f.whenStable();
      const card = f.nativeElement.querySelector('article') as HTMLElement;
      expect([...card.children].map((element) => element.className || element.tagName)).toEqual([
        'card-title',
        'card-meta pr-meta',
        'card-tags',
        'APP-ATTENTION-DETAIL',
        'card-preview',
        'card-people',
        'linked-items',
        'card-footer',
      ]);
      expect(card.classList.contains('needs-attention')).toBe(true);
      expect(card.querySelector('details')).toBeNull();
      expect(card.querySelector('.card-preview')?.textContent).toContain(pr().description);
      expect(card.querySelector('.card-meta')?.textContent).toContain('Created');
      expect(card.querySelector('.card-meta')?.textContent).toContain('Last activity');
      expect(card.querySelector('.card-meta')?.textContent).toContain('Waiting since');
      expect(f.componentInstance.reviewers().map((p) => p.username)).toEqual(['bob', 'zoe']);
      expect(card.querySelector('.card-people')?.textContent).toContain('pat');
      const linked = card.querySelector('.linked-items')!;
      expect(linked.querySelector('a')?.getAttribute('href')).toBe(issue().url);
      expect(linked.querySelector('a')?.textContent).toBe('DEV-1');
      expect(linked.textContent).toContain('DEV-2');
      expect(linked.querySelectorAll('a')).toHaveLength(1);
      expect(card.querySelectorAll('.card-tags a, button .card-tag')).toHaveLength(0);
      expect(card.querySelector('.card-tags')?.textContent).toContain('Ready for review');
      expect(card.querySelector('.attention-tag a')?.getAttribute('href')).toBe(
        attention().threads[0].url,
      );
      expect(card.querySelector('app-attention-detail time')).toBeNull();
      expect(card.querySelector('.card-secondary-action')?.textContent).toContain('Link to Jira');
      expect(card.querySelector('.card-primary-action')?.textContent).toContain(
        attentionMode ? 'Create review ticket' : 'Create Jira ticket',
      );
    }
  });
  it('does not treat the selected tab or a newer commit as attention truth', async () => {
    const f = TestBed.createComponent(PrCard);
    const waiting = attention('2026-10-01T00:00:00Z');
    f.componentRef.setInput(
      'pr',
      pr({ attention: waiting, last_commit_at: '2026-11-09T00:00:00Z' }),
    );
    await f.whenStable();
    expect(f.nativeElement.querySelector('article').classList.contains('needs-attention')).toBe(
      true,
    );
    expect(
      f.nativeElement.querySelector('.card-meta time[datetime="2026-10-01T00:00:00Z"]'),
    ).not.toBeNull();
    f.componentRef.setInput('pr', pr({ attention: null }));
    f.componentRef.setInput('attentionMode', true);
    await f.whenStable();
    expect(f.nativeElement.querySelector('article').classList.contains('needs-attention')).toBe(
      false,
    );
    expect(f.nativeElement.querySelector('app-attention-detail')).toBeNull();
    expect(f.nativeElement.querySelector('.card-meta').textContent).not.toContain('Waiting since');
  });
  it('keeps labels, people and stale status without showing coverage warnings or notes', async () => {
    const store = TestBed.inject(BoardStore);
    store.board.set(
      board({
        sources: [
          {
            ...board().sources[0],
            status: 'backoff',
            coverage_notes: ['Older review history is unavailable.'],
          },
        ],
      }),
    );
    const f = TestBed.createComponent(PrCard);
    const long = 'long-reviewer-'.repeat(30);
    f.componentRef.setInput(
      'pr',
      pr({
        labels: ['long-label-'.repeat(30)],
        reviewers: [{ ...person, username: long }],
        attention: { ...attention(), coverage: 'partial' },
      }),
    );
    await f.whenStable();
    expect(f.nativeElement.querySelector('.card-people').textContent).toContain(long);
    expect(f.nativeElement.querySelector('.card-tags').textContent).toContain(
      'long-label-'.repeat(30),
    );
    expect(f.nativeElement.textContent).toContain('Stale snapshot');
    expect(f.nativeElement.textContent).not.toContain('Partial history');
    expect(f.nativeElement.textContent).not.toContain('Older review history is unavailable.');
  });
  it('distinguishes the actual accepted label without changing its wording or treating other labels as accepted', async () => {
    const f = TestBed.createComponent(PrCard);
    f.componentRef.setInput('pr', pr({ labels: ['Accepted', 'backend'] }));
    await f.whenStable();
    const labels = f.nativeElement.querySelectorAll('.label-tag');
    expect(labels[0].textContent).toBe('Accepted');
    expect(labels[0].classList.contains('accepted')).toBe(true);
    expect(labels[1].classList.contains('accepted')).toBe(false);
  });
});
