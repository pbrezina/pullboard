# PullBoard contributor contract

Read `api/CONTRACT.md` and `api/openapi.json` before changing API-facing code.
The OpenAPI document is the shared wire contract; coordinate changes in both clients
and tests rather than inventing alternate payloads.

## Non-negotiable invariants

- Application configuration is one YAML file. Secrets are literal YAML values or
  OS keyring references, never environment-variable secrets. Never return or log them.
- No authentication UI. Local access only; container ports must publish on loopback.
- All forge adapters are read-only and use the shared bounded Reqwest transport.
  GitHub GraphQL POSTs are query-only; no forge write interface is permitted.
- Jira writes occur only after explicit user submission. Read-only test mode returns
  successful, explicitly simulated responses and performs **zero** upstream writes.
- Attention is per-thread and based exclusively on upstream timestamps. A user's
  reply or upstream resolution clears a thread; pushing a commit does not clear it.
  Flat PR discussion is one separate thread. Bot events do not create attention.
- SQLite is an ephemeral cache, initialized without migrations. Publish each complete
  source card snapshot independently and atomically; Jira creation options are a separate
  complete atomic generation. Failure retains only that domain's scoped last-good data
  and marks its independent health stale; never undo another domain/source's commit.
- Do not invent upstream data, Jira IDs, supported API coverage, or test results.
- Rust application code forbids unsafe. Load the rust-skills skill for Rust work.
- Angular uses current stable, standalone components, signals, Tailwind and CDK.
  Load angular-developer, angular-new-app, frontend-design and web-design-guidelines.
- Documentation uses mdBook: load the mdbook skill. `book.toml` and Cargo manifests
  are tool metadata, not alternative application configuration formats.
- License: GPL-3.0-or-later.

## Layout and workflow

`backend/`: Rust binary/library and tests. `frontend/`: Angular CLI workspace.
`crates/pullboard-core/`: shared strict YAML/policy and bounded read-only transport.
`setup/`: disposable Linux x86-64 inline prompt wizard; no server/SQLite/keyring dependencies.
`api/`: shared contract. `config/`: sanitized YAML example(s).
`docs/`: mdBook sources. Root Dockerfile/Makefile/CI: delivery tooling.

Implementation phases are sequential: contract, backend agent, frontend agent,
documentation/DevOps agent, then lead integration verification. Agents should own
their assigned directories, preserve other work, and report actual tests and limits.

## Additional agreed behavior

- Discover relevant PRs from configured accounts, including private repositories
  allowed by credentials; optional include/exclude filters after discovery.
- Multiple instances per forge have unique stable source IDs and separate identities.
- Required Attention has Jira mentions, requested reviews/mentions, and own PR feedback.
  Sort by oldest unanswered event. Overview has assigned Jira issues and own open PRs,
  sorted by title. Reviewer/assignee lists exclude self for PR display and sort ascending.
- Jira selectors include all active/future sprints and no sprint. PR selectors include
  all forges and each instance. Both have case-insensitive title filters, independent
  per tab; show filtered and total attention counts. Persist display preferences only.
- Who-am-I shows verified identities and the discovered first YAML Jira project's real
  key/name (or unavailable), never a hardcoded project or a later-project fallback.
  Current sprints are only pattern-matching active sprints, with end dates and links
  formed by setting selectedSprint on the trusted board_url. Show no-match/ambiguity
  explicitly; all board-returned active/future sprints remain selectable, grouped by state.
- `sprint_name_pattern` is a bounded Rust regex search, case-sensitive unless inline
  flags change it; anchors are optional. No percent-token substitutions or legacy modes.
  Compile once on parsing; matching affects defaults only, never sprint availability.
- Setup-generated YAML defaults to Live after an explicit Test-mode question; preserve
  the existing setting on edit. The backend's omitted-property default stays Test.
  Setup never writes upstream; optional synthetic AI tests need explicit billable consent.
  `--jira-cloud-url` is an editable tenant-root prefill, never an automatic credential
  destination change. Tokens are not CLI/environment inputs or terminal/log output.
- Overview initially/reset selects an actual sprint ID only for one current match;
  zero/multiple matches leave Jira results empty until an explicit filter choice.
  Attention defaults to all including no sprint. No synthetic eligible-sprints option.
  Preserve deliberate per-tab selections across refreshes; reset clears that tab's
  text/forge/repository filters and reapplies its sprint default. Migrate saved active sentinel.
