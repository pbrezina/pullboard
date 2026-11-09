# PullBoard REST contract — v1.4.0 (cached Jira choices and explicit defaults)

The normative machine-readable schemas are in [openapi.json](openapi.json).
All paths are same-origin under `/api`, JSON uses `snake_case`, IDs are opaque strings
(including sprint IDs), times are UTC RFC 3339 strings, and nullable fields are sent
explicitly as `null`. Arrays are always present (possibly empty). Unknown request
fields are rejected. Error responses use `ErrorResponse` even for routing, extraction,
validation and upstream errors. No endpoint returns secrets or raw upstream errors.
This extends v1.3.0 with a nullable current project, cached per-type status choices
and assignee emails, and revises dashboard/editor defaults. Existing create request
fields, including name-based `status`, are unchanged. Source-health/progress and
runtime-mode rules remain mandatory. Unknown query keys
are also rejected. A nullable property is still required unless explicitly documented
otherwise.

## Endpoints

| Method | Path | Request | Success |
|---|---|---|---|
| GET | `/api/health` | none | 200 `Health` |
| GET | `/api/context` | none | 200 `Context` |
| GET | `/api/runtime-mode` | none | 200 `RuntimeMode` |
| POST | `/api/runtime-mode` | `ChangeRuntimeModeRequest` | 200 `RuntimeMode` |
| GET | `/api/board` | none | 200 `Board` |
| GET | `/api/sync` | none | 200 `SyncStatus` |
| POST | `/api/sync` | `{}` | 202 `SyncStatus` |
| GET | `/api/jira/options?project_key=KEY&purpose=review&issue_type=Task&query=text` | project/purpose required; type/query optional | 200 `JiraOptions` |
| POST | `/api/ticket-drafts` | `DraftRequest` | 200 `TicketDraft` |
| POST | `/api/jira/issues` | `CreateIssueRequest` | 200 `CreateIssueResult` |
| POST | `/api/jira/issues/{issue_key}/links` | `LinkRequest` | 200 `LinkResult` |
| POST | `/api/upstream-issues/resolve` | `ResolveUpstreamIssueRequest` | 200 `ResolvedUpstreamIssue` |
| POST | `/api/upstream-issues/ticket-drafts` | `UpstreamIssueDraftRequest` | 200 `UpstreamIssueDraft` |
| POST | `/api/upstream-issues/jira-issues` | `CreateUpstreamIssueRequest` | 200 `CreateIssueResult` |

All POSTs require JSON plus `X-PullBoard-Request: 1`. Browser Origin, when present,
must match a configured allowed local origin; reject cross-origin writes (403).
No permissive CORS. Set no-store on API responses. Requests have bounded body sizes.
Unrecognized `/api/*` paths return JSON 404, never the SPA. `/` serves compiled Angular.
Reject non-loopback Host/absolute request authority (including DNS rebinding), and
duplicate/invalid security headers. Mode changes have the same POST protections as
Jira actions; a mode revision is a concurrency precondition, not authentication.

Errors: 400 malformed JSON/unknown fields; 403 origin/header rejection; 404 unknown
cached PR/issue/route; 409 conflicting action; 422 invalid values/configured project;
429 upstream backoff; 502 sanitized upstream failure; 503 source not ready.
Error `code` is a stable snake_case string, `message` is safe for user display.
For all four revision-guarded POSTs, a well-formed but stale/unknown revision returns
`409 mode_conflict` in the unchanged error envelope, before admission or upstream
calls. Missing, null, empty, overlong or non-string revisions are `400 invalid_request`.
Do not return a replacement revision in the error or automatically retry the request.
For options/drafts/create (including upstream issue drafts/create), use
`503 jira_not_ready` before verified identity or complete initial choice discovery;
`502 jira_discovery_failed` after failed reads/permissions/pagination when no usable
last-good scoped choice snapshot exists; `502 jira_metadata_ambiguous` for ambiguous field/schema
resolution; `422 invalid_choice` for an explicitly submitted disallowed project,
type, team, sprint or assignee; and `422 create_blocked` for known required-field or
capability blockers. Missing/invalid required query values are 400. Keep the existing
`ErrorResponse` envelope, not an alternate metadata error payload. A failed lookup
must not masquerade as an empty successful pool. A retained complete choice snapshot
is usable with stale Jira source health; reads/create never retry choice discovery.
A known unsupported mechanism or
unavailable default can instead be a successful options response with diagnostics.

## Snapshots and source health

`Board` is a single coherent cached snapshot. `revision` increases on publication;
`generated_at` is null before the first publication. Each source refresh either
replaces all that source's data, or retains its previous data with error/backoff
health. Publication of the combined snapshot uses one SQLite transaction. Reads must
never observe the delete/insert gap, including concurrent refreshes.

`SourceHealth.status` is exactly `pending`, `ok`, `error`, or `backoff`. A complete
successful refresh is **always `ok`**, for Jira and every forge, even with nonempty
`coverage_notes`. These notes are neutral API/permission/indexing/history details,
not a warning severity, failed refresh or reason for an amber source indicator.
`Attention.coverage: "partial"` remains valid and independent; never promote it to
source health or silently treat unknown attention history as handled. A failed
required read/page is an error, not successful limited coverage.

`pending` means no verified successful snapshot yet. `ok` clears `error`/`retry_at`
to null and updates `last_success_at`. `error`/`backoff` are stale (or unavailable
without a last-good snapshot), retain that source's last-good data/coverage notes
and `last_success_at`, and carry a sanitized nonempty `error`. Only `backoff` has
non-null `retry_at`. `last_attempt_at` is the start of the latest actual attempt;
skipping a source during backoff does not advance it. Expired backoff remains
flagged until the next attempt, rather than silently becoming successful.

`Board.sources` describes its published snapshot. `SyncStatus.sources` is current
process health: normally the same, but advances actual attempt start timestamps and
can report a failure/backoff before publication or a failed cache publication.
An infrastructure failure before an attempt starts can have null `last_attempt_at`.
Success becomes `ok` only after commit.
Use sync health/progress for the live status strip; a `failed`/`backoff` phase must
not be rendered green just because the still-visible Board has old `ok` health.
Progress updates never change cards, identities or cache revision. The existing
principal quarantine rules override retention when identity verification changes.

`SyncStatus.state` is `idle` or `syncing`. POST sync coalesces with an existing job;
workers never overlap. Default interval is 3600 seconds. A successful live Jira
write schedules a refresh (including a follow-up if a refresh is already running);
simulated writes do not mutate the cache or schedule a write-triggered refresh.

### Per-source progress (normative)

`SyncStatus.progress` contains exactly one `SourceProgress` per configured source,
in the same source order as `sources`, joined by unique `source_id`. No aggregate
percentage is defined. The entire GET/POST sync response is a coherent in-memory
status read. Polling remains the transport; clients may miss short-lived phases.

