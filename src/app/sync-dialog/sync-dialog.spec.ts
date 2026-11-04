import { ComponentFixture, TestBed } from '@angular/core/testing';
import { SyncDialog, SyncDialogData } from './sync-dialog';
import { MAT_DIALOG_DATA, MatDialogRef } from '@angular/material/dialog';
import { vi } from 'vitest';

describe('SyncDialog', () => {
  let component: SyncDialog;
  let fixture: ComponentFixture<SyncDialog>;

  beforeEach(async () => {
    await TestBed.configureTestingModule({
      imports: [SyncDialog],
      providers: [
        {
          provide: MAT_DIALOG_DATA,
          useValue: {
            suggestion: { title: 'Track work', description: 'Scope', storyPoints: 3 },
            draft: false,
            generated: true,
            sprints: [
              { id: 2, name: 'Current sprint', state: 'active' },
              { id: 3, name: 'Next sprint', state: 'future' },
            ],
            defaultSprintId: 2,
          },
        },
        { provide: MatDialogRef, useValue: { close: vi.fn() } },
      ],
    }).compileComponents();

    fixture = TestBed.createComponent(SyncDialog);
    component = fixture.componentInstance;
    await fixture.whenStable();
  });

  it('should create', () => {
    expect(component).toBeTruthy();
    const checkbox: HTMLInputElement = fixture.nativeElement.querySelector('input[type=checkbox]');
    const select: HTMLSelectElement = fixture.nativeElement.querySelector('select');
    expect(checkbox.checked).toBe(false);
    expect(select.value).toBe('2');
    expect(select.disabled).toBe(true);
  });

  it('includes the selected future sprint only after opting in', async () => {
    const checkbox: HTMLInputElement = fixture.nativeElement.querySelector('input[type=checkbox]');
    checkbox.click();
    await fixture.whenStable();
    const select: HTMLSelectElement = fixture.nativeElement.querySelector('select');
    expect(select.disabled).toBe(false);
    select.value = '3';
    select.dispatchEvent(new Event('input'));
    select.dispatchEvent(new Event('change'));
    await fixture.whenStable();
    fixture.nativeElement
      .querySelector('form')
      .dispatchEvent(new Event('submit', { cancelable: true }));
    await fixture.whenStable();
    expect(TestBed.inject(MatDialogRef).close).toHaveBeenCalledWith({
      title: 'Track work',
      description: 'Scope',
      storyPoints: 3,
      sprintId: 3,
    });
  });

  it('renders editable signal-form fields and confirms reviewed values', async () => {
    const input: HTMLInputElement = fixture.nativeElement.querySelector('input');
    input.value = 'Reviewed title';
    input.dispatchEvent(new Event('input'));
    await fixture.whenStable();
    const form: HTMLFormElement = fixture.nativeElement.querySelector('form');
    form.dispatchEvent(new Event('submit', { cancelable: true }));
    await fixture.whenStable();
    expect(TestBed.inject(MatDialogRef).close).toHaveBeenCalledWith({
      title: 'Reviewed title',
      description: 'Scope',
      storyPoints: 3,
    });
  });

  it('omits sprint assignment after opting back out', async () => {
    const checkbox: HTMLInputElement = fixture.nativeElement.querySelector('input[type=checkbox]');
    checkbox.click();
    await fixture.whenStable();
    checkbox.click();
    await fixture.whenStable();
    fixture.nativeElement
      .querySelector('form')
      .dispatchEvent(new Event('submit', { cancelable: true }));
    await fixture.whenStable();
    expect(TestBed.inject(MatDialogRef).close).toHaveBeenCalledWith({
      title: 'Track work',
      description: 'Scope',
      storyPoints: 3,
    });
  });

  it('requires a valid sprint only when opted in', async () => {
    const checkbox: HTMLInputElement = fixture.nativeElement.querySelector('input[type=checkbox]');
    checkbox.click();
    await fixture.whenStable();
    const select: HTMLSelectElement = fixture.nativeElement.querySelector('select');
    select.value = '';
    select.dispatchEvent(new Event('input'));
    select.dispatchEvent(new Event('change'));
    await fixture.whenStable();
    expect(
      (fixture.nativeElement.querySelector('button[type=submit]') as HTMLButtonElement).disabled,
    ).toBe(true);
    fixture.nativeElement
      .querySelector('form')
      .dispatchEvent(new Event('submit', { cancelable: true }));
    await fixture.whenStable();
    expect(TestBed.inject(MatDialogRef).close).not.toHaveBeenCalled();
  });
});

describe('Review ticket confirmation', () => {
  const prefix = 'Review PR org/repo#42';
  let fixture: ComponentFixture<SyncDialog>;
  beforeEach(async () => {
    TestBed.configureTestingModule({
      providers: [
        {
          provide: MAT_DIALOG_DATA,
          useValue: {
            suggestion: {
              title: 'Inspect cleanup',
              description: 'Review shutdown paths',
              storyPoints: 2,
            },
            generated: true,
            draft: false,
            sprints: [],
            titlePrefix: prefix,
            previousTickets: [{ key: 'DEV-12', url: 'https://team.atlassian.net/browse/DEV-12' }],
          } satisfies SyncDialogData,
        },
        { provide: MatDialogRef, useValue: { close: vi.fn() } },
      ],
    });
    fixture = TestBed.createComponent(SyncDialog);
    await fixture.whenStable();
  });

  it('fixes the prefix outside the editable suffix and discloses additional creation and destination', async () => {
    const element: HTMLElement = fixture.nativeElement;
    expect(element.textContent).toContain(prefix);
    expect(element.textContent).toContain('additional review ticket');
    expect(element.textContent).toContain('Unassigned');
    expect(element.textContent).toContain('JIRA default');
    expect(element.querySelector('a')?.textContent).toBe('DEV-12');
    const title = element.querySelector('input')!;
    expect(title.value).toBe('Inspect cleanup');
    title.value = 'Edited summary';
    title.dispatchEvent(new Event('input'));
    await fixture.whenStable();
    element.querySelector('form')!.dispatchEvent(new Event('submit', { cancelable: true }));
    await fixture.whenStable();
    expect(TestBed.inject(MatDialogRef).close).toHaveBeenCalledWith({
      title: `${prefix}: Edited summary`,
      description: 'Review shutdown paths',
      storyPoints: 2,
    });
  });

  it('counts the prefix toward the 255-character title limit', async () => {
    const title = fixture.nativeElement.querySelector('input') as HTMLInputElement;
    title.value = 'x'.repeat(255 - prefix.length - 1);
    title.dispatchEvent(new Event('input'));
    await fixture.whenStable();
    expect(fixture.nativeElement.querySelector('button[type=submit]').disabled).toBe(true);
    title.value = 'x'.repeat(255 - prefix.length - 2);
    title.dispatchEvent(new Event('input'));
    await fixture.whenStable();
    expect(fixture.nativeElement.querySelector('button[type=submit]').disabled).toBe(false);
  });
});
