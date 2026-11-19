import { Component, computed, input, linkedSignal, output } from '@angular/core';
import {
  attentionPresets,
  attentionWindowDays,
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
  readonly retentionDays = input.required<number>();
  readonly selectionChange = output<AttentionWindow>();
  readonly presets = attentionPresets;
  readonly effective = computed(() =>
    this.selection() === 'default' ? this.defaultDays() : this.selection(),
  );
  readonly allRetained = computed(() => this.effective() === 'all');
  private readonly editState = linkedSignal({
    source: this.selection,
    computation: (): { value: string | null; invalid: boolean } => ({
      value: null,
      invalid: false,
    }),
  });
  readonly draft = computed(() =>
    String(
      this.editState().value ??
        attentionWindowDays(this.selection(), this.defaultDays()) ??
        this.retentionDays(),
    ),
  );
  readonly invalid = computed(() => this.editState().invalid);
  readonly fieldSelected = computed(
    () => !this.presets.some((preset) => preset === this.effective()),
  );
  private presetPointer = false;

  edit(value: string) {
    this.editState.set({ value, invalid: false });
  }

  commit(event: Event) {
    if (event.type === 'blur' && this.presetPointer) return;
    if (event instanceof KeyboardEvent) event.preventDefault();
    const { value } = this.editState();
    if (value === null) return;
    const text = value.trim();
    if (!text) {
      this.restoreDefault();
      return;
    }
    const days = parseAttentionDays(text);
    if (days === null) {
      this.editState.set({ value, invalid: true });
      return;
    }
    this.clearDraft();
    this.selectionChange.emit(`custom:${days}`);
  }

  clearDraft() {
    this.presetPointer = false;
    this.editState.set({ value: null, invalid: false });
  }

  select(preset: AttentionPreset) {
    this.clearDraft();
    this.selectionChange.emit(preset);
  }

  setPresetPointer(active: boolean) {
    this.presetPointer = active;
  }

  restoreDefault(event?: Event) {
    event?.preventDefault();
    this.clearDraft();
    this.selectionChange.emit('default');
  }
}