Every entry has `source_id`, `phase`, `completed_items`, nullable `total_items`,
nullable `started_at`, nullable `finished_at`, and integer `elapsed_ms` (all required).
Elapsed time is source-attempt duration, measured with a monotonic clock, including
preparation, discovery, detail fetching and waiting for/performing publication, but
not queued or skipped-backoff time. Sample it on each status read while active;
freeze it at completion/failure/backoff. Wall-clock timestamps are for display,
not a duration calculation. Timing/progress is process-local, never restored as an
active/completed attempt from SQLite or persisted as user preferences.

| Phase | Meaning |
|---|---|
| `idle` | Startup before any job. Zero completed/elapsed, null total/start/finish. |
| `queued` | Accepted job, source waiting to start. Same zero/null reset as idle. |
| `preparing` | Source attempt starts: identity verification, account-scoped metadata/policy preparation. Start set, finish null. |
| `discovering` | Enumerating/paginating and deduplicating issue/PR candidates. Total remains null until the full candidate inventory is known. |
| `fetching` | Fetching all required candidate details/history/links and evaluating items. Start set, finish null. |
| `publishing` | Source fetch succeeded, waiting for the combined atomic transaction. Total is known and equals completed; not completed/green yet. |
| `completed` | Successful atomic publication. Total equals completed; start/finish set; elapsed frozen. |
| `failed` | Actual source or publication failure. Finish set; retain only this attempt's honest counters and freeze elapsed. No partial source replacement. |
| `backoff` | Rate-limited attempt or skipped retry delay. Retry deadline is in source health, not a work total. |

`preparing`, `discovering`, `fetching`, `publishing` are active phases. Adapters
may interleave discovery and fetching, or skip an inapplicable/instant phase; they
must report actual work, not timer-driven invented stages. A completed source can
coexist with `state: "syncing"` briefly while the worker finalizes the job.

Counters count **unique fully fetched/evaluated Jira issues, PRs or upstream issues**, not HTTP
requests, pages, identities, repositories, comments, sprints, metadata, retained
cards or attention threads. Deduplicate candidate identity across searches/pages
(Jira upstream ID; forge stable item ID scoped to source/repository and item kind)
before hydration. Each forge's inventory/counters include both PRs and upstream
issues; an issue discovered through multiple assignment searches counts once.
An item increments completed once, only after all reads needed for its evaluation
succeed. Items filtered out after hydration still count; candidates rejected before
being scheduled for hydration do not. Thus completed/total can exceed retained
cards and are not board counts. A partly fetched item does not increment completed.

Use `total_items: null` if the full unique hydration workload is not yet known;
never use page estimates, sum duplicate search totals, or reuse the last run's total.
If known, total is >= completed; invalidate it to null before extending an inventory
previously thought complete. A known zero means genuinely empty work, not unknown.
At successful publication total is known and equals completed, including `0/0` for
an empty source. Only known positive totals support an item fraction/percentage;
100% fetched during `publishing` does not mean publication has completed. Unknown
totals need an indeterminate bar with the actual count/phase/elapsed duration.

Each newly accepted job clears job `finished_at`, sets job `started_at` at enqueue,
and resets **all** source progress before work begins. Eligible sources become
queued; sources with an unexpired retry deadline become backoff with zero counters,
null total/start/finish and zero elapsed (no attempt). Coalescing does not reset
anything. A new attempt starts at preparing with fresh counters/timing. An in-attempt
rate limit becomes backoff with its actual counters/start/finish/duration. Any
failure, including preparation failure, must exit active phases; a failure before
the source could start has null start, finish set and zero elapsed/counters. A cache
publication failure marks every source still awaiting publication failed, preserves
the old board/revision and does not advance successful timestamps. No fake success
on error, retained-data counts or previous-run duration. Terminal states remain
until a new job; retry deadline expiry alone does not start progress.

Job `finished_at` is set when all work/publication ends; job `last_success_at` changes
only if every configured source refreshed and publication succeeded (not if any
source failed or was skipped in backoff). With zero configured sources the job is a
successful no-op, progress/sources are empty, and the board need not be republished.

Mock examples (not upstream coverage claims) are linked from OpenAPI's sync response:
[startup](examples/sync-startup.json), [active phases](examples/sync-active.json),
[all five source kinds successful with neutral notes, including empty work](examples/sync-completed.json),
[retained-data failure](examples/sync-failed.json), and
[next-job skipped-backoff reset](examples/sync-backoff-reset.json). OpenAPI schema
examples also cover every progress phase, unknown/known totals, in-attempt backoff,
test/live/toggled-back/restarted mode revisions, guarded requests and simulations.
For example, 25 fully evaluated Jira candidates can yield only 9 retained cards;
this is not a partial refresh, and neither number is an attention count.

Run `python api/validate_contract.py` with `jsonschema` and `openapi-spec-validator`
installed for offline schema/reference/example/negative-case validation. It checks
the contract itself, not compatibility of a not-yet-updated backend implementation.

Only identities verified against the current credentials' upstream `/myself` or
`/user` appear in `Context.identities`. `Identity.account_id` is the opaque stable
upstream principal ID, scoped by service/source, never a configured login or token
fingerprint. Display names/logins are discovered; an upstream with no username may
return `username: ""`, not an invented login. Before verification the source has no
identity or private snapshot exposed, and health is pending/error as appropriate.
Identity bootstrap does not require the full issue refresh to succeed. Projects in
`Context.jira_projects` are discovered selected projects with real names and allowed
create issue-type names; they contain no defaults. Before discovery the array is empty;
`Board.sources`/`SyncStatus.sources` distinguish pending/failure from no configured Jira.
Preserve configured YAML project order, not alphabetical/discovery completion order.
`Context.current_project` is the discovered `Project` for the **first configured Jira
project**, or explicit null with Jira disabled, pending or unavailable discovery. Never
substitute a later discovered project, hardcode IDM, or infer it from issue contents.
Same-scope last-good discovery may remain visible on failure with stale source health.
The header uses this project and its real key/name; source draft `default_project`
overrides do not change the header's project.
The [current-context fixture](examples/context-current.json) illustrates discovery
for an example YAML project order `[IDM, CORE]`, not a hardcoded runtime default.

Startup quarantines persisted private data until the current principal is verified.
Verified cache scope includes source ID, kind, normalized service URL, account ID and
relevant project/board/policy scope. A changed principal/service cannot expose an old
user's snapshot, identity or metadata, even if the next read fails. Same-account
refresh failures preserve only that account's last-good data, marked stale in source
health. Choice snapshots are complete and scoped to the verified account, service,
configured projects and committed group/metadata policy. Principal/service scope
changes quarantine incompatible snapshots immediately. Policy changes take effect
only with the next successful atomic refresh; never combine an old pool with a newly
evaluated group policy. Final upstream writes, not cached feasibility/status inventory,
determine current Jira write permission. See choice-cache rules below.

