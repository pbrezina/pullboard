import { Component, inject, signal } from '@angular/core';
import {
  FormField,
  form,
  required,
  maxLength,
  min,
  max,
  validate,
  submit,
  disabled,
} from '@angular/forms/signals';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatButtonModule } from '@angular/material/button';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { JiraSprint, SyncReceipt, TicketCreation, TicketSuggestion } from '../core/models';
import { TEST_MODE } from '../core/test-mode';

export interface SyncDialogData {
  suggestion: TicketSuggestion;
  draft: boolean;
  generated: boolean;
  sprints: JiraSprint[];
  defaultSprintId?: number;
  /** Review tickets edit only the suffix; the application owns this prefix. */
  titlePrefix?: string;
  previousTickets?: SyncReceipt[];
}

@Component({
  imports: [FormField, MatDialogModule, MatButtonModule, MatFormFieldModule, MatInputModule],
  selector: 'app-sync-dialog',
  styleUrl: './sync-dialog.scss',
  templateUrl: './sync-dialog.html',
})
export class SyncDialog {
  private readonly dialog = inject(MatDialogRef<SyncDialog, TicketCreation>);
  protected readonly data = inject<SyncDialogData>(MAT_DIALOG_DATA);
  protected readonly testMode = inject(TEST_MODE);
  protected readonly model = signal({
    ...this.data.suggestion,
    addToSprint: false,
    sprintId: this.data.defaultSprintId?.toString() ?? '',
  });
  protected readonly fields = form(this.model, (path) => {
    required(path.title);
    maxLength(path.title, 255 - (this.data.titlePrefix ? this.data.titlePrefix.length + 2 : 0));
    validate(path.title, ({ value }) =>
      value().trim() ? undefined : { kind: 'blank', message: 'Enter a title.' },
    );
    required(path.description);
    min(path.storyPoints, 0);
    max(path.storyPoints, 100);
    validate(path.storyPoints, ({ value }) =>
      Number.isFinite(value())
        ? undefined
        : { kind: 'number', message: 'Enter a finite estimate.' },
    );
    disabled(path.addToSprint, () => !this.data.sprints.length);
    disabled(path.sprintId, ({ valueOf }) => !valueOf(path.addToSprint));
    validate(path.sprintId, ({ value, valueOf }) =>
      !valueOf(path.addToSprint) ||
      this.data.sprints.some((sprint) => String(sprint.id) === value())
        ? undefined
        : { kind: 'sprint', message: 'Select an active or future sprint.' },
    );
  });

  protected confirm(event: Event): void {
    event.preventDefault();
    void submit(this.fields, async () => {
      const { addToSprint, sprintId, ...suggestion } = this.model();
      this.dialog.close({
        ...suggestion,
        title: this.data.titlePrefix
          ? `${this.data.titlePrefix}: ${suggestion.title.trim()}`
          : suggestion.title,
        ...(addToSprint ? { sprintId: Number(sprintId) } : {}),
      });
    });
  }
}
