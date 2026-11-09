# PullBoard contributor contract

Read `api/CONTRACT.md` and `api/openapi.json` before changing API-facing code.
The OpenAPI document is the shared wire contract; coordinate changes in both clients
and tests rather than inventing alternate payloads.

## Non-negotiable invariants

- Application configuration is one YAML file. Secrets are literal YAML values or
  OS keyring references, never environment-variable secrets. Never return or log them.
- No authentication UI. Local access only; container ports must publish on loopback.
- All forge adapters are read-only. GitHub uses octocrab; GitLab uses gitlab.
- Jira writes occur only after explicit user submission. Read-only test mode returns
  successful, explicitly simulated responses and performs **zero** upstream writes.
- Attention is per-thread and based exclusively on upstream timestamps. A user's
  reply or upstream resolution clears a thread; pushing a commit does not clear it.
  Flat PR discussion is one separate thread. Bot events do not create attention.
- SQLite is an ephemeral cache, initialized without migrations. Replace snapshots
  atomically. On source failure preserve that source's last good data and mark it stale.
- Do not invent upstream data, Jira IDs, supported API coverage, or test results.
- Rust application code forbids unsafe. Load the rust-skills skill for Rust work.
- Angular uses current stable, standalone components, signals, Tailwind and CDK.
  Load angular-developer, angular-new-app, frontend-design and web-design-guidelines.
- Documentation uses mdBook: load the mdbook skill. `book.toml` and Cargo manifests
  are tool metadata, not alternative application configuration formats.
- License: GPL-3.0-or-later.

## Layout and workflow

`backend/`: Rust binary/library and tests. `frontend/`: Angular CLI workspace.
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
- Overview initially/reset selects an actual sprint ID only for one current match;
  zero/multiple matches leave Jira results empty until an explicit filter choice.
  Attention defaults to all including no sprint. No synthetic eligible-sprints option.
  Preserve deliberate per-tab selections across refreshes; reset clears that tab's
  text/forge/repository filters and reapplies its sprint default. Migrate saved active sentinel.
- Every PR/review/upstream ticket starts Unassigned, No sprint and Jira default/no
  transition. Legacy assignee: me is accepted but never preselects self. Optional
  blanks are normal; Jira-required values require explicit input. Review excludes self.
- Discover complete assignee pools, per-project/type metadata/statuses and effective
  policy during normal Jira refresh; publish atomically with cards in ephemeral SQLite.
  Options/drafts/create use one committed generation, never repeat group/user/metadata
  discovery. Retain complete same-scope last-good choices as stale on refresh failure;
  without an initial cache, fail closed. Group/policy changes apply next successful refresh.
  Principal/service/config scope protection and secret redaction remain mandatory.
- Assignee search filters the full cached pool locally by name/username/exposed email,
  with one accessible virtualized combobox, Unassigned and keyboard navigation; no
  options request per keystroke and no inferred emails. Use Jira source health for staleness.
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
- Successful simulated writes notify explicitly but do not fabricate IDs or cache data.
- Keyring support in Linux containers is opt-in through a documented restricted
  host Secret Service bridge; no automatic plaintext fallback or full-session exposure.