## Attention and display semantics

`Attention.threads` contains only unanswered/unresolved upstream discussion streams.
For each stream, the user's latest reply/qualifying review clears prior events;
explicit resolution clears the stream. Subsequent events reopen attention. A push
alone never clears a thread. General flat comments are one stream, separate from
review threads. Ignore bots and the user's own events when opening attention.
An unanswered review request is its own stream; a submitted review clears that
request, but not unrelated review threads. Jira descriptions/comments use explicit
account mentions and the user's subsequent comment clears prior mentions on that
issue. For Jira, a resolution field or Done status category does not clear attention:
only status `Closed` excludes the issue; every other status remains open. Reply
clearing still applies independently of status.

`waiting_since` is the oldest still-unanswered upstream timestamp, never local
discovery time. Never manufacture timestamps when history is inaccessible.
`reasons` is the unique set of thread reasons. Empty attention is `null`.
Attention columns sort oldest `waiting_since` first; Overview sorts title ascending
(case-insensitive, stable ID tiebreaker). PR reviewers/assignees exclude self and
sort by username ascending. Users may appear as the author/reporter independently.

`is_mine` distinguishes authored PRs from review/mention items. An item can carry
multiple reasons. Jira Overview includes only configured-project issues with
`is_assigned_to_me: true`; mention discovery may also cache issues assigned to others
or unassigned, in any accessible project (not just configured projects). Mention
search matches are candidates, not proof of unanswered attention: handled or false
positive matches have `attention: null`. `linked_issue_keys` and `linked_pr_ids`
represent discovered references and Jira weblinks. Links and all displayed URLs must be HTTP(S), rendered
as text links, never unsanitized HTML. Descriptions are normalized plain text with
newlines (Jira writes encode this into Atlassian Document Format).

Jira discovery uses two separate server-side JQL searches:

- Assigned issues: `project in (<configured project keys>) AND
  assignee = currentUser() AND status != "Closed"`. The placeholder is the configured
  project-key list, not a hardcoded project or an omitted restriction.
- Mention candidates: `status != "Closed" AND (description ~ currentUser() OR
  comment ~ currentUser())`, across **all accessible projects**, without a project
  or assignee restriction.

Both searches use bounded page sizes and follow complete upstream pagination; there
is no date cutoff or arbitrary result cap that silently drops older mentions.
Neither query filters resolution or status category. Never scan all project issues
or fall back to an unfiltered query if mention search fails. A search/page failure
fails the Jira source refresh and retains its last-good snapshot with stale/error
health, rather than publishing an empty or incomplete search result as success.
Deduplicate by stable upstream issue ID across both searches and all pages **before**
fetching per-issue comments/changelog/weblinks. Inspect explicit ADF mentions and
upstream reply timestamps to decide attention, not the search match alone; retain
the existing per-thread clearing and upstream-only timestamp rules. Metadata and
ticket creation remain restricted to configured projects; cross-project mention
discovery does not authorize writes to additional projects or expand the configured
project inventory. Search indexing/history limits are disclosed in source coverage
notes, never worked around with an unfiltered scan or invented attention history.

Sprint selections use actual sprint IDs; `none` is a frontend-only sentinel and
matches an empty `sprint_ids` array. There is **no synthetic eligible-active-sprints
filter option**. Required Attention defaults to all sprints **including no sprint**
so unscheduled mentions are not hidden; preserve a user's deliberate per-tab filter.
Overview initially selects an actual `Context.current_sprints` ID **only when exactly
one matching active sprint exists**. Zero matches use an explicit no-current-sprint
state; multiple matches use an explicit ambiguous-current-sprint/choose-sprint state.
Both leave Overview Jira results empty until the user deliberately selects a filter,
with no automatic first/minimum-ID choice, union or all/no-sprint fallback. Do not
invent a selectable ID for either state. Before initial discovery completes, an untouched
Overview may initialize once discovery yields exactly one matching active sprint. Subsequent refreshes
preserve deliberate edits (including all/no-sprint) rather than reapplying defaults;
an unavailable explicitly selected ID is not silently replaced. Reset filters selects
the sole current actual sprint ID for Overview, or the explicit no-current/choose-sprint
state for zero/multiple matches; it selects all including no sprint for Attention,
and clears title/forge/repository filters in the reset tab. Upstream issues likewise
reset to all forges/repositories and empty text. Filter state is independent per tab.
PR forge filters use instance `source_id`, not just forge type. Title filters are
case-insensitive. Attention badges count unfiltered items; show X of Y with filters.

`Board.sprints` and options return **every** active/future sprint returned by the one
configured board, including foreign-origin and nonmatching names. Context returns
only active, configured-full-name-pattern matching sprints (`default_eligible: true`)
in `current_sprints`, sorted by case-insensitive name then opaque ID ascending.
This is display order, not selection priority. The header shows only these configured
current sprints, not foreign/nonmatching ones (including CORE nonmatches). With multiple
matches it may list/link the candidates but must explain that the current sprint is
ambiguous; it must not present any candidate as the automatically selected current sprint.
`Sprint.board_id` and `board_url`
refer to the board queried, not the origin board or a claim of ownership. Never query
another board using `originBoardId`. `Sprint.team` is a reliably discovered ownership
display name, otherwise null; neither configured team, name pattern nor membership
of a board proves ownership. `team` here is not a ticket team selection key.
`default_eligible` is exactly active AND full-name pattern match; future sprints are
always false. It restricts defaults only, never availability, issue discovery or
attention. No matching active sprint is an empty `current_sprints` array; display it
explicitly rather than guessing a sprint. `profile_url` may be null. Board links use
the trusted HTTP(S) `board_url`. For a sprint view, parse that URL and set its
`selectedSprint` query parameter to the actual opaque sprint ID, preserving the board
path and other query parameters (including `rapidView`). Never concatenate unescaped
query values or infer another board. No additional sprint URL field is needed.

## Upstream issues and deterministic Jira cloning (normative)

### Assigned discovery and snapshot

`Board.upstream_issues` is always present, including `[]` at startup. It contains
only **open non-PR/MR issues assigned to the verified identity** on each configured
GitHub, GitLab, Codeberg or Forgejo instance, including accessible private repositories
permitted by that instance's include/exclude repository policy. `UpstreamIssue.id`
is an opaque stable issue ID scoped by source/repository/item kind; do not interpret
it as a PR ID or assume numeric IDs are globally unique. `number` is repository-local;
`owner` is the namespace (including nested GitLab groups), `repository` the repository
name. `reporter` is the upstream author, nullable for an unavailable/deleted author;
do not invent a person. Assignees include self; PR self-exclusion does not apply.
Descriptions are normalized plain text with newlines; labels are plain-text names,
not Jira label IDs. No comments, attachments, attention, Jira clone status or dedup
fields are introduced. An upstream issue never creates Required Attention.

