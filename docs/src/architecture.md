# Architecture and contributor boundaries

## Map for future contributors and AI agents

Read root `AGENTS.md`, `api/CONTRACT.md`, then `api/openapi.json`. Also follow any
directory-specific `AGENTS.md`. Do not change payloads independently in a client.
The implementation phases are contract → backend → frontend → docs/DevOps → lead
integration verification; keep directory ownership and coordinate cross-boundary
fixes. Do not delete unfamiliar work or claim tests you did not run.

| Area | Responsibilities |
| --- | --- |
| `api/` | Shared normative wire schemas and behavioral contract |
| `backend/src/config.rs` | Strict single-YAML config, redacted secrets/keyring resolution |
| `backend/src/forge/` | Read-only GitHub, GitLab, Forgejo/Codeberg discovery and response models |
| `backend/src/upstream.rs` | Assignment-scoped issue discovery, configured-instance URL validation, deterministic copy text and bounded scoped manual selections |
| `backend/src/transport.rs` | Shared Reqwest forge reads, per-instance credentials/cache, bounded bodies, safe URLs/backoff and query-only GitHub GraphQL |
| `backend/src/request_budget.rs` | Shared origin HTTP admission, separate primary buckets and shared backoff |
| `backend/src/attention.rs` | Timestamp-based independent stream resolution |
| `backend/src/jira.rs`, `jira/discovery.rs` | Refresh-time complete scoped choices/policy, Cloud identity/typed metadata/statuses, board sprints, candidate eligibility and explicit write gates |
| `backend/src/ai.rs` | Selected untrusted PR proposal generation and deterministic fallback |
| `backend/src/cache.rs`, `sync.rs` | Independent atomic source snapshots and complete options generations, coalesced ticket-favoring concurrent worker, publication revisions and separate ticket/options progress/health |
| `backend/src/runtime_mode.rs` | Process-wide mode/revision, atomic toggle and action admission |
| `backend/src/api.rs`, `main.rs` | Same-origin routes/static assets, CLI and graceful lifecycle |
| `frontend/src/app/core/` | Typed API/state/display preferences, sync/mode polling and explicit stale-mode review |
| `frontend/src/app/upstream-issues/` | Compact assigned-issue rows, independent display filters and transient URL lookup |
| `frontend/src/app/` | Standalone signal-driven board, cards, filters, dialogs, health/identity UI |
| `config/` | Sanitized examples only, never real credentials |
| `docs/`, root delivery files | mdBook, source builds, image hardening and CI |

Rust application code forbids unsafe. Load the Rust skill for Rust changes. Angular
uses current stable standalone components, signals, Tailwind and CDK; load the
Angular/design/accessibility skills required by root instructions for UI work.
Documentation uses mdBook's built-in include preprocessing, no custom backend.

## Data and action paths

```text
one YAML → validated configuration → credentials resolved
          ↓
principal verification → account-scoped cache restore/purge
          ├→ source card discovery/hydration → independent atomic source publications
          ├→ creation metadata/statuses ────────────────────────────┐
          └→ complete membership candidates → bounded eligibility ┤
             shared complete sprint read ──────────────────────────┤
                                                                 ↓
                          complete atomic options generation (own sprints/policy)
          ↓
latest committed cache → same-origin REST → Angular signals
       └──────→ board_revision + separate ticket/options status → HTTP polling

selected cached PR → AI/template → editable draft → explicit user submit
                                                ↓
                           atomic revision check + captured Test/Live mode
                                                ↓
                         Test: simulated success (no write/refresh)
                         Live: real result + follow-up warnings → refresh
```

Upstream issues take a separate deterministic path: assigned board issue **or**
validated configured-instance URL → trusted selection → implementation template →
shared editable editor → explicit guarded Jira create/web link. They never enter
AI or add attention. Manual content stays outside Board/SQLite in a process-local
128-entry/8 MiB store scoped to the verified source/service/principal/repository
policy. Refresh preserves those selections; eviction/restart/scope invalidation
requires explicit recovery, not automatic create replay. Jira cloning does not
introduce duplicate searches, clone state or broad Jira/forge scans. See
[upstream issue flow](upstream-issues.md) and [v1.7 API](api.md).

