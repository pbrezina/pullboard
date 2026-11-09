import { Component, computed, input, linkedSignal, model, signal, viewChild } from '@angular/core';
import { CdkVirtualScrollViewport, ScrollingModule } from '@angular/cdk/scrolling';
import type { FormValueControl } from '@angular/forms/signals';
import type { JiraAssignee } from '../../core/api-types';

/** Full cached eligible pool only. Search never changes the committed selection. */
@Component({
  selector: 'app-assignee-picker',
  imports: [ScrollingModule],
  template: `
    <label for="ticket-assignee">Assignee</label>
    <input
      id="ticket-assignee"
      name="assignee"
      role="combobox"
      autocomplete="off"
      spellcheck="false"
      aria-autocomplete="list"
      aria-haspopup="listbox"
      aria-controls="assignee-options"
      aria-describedby="assignee-selection assignee-error"
      [attr.aria-expanded]="open()"
      [attr.aria-activedescendant]="open() ? 'assignee-option-' + active() : null"
      [attr.aria-invalid]="invalid() || !!error()"
      [disabled]="disabled()"
      [value]="open() ? query() : selectedLabel()"
      placeholder="Search name, username or email…"
      (focus)="show()"
      (click)="show()"
      (input)="search($event)"
      (keydown)="keydown($event)"
      (blur)="open.set(false)"
    />
    <span id="assignee-selection" class="muted">Selected: {{ selectedLabel() }}</span>
    <span id="assignee-error" class="error-text" aria-live="polite">{{ error() }}</span>
    @if (open()) {
      <div class="picker-popup">
        <cdk-virtual-scroll-viewport
          role="listbox"
          aria-label="Eligible assignees"
          id="assignee-options"
          [itemSize]="48"
          [minBufferPx]="240"
          [maxBufferPx]="480"
          [style.height.px]="height()"
        >
          <div
            *cdkVirtualFor="let person of choices(); let index = index; trackBy: trackPerson"
            role="option"
            [id]="'assignee-option-' + index"
            [attr.aria-selected]="value() === (person?.id ?? '')"
            [attr.aria-posinset]="index + 1"
            [attr.aria-setsize]="choices().length"
            [class.active]="active() === index"
            class="picker-option"
            (mousedown)="$event.preventDefault()"
            (click)="choose(index)"
          >
            <span>{{ person?.display_name || person?.username || 'Unassigned' }}</span>
            @if (person?.email) {
              <span class="muted">{{ person.email }}</span>
            }
          </div>
        </cdk-virtual-scroll-viewport>
        @if (!matches().length) {
          <p role="status" class="muted">No matching assignees.</p>
        }
        <span class="muted picker-help"
          >{{ matches().length }} matching · ↑ ↓ to navigate, Enter to select</span
        >
      </div>
    }
  `,
  styles: `
    :host {
      display: flex;
      flex-direction: column;
      gap: 0.3rem;
      position: relative;
      min-width: 0;
    }
    .picker-popup {
      position: absolute;
      top: 100%;
      left: 0;
      right: 0;
      z-index: 5;
      background: var(--surface);
      border: 1px solid var(--line);
      border-radius: 0.35rem;
      box-shadow: 0 6px 18px #0003;
    }
    cdk-virtual-scroll-viewport {
      display: block;
      width: 100%;
    }
    .picker-option {
      height: 48px;
      padding: 5px 10px;
      display: flex;
      flex-direction: column;
      justify-content: center;
      cursor: pointer;
      overflow: hidden;
    }
    .picker-option span {
      white-space: nowrap;
      overflow: hidden;
      text-overflow: ellipsis;
    }
    .picker-option.active,
    .picker-option:hover {
      background: var(--soft);
      outline: 2px solid var(--accent);
      outline-offset: -2px;
    }
    .picker-help,
    .picker-popup p {
      display: block;
      padding: 0.4rem 0.6rem;
      margin: 0;
    }
  `,
})
export class AssigneePicker implements FormValueControl<string> {
  readonly value = model('');
  readonly disabled = input(false);
  readonly invalid = input(false);
  readonly error = input('');
  readonly people = input<JiraAssignee[]>([]);
  readonly open = signal(false);
  readonly query = signal('');
  private readonly viewport = viewChild(CdkVirtualScrollViewport);
  readonly matches = computed(() => {
    const query = this.query().trim().toLocaleLowerCase();
    return this.people().filter((p) =>
      [p.display_name, p.username, p.email ?? ''].some((v) =>
        v.toLocaleLowerCase().includes(query),
      ),
    );
  });
  readonly choices = computed(() => [null, ...this.matches()]);
  readonly active = linkedSignal<(JiraAssignee | null)[], number>({
    source: this.choices,
    computation: (choices, previous) => Math.min(previous?.value ?? 0, choices.length - 1),
  });
  readonly height = computed(() => Math.min(6, this.choices().length) * 48);
  readonly selectedLabel = computed(() => {
    const person = this.people().find((p) => p.id === this.value());
    return person
      ? person.display_name || person.username
      : this.value()
        ? 'Selected assignee unavailable'
        : 'Unassigned';
  });
  readonly trackPerson = (_: number, person: JiraAssignee | null) => person?.id ?? '';
  show() {
    if (this.open() || this.disabled()) return;
    this.query.set('');
    this.active.set(0);
    this.open.set(true);
  }
  search(event: Event) {
    this.query.set((event.target as HTMLInputElement).value);
    this.active.set(0);
    this.open.set(true);
    this.viewport()?.scrollToIndex(0);
  }
  choose(index: number) {
    this.value.set(this.choices()[index]?.id ?? '');
    this.open.set(false);
  }
  keydown(event: KeyboardEvent) {
    if (event.key === 'Escape' && this.open()) {
      event.preventDefault();
      event.stopPropagation();
      this.open.set(false);
      return;
    }
    if (!['ArrowDown', 'ArrowUp', 'Enter', 'Home', 'End'].includes(event.key)) return;
    if (['Enter', 'Home', 'End'].includes(event.key) && !this.open()) return;
    event.preventDefault();
    this.show();
    if (event.key === 'Enter') {
      this.choose(this.active());
      return;
    }
    const last = this.choices().length - 1;
    this.active.set(
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? last
          : Math.max(0, Math.min(last, this.active() + (event.key === 'ArrowDown' ? 1 : -1))),
    );
    this.viewport()?.scrollToIndex(Math.max(0, this.active() - 2));
  }
}