All four forge kinds use assignment-scoped discovery for their verified principal,
exclude PR/MR candidates even when an issue API returns them, and follow complete
pagination. Repository filters apply after identity-scoped discovery or to scoped
queries; never substitute a broad all-issues scan. No date cutoff, arbitrary item
cap, first-page-only result, or silent search-limit truncation is permitted. If an
upstream search ceiling requires partitioning, enumerate all partitions or fail the
source refresh; a failed/unsupported assignment read/page cannot become an empty
successful result or a broad fallback. Hydrate and verify open/assigned/non-PR state
before retention. A repository enumerator may discover accessible repositories,
but issue reads within them must still be assignment-scoped, not scans of every issue.

PRs and upstream issues share their instance's `SourceHealth`, identity quarantine,
progress and atomic source replacement: failure in either collection retains **both**
last-good collections for the same verified account and marks the source stale.
Successful empty assigned discovery replaces the previous issues with `[]`.
Publication cannot mix old PRs with new issues for one source. All board invariant
and SQLite publication rules apply; no separate source, sync job or issue health API.
Existing Jira assigned/mention discovery stays exactly as specified above. Cloning
does not search Jira for duplicates, scan Jira projects, or discover clone history.

The Upstream issues tab sorts by title case-insensitively with stable ID tiebreaker,
and uses compact full-width rows with title, labels, description snippet, reporter
and namespace/repository. Its forge-instance, repository and title filters are
independent of PR/tab filters; only display preferences may be persisted. Use the
existing accessible editor/explicit actions, not drag-only controls or fixed PR cards.

### Configured-instance URL resolution and trusted storage

`POST /api/upstream-issues/resolve` takes exactly `{source_id, url}`. `source_id`
is a configured forge ID or explicit null. Null selects the unique configured
instance matching the URL's derived web origin/base-path and issue route; ambiguity requires an
explicit source selection (`422 ambiguous_upstream_source`), never guessing an
account or trying several credentials. A non-null source must match that same URL.
No Jira readiness or mode revision is needed for this read-only operation.

Derive the trusted **web base** from configuration, not from submitted URLs or
redirects: GitLab/Codeberg/Forgejo use their configured base unchanged. GitHub's
configured `https://api.github.com/` maps only to `https://github.com/`; a GitHub
Enterprise API base ending in `/api/v3` maps to the same origin with that suffix
removed. Other GitHub bases retain their configured origin/path. These are fixed
adapter mappings, not arbitrary host substitution, discovered redirect targets or
a new YAML setting. Authenticated calls still use the configured API transport.

Validate the URL **before any upstream request**. Only HTTP(S) on that instance's
derived web origin (scheme, hostname and effective port) within its exact web
base-path boundary is allowed. Reject userinfo, query strings, fragments,
backslashes, dot/path traversal, encoded path separators, malformed/nonpositive issue
numbers, unknown sources/hosts/ports and ambiguous routes. Valid canonical paths,
relative to the derived web base-path, are:

| Forge | Issue path |
|---|---|
| GitHub | `/{owner}/{repository}/issues/{number}` |
| GitLab | `/{namespace}/{repository}/-/issues/{number}` (nested namespace allowed) |
| Codeberg / Forgejo | `/{owner}/{repository}/issues/{number}` |

Reject `/pull/`, `/pulls/` and `/-/merge_requests/` routes and PR markers in fetched
issue payloads (including a PR served at an `/issues/` path). API requests are built
from validated route segments and the configured adapter base, **not** by fetching
the submitted browser URL. Respect configured repository policy on explicit lookups
too. Never follow redirects for resolution or forward credentials to another origin;
a repository transfer/redirect requires a new valid URL. Validate returned browser
URLs against the same derived source web base/path/repository/number before
storing/displaying/linking.
This is not an arbitrary URL proxy and does not expand forge write capabilities.

The response is `ResolvedUpstreamIssue {issue, warning}`. Explicit lookup may return
an **unassigned and/or closed** issue; `warning` is nonblank for `state: "closed"`,
otherwise null. Show the closed warning both on resolution and in the clone editor;
closed is not a blocker and no additional acknowledgement flag is required.

On success the backend stores the trusted `UpstreamIssue` **outside the board** in
a bounded process-local resolution store. Later draft/create requests select it by
`upstream_issue_id: issue.id`, just like a board issue; they never accept client issue
content or a client link URL. Scope every store entry and lookup to source ID, kind,
normalized configured service URL, verified upstream account ID and repository policy.
Invalidate/quarantine entries when credentials/principal/service/policy change; do
not reuse another account's resolution even when the textual issue ID is identical.
IDs are selectors, not authorization or mode tokens. No extra bearer token is used.
Current verified board issues and stored manual resolutions are both valid inputs;
draft and create must not require a manual issue to belong to the board, be assigned
or be open. Revalidate current source scope/repository policy on every use.
If both stores contain the same ID, the current scoped manual resolution takes
precedence, so a closed/unassigned lookup cannot silently use older open board data.
Ordinary board refreshes do not overwrite that manual entry; explicit re-resolution
replaces it with the newly verified response.

A manual lookup never changes `Board`, its revision, sync health/progress or its
assignment/closed filters. It is not persisted/restored through SQLite or client
preferences. Keep the resolved issue usable across ordinary board refreshes and
from draft through explicit create, subject only to bounded-store eviction, process
restart or scope invalidation. Missing/evicted/invalidated selectors return
`404 upstream_issue_not_found`; preserve edits and require explicit re-resolution,
not background create replay. Failed manual reads preserve other entries and do not
publish empty or stale manual results as success. Source reads still honor backoff.

Resolution errors use `ErrorResponse`: `422 invalid_upstream_url` for an invalid or
off-instance URL/source, `422 repository_not_allowed` for repository policy rejection,
`422 not_upstream_issue` for a PR/MR payload, `404 upstream_issue_not_found` for absent
issues, `503 upstream_not_ready` before source identity verification, `429` for
backoff and sanitized `502` for read/redirect/permission failures. A selected forge
must remain verified before draft/create can expose stored private content. Initial
full assigned discovery is not required for an otherwise verified manual lookup.

### Draft, options and guarded create flow

1. A board Clone action supplies its issue ID directly; a URL action resolves first
   and then uses the returned issue ID. POST `/api/upstream-issues/ticket-drafts`
   with exactly `{upstream_issue_id, project_key}` (project is explicitly nullable).
   Null selects the source's configured `default_project`; if absent, use the first
   configured Jira project in YAML order, as for PR drafts. An explicit project must
   be configured. Never hardcode IDM or infer a project from issue content, labels,
   repository names, discovery sort order or Jira-wide searches.
