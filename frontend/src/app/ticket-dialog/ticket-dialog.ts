import { Component, computed, DestroyRef, effect, inject, signal, untracked } from '@angular/core';
import { HttpErrorResponse } from '@angular/common/http';
import { DIALOG_DATA, DialogRef } from '@angular/cdk/dialog';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import {
  disabled,
  form,
  FormField,
  maxLength,
  required,
  submit,
  validate,
} from '@angular/forms/signals';
import { firstValueFrom, Subscription, type Observable } from 'rxjs';
import { Api, errorMessage, isModeConflict } from '../core/api';
import { ActionModeReview } from '../core/mode-confirm';
import { Actions } from '../core/actions';
import { BoardStore } from '../core/board-store';
import type {
  CreateIssueRequest,
  FieldCapability,
  JiraOptions,
  PullRequest,
  Purpose,
  TicketDraft,
  RuntimeMode,
  UpstreamIssue,
  UpstreamIssueDraft,
  CreateUpstreamIssueRequest,
} from '../core/api-types';
import { ExternalLink } from '../ui/external-link/external-link';
import { AssigneePicker } from '../ui/assignee-picker/assignee-picker';

export interface PrTicketDialogData {
  kind: 'pr';
  pr: PullRequest;
  purpose: Purpose;
  mode: RuntimeMode;
}
export interface UpstreamTicketDialogData {
  kind: 'upstream';
  issue: UpstreamIssue;
  warning: string | null;
  purpose: 'implementation';
  mode: RuntimeMode;
}
export type TicketDialogData = PrTicketDialogData | UpstreamTicketDialogData;
export interface TicketEditor {
  project: string;
  issueType: string;
  title: string;
  description: string;
  points: string;
  assignee: string;
  sprint: string;
  team: string;
  status: string;
}
export function createPayload(
  editor: TicketEditor,
  data: TicketDialogData,
): CreateIssueRequest | CreateUpstreamIssueRequest {
  const fields = {
    expected_mode_revision: data.mode.mode_revision,
    project_key: editor.project,
    issue_type: editor.issueType,
    title: data.kind === 'upstream' ? editor.title : editor.title.trim(),
    description: editor.description,
    story_points: editor.points.trim() ? Number(editor.points) : null,
    assignee_id: editor.assignee || null,
    sprint_id: editor.sprint || null,
    team: editor.team || null,
    status: editor.status || null,
  };
  return data.kind === 'upstream'
    ? { ...fields, upstream_issue_id: data.issue.id, purpose: 'implementation' }
    : { ...fields, pull_request_id: data.pr.id, purpose: data.purpose };
}