Keep upstream IDs and timestamps real. Never infer handled attention from a commit,
manufacture missing event times, synthesize Jira IDs, or conceal coverage gaps. A
new adapter must preserve last-good snapshots on failure and expose limitations.
Only verified same-scope snapshots may be retained; startup quarantine must not
expose a prior account. Identity bootstrap and ticket visibility are distinct from
complete options readiness. Normal refresh publishes complete Jira choices, their own
sprints and effective group/default/pattern policy as one independent generation; options/drafts/create use
that committed generation without repeating metadata, group or per-user discovery.
Options failure retains complete same-scope last-good choices as stale and usable,
with independent health/readiness, without undoing fresh cards. Before a verified
complete generation, creation is blocked; no optimistic assignments are exposed.
Scope quarantine applies immediately to incompatible cards and choices. Policy changes
take effect only at successful options commit, not by combining new rules with an old
pool, live YAML pattern policy or newer board sprints. Final create uses the latest committed choices rather than
trusting the client picker; Jira enforces current write permissions.
Adding a provider method must not expand forge write capability.

Forge adapters own endpoint selection, response models and pagination semantics;
there are no GitHub/GitLab SDK clients. Their shared Reqwest transport retains
forge-specific authentication, GitHub API-version headers and enterprise/subpath
URL mappings. Each configured instance has its own client and ETag cache, while
request budgets remain shared by origin. Headers publish throttling before body
consumption so queued callers see header backoff before slow or failing bodies
finish; bounded GitHub error bodies also detect origin-wide secondary limits.
Permits cover body reads. Only the bounded REST retry loop retries requests;
GraphQL is query-only and is not automatically retried.

Source health describes refresh success, not universal API coverage: successful
Jira/forge sources are `ok` with neutral coverage notes; per-item partial attention
remains independent. One locked process-local sync state tracks phases, unique fully
evaluated candidates, nullable totals and monotonic elapsed time independently of
SQLite. Each source's old cards stay visible through its own `publishing`; its
commit freezes source progress while other work continues. Every committed snapshot,
including options-only, health and quarantine/restoration commits, advances the shared
board revision. Failures retain only their own verified same-scope data and surface
independent live failed/backoff health without pretending old data is fresh. Clients
join ticket progress by source ID and use `SyncStatus.jira_options` for choices.
They poll `SyncStatus.board_revision` during and between jobs, reload Board/Context
on changes and retry failed dependent reads without consuming the revision. Late reads
cannot regress applied state. Polling preserves filters, editor edits and captured
mode revisions, without automatic replay; there is no new streaming transport.
All progress stays in the original Source health disclosure as compact rows, including
completed bars. Jira adds group/project rows with one shared choices phase/duration.
Routine health timestamps and coverage notes are behind source Diagnostics.

Runtime mode is initialized from YAML, shared across clients and never persisted.
The same lock compares revision and captures action mode before I/O/business checks,
or changes mode/revision atomically. Fresh revisions on toggle/restart invalidate old
dialogs even if the boolean returns to the same value. Locks are not held across I/O;
all follow-ups use the admitted immutable mode. Switching Test is not revocation:
admitted live operations may still finish. UI confirmations freeze mode/token pairs;
polling never rebases them. Conflicts preserve edits and require explicit review and
a separate submission, never automatic retry. Simulations do not schedule write
refreshes, unlike successful live actions. Existing M:N single-pair linking is unchanged.

Both card types share one compact 300px-high layout with visible summaries in
Upstream Pull Requests (PR repository/people, Jira assignee, link counts and
description previews) and concise attention summaries before excerpts. Native
details preserve full content/links in a
scrollable reading area without changing grid dimensions; keyboard linking and
light/dark/system themes remain available. No density setting is introduced.
The separate Upstream issues tab uses compact full-width rows, not this card grid;
its instance/repository/title preferences are independent of Jira/PR tab filters.