2. Return `UpstreamIssueDraft`: the editable draft shape uses `upstream_issue_id`
   instead of `pull_request_id`, `purpose: "implementation"`, and
   `generated_by: "template"` **always**. No AI/provider call, paraphrasing, summary
   prefix, comment/attachment copying or upstream write. Preserve the exact title;
   if it exceeds Jira's create limit the user must edit it, not silently truncate.
   Description is deterministic: take normalized issue description if nonempty,
   followed by a blank line, then `Upstream issue: <trusted canonical issue URL>`;
   if labels exist append a newline and `Labels: <names joined by comma + space>`
   in their returned order. For an empty description start with `Upstream issue:`
   without a leading blank line. Labels and URL are plain text, never Jira labels
   or executable instructions. Do not silently truncate an overlong description.
3. Resolve issue type/team and diagnostics using the shared `implementation` Jira
   options/default resolver. `assignee_id`, `sprint_id`, `story_points` and
   `suggested_status` start null: an upstream issue has no PR draft/ready state,
   so neither workflow suggestion is selected automatically. `warning` carries
   the closed-issue warning or null, not an AI fallback warning. Options and metadata
   readiness/capability/default errors follow existing Jira rules.
4. Edit/reload `/api/jira/options` with `purpose=implementation`. Project/type changes
   use the same allowed inventories, capabilities and final validation as PRs;
   preserve deliberate edits/nulls. Final POST `/api/upstream-issues/jira-issues`
   takes `CreateUpstreamIssueRequest`: existing editor/create fields with
   `upstream_issue_id` replacing `pull_request_id` and purpose fixed to implementation.
   All fields are required, including nullable ones; unknown fields and a review
   purpose are rejected. Do not alter existing `DraftRequest`, `TicketDraft`,
   `CreateIssueRequest`, `LinkRequest`, PR endpoints or PR-only linking scope.
5. This final create is the **only** Jira-writing upstream-issue endpoint. Apply
   identical strict extraction/security and atomic `expected_mode_revision`
   admission **before selector lookup, business validation or upstream calls**, with
   the same in-flight toggle/conflict rules. The editor captures and presents the
   mode/revision when opened; never silently rebase it while resolving or polling.
   Simulated submissions still validate trusted selection/options, return the shared
   `CreateIssueResult` with `outcome: "simulated"`, `issue: null`, an explicit success
   message, **zero** Jira/forge writes, no cache mutation or write-triggered refresh.
6. An admitted live submit creates Jira from the edited values, then adds a Jira web
   link using the server-trusted upstream issue URL/title. Apply only explicitly
   selected supported sprint/status follow-ups. Link/follow-up failure after accepted
   creation returns created success plus warnings, never an error inviting duplicate
   creation. The existing confirmed-reference, accepted-create/unknown-key,
   `write_outcome_unknown` and write-triggered refresh safeguards all apply. Do not
   fabricate Jira IDs, cache an invented issue or retry an uncertain create. No new
   standalone issue-link endpoint, upstream writes, clone tracking/status/filter,
   Jira duplicate search or automatic duplicate prevention is introduced.

Illustrative mock fixtures (not upstream coverage or live Jira claims):
[all-four-forge assigned board](examples/board-upstream-issues.json),
[closed unassigned manual resolution](examples/upstream-resolved-closed.json),
[deterministic editable draft](examples/upstream-draft-closed.json), and
[guarded create request](examples/upstream-create-request.json). The manual fixture
is deliberately absent from the board fixture and remains valid through draft/create.

## Ticket generation and explicit writes (existing PR flow)

1. POST a cached PR ID, purpose (`implementation` or `review`) and nullable configured
   project to `/ticket-drafts`. Backend selects project/team defaults, asks configured
   AI for title/description, and returns a complete editable draft. AI failures or
   missing AI configuration produce `generated_by: "template"` and a safe warning.
   Jira discovery readiness is not an AI failure: return the readiness/discovery
   error instead of inventing a draft from configured metadata. Drafts carry
   `diagnostics` independently of the nullable AI `warning`.
   Review titles use `Review <repository>/<number>: <PR title>` for both AI and
   template drafts, using the repository name (without its owner/namespace) and
   repository-local PR/MR number. The backend sets this title independently of AI
   output, limited to 255 characters; it remains editable before submission.
2. Every new PR/review or upstream editor starts **Unassigned**, **No sprint** and
   **Jira default / no transition** (`assignee_id`, `sprint_id`, `suggested_status`
   all null), regardless of current sprint, self identity or workflow mapping.
   User edits title/description/project/type/story points/assignee/sprint/team/status.
   `/jira/options` supplies discovered permitted issue types, team choices, all
   board-returned active/future sprints, eligible assignees, purpose defaults and
   selected-type capabilities. Switching project, purpose or type reloads relevant
   options from cache. Load the complete assignee list with omitted/empty `query`
   before enabling the dialog's selector, then filter locally by name/username/email
   for instant search; never search upstream arbitrary users. Email is shown/searched
   only when Jira exposes it. A stale retained cache is labeled via Jira source health.
3. Only final POST `/jira/issues` may create an issue. Normally the response returns
   its real confirmed key/URL/title and warnings. Associate the selected PR by Jira
   weblink. If a follow-up sprint assignment/link/transition fails after creation,
   report a warning and the created issue, **not** an error inviting duplicate creation.
   If Jira accepted creation but its success response has no safely usable issue key
   (including a malformed/truncated body), return 200 `outcome: "created"` with
   `issue: null` and at least one nonblank warning. The message/warnings explain the
   accepted creation, unconfirmed key and need to inspect Jira rather than resubmit.
   Do not fabricate a key/URL/cache entry or attempt key-dependent follow-ups without
   a usable key. Frontend handles this as accepted-write success, shows the message
   and warnings without an issue link, and never retries creation merely because
   `issue` is null. The normal live-write refresh still applies. This preserves the
   existing accepted-create/unknown-ID safeguard, not a simulated outcome or new
   error inviting duplicate creation. Workflow mappings suggest statuses; apply only
   the explicitly submitted `status`, never background changes.
4. Drag PR onto issue, or use accessible Link PR action, to POST the PR ID to
   `/jira/issues/{key}/links`. Look up its trusted URL in the cache. Do not accept
   arbitrary upstream URLs. Stable Jira global IDs make duplicate links idempotent.

Read-only test mode validates requests and returns 200 `outcome: "simulated"` with
a success message. It performs **zero** Jira writes (including links/transitions),
returns `issue: null`, and never creates synthetic cache entries/IDs or schedules
a write-triggered refresh. Read-only validation uses the committed choice cache;
normal background refresh may still perform discovery. Frontend uses
the normal success path and an explicit simulated success notification.