@Component({
  imports: [FormField, ExternalLink, AssigneePicker],
  selector: 'app-ticket-dialog',
  styleUrl: './ticket-dialog.css',
  templateUrl: './ticket-dialog.html',
})
export class TicketDialog {
  readonly data = inject<TicketDialogData>(DIALOG_DATA);
  readonly source = this.data.kind === 'upstream' ? this.data.issue : this.data.pr;
  readonly review = new ActionModeReview(this.data.mode);
  readonly ref = inject<DialogRef>(DialogRef);
  readonly store = inject(BoardStore);
  private readonly api = inject(Api);
  private readonly actions = inject(Actions);
  private readonly destroy = inject(DestroyRef);
  readonly draft = signal<TicketDraft | UpstreamIssueDraft | null>(null);
  readonly warning = computed(() =>
    this.data.kind === 'upstream'
      ? (this.data.warning ??
        this.draft()?.warning ??
        (this.data.issue.state === 'closed'
          ? 'This upstream issue is closed. Review it before creating a Jira ticket.'
          : null))
      : this.draft()?.warning,
  );
  readonly options = signal<JiraOptions | null>(null);
  readonly loading = signal(true);
  readonly optionsLoading = signal(false);
  readonly busy = signal(false);
  readonly error = signal('');
  readonly optionsError = signal('');
  readonly jiraHealth = computed(
    () =>
      this.store.sync()?.sources.find((s) => s.source_id === 'jira') ??
      this.store.board()?.sources.find((s) => s.source_id === 'jira'),
  );
  readonly staleChoices = computed(() => {
    const health = this.jiraHealth();
    return (
      !!this.options() &&
      !!health?.last_success_at &&
      (health.status === 'error' || health.status === 'backoff')
    );
  });
  readonly sprintGroups = computed(() =>
    ['active', 'future'].map((state) => ({
      label: state === 'active' ? 'Active' : 'Future',
      sprints: this.options()?.sprints.filter((s) => s.state === state) ?? [],
    })),
  );
  readonly statusChoices = computed(() =>
    (this.options()?.statuses ?? []).map((s) => ({
      ...s,
      ambiguous: this.options()!.statuses.filter((other) => other.name === s.name).length > 1,
    })),
  );
  readonly ambiguousStatuses = computed(() => this.statusChoices().some((s) => s.ambiguous));
  readonly choiceWarnings = signal<string[]>([]);
  readonly outcomeUnknown = signal(false);
  private readonly eligibleAssignees = signal<string[]>([]);
  readonly assignees = computed(() => {
    const self = this.store.context()?.identities.find((i) => i.kind === 'jira')?.account_id;
    return (this.options()?.assignees ?? []).filter(
      (p) => this.data.purpose !== 'review' || p.id !== self,
    );
  });
  readonly diagnostics = computed(() => {
    const o = this.options();
    const all = o
      ? [...o.diagnostics, ...o.capabilities.blockers]
      : (this.draft()?.diagnostics ?? []);
    return [...new Map(all.map((d) => [`${d.code}:${d.field}`, d])).values()].filter(
      (d) => d.code !== 'assignee_required' || !this.fieldErrors().assignee,
    );
  });
  readonly fieldErrors = computed(() => {
    const o = this.options();
    if (!o) return {};
    const m = this.model();
    const errors: Partial<Record<'assignee' | 'team' | 'sprint' | 'points' | 'status', string>> =
      {};
    if (!o.capabilities.unassigned_allowed && !m.assignee)
      errors.assignee = 'Select an eligible assignee; this issue type cannot be unassigned.';
    if (m.assignee && !this.eligibleAssignees().includes(m.assignee))
      errors.assignee = 'Select an eligible assignee for this project and purpose.';
    for (const [field, label, value, capability] of [
      ['team', 'Team', m.team, o.capabilities.team],
      ['sprint', 'Sprint', m.sprint, o.capabilities.sprint],
      ['points', 'Story points', m.points, o.capabilities.story_points],
    ] as const) {
      if (capability.required && !value.trim()) errors[field] = `${label} is required.`;
      if (value && !this.supports(capability))
        errors[field] = `${label} is not available for this issue type. Clear it to continue.`;
    }
    if (m.team && !o.teams.some((t) => t.id === m.team)) errors.team = 'Select a discovered team.';
    if (m.sprint && !o.sprints.some((s) => s.id === m.sprint))
      errors.sprint = 'Select an available sprint.';
    if (m.status && o.statuses.filter((s) => s.name === m.status).length !== 1)
      errors.status = 'Select an unambiguous status or Jira default — no transition.';
    return errors;
  });
  readonly selectionErrors = computed(() => Object.values(this.fieldErrors()));
  readonly createBlocked = computed(
    () =>
      this.busy() ||
      this.loading() ||
      this.optionsLoading() ||
      !this.options()?.capabilities.can_create ||
      !!this.optionsError() ||
      this.outcomeUnknown() ||
      this.review.stale(),
  );
  readonly model = signal<TicketEditor>({
    project: '',
    issueType: '',
    title: '',
    description: '',
    points: '',
    assignee: '',
    sprint: '',
    team: '',
    status: '',
  });
  readonly fields = form(this.model, (p) => {
    required(p.project);
    required(p.issueType);
    validate(p.title, ({ value }) =>
      value().trim() ? undefined : { kind: 'required', message: 'Enter a ticket title.' },
    );
    maxLength(p.title, 255);
    maxLength(p.description, 100_000);
    validate(p.points, ({ value }) =>
      value().trim() && (!Number.isFinite(Number(value())) || Number(value()) < 0)
        ? { kind: 'points', message: 'Use a non-negative number or leave blank.' }
        : undefined,
    );
    disabled(p, { when: () => this.busy() || this.loading() });
    disabled(p.assignee, {
      when: () =>
        !this.options() ||
        this.options()?.project_key !== this.model().project ||
        this.options()?.issue_type !== (this.model().issueType || null),
    });
    disabled(p.points, { when: () => !this.supports(this.options()?.capabilities.story_points) });
    disabled(p.team, { when: () => !this.supports(this.options()?.capabilities.team) });
    disabled(p.sprint, { when: () => !this.supports(this.options()?.capabilities.sprint) });
  });
  private optionsRequest?: Subscription;
  private baseline = '';
  private optionsVersion = 0;
  private validatedScope = '';
  private resolveProjectType = false;
  private readonly requestedRefresh = signal<string | null>(null);