The header's real current project key/name comes from the first YAML Jira project,
with a minimal early lookup if needed and no later-project fallback. It and board/current
sprints publish with ticket/display work, without waiting for creation metadata;
issue types may remain empty until options commit. Ordered `board_ids` supply a complete
active/future union, deduplicated by sprint ID with earliest-configured-board provenance,
never `originBoardId` traversal. Only active first-pattern matches returned by the
first board are current; additional boards/patterns are never a fallback. Header links
and estimation metadata use only the first board. Overview selects an
actual ID initially/reset only for one match; zero/multiple matches require deliberate
choice, while Attention defaults all/no sprint. No synthetic eligible-sprints filter
or automatic union remains. Every ticket editor starts Unassigned/No sprint/no
transition. Creation sprint choices match any pattern in one committed generation's
own complete multi-board snapshot, independently of team/purpose; display sprints,
existing assignments and Attention remain unfiltered by pattern. A lazy-loaded editor uses a complete locally filtered virtualized
assignee list and per-type discovered status choices. Explicit unique-name status
submission allows at most one direct transition after creation; failure preserves
created success and its persistent linked warning, never triggers duplicate creation.

The container has three build stages: Node builds locked frontend assets, Rust
builds the locked backend, Alpine runtime contains only the binary, CA roots,
assets, safe config and license. Rust is PID 1. No Node, compiler, Cargo, nginx,
live secret YAML or whole desktop session ships in it.

Tool metadata (Cargo manifests, Angular/npm files, `book.toml`, Make/CI options) is
not application configuration. Do not “simplify” deployment by introducing app
token environment overrides, auth UI, migration state or background Jira writes.

## Bounded read scheduling

There is one owned/cancellable coalesced refresh job, with at most four source read
futures active concurrently; this source ceiling is not an extra configuration knob.
Verify identity once per source attempt. Independent assigned/mention searches and
sprint reads overlap within bounds; deduplicate the full candidate inventory before
Jira hydration. Each complete source card snapshot publishes immediately and
independently: Jira tickets/sprints together, each forge's PRs/upstream issues together.
No source waits for slower sources or creation choices to publish. Failures do not
short-circuit unrelated sources, expose partial pages/items or undo another commit;
each failed domain retains only its own verified same-scope last-good data and health.
Identity verification/quarantine remains a separate safety gate.

After verified Jira identity, ticket work, creation metadata/statuses and group discovery
may run concurrently. Eligibility waits only for complete deduplicated membership
candidates (or complete human inventory without groups), not metadata or tickets.
One owned bounded project/account queue allows at most one eligibility request/item
in flight while any configured card source is queued, preparing, discovering, fetching
or publishing. A watch-driven scheduler expands to existing configured item concurrency
when all card attempts terminate, including failed/skipped-backoff attempts. This is
neither sequential per-project batches nor detached work. Metadata/groups may compete
with cards; transport fairness/backoff apply, with no preemption or fixed-latency promise.
HTTP cap 1 naturally serializes reads.

Group pagination stays indeterminate until complete; project progress counts finished
checks, including negative results, never tickets or only retained assignees. Options
preparing/discovering may overlap checks or show every project completed while metadata
is pending; fetching means metadata/groups complete with eligibility outstanding, not
a card barrier. Reuse shared prerequisites/sprint reads without a ticket-hydration barrier.
Complete options, including their own full sprint snapshot, may publish before cards
in their independent atomic transaction/health domain. Branch-specific failures do
not cancel the other domain or undo its commit; shared prerequisite/backoff failures
propagate honestly. Overall job success still requires every ticket source and options.

Within each source, bounded item hydration overlaps independent required reads.
Cursor-dependent pagination stays sequential. GitHub batches obey both the item
limit and the effective batch size; there is no unbounded task or request fanout.
All these reads pass through the origin-shared request budget, whose permit lasts
through response consumption. The existing per-origin HTTP cap defaults to **4**,
as does item concurrency; actual limits come from the single YAML. Nested stages do
not multiply or silently raise caps. Pagination, response/time bounds and backoff
remain unchanged; hybrid eligibility and new bulk APIs are out of scope. See
[configuration](configuration.md#controls) for
the three YAML knobs and [providers](providers.md#shared-request-budgets-and-rate-limits)
for conservative cross-credential sharing and quota/backoff behavior. Contract v1.7
preserves v1.6's concurrent options progress combinations without new wire
fields/phases, concurrency settings, create fields, attention rules or write permissions. There is no
measured speedup claim for this scheduling change.

Application-owned INFO diagnostics report safe source read/publication durations,
creation discovery/eligibility timing, first-card/options/job timing and origin-budget
idle counters; dependency logging is WARN and above. They do not
include raw upstream URLs, query/payload contents or credentials. See
[log interpretation](troubleshooting.md#refresh-timing-and-safe-logs) before using
these diagnostics for measurement.