### Application-wide runtime mode and admission (normative)

`GET /api/runtime-mode` returns exactly
`{read_only_test_mode: boolean, mode_revision: string}`. `true` is simulated/test;
`false` is live Jira writes. This is one process-wide setting shared by all browser
tabs/clients and all Jira actions, initialized from YAML on every process start.
An override never edits YAML, survives no restart, and is never stored in SQLite,
localStorage or a user preference. Forge adapters remain read-only in **both** modes.

`Health` and `Context` retain `read_only_test_mode` and add `mode_revision`; each pair
is sampled atomically from the same runtime state, never the immutable YAML value.
Separate HTTP responses may straddle a toggle; do not combine one response's mode
with another's revision. Context polling or GET runtime-mode refreshes other tabs;
there is no stream or global ordering between read responses. Discard reads already
in flight when applying a locally successful mode change, then poll afresh.

`POST /api/runtime-mode` accepts exactly
`{read_only_test_mode: boolean, expected_mode_revision: string}` and returns the
resulting `RuntimeMode`. The revision is an opaque nonempty string, at most 128
characters. Clients must only compare equality; it is not `Board.revision`, a
timestamp or an ordered number. Generate a fresh unguessable process revision on
**every restart**, and a never-reused revision for each actual mode change, including
test -> live -> test. A matching no-op request succeeds without changing revision.
Compare the precondition and change value/revision under one lock/atomic operation.
Two concurrent changes with the same old revision cannot both change mode.

`CreateIssueRequest`, `CreateUpstreamIssueRequest` and `LinkRequest` require
`expected_mode_revision` in the
JSON body. After security checks and strict structural extraction, atomically compare
that revision and snapshot the mode under the same mode lock used by toggles. This
is **action admission**, before discovery/read validation or any upstream request;
release the lock before I/O. A mismatched token is `409 mode_conflict` even if the
other submitted business values would fail validation. Admission does not bypass
normal authorization, configured-project, cache, eligibility or backoff checks.
Read-only options/drafts/refreshes need no mode token and cannot write Jira.

An admitted action uses its captured mode for **all** steps, including create,
weblink, sprint assignment and workflow transitions. A later toggle does not cancel,
upgrade, downgrade or revalidate that action's revision. An admitted simulated
operation remains zero-write after switching live. An admitted live operation may
finish and perform its explicitly requested follow-ups **after switching to test**;
the toggle is an admission boundary, not an in-flight revocation/drain barrier.
Its response reflects the admitted action (`created`/`linked`/`simulated`), not the
current indicator. Switching modes alone never schedules sync or touches upstream.
No new action queue, batch, unlink, or standalone linking-scope expansion is introduced: existing
many-to-many single-PR/issue linking remains unchanged and idempotent for duplicates.

Frontend must show a persistent clear test/live indicator and a confirmation dialog
before submitting a change to live, explaining the in-flight rule. A dialog captures
the mode/revision **together when opened**, presents its captured action mode, and
submits that original token, never the latest polled token. Polling or another tab's
toggle must not silently relabel/upgrade/rebase an open dialog; invalidate/block it
on a known mismatch. A direct explicit link gesture likewise uses the mode/revision
the user saw for that gesture. On conflict, preserve edits, refresh runtime mode,
explain the stale action, and require a new deliberate confirmation/submission with
the new mode/revision. Never automatically retry or replace a token on conflict.
The same applies to the mode confirmation dialog itself. A test->live->test round
trip and a server restart invalidate old dialogs even if the boolean is unchanged.
Mode admission is not a durable idempotency key: preserve unknown-create-outcome
safeguards, never replay an unconfirmed live create after disconnect/restart.

### Options, eligibility and defaults (normative)

#### Refresh-time choice cache and freshness

Normal Jira refresh fully loads all configured projects' create metadata, every
allowed issue type's capabilities/team choices, complete eligible assignee pools,
all configured-board active/future sprints and per-type workflow statuses **before**
publishing success. GET options and both draft/create flows consume this cache;
they never enumerate groups, page assignable users, perform per-user eligibility
checks or rediscover metadata/statuses. Opening/searching a dialog must not start
discovery. AI draft generation and explicit live create/follow-up calls remain allowed
for their existing purposes; this is not a general ban on those requests.

Fetch workflow inventory with `GET /rest/api/3/project/{key}/statuses` (Browse Projects),
grouped by upstream issue type. Map each allowed create type uniquely to its real
upstream type and cache that type's status IDs/names. Do not use issue transitions
or Jira-wide statuses as the project/type choice inventory. Status inventory is
**not proof of a permitted transition** for a not-yet-created issue.

Publish choices, their effective project/group policy and Jira source data atomically.
A failed required read/page fails the Jira refresh, preserves the complete same-scope
last-good cache and marks Jira `SourceHealth` error/backoff; never publish an empty,
unrestricted, partially enumerated or mixed-generation pool. Before the first usable
cache, options/draft/create fail with the readiness/discovery error above, not an
apparently successful empty pool. A genuinely fully discovered empty pool is valid.
Cache policy changes affect validation only after the next successful refresh;
validation uses one committed snapshot regardless of the last search query. Final
live create is still subject to Jira's current upstream write/assignment permissions.

No new freshness fields or endpoints: Jira `SourceHealth` (`source_id: "jira"`) in
`Board.sources`/live `SyncStatus.sources` covers the choice cache too. `last_success_at`
is its last successful atomic publication; error/backoff with non-null last success
means retained stale choices. Pending/failure without a scoped last-good cache means
unavailable. Prefer live sync health when it reports a failure before board publication.
An active refresh leaves the previous complete choices usable until commit. Clients
may keep their already loaded choice list while editing; final create validates
against the latest committed cache and returns `422 invalid_choice` if no longer
allowed, preserving edits and never automatically retrying a create.

`JiraOptions` echoes `project_key`, `purpose`, and nullable selected `issue_type`.
An explicit issue type must exactly name a discovered allowed type (otherwise 422).
Without it, select the resolved purpose default, or null if unavailable; never choose
the first/minimum type. `issue_types` remains the full discovered allowed inventory.
Type names must resolve uniquely; ambiguous names are a discovery error, not first-match.
`teams`, capabilities and all non-type defaults are scoped to the selected type.
With no selected type, `teams` and `statuses` are empty, field modes are unknown, required is false
(no known requirement), `unassigned_allowed` is false, and creation is blocked with
`issue_type_required`; assignees and sprints can still be discovered. A completed
empty type inventory is a valid 200 blocked response, not pending metadata.

