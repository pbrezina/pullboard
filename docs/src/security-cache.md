# Security, snapshots and refresh

## Local only

There is no authentication UI, user login or remote multi-user isolation. Bind
native runs to loopback; container bind `0.0.0.0` is safe only with host publishing
`127.0.0.1:HOST:CONTAINER`. Do not publish to all interfaces or expose via a shared
proxy. Loopback is not protection against malicious software already running as
your user.

API Host validation rejects non-loopback hosts to reduce DNS-rebinding attacks.
Every API POST requires JSON and `X-PullBoard-Request: 1`. Browser Origin, when
present, must match an exact YAML `allowed_origins` loopback origin. There is no
permissive CORS. These controls complement local publishing, not replace it.

The server sets API `Cache-Control: no-store`, CSP (`script-src 'self'`,
`style-src 'self' 'unsafe-inline'`, fonts/connections self, no framing),
`Referrer-Policy: no-referrer` and `X-Content-Type-Options: nosniff`. Descriptions
are plain text; links must be HTTP(S), not arbitrary executable HTML. Static
assets/fonts are served by the Rust process; no CDN or Node/nginx runtime is needed.
Request and upstream response sizes are bounded; errors are sanitized.

All forge adapters expose read operations only, in both runtime modes. Jira writes
are gated by explicit submission and the mode captured at server admission. Test
actions perform zero upstream writes; switching to Test does not revoke already
admitted live actions. The shared runtime override is not authentication or a
per-tab permission. See [action mode and review](tickets-ai.md#runtime-testlive-mode).
AI receives no tools. Never attach your local YAML, SQLite cache, raw authenticated
network trace or keyring contents to a report.
The cache contains private upstream content even though it is disposable. A tmpfs
can be swapped by the host; it is not a guarantee that sensitive content never
reaches disk. Apply your host's swap/encryption policy accordingly.

## Coherent snapshots

SQLite is an ephemeral cache initialized without migrations. Each source publishes
its **complete card snapshot independently and atomically as soon as it is ready**:
Jira tickets and board sprints replace together; each forge's PRs and assigned
upstream issues replace together. There is no per-ticket/page publication and no
wait for slower sources or creation options. A board read is coherent latest committed
state, not one all-sources refresh generation. Serialized transactions merge into
that state and reconcile cross-source links without lost updates or delete/insert gaps.
A same-scope failure retains only that domain's last-good data and last-success
timestamp with stale health; it cannot undo another source/domain's successful commit.

On startup persisted private snapshots, identities and weblinks are quarantined
until the current credentials' upstream principal is verified. Only an exact scope
match can restore data: source ID, kind, normalized service URL, real account ID,
and relevant project/ordered-board or forge-filter scope. Changed principal/service or
incompatible configured scope quarantines old private data in **both card and options
domains** before later read failures can expose it; derived cross-source links are
recomputed. Jira group/default/pattern policy is instead committed with its complete choice
generation, never mixed into old pools.
No token/token fingerprint is an identity key. Failed bootstrap exposes no old
account data. Identity verification
is published independently of the full issue refresh, so an identity may be visible
even while project/issue discovery fails. Incompatible cache formats are discarded;
quarantine is ephemeral and an exit before verification can lose last-good data.

Jira's cache scope also includes a discovery-policy version. Snapshots from the
former broad project-issue scan are invalidated, even for the same account and YAML;
their issues and weblinks cannot return as last-good data after a failed refresh.
Fresh targeted searches must establish a new snapshot.

Each forge's snapshot contains both PRs and assigned upstream issues. They publish
atomically together: a failure in either collection retains both last-good
collections for the verified scope. Manually resolved issues are different: they
never enter the board or SQLite. Their backend store is scoped to the verified
source/service/principal/repository policy and bounded to 128 entries/8 MiB of
serialized issue content.
Ordinary refreshes do not overwrite them; eviction, restart or scope invalidation
requires explicit re-resolution. URLs are restricted to configured origins/base
paths and repository policy, with no redirects or arbitrary-host reads. See
[upstream issue URL rules and recovery](upstream-issues.md).

Jira choices are a typed **refresh-time SQLite snapshot**: complete project/type
metadata/capabilities/team/status choices, all permitted assignees and the complete
active/future sprint union from every configured board, plus effective group/default/pattern
policy. They publish as one **separate complete
atomic generation**, independently of cards. Options keep their own committed sprint
snapshot: inclusion and validation never use live YAML, newer board sprints or partially
refreshed policy. Creation exposes only names matching any of that generation's regexes;
display filters and existing assignments retain the full sprint inventory. Failed
multi-board pagination cannot publish a partial union or mutate a retained generation.
This cache is not a separate authorization database. Overlapping refreshes coalesce;
options/drafts/create use a single committed generation and never repeat group/user/
metadata discovery. Compatible restored choices remain quarantined until current
identity verification.

A failed options refresh retains complete same-scope last-good choices as usable
but stale. `SyncStatus.jira_options` independently exposes their `ready`, `health`
and `progress`; Jira ticket health does not describe choice freshness. Before verified
complete initial discovery and commit, creation is blocked and options/drafts/create
fail explicitly with 503 readiness or 502 discovery/ambiguity errors, not empty success.
An active refresh leaves previous complete choices ready until replacement. Expected
assignability is not verification: no optimistic assignments or partial pools appear.
Group/default/pattern-policy changes become effective only on successful options publication,
not by reevaluating new policy against an old pool. Final create validates the latest
committed generation independent of the editor's last search; no-longer-allowed
selections are rejected without replay. Jira still enforces current write permissions.
See [choice freshness](jira.md#refresh-time-choices-and-freshness).

## Source health

Health states are exactly `pending`, `ok`, `error`, `backoff`. Successful complete
refreshes are **always `ok` (green)** for Jira and every forge, even with nonempty
`coverage_notes`. Expand **Source health → Diagnostics** for neutral
API/history limitations. These notes do not make a successful source amber or imply
complete upstream history. Per-item `Attention.coverage: "partial"` is independent;
unknown attention history is never treated as answered.

`pending` means no verified successful snapshot yet. Real required-read/page errors,
permission failures or safety bounds mark the source failed and retain only verified
same-scope last-good data as stale, not a silently truncated success. `backoff` shows
the retry deadline and remains flagged until another attempt, even after that deadline
passes. Inspect `last_attempt_at`, `last_success_at`, sanitized error and `retry_at`.
Other sources can still refresh successfully.

The live status strip uses `/api/sync` health/progress. A current failure or backoff
is shown even while the published board still carries the preceding snapshot's
successful health. Fetch success becomes `ok` only after that domain's transaction
commits. Jira options have independent persisted health and attempt/success/retry
timestamps; an options failure never fails ticket health or un-completes ticket progress.
Progress alone does not publish cards, identities or a new board revision;
account-verification quarantine still overrides old-data retention.

## Per-source progress

All source and Jira group/project progress bars stay inside the existing expandable
**Source health** box, joined by stable source ID. Compact rows show names, bars,
fully evaluated counts, known/unknown totals and elapsed time. Completed rows remain
after refresh finishes. Jira's choices phase and shared options elapsed time appear
once above its group/project rows; individual group/project durations are not reported.
Errors/backoff remain visible inside the box; routine timestamps and coverage notes
are under each source's **Diagnostics**. This uses no new display preference.
There is no aggregate percentage or invented work total.

| Phase | Meaning |
| --- | --- |
| `idle` | Process startup before any job; no work counted. |
| `queued` | Accepted job, waiting to start this source. |
| `preparing` | Verifying identity once and preparing ticket/display prerequisites, not creation choices. |
| `discovering` | Enumerating pages and deduplicating candidate issues/PRs. |
| `fetching` | Reading required details/history/links and evaluating candidates. |
| `publishing` | Fetching finished; waiting for or performing this source's atomic publication. |
| `completed` | Publication committed successfully. |
| `failed` | Source/preparation/publication failure; no partial source replacement. |
| `backoff` | Rate-limited attempt, or source skipped until its retry deadline. |

Discovery and fetching can interleave; short/inapplicable phases may not appear in
polls. The count means **unique fully fetched/evaluated issues or PRs**, not HTTP
requests, pages, repositories, comments, metadata, attention threads or visible cards.
Duplicate candidates count once. Candidates excluded after evaluation still count;
partly fetched items do not. Forge counts combine PRs and upstream issues, not just
the current tab's rows. For example, 25 evaluated Jira candidates might yield
9 retained cards; that is not an incomplete refresh (illustrative numbers only).

Until the full unique workload is known, the total is `null` and the active bar is
indeterminate, with the actual count and `?` for the unknown total. A known positive total
permits an item fraction; `0 / 0` is genuinely empty successful work, not an estimated
percentage. At `publishing`, all this source's items are fetched but its replacement
is **not yet committed**; other sources may already be visible. A publication failure
leaves that domain's old data intact and does not advance the revision. Already committed
sources stay completed. A source's counters, finish time and elapsed duration freeze at
its own commit, even while the job remains `syncing` for other sources or options.

Elapsed time comes from a server monotonic clock: it includes preparation, discovery,
fetching and publication waiting/work, but excludes queue time and skipped backoff.
It freezes on completion/failure/backoff. A skipped source has zero counters/elapsed
and unknown total, not the preceding run's work; a rate-limited attempt retains its
actual work and duration. Each new job resets every source's progress; a coalesced
request does not. Terminal progress stays until the next job and resets on restart;
it is not persisted in SQLite or preferences.

## Ticket-favoring concurrency and options progress

The default worker interval is **3600 seconds**. A manual refresh coalesces with an
existing job; jobs do not overlap. Up to four source read futures run concurrently,
each publishing independently. Identity is verified once per source attempt. Jira
performs bounded independent assigned/mention searches and sprint reads, deduplicates
the full inventory and hydrates tickets concurrently. Jira tickets, board/current
sprints and the real first configured project's key/name become available without
waiting for creation choices. That project's issue types may still be empty pending
options metadata; a later project is never substituted.

After Jira identity verification, ticket work, creation metadata/statuses and group
discovery may run concurrently. Eligibility waits **only for the complete deduplicated
membership candidate population**, not metadata or tickets. With no configured groups,
the complete active-human candidate inventory is required instead; partial pages never
start checks. One owned bounded queue spans project/account pairs, not sequential
project batches or independent per-project queues.

While **any configured card source is queued, preparing, discovering, fetching or
publishing**, at most **one eligibility request/item** is in flight. A watch-driven
scheduler automatically expands to the existing configured item concurrency once
**all card attempts are completed, failed or backoff**, including skipped backoff.
Card completion is not a gate to starting eligibility. This is not preemption or a
fixed-latency guarantee: metadata/groups may compete, transport fairness/backoff still
apply, and HTTP cap 1 naturally serializes reads.

Reuse verified prerequisites and complete sprint reads without waiting for ticket
hydration. Options may commit before tickets, but only as a complete atomic generation
with their own full sprint snapshot. Branch-specific errors never cancel the other
domain or undo its commit; shared identity/prerequisite/backoff failures propagate to
each dependent domain honestly. All work belongs to the cancellable job; no detached
checks or partial options commit.

Jira's compact choices rows inside Source health report a shared phase/duration,
group discovery and **per-project eligibility**, never ticket counts:

- Group pagination remains indeterminate (`0`, unknown total) until complete verified
  membership discovery. Completed counts are unique members, including honest `0/0`.
  Repeated group names and overlapping member accounts are deduplicated; an account
  is checked once per project. With no configured groups, complete bounded directory
  discovery is used, without an invented all-users group row.
- Project totals are unknown until the complete candidate population is known.
  Then counters count finished project/account checks, **including negative
  eligibility results**, not retained assignees, pages or requests. Failed partial
  checks do not increment. A finished project row alone does not make options ready.
- `preparing`/`discovering` cover metadata/groups and may overlap project checks once
  all membership discovery is complete. All projects may finish while metadata is
  still discovering. `fetching` means metadata/group discovery is complete with
  eligibility outstanding, not an all-tickets barrier. `publishing`/`completed`
  distinguish staging from the complete options commit. No aggregate options
  percentage or fabricated metadata total is defined; rows do not expose in-flight
  concurrency.
- Options health and complete scoped choices persist independently of cards; readiness
  still requires current identity verification. Active timing and group/project progress
  are process-local and reset on restart/new jobs, like ticket progress.

All work retains the existing shared per-origin HTTP cap, default **4** requests,
configurable in the single YAML; item concurrency also defaults to 4. Actual YAML
limits remain authoritative even after eligibility expands. Nested work never
multiplies these limits. Pagination, body/time limits and backoff remain mandatory
across both Jira domains. There is no new concurrency setting, raised cap, hybrid
eligibility or new bulk API work, and no promised or measured speedup for this change.
See [configured bounds](configuration.md#controls).

A source failure does not cancel unrelated source reads; sources sharing an origin
can still share rate-limit backoff. Real successful Jira actions schedule refresh;
simulated ones do not mutate issue/source snapshots or schedule refresh. Ticket
validation uses committed choices; normal refresh, explicit URL lookup and enabled
PR AI drafting still have their own read/provider calls. Simulations perform zero
upstream writes and publish no fabricated cache data. The UI may continue showing
the last good data while refreshing or backing off.

## Publication-aware polling

Every committed cache snapshot advances `Board.revision`, including options-only,
persisted health and scope quarantine/restoration commits. Pure progress updates do
not. `/api/sync` exposes the latest committed value as `SyncStatus.board_revision`.
The browser polls about every two seconds while active and every 10–30 seconds while
idle, reloading Board/Context on revision changes **during the job**, not just when
it finishes. Mode has separate two-second polling. These are HTTP polls, not a stream;
phases or multiple revisions may be missed without losing committed data.

Clients read status before dependent Board/Context reads, consume a revision only
after those reads succeed, retry failed reads and reject late regressive responses.
Mid-job publications preserve filters, deliberate sprint choices, editor edits and
captured Test/Live revisions; they never rebase or resubmit an action. Exact wire and
ordering rules remain in `api/CONTRACT.md` and `api/openapi.json`.

The job stays `syncing` until all source/options work terminates. Its last-success
time advances only when **every configured ticket source and Jira options (if enabled)
commit successfully**, never on required failure or skipped backoff. Fresh cards can
coexist with stale ready options; options can succeed despite a ticket failure if
their prerequisites are verified. With no sources, options is null and refresh is a
successful no-op with empty progress.

Stop the server before removing **only your configured cache file** to recover
from incompatible/corrupt disposable state. Never use broad deletion commands on
the config directory. A fresh start fetches real upstream data again. No cache
migration or synthetic replacement data is provided.