  constructor() {
    effect(() => {
      const previous = this.requestedRefresh();
      const success = this.store
        .board()
        ?.sources.find((s) => s.source_id === 'jira')?.last_success_at;
      if (previous !== null && success && success !== previous) {
        untracked(() => {
          this.requestedRefresh.set(null);
          this.loadOptions();
        });
      }
    });
    this.ref.disableClose = true;
    this.ref.backdropClick.pipe(takeUntilDestroyed()).subscribe(() => this.cancel());
    this.ref.keydownEvents.pipe(takeUntilDestroyed()).subscribe((e) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        this.cancel();
      }
    });
    const beforeUnload = (event: BeforeUnloadEvent) => {
      if (this.isDirty() || this.busy()) event.preventDefault();
    };
    window.addEventListener('beforeunload', beforeUnload);
    this.destroy.onDestroy(() => {
      this.optionsRequest?.unsubscribe();
      window.removeEventListener('beforeunload', beforeUnload);
    });
    this.generate();
  }
  generate() {
    this.loading.set(true);
    this.error.set('');
    const request: Observable<TicketDraft | UpstreamIssueDraft> =
      this.data.kind === 'upstream'
        ? this.api.draftUpstreamIssue({ upstream_issue_id: this.data.issue.id, project_key: null })
        : this.api.draft({
            pull_request_id: this.data.pr.id,
            purpose: this.data.purpose,
            project_key: null,
          });
    request.pipe(takeUntilDestroyed(this.destroy)).subscribe({
      next: (draft) => {
        this.draft.set(draft);
        this.model.set({
          project: draft.project_key,
          issueType: draft.issue_type ?? '',
          title: draft.title,
          description: draft.description,
          points: draft.story_points === null ? '' : String(draft.story_points),
          assignee: draft.assignee_id ?? '',
          sprint: draft.sprint_id ?? '',
          team: draft.team ?? '',
          status: draft.suggested_status ?? '',
        });
        this.baseline = JSON.stringify(this.model());
        this.loading.set(false);
        this.loadOptions();
      },
      error: (e) => {
        this.error.set(errorMessage(e));
        this.loading.set(false);
      },
    });
  }
  projectChanged(event: Event) {
    this.changeProject((event.target as HTMLSelectElement).value);
  }
  changeProject(value: string) {
    const allowed = this.store.context()?.jira_projects.find((p) => p.key === value)?.issue_types;
    this.resolveProjectType =
      !!this.model().issueType && !allowed?.includes(this.model().issueType);
    this.model.update((m) => ({
      ...m,
      project: value,
      issueType: this.resolveProjectType ? '' : m.issueType,
    }));
    this.loadOptions();
  }
  typeChanged(event: Event) {
    this.model.update((m) => ({
      ...m,
      issueType: (event.target as HTMLSelectElement).value,
    }));
    this.resolveProjectType = false;
    this.loadOptions();
  }
  refreshChoices() {
    this.requestedRefresh.set(
      this.store.board()?.sources.find((s) => s.source_id === 'jira')?.last_success_at ?? '',
    );
    this.store.refresh();
  }
  loadOptions() {
    const { project, issueType } = this.model();
    if (!project) return;
    const scope = `${project}:${issueType}`;
    // Scope changes need the complete eligible pool before invalidating a selection.
    const version = ++this.optionsVersion;
    this.optionsRequest?.unsubscribe();
    this.optionsLoading.set(true);
    this.optionsError.set('');
    this.optionsRequest = this.api
      .options(project, this.data.purpose, issueType || null)
      .pipe(takeUntilDestroyed(this.destroy))
      .subscribe({
        next: (options) => {
          if (
            version !== this.optionsVersion ||
            project !== this.model().project ||
            issueType !== this.model().issueType ||
            options.project_key !== project ||
            options.purpose !== this.data.purpose ||
            (issueType && options.issue_type !== issueType)
          )
            return;
          this.options.set(options);
          this.optionsLoading.set(false);
          this.eligibleAssignees.set(this.assignees().map((p) => p.id));
          if (this.validatedScope && this.validatedScope !== scope) this.reconcile(options, true);
          else if (this.resolveProjectType) this.reconcile(options, true);
          this.validatedScope = `${project}:${this.model().issueType}`;
        },
        error: (e) => {
          if (version !== this.optionsVersion) return;
          this.optionsError.set(errorMessage(e));
          this.optionsLoading.set(false);
        },
      });
  }
  supports(capability: FieldCapability | undefined) {
    return capability?.mode === 'create' || capability?.mode === 'follow_up';
  }
  capabilityHelp(label: string, capability: FieldCapability | undefined) {
    if (!capability) return `${label} availability is not ready.`;
    if (capability.mode === 'follow_up')
      return `${label} will be applied after creation, not sent on the create screen. If assignment fails, the created ticket is kept and a warning is shown; do not create it again.`;
    if (!this.supports(capability))
      return (
        capability.diagnostic?.message ??
        `${label} is ${capability.mode} for this issue type. Leave it blank.`
      );
    return capability.required ? `${label} is required.` : '';
  }
  private reconcile(options: JiraOptions, validateAssignee: boolean) {
    const warnings: string[] = [];
    this.model.update((m) => {
      const next = { ...m };
      if (this.resolveProjectType) {
        next.issueType = options.issue_type ?? '';
        warnings.push(
          'The previous issue type is not allowed in this project. The resolved purpose default is shown; review it before creating.',
        );
        this.resolveProjectType = false;
      }
      const clear = (
        field: 'assignee' | 'team' | 'sprint' | 'points' | 'status',
        label: string,
        invalid: boolean,
      ) => {
        if (next[field] && invalid) {
          next[field] = '';
          warnings.push(
            `${label} was cleared because it is no longer allowed for this project or issue type. Select a new value if needed.`,
          );
        }
      };
      clear(
        'assignee',
        'Assignee',
        validateAssignee && !this.eligibleAssignees().includes(m.assignee),
      );
      clear(
        'team',
        'Team',
        !this.supports(options.capabilities.team) || !options.teams.some((t) => t.id === m.team),
      );
      clear(
        'sprint',
        'Sprint',
        !this.supports(options.capabilities.sprint) ||
          !options.sprints.some((s) => s.id === m.sprint),
      );
      clear('points', 'Story points', !this.supports(options.capabilities.story_points));
      clear(
        'status',
        'Workflow status',
        options.statuses.filter((s) => s.name === m.status).length !== 1,
      );
      return next;
    });
    if (warnings.length) this.choiceWarnings.update((old) => [...new Set([...old, ...warnings])]);
  }
  cancel() {
    if (this.busy()) return;
    if (
      !this.isDirty() ||
      window.confirm(
        this.outcomeUnknown()
          ? 'Close this editor? Creation may have succeeded. Check Jira before creating another ticket.'
          : 'Discard your ticket edits? No Jira ticket has been created.',
      )
    )
      this.ref.close();
  }
  private isDirty() {
    return !!this.baseline && JSON.stringify(this.model()) !== this.baseline;
  }
  save(event: Event) {
    event.preventDefault();
    this.fields().markAsTouched();
    if (this.selectionErrors().length) {
      queueMicrotask(() =>
        document
          .querySelector<HTMLElement>('.ticket-form [aria-invalid="true"]:not(:disabled)')
          ?.focus(),
      );
      return;
    }
    if (this.createBlocked()) return;
    void submit(this.fields, async () => {
      this.busy.set(true);
      this.error.set('');
      try {
        const payload = createPayload(this.model(), { ...this.data, mode: this.review.captured() });
        const request =
          'upstream_issue_id' in payload
            ? this.actions.createUpstreamIssue(payload)
            : this.actions.create(payload);
        await firstValueFrom(request.pipe(takeUntilDestroyed(this.destroy)));
        this.ref.close();
      } catch (e) {
        if (isModeConflict(e)) this.review.invalidate();
        if (
          e instanceof HttpErrorResponse &&
          (e.status === 0 || e.error?.error?.code === 'write_outcome_unknown')
        )
          this.outcomeUnknown.set(true);
        this.error.set(
          this.outcomeUnknown()
            ? 'Creation could not be confirmed. Check Jira for the ticket before any further creation.'
            : errorMessage(e),
        );
      } finally {
        this.busy.set(false);
      }
    });
    if (this.fields().invalid())
      queueMicrotask(() =>
        document.querySelector<HTMLElement>('.ticket-form [aria-invalid="true"]')?.focus(),
      );
  }
}