- Eligibility is the union of fully discovered configured group memberships, deduped
  by accountId, intersected with project-assignable, active human Jira accounts.
  Missing **or empty** `assignee_groups` means all project-assignable active humans.
  Any explicit group missing/ambiguous/forbidden or any failed/incomplete page fails
  the refresh closed, never unrestricted or partial results (502 on options/draft/create
  without a last-good cache). Resolve group names to real IDs
  server-side; clients cannot submit groups. Review excludes the verified self ID;
  implementation retains self if otherwise eligible. Sort display name case-insensitively,
  then username, then ID. `JiraAssignee` preserves Person's fields and adds required
  nullable `email`: real upstream email if exposed, otherwise null, never inferred
  from username/configured credentials or fetched via extra per-user requests.
  `query` is a case-insensitive substring filter over username, display name and
  non-null email within that complete pool; it does not filter defaults or teams.
  A valid nonmatching query yields `assignees: []`. Final create revalidates the same
  cached policy, including self exclusion, independent of the last search query.
- `defaults` is for this purpose only: nullable `issue_type`, `assignee_id`, opaque
  `team`, `sprint_id`, and `workflow: {draft, ready}` (both nullable suggestions).
  Built-in issue type is Task for both purposes, a suggestion, not a claim of
  upstream support. Unavailable Task yields null and `issue_type_default_unavailable`;
  never silently choose another type. `assignee_id` and `sprint_id` defaults are
  always null for both purposes. Legacy configured `assignee: me` remains accepted
  for YAML compatibility but does not preselect self or trigger a missing-self warning.
  Workflow mappings remain suggestions in `defaults.workflow`, not selected editor
  values. A null assignee means **Unassigned**, not self or Jira project auto-assignment,
  and is accepted on final create only if `unassigned_allowed` is true; otherwise
  retain null in the draft with `assignee_required` and require an eligible selection.
- `TeamChoice` is exactly `{id, name}`. Request/draft/default `team` is nullable and
  otherwise a returned opaque `id`, not a label, field ID or arbitrary free text.
  Keys are scoped to current account/project/type metadata and revalidated at submit.
  Backend privately resolves the selected semantic field and typed permitted values:
  an AssignedTeam select option writes its option object, not its label; Atlassian
  Team requires separately proven UUID/string discovery, not a guessed select option.
  Ambiguous field/schema resolution is 502; an unsupported known mechanism returns
  no team choices and an explicit capability diagnostic. Missing/ambiguous configured
  default option yields null; only a required team emits
  `team_default_unavailable`/`team_default_ambiguous`,
  never a first match. A known optional field can be explicitly
  cleared by the user; unsupported required team adds a create blocker.
- Ticket creation never derives a sprint default from `default_eligible`: no, one
  or multiple matching active sprints all produce null without a missing/ambiguous
  sprint-default diagnostic. All returned sprints remain visible for explicit selection;
  capability may disable submission of a non-null sprint. Team/sprint are independent:
  no filtering, default inference, reset, ownership assertion or validation tying
  a team choice to a sprint. Required sprint values still require explicit selection.
- Options defaults and draft initial values use the same resolver. Draft `issue_type`
  is now nullable when no supported default exists; final create still requires a
  nonempty supported string. Draft `suggested_status` is always null, including PRs
  with a configured workflow suggestion. A draft's null/default diagnostics do not prohibit
  editing into a valid request. Preserve deliberate edits/nulls across option refreshes.

`JiraOptions.statuses` is an always-present array of `StatusChoice {id, name}` for
the selected project/type, deduped by opaque upstream status ID, sorted by
case-insensitive name then ID. With no type it is empty. Both create requests keep
the existing `status: string | null`: send a selected choice's **name**, not its ID.
Null means Jira's initial/default state and no transition request. A non-null name
must exactly and uniquely resolve within the cached project/type inventory;
otherwise `422 invalid_choice` before creation. Duplicate-name choices must not be
silently first-matched; require an unambiguous supported choice. IDs remain available
for backend resolution without changing request compatibility. The editor disables
ambiguous-name choices with neutral help rather than submitting an ID as a name.

Status is not put in the Jira create body. After confirmed live creation, for an
explicitly selected status, inspect the actual initial state. If already at the
desired ID, do nothing. Otherwise inspect available direct issue transitions and
perform **at most one direct transition POST** whose destination
matches that ID. Missing permission/direct transition, read failure, ambiguous direct
transition or rejected transition is created success plus a safe nonblocking warning.
Never retry, rollback, create again or search a multi-hop workflow path. No status
choice/default authorizes a sync-time transition. Simulated actions perform zero
writes, fabricate no IDs/state and do not pretend to have proved transition availability.

`CreateCapabilities` has `can_create`, `unassigned_allowed`, `story_points`, `team`,
`sprint`, and `blockers`. Each field capability is `{mode, required, diagnostic}`;
mode is `create`, `follow_up`, `unsupported`, or `unknown`, required is boolean,
diagnostic is nullable `JiraDiagnostic`. Only `create`/`follow_up` authorize non-null
values. Optional unknown/unsupported fields can be left null; required ones block
creation. `can_create` is metadata feasibility: true iff a type is selected and
`blockers` is empty, **not** a claim that the current form's required values are filled
or final authorization has passed. Required fields outside the supported editor
without a proven upstream default add `required_field_unsupported` blockers; never
invent their values. Missing/incomplete upstream reads return 502/503 instead of
`unknown` success. `unknown` represents a successfully read but unrecognized schema.

Only advertise `follow_up` for an implemented, discovered supported mechanism.
In particular, if Sprint is absent from create metadata, never put it in the create
body. Advertise follow_up only if an independently supported post-create sprint
mechanism is implemented and validated; otherwise mark unsupported and reject a
non-null sprint with 422. The API does **not** promise a follow-up or Atlassian Team
implementation. Any follow-up is explicit-submit-only, read-only-mode gated, and
post-create failure returns the real created issue with warnings, never a duplicate
creation invitation. Preserve unknown-outcome safeguards when create itself may
have succeeded but its response could not be confirmed.

`JiraDiagnostic` is `{code, field, message}`: stable snake_case code, nullable public
editor field (`issue_type`, `assignee_id`, `team`, `sprint_id`, `story_points`, `status`),
safe display message, no secrets/raw upstream errors/field IDs. `diagnostics` contains
default/choice guidance; field `diagnostic` explains a capability; `blockers` contains
metadata-level prohibitions. Do not infer severity from prose. In addition to codes
above, freeze `issue_type_default_unavailable`, `assignee_default_ineligible`,
`required_field_unsupported`, and `<team|sprint|story_points>_<unsupported|unknown>`.
Clients display unknown future diagnostic codes safely, without using text as logic.
An empty **optional** assignee/sprint/team/story-points/status is normal, not a
warning. Do not emit `sprint_default_none`, `sprint_default_ambiguous`,
`assignee_default_ineligible` or optional-team-default warnings for the new null
initial values. Unsupported optional capability explanations may remain neutral
field help; do not render them as warnings when the field is left empty. Jira-required
inputs remain required, with explicit selection/validation (`assignee_required` etc.);
never silently fill required selections or downgrade a discovery failure to guidance.

