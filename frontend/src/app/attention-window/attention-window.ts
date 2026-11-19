import { Component, computed, input, linkedSignal, output } from '@angular/core';
import {
  attentionPresets,
  parseAttentionDays,
  type AttentionPreset,
  type AttentionWindow,
} from '../core/attention-window';

@Component({
  selector: 'app-attention-window',
  templateUrl: './attention-window.html',
  styleUrl: './attention-window.css',
})
export class AttentionWindowControl {
  readonly selection = input.required<AttentionWindow>();
  readonly defaultDays = input.required<number>();
  readonly selectionChange = output<AttentionWindow>();
  readonly presets = attentionPresets;
  readonly effective = computed(() =>
    this.selection() === 'default' ? this.defaultDays() : this.selection(),
  );
  readonly customSelected = computed(() => String(this.selection()).startsWith('custom:'));
  readonly unlistedDefault = computed(
    () => this.selection() === 'default' && !this.presets.some((p) => p === this.defaultDays()),
  );
  readonly draft = linkedSignal(() =>
    this.customSelected() ? this.selection().toString().slice(7) : '',
  );
  readonly invalid = linkedSignal({ source: this.selection, computation: () => false });
  private edited = false;
  private presetPointer = false;

  edit(value: string) {
    this.edited = true;
    this.draft.set(value);
    this.invalid.set(false);
  }

  commit(event: Event) {
    if (event.type === 'blur' && this.presetPointer) return;
    if (event instanceof KeyboardEvent) event.preventDefault();
    if (!this.edited) return;
    const days = parseAttentionDays(this.draft().trim());
    this.invalid.set(days === null);
    if (days === null) return;
    this.edited = false;
    this.draft.set(String(days));
    this.selectionChange.emit(`custom:${days}`);
  }

  clearDraft() {
    this.edited = false;
    this.presetPointer = false;
    this.draft.set('');
    this.invalid.set(false);
  }

  select(preset: AttentionPreset) {
    this.clearDraft();
    this.selectionChange.emit(preset);
  }

  setPresetPointer(active: boolean) {
    this.presetPointer = active;
  }

  restoreDefault(event: Event) {
    event.preventDefault();
    this.clearDraft();
    this.selectionChange.emit('default');
  }
}