- Every PR/review/upstream ticket starts Unassigned, No sprint and Jira default/no
  transition. Legacy assignee: me is accepted but never preselects self. Optional
  blanks are normal; Jira-required values require explicit input. Review excludes self.
- Refresh with ticket-favoring bounded concurrency: after one verified identity per
  source attempt, Jira tickets/search/sprints/hydration, creation metadata/statuses and
  group discovery may run concurrently. Publish complete Jira tickets/sprints and each
  forge's PRs/upstream issues independently as ready; complete options may commit first.
  Eligibility waits only for the complete deduplicated membership candidate population,
  not tickets or metadata. Use one owned bounded project/account queue: at most one
  eligibility request/item in flight while any configured card source is queued,
  preparing, discovering, fetching or publishing; automatically expand to the existing
  configured item concurrency once all card attempts terminate (completed/failed/backoff).
  This is not preemption or a fixed-latency promise: metadata/groups may compete, shared
  transport fairness/backoff still apply, and HTTP cap 1 naturally serializes reads.
  Keep the shared per-origin HTTP cap (default 4; actual YAML may differ), pagination,
  body/time/backoff bounds; no new YAML cap, cap increase, nested multiplication or
  hybrid/bulk API work. Reuse verified prerequisites/sprint reads without waiting for
  ticket hydration; each options generation still owns its complete sprint snapshot.
  Branch-specific errors never cancel the other domain's work or undo its commit;
  shared identity, prerequisite and backoff failures propagate honestly.
- Discover complete assignee pools, per-project/type metadata/statuses and effective
  policy in the owned/cancellable background refresh, never options/drafts/create.
  Publish choices atomically as one independent generation with its own sprint snapshot;
  validate against one committed generation, never newer board sprints or partial pools.
  Retain complete same-scope last-good choices as stale on options failure; without an
  initial cache, fail closed. Policy changes apply next successful options publication.
  Principal/service/config scope changes quarantine incompatible private data in both
  domains immediately. No optimistic assignment; secret redaction remains mandatory.
- SyncStatus.board_revision signals every committed cache snapshot, including options
  and scope quarantine/restoration, while syncing. Poll clients must load mid-job
  publications without losing filters, dialog edits or captured mode revisions.
  Ticket progress freezes at its source commit; job success requires every ticket source
  and Jira options to succeed. jira_options has independent persisted health/readiness
  and process-local metadata/group/project-check progress, never ticket counts. Its
  preparing/discovering phases may overlap project checking after all groups complete;
  fetching means metadata/groups discovery complete with outstanding eligibility, not
  an all-tickets barrier. All project rows may finish while metadata is still discovering.
- Discover the first YAML Jira project's real key/name with a minimal early lookup
  if needed (issue_types may be empty until options metadata commits); publish current
  sprints with Jira cards, without waiting for creation options or substituting a project.
- Assignee search filters the full cached pool locally by name/username/exposed email,
  with one accessible virtualized combobox, Unassigned and keyboard navigation; no
  options request per keystroke and no inferred emails. Use jira_options.health/readiness
  for choices and Jira source health for tickets; options failure alone never fails tickets.
- AI defaults to OpenAI model `gpt-6-luna`, passes this ID unchanged, supports
  Anthropic/Ollama, and falls back to an editable deterministic template on failure.
  AI sees only selected PR content, treats it as untrusted, and never executes tools.
- Workflow mappings remain metadata suggestions, never initially selected status.
  Offer actual selected-project/type statuses; submit exact unique names, not IDs or
  free text, with ambiguous names unavailable. Null means Jira default/no transition.
  Explicit live creation precedes at most one direct requested transition (already at
  target is a no-op); unavailable/rejected transitions return created success plus a
  nonblocking persistent dismissible warning retaining the real issue link. No rollback,
  automatic retry, multi-hop path or duplicate creation; sync never writes.
- PR and Jira cards have identical fixed dimensions. Desktop-first, data-dense layout,
  light/dark/system themes, accessible controls, keyboard alternative to drag linking.
  Keep the original compact Source health disclosure with rows; no concurrency UI redesign.
- Successful simulated writes notify explicitly but do not fabricate IDs or cache data.
- Keyring support in Linux containers is opt-in through a documented restricted
  host Secret Service bridge; no automatic plaintext fallback or full-session exposure.