### Discovery scenario examples

These complete `JiraOptions` fixtures are illustrative mock responses, not live
discovery claims or runtime defaults:

| Example | Behavior |
|---|---|
| [Implementation](examples/options-implementation.json) | Task/null; eligible self retained but not selected; No sprint, CORE/future still available, ownership unknown; status choices cached. Assumes Sprint is actually on the create screen. |
| [Review / multiple active sprints](examples/options-review-ambiguous.json) | Task/null; self excluded; two matching active sprints, null creation default, no editor warning. Header explains ambiguity; Overview initially/reset requires an explicit sprint choice, with no automatic selection or all-sprints fallback. |
| [Missing create Sprint](examples/options-sprint-unsupported.json) | All board sprints remain visible; absent Sprint create field and no implemented follow-up yields unsupported, null sprint default, non-null submission rejected. Creating with null is still possible. |
| [Explicit Story type](examples/options-story.json) | Different per-type status inventory; same complete assignee/sprint pools; optional missing team remains null without warnings. |

The second example uses a separate two-matching-active-sprints fixture; it is not
an additional claim about the first example's board. With no matching active sprint,
creation still starts null without a warning. With a validated supported sprint follow-up,
mode would instead be `follow_up`; create body still omits Sprint and failure after
creation yields the real issue and a warning. Never advertise that mode speculatively.

### Example final create request

```json
{
  "pull_request_id": "work-github:acme/widget:42",
  "expected_mode_revision": "mode_mock_boot_a_test_1",
  "purpose": "review",
  "project_key": "DEV",
  "issue_type": "Task",
  "title": "Review widget/42: Cache invalidation",
  "description": "Review the cache invalidation changes and tests.\n\nPR: https://github.com/acme/widget/pull/42",
  "story_points": 2,
  "assignee_id": null,
  "sprint_id": "101",
  "team": "team-choice-platform",
  "status": "Review"
}
```

```json
{"outcome":"simulated","issue":null,"message":"Ticket creation simulated (read-only mode).","warnings":[]}
```

Accepted live creation with an unconfirmed issue key (illustrative malformed-success
response safeguard, not a claim of a particular created issue):

```json
{"outcome":"created","issue":null,"message":"Jira accepted ticket creation but returned no usable issue key. Inspect Jira before retrying.","warnings":["The issue could not be safely linked or transitioned; do not resubmit creation."]}
```

## Configuration boundaries

Single YAML controls server bind/port/allowed origins/static directory/database,
sync interval, Jira URL/credentials/selected project keys/discovery policy,
forge instance IDs/URLs/discovery filters/default project, and AI provider/model/base URL.
Secrets are `{ "value": "literal" }` or
`{ "keyring": { "service": "pullboard", "account": "work-github" } }` in YAML.
Exactly one secret source is allowed. No secret/environment interpolation.
Default AI provider/model: OpenAI / `gpt-6-luna`; never silently substitute a model.
Prefer checked config with read-only mode on. Keyring errors must be explicit.
YAML `read_only_test_mode` is only the runtime mode's startup value; a user-confirmed
runtime override is the sole exception to YAML controlling the current action mode.

### Frozen policy-only YAML shape

When Jira is enabled, `projects` is a nonempty unique array of project-key strings;
`board_id` (positive integer), `team` (nonempty semantic default name), and
`sprint_name_pattern` are **required with no defaults**. Optional `team_field_name`
is an exact semantic field name, default `AssignedTeam`; never accept `customfield_*`
or another raw field ID as a selector. No configured identities/display names,
project names/type inventories, custom field IDs, team option/UUID inventories,
team/project/board mappings or sprint inventories. Reject removed/unknown keys
without echoing their values. Retain Jira Basic-auth `email`, URL and credentials.
Forge `id` remains stable; optional `name` defaults to that ID; remove forge `account`
metadata. No username/account ID is needed in YAML for any service.

Pattern syntax escapes all literals and anchors the whole name. `%YEAR` matches
exactly four ASCII digits; `%NUMBER` matches one or more ASCII digits, not a runtime
year/counter substitution. Reject all other/malformed percent tokens. A nonempty
literal-only pattern is valid. Never interpret input as regex or filter discovered issues.
Optional `assignee_groups` is an array of nonempty semantic names, deduplicated by
exact name; absent/empty has the unrestricted project-assignable meaning above.

The **only** project override map is `project_overrides`, keyed by a configured
project key. Each value accepts **only** `defaults`; it cannot replace board, team,
pattern, groups or field selectors. Both global `defaults` and override `defaults`
accept only `implementation`/`review`. Each supplied purpose object requires
`issue_type` (nonempty exact type name) and `assignee` (implementation: `me` or null;
review: null only), with optional `workflow`. `workflow`, if supplied, requires both
`draft` and `ready`, each a nonempty status suggestion or null. Absent purpose objects
inherit global then built-in Task/null for both purposes, with built-in workflow null/null.
A supplied purpose replaces issue_type/assignee; absent workflow inherits that
purpose's global workflow. Explicit null workflow entries clear inherited suggestions.
Legacy implementation `assignee: me` is accepted but no longer auto-selects anyone;
all editor assignee/sprint/status initial values remain null.
An empty defaults/override map is allowed and changes nothing. Unknown project keys
and any other policy properties are rejected. No configured fixed account-ID assignee,
default sprint ID, per-project inventory or legacy top-level workflow shape remains.
Keep existing explicit workflow intent by migrating it into these purpose policies;
sync never applies it. This scope intentionally does not add multi-Jira or board browsing.

```yaml
projects: [IDM]
board_id: 14585
team: rhel-idm-sssd-identity
sprint_name_pattern: '%YEAR-IDM-SSSD-IDENTITY-S%NUMBER'
assignee_groups: [rhel-sst-idm-sssd]
defaults:
  implementation: {issue_type: Task, assignee: null}
  review: {issue_type: Task, assignee: null}
project_overrides:
  IDM:
    defaults:
      review:
        issue_type: Task
        assignee: null
        workflow: {draft: null, ready: Review}
```

The numbers/names above are illustrative policy, not verified tenant metadata or
hardcoded discovery fallbacks. Discovered fields/options/types must be validated
from upstream read responses, including complete pagination and permissions.

Host defaults bind loopback; container YAML may bind `0.0.0.0:8080` with host
publishing `127.0.0.1:8080:8080`. No forge write capability exists in the REST surface
or adapter interface. `server.allowed_origins` explicitly defines browser origins.
