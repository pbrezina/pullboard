import { TestBed } from '@angular/core/testing';
import { DIALOG_DATA, DialogRef } from '@angular/cdk/dialog';
import { vi } from 'vitest';
import { LinkDialog } from './link-dialog';
import { BoardStore } from '../core/board-store';
import { Actions } from '../core/actions';
import { RuntimeMode } from '../core/runtime-mode';
import { HttpErrorResponse } from '@angular/common/http';
import { board, context, foreignMention, issue, pr, runtimeMode } from '../testing/fixtures';
describe('Accessible linking alternative', () => {
  it('chooses from cached issues and submits the selected pair only after confirmation', async () => {
    const close = vi.fn();
    TestBed.configureTestingModule({
      providers: [
        { provide: DIALOG_DATA, useValue: { prId: pr().id, mode: runtimeMode() } },
        { provide: DialogRef, useValue: { close } },
      ],
    });
    TestBed.inject(BoardStore).board.set(board());
    TestBed.inject(BoardStore).context.set(context());
    TestBed.inject(RuntimeMode).value.set(runtimeMode());
    const link = vi
      .spyOn(TestBed.inject(Actions), 'link')
      .mockImplementation((_i, _p, _m, callbacks) => callbacks?.success());
    const f = TestBed.createComponent(LinkDialog);
    await f.whenStable();
    expect(link).not.toHaveBeenCalled();
    expect(f.nativeElement.textContent).toContain('Simulate link');
    f.componentInstance.model.update((m) => ({ ...m, issue: 'DEV-1' }));
    f.componentInstance.link(new Event('submit'));
    expect(link).toHaveBeenCalledWith('DEV-1', pr().id, runtimeMode(), expect.any(Object));
    expect(close).toHaveBeenCalledOnce();
  });
  it('keeps selections open after a server mode conflict without silently replacing its token', async () => {
    const close = vi.fn();
    TestBed.configureTestingModule({
      providers: [
        { provide: DIALOG_DATA, useValue: { prId: pr().id, mode: runtimeMode() } },
        { provide: DialogRef, useValue: { close } },
      ],
    });
    TestBed.inject(BoardStore).board.set(board());
    TestBed.inject(BoardStore).context.set(context());
    TestBed.inject(RuntimeMode).value.set(runtimeMode());
    const refresh = vi.spyOn(TestBed.inject(RuntimeMode), 'refresh').mockImplementation(() => {});
    const link = vi
      .spyOn(TestBed.inject(Actions), 'link')
      .mockImplementation((_i, _p, _m, callbacks) =>
        callbacks?.error(
          new HttpErrorResponse({
            status: 409,
            error: { error: { code: 'mode_conflict', message: 'Review changed mode.' } },
          }),
        ),
      );
    const f = TestBed.createComponent(LinkDialog);
    f.componentInstance.model.update((m) => ({ ...m, issue: 'DEV-1', query: 'Keep search' }));
    f.componentInstance.link(new Event('submit'));
    await f.whenStable();
    expect(f.componentInstance.model()).toEqual({
      issue: 'DEV-1',
      pr: pr().id,
      query: 'Keep search',
    });
    expect(f.componentInstance.review.stale()).toBe(true);
    expect(f.componentInstance.review.captured()).toEqual(runtimeMode());
    expect(refresh).toHaveBeenCalledOnce();
    f.componentInstance.link(new Event('submit'));
    expect(link).toHaveBeenCalledOnce();
    expect(close).not.toHaveBeenCalled();
  });
  it('does not strand a dialog busy when the same link is already in flight', async () => {
    TestBed.configureTestingModule({
      providers: [
        { provide: DIALOG_DATA, useValue: { prId: pr().id, mode: runtimeMode() } },
        { provide: DialogRef, useValue: { close: vi.fn() } },
      ],
    });
    TestBed.inject(BoardStore).board.set(board());
    TestBed.inject(BoardStore).context.set(context());
    TestBed.inject(RuntimeMode).value.set(runtimeMode());
    const actions = TestBed.inject(Actions);
    actions.linking.set(new Set([JSON.stringify(['DEV-1', pr().id])]));
    const link = vi.spyOn(actions, 'link').mockImplementation(() => {});
    const f = TestBed.createComponent(LinkDialog);
    f.componentInstance.model.update((m) => ({ ...m, issue: 'DEV-1' }));
    f.componentInstance.link(new Event('submit'));
    expect(link).not.toHaveBeenCalled();
    expect(f.componentInstance.busy()).toBe(false);
    actions.linking.set(new Set());
    await f.whenStable();
    expect(f.componentInstance.canLink()).toBe(true);
  });
  it('only offers configured-project targets and refuses foreign mentions on submission', async () => {
    const close = vi.fn();
    TestBed.configureTestingModule({
      providers: [
        { provide: DIALOG_DATA, useValue: { prId: pr().id, mode: runtimeMode() } },
        { provide: DialogRef, useValue: { close } },
      ],
    });
    const store = TestBed.inject(BoardStore);
    store.board.set(board({ issues: [issue(), foreignMention(), foreignMention(true)] }));
    store.context.set(context());
    TestBed.inject(RuntimeMode).value.set(runtimeMode());
    const link = vi.spyOn(TestBed.inject(Actions), 'link').mockImplementation(() => {});
    const f = TestBed.createComponent(LinkDialog);
    await f.whenStable();
    expect(f.componentInstance.issues().map((i) => i.key)).toEqual(['DEV-1']);
    expect(f.nativeElement.querySelector('#link-issue').textContent).not.toContain('FOREIGN-');
    for (const assignedToMe of [false, true]) {
      f.componentInstance.model.update((m) => ({ ...m, issue: foreignMention(assignedToMe).key }));
      await f.whenStable();
      expect(f.nativeElement.querySelector('button[type="submit"]').disabled).toBe(true);
      f.componentInstance.link(new Event('submit'));
    }
    expect(link).not.toHaveBeenCalled();
    expect(close).not.toHaveBeenCalled();
    f.componentInstance.model.update((m) => ({ ...m, issue: 'DEV-1' }));
    await f.whenStable();
    expect(f.nativeElement.querySelector('button[type="submit"]').disabled).toBe(false);
    store.context.set(context({ jira_projects: [] }));
    await f.whenStable();
    expect(f.componentInstance.issues()).toEqual([]);
    f.componentInstance.link(new Event('submit'));
    expect(link).not.toHaveBeenCalled();
  });
  it('refuses a foreign issue supplied as the fixed dialog target', async () => {
    const close = vi.fn();
    const foreign = foreignMention(true);
    TestBed.configureTestingModule({
      providers: [
        { provide: DIALOG_DATA, useValue: { issueKey: foreign.key, mode: runtimeMode() } },
        { provide: DialogRef, useValue: { close } },
      ],
    });
    const store = TestBed.inject(BoardStore);
    store.board.set(board({ issues: [foreign] }));
    store.context.set(context());
    TestBed.inject(RuntimeMode).value.set(runtimeMode());
    const link = vi.spyOn(TestBed.inject(Actions), 'link').mockImplementation(() => {});
    const f = TestBed.createComponent(LinkDialog);
    f.componentInstance.model.update((m) => ({ ...m, pr: pr().id }));
    await f.whenStable();
    expect(f.nativeElement.querySelector('button[type="submit"]').disabled).toBe(true);
    f.componentInstance.link(new Event('submit'));
    expect(link).not.toHaveBeenCalled();
    expect(close).not.toHaveBeenCalled();
  });
});
