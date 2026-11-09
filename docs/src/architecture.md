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
| `backend/src/forge/` | Read-only GitHub/octocrab, GitLab/gitlab, Forgejo/Codeberg discovery |
| `backend/src/upstream.rs` | Assignment-scoped issue discovery, configured-instance URL validation, deterministic copy text and bounded scoped manual selections |
| `backend/src/transport.rs` | Bounded authenticated upstream reads, safe URLs/backoff |
| `backend/src/request_budget.rs` | Shared origin HTTP admission, separate primary buckets and shared backoff |
| `backend/src/attention.rs` | Timestamp-based independent stream resolution |
| `backend/src/jira.rs`, `jira/discovery.rs` | Refresh-time complete scoped choices/policy, Cloud identity/typed metadata/statuses, board sprints, candidate eligibility and explicit write gates |
| `backend/src/ai.rs` | Selected untrusted PR proposal generation and deterministic fallback |
| `backend/src/cache.rs`, `sync.rs` | Atomic SQLite cards/choices/policy snapshots, coalesced refresh worker and live per-source progress/health |
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
principal verification → account-scoped cache restore/purge → read-only discovery
          ↓
typed Jira metadata / normalized threads/items/source health
          ↓
coalesced sync → atomic SQLite snapshot → same-origin REST → Angular signals
       └──────→ process-local source progress/health → HTTP polling

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
[upstream issue flow](upstream-issues.md) and [v1.4 API](api.md).

Keep upstream IDs and timestamps real. Never infer handled attention from a commit,
manufacture missing event times, synthesize Jira IDs, or conceal coverage gaps. A
new adapter must preserve last-good snapshots on failure and expose limitations.
Only verified same-scope snapshots may be retained; startup quarantine must not
expose a prior account. Identity bootstrap is distinct from complete Jira snapshot
readiness. Normal refresh publishes complete Jira choices and effective policy with
cards; options/drafts/create use that committed generation without repeating metadata,
group or per-user discovery. Failure retains complete same-scope last-good choices
as stale. Policy changes take effect only at successful commit, not by combining new
rules with an old pool. Final create uses the latest committed choices rather than
trusting the client picker; Jira enforces current write permissions.
Adding a provider method must not expand forge write capability.

Source health describes refresh success, not universal API coverage: successful
Jira/forge sources are `ok` with neutral coverage notes; per-item partial attention
remains independent. One locked process-local sync state tracks phases, unique fully
evaluated candidates, nullable totals and monotonic elapsed time independently of
SQLite. The old board stays visible through `publishing`; only a successful combined
transaction advances its revision. Failures retain verified same-scope data and
surface live failed/backoff health without pretending old data is fresh. Clients
join progress by source ID and poll rather than adding a streaming transport.
Active refreshes and real failures/backoff retain separate panels; healthy idle
collapses completed counters/durations into the existing Source health details.

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

The header's current project comes from the first YAML Jira project's discovered
metadata; only pattern-matching active sprints are current. Overview selects an
actual ID initially/reset only for one match; zero/multiple matches require deliberate
choice, while Attention defaults all/no sprint. No synthetic eligible-sprints filter
or automatic union remains. Every ticket editor starts Unassigned/No sprint/no
transition. A lazy-loaded editor uses a complete locally filtered virtualized
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

There is one coalesced refresh job, with at most four source read futures active
concurrently; this source ceiling is not an extra configuration knob. Independent
source outcomes are collected without short-circuiting other sources on failure,
then ordered deterministically for one final atomic snapshot publication. Each
successful source replaces its data; each failed source retains only its verified
same-scope last-good data and gets its own stale/error/backoff health. A fast source
can wait in `publishing` for a slower source; it does not publish cards early.
Identity verification/quarantine remains a separate safety gate, not a partial
item publication.

Within each source, bounded item hydration overlaps independent required reads.
Cursor-dependent pagination stays sequential. GitHub batches obey both the item
limit and the effective batch size; there is no unbounded task or request fanout.
All these reads pass through the origin-shared request budget, whose permit lasts
through response consumption. See [configuration](configuration.md#controls) for
the three YAML knobs and [providers](providers.md#shared-request-budgets-and-rate-limits)
for conservative cross-credential sharing and quota/backoff behavior. This changes
read scheduling, not the public API, attention semantics or write permissions.

Application-owned INFO diagnostics report safe source read duration/item counts
and origin-budget idle counters; dependency logging is WARN and above. They do not
include raw upstream URLs, query/payload contents or credentials. See
[log interpretation](troubleshooting.md#refresh-timing-and-safe-logs) before using
these diagnostics for measurement.
