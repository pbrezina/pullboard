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

SQLite is an ephemeral cache initialized without migrations. A publication is one
coherent combined snapshot, written atomically in a transaction. A successful
source refresh replaces that source's snapshot. A same-account failure preserves its
last-good data and last-success timestamp while updating health. The browser never
sees a delete/insert gap as a half-empty board.

On startup persisted private snapshots, identities and weblinks are quarantined
until the current credentials' upstream principal is verified. Only an exact scope
match can restore data: source ID, kind, normalized service URL, real account ID,
and relevant project/board or forge-filter scope. Changed principal/service or
incompatible configured scope purges old private data before later read failures
can expose it; derived cross-source links are recomputed. Jira group/default policy
is instead committed with its complete choice generation, never mixed into old pools.
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
metadata/capabilities/team/status choices, all permitted assignees and board sprints,
plus their effective group/default policy. They publish atomically with Jira cards
and identity, not as a separate authorization database. Overlapping refreshes coalesce;
options/drafts/create use a single committed generation and never repeat group/user/
metadata discovery. Compatible restored choices remain quarantined until current
identity verification.

A failed refresh retains complete same-scope last-good choices and context as usable
but stale; Jira source health covers their freshness. Before usable initial discovery,
ticket endpoints fail explicitly with 503 readiness or 502 discovery/ambiguity errors,
not empty success. An active refresh leaves previous choices intact until commit.
Group/default-policy changes become effective only on successful refresh, not by
reevaluating new policy against an old pool. Final create validates the latest
committed generation independent of the editor's last search; no-longer-allowed
selections are rejected without replay. Jira still enforces current write permissions.
See [choice freshness](jira.md#refresh-time-choices-and-freshness).

## Source health

Health states are exactly `pending`, `ok`, `error`, `backoff`. Successful complete
refreshes are **always `ok` (green)** for Jira and every forge, even with nonempty
`coverage_notes`. Expand Source health → **Coverage information** for neutral
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
successful health. Fetch success becomes `ok` only after the combined transaction
commits. Progress alone does not publish cards, identities or a new board revision;
account-verification quarantine still overrides old-data retention.

## Per-source progress

During an ongoing refresh, configured sources have separate panels/bars above the
board, joined by stable source ID, with actual phase, fully evaluated item count,
known/unknown total and elapsed time. Real failures/backoff remain prominent there.
When healthy and idle, the panels collapse: expand the existing **Source health →
Details** to read completed counts and durations. This uses no new display preference.
There is no aggregate percentage or invented work total.

| Phase | Meaning |
| --- | --- |
| `idle` | Process startup before any job; no work counted. |
| `queued` | Accepted job, waiting to start this source. |
| `preparing` | Verifying identity and preparing account metadata/policy. |
| `discovering` | Enumerating pages and deduplicating candidate issues/PRs. |
| `fetching` | Reading required details/history/links and evaluating candidates. |
| `publishing` | Fetching finished; waiting for or performing the combined atomic publication. |
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
indeterminate, with the actual count shown as “total unknown.” A known positive total
permits an item fraction; `0 / 0` is genuinely empty successful work, not an estimated
percentage. At `publishing`, all items are fetched but the board is **not yet updated**.
The previous board remains visible until the combined snapshot commits atomically.
A publication failure preserves its revision/data and marks waiting sources failed.

Elapsed time comes from a server monotonic clock: it includes preparation, discovery,
fetching and publication waiting/work, but excludes queue time and skipped backoff.
It freezes on completion/failure/backoff. A skipped source has zero counters/elapsed
and unknown total, not the preceding run's work; a rate-limited attempt retains its
actual work and duration. Each new job resets every source's progress; a coalesced
request does not. Terminal progress stays until the next job and resets on restart;
it is not persisted in SQLite or preferences.

The default worker interval is **3600 seconds**. A manual refresh coalesces with an
existing job; jobs do not overlap. Within that job, up to four source read futures
run concurrently, with independent outcomes and one final atomic publication. A
source failure does not cancel unrelated source reads; sources sharing an origin
can still share rate-limit backoff. Real successful Jira actions schedule refresh;
simulated ones do not mutate issue/source snapshots or schedule refresh. Ticket
validation uses committed choices; normal refresh, explicit URL lookup and enabled
PR AI drafting still have their own read/provider calls. Simulations perform zero
upstream writes and publish no fabricated cache data. The UI may continue showing
the last good data while refreshing or backing off.

The browser polls sync about every two seconds while active and every 10–30 seconds
while idle, reloading board/context at observed job/publication boundaries. Mode has
separate two-second polling. These are lightweight HTTP polls, not a stream; brief
phases may be missed. Job last-success time advances only when all configured sources
succeed and publication commits, not when a source fails or is skipped in backoff.
With no sources, refresh is a successful no-op with empty progress.

Stop the server before removing **only your configured cache file** to recover
from incompatible/corrupt disposable state. Never use broad deletion commands on
the config directory. A fresh start fetches real upstream data again. No cache
migration or synthetic replacement data is provided.
