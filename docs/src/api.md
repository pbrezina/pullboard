# REST API

`api/openapi.json` is the normative OpenAPI 3.1 wire contract; `api/CONTRACT.md`
explains its invariants. Both live in the repository and must be read before
API-facing changes. This guide does not define an alternative payload format.
The current contract is **v1.4.0**: nullable current project, refresh-time cached Jira
choices, exposed assignee emails, per-type status inventories and explicit editor/
dashboard defaults. Existing name-based create `status` fields and upstream-issue
resolve/draft/create endpoints remain; this is not an alternate payload format.

| Method | Endpoint | Result |
| --- | --- | --- |
| GET | `/api/health` | Process health/version and atomic runtime mode/revision pair; not upstream health |
| GET | `/api/context` | Verified identities, nullable first configured current project, pattern-matching active sprints, discovered projects, AI summary and atomic mode/revision pair |
| GET | `/api/runtime-mode` | `{read_only_test_mode, mode_revision}` shared by all clients |
| POST | `/api/runtime-mode` | `{read_only_test_mode, expected_mode_revision}` → resulting mode/revision pair |
| GET | `/api/board` | Single coherent snapshot, revision and source health |
| GET | `/api/sync` | Worker status, live source health/backoff and per-source progress |
| POST | `/api/sync` | `{}` requests/coalesces a refresh; 202 |
| GET | `/api/jira/options?project_key=DEV&purpose=review&issue_type=Task&query=text` | Cached types/teams/statuses, all board active/future sprints, permitted assignees, purpose defaults/capabilities/diagnostics |
| POST | `/api/ticket-drafts` | Cached PR ID, purpose and optional project → editable proposal, no Jira writes |
| POST | `/api/jira/issues` | Explicit create request including `expected_mode_revision` → created or simulated result |
| POST | `/api/jira/issues/{issue_key}/links` | Cached PR ID plus `expected_mode_revision` → linked, duplicate or simulated result |
| POST | `/api/upstream-issues/resolve` | `{source_id, url}` → trusted `{issue, warning}`; configured-instance read, no board mutation |
| POST | `/api/upstream-issues/ticket-drafts` | `{upstream_issue_id, project_key}` → editable deterministic implementation draft; no AI/writes |
| POST | `/api/upstream-issues/jira-issues` | Explicit implementation create with `upstream_issue_id` and `expected_mode_revision` → shared created/simulated result |

Example local checks (no credentials in these requests):

```sh
curl --fail http://127.0.0.1:8080/api/health
curl --fail http://127.0.0.1:8080/api/runtime-mode
curl --fail http://127.0.0.1:8080/api/sync
curl --fail -X POST http://127.0.0.1:8080/api/sync \
  -H 'Content-Type: application/json' -H 'X-PullBoard-Request: 1' --data '{}'
```

Browser Origin is checked when present. CLI requests may omit it but still need
the custom header and JSON for POST. Local Host checks apply to reads too.

## Upstream issue selection and creation

`Board.upstream_issues` is a **required array**, including `[]` at startup, containing
only open non-PR/MR issues assigned to the verified identity on configured GitHub,
GitLab, Codeberg and Forgejo sources after repository policy. Issue IDs are opaque,
source/repository/item-kind scoped selectors, not PR IDs or authorization tokens.
Reporter may be null; assignees include self. No attention or clone-status fields
are added. Each forge atomically replaces or retains **both** PRs and issues under
the same source health, quarantine and progress rules.

Both resolve fields are required: `source_id: null` selects the unique configured
source matching the URL's derived web origin/base-path/issue route. Multiple matches
require explicit selection (`422 ambiguous_upstream_source`). A selected source
must match the URL. The server rejects noncanonical/off-instance URLs, credentials,
query/fragment/traversal, PR/MR routes and payloads, and repository-policy violations
before accepting trusted content; redirects are not followed. Authenticated reads
use configured API transport, not the submitted browser URL. See
[canonical routes and GitHub web-base mapping](upstream-issues.md#canonical-urls-only).

Resolve is read-only and needs no Jira readiness or mode revision. It can return
closed/unassigned issues: `warning` is nonblank for closed issues, otherwise null.
It does not publish to Board or alter sync health/progress. Trusted manual content
is stored process-locally in the current verified source/service/principal/policy
scope, at most 128 entries/8 MiB of serialized issue content. It survives ordinary
board refreshes and takes precedence over a board copy with the same ID, but not
eviction, restart or scope invalidation. An unavailable selection returns
`404 upstream_issue_not_found`; preserve edits and explicitly resolve again, never
automatically replay creation. No client issue content or link URL is accepted.

Draft requests select a current board or manual issue. Required nullable
`project_key: null` uses the source's `default_project`, then the first configured
Jira project in YAML order—not a hardcoded project. `UpstreamIssueDraft` always has
`purpose: "implementation"`, `generated_by: "template"`, null initial assignee,
sprint, story points and suggested status. It preserves exact title and normalized
description, appending the trusted upstream URL and labels as plain text. No AI, comments/attachments,
Jira duplicate search or silent truncation. Jira implementation options/capabilities
and readiness rules apply; show closed warnings in the editor.

Final create uses `CreateUpstreamIssueRequest`: existing editor fields with
`upstream_issue_id` replacing `pull_request_id`, purpose fixed to implementation,
and required `expected_mode_revision`. All fields, including nullable fields, are
required. After strict structural/security checks, mode admission occurs **before**
selector lookup, business validation or upstream calls. Only this endpoint writes
Jira: live create uses edited fields, then a server-trusted issue web link and any
explicitly selected supported follow-ups. Shared simulated/accepted/unknown-outcome
safeguards apply. No forge writes, standalone issue-link endpoint, clone tracking,
Jira-wide scan, automatic deduplication or replay are introduced; PR endpoints and
PR-only linking are unchanged.

Resolve also uses the ordinary error envelope: `422 invalid_upstream_url`,
`422 repository_not_allowed`, `422 not_upstream_issue`,
`404 upstream_issue_not_found`, `503 upstream_not_ready`, 429 backoff or sanitized
502 read/permission/redirect failure. The forge must be verified before exposing
private content in resolve/draft/create; initial assigned discovery is not required.
Illustrative fixtures live at `api/examples/{board-upstream-issues,upstream-resolved-closed,upstream-draft-closed,upstream-create-request}.json`.

## Refresh and publication

`SourceHealth.status` is `pending|ok|error|backoff`, never `partial`. A successful
refresh is always `ok`; `coverage_notes` are neutral information. Per-item
`Attention.coverage: "partial"` remains independent. `Board.sources` describes the
published snapshot; `SyncStatus.sources` can already report current failure/backoff
while its cards remain visible. A successful source is not green until publication.

`SyncStatus.progress` has one entry per configured source, matching `sources` IDs
and order: `source_id`, `phase`, `completed_items`, nullable `total_items`, nullable
`started_at`/`finished_at`, integer `elapsed_ms`. Counts are deduplicated **fully
fetched/evaluated** issues/PRs, including candidates filtered out afterward, not retained
cards or requests. Unknown workload is `null`, empty successful work is `0/0`.
Forge counts include fully evaluated PRs **and** upstream issues, not just list rows.
There is no aggregate percentage. Timing is monotonic source-attempt duration including
publication waiting, excluding queue/skipped backoff. All progress is process-local.
See [phase/reset/failure semantics](security-cache.md#per-source-progress) and the
normative contract; examples live at `api/examples/sync-*.json` in the repository.

`Board.revision` increases on atomic publication; `generated_at` is null before the
first publication. Progress does not change that revision. A publication failure
preserves the old board. Polling remains the transport, not streaming.

## Runtime mode preconditions

`read_only_test_mode: true` means simulated Jira actions; `false` permits explicit
live submissions. YAML initializes the backend-global mode on every restart; overrides
are never persisted or written to YAML. Forge adapters stay read-only in both modes.
Health, Context and RuntimeMode each return an atomic boolean/revision pair; never
combine values from separate responses.

`mode_revision` is an opaque equality-only, nonempty string of at most 128 characters,
fresh on each restart and actual toggle, including toggling back. A matching no-op
mode change keeps the revision. It is not a board revision, authentication token or
durable idempotency key. All four revision-guarded POSTs (mode change, PR create,
PR link and upstream-issue create) require the original
`expected_mode_revision` captured with the mode when the dialog/gesture begins.
Missing/null/empty/overlong/non-string tokens yield `400 invalid_request`;
well-formed stale/unknown tokens yield `409 mode_conflict` after security/structural
checks but **before admission, upstream calls or business validation**. The ordinary
error envelope contains no replacement token. Preserve edits, refresh mode and require
explicit review plus a separate submission; never auto-retry or silently replace tokens.

The server atomically compares the token and captures the mode at admission, then
uses that captured mode for every action step/follow-up. Later toggles cannot upgrade
simulations or cancel admitted live work: live operations may finish after switching
to Test. A toggle alone makes no upstream calls and schedules no refresh. The UI
confirms Live changes, polls shared mode, discards pre-toggle in-flight mode reads
and blocks known-stale dialogs. Read-only options/drafts/refresh need no mode token.
See [user review flow](tickets-ai.md#stale-dialogs-and-gestures).

## Discovery and errors

JSON uses `snake_case`; IDs are opaque strings (including sprints), timestamps are
UTC RFC 3339, nullable fields are explicit `null`, arrays always exist. Unknown
request fields are rejected. No endpoint returns credentials or raw upstream errors.
Unknown query keys are rejected too. Options requires `project_key` and `purpose`;
`issue_type` is optional (exact discovered name), `query` filters cached assignees by
username/display name/non-null email only, never authorization/defaults. Omit query
to load the complete pool for local filtering. Groups are server policy, not browser
parameters.
Unknown `/api/*` paths are JSON 404, **never** the SPA index.

`Identity.account_id` is the verified upstream principal; pre-bootstrap identities
are empty, not synthesized. `Project` contains discovered key/name/issue_types only.
`Sprint.team` is nullable ownership, `default_eligible` controls defaults only.
Ticket `team` is a returned opaque `teams[].id`, not a label, field ID or sprint team.
Purpose-specific defaults include nullable type/team and workflow suggestions;
assignee/sprint defaults and draft `suggested_status` are always null.
Drafts may have null type and carry Jira `diagnostics` separate from AI `warning`.
Selected-type capabilities (`create`, `follow_up`, `unsupported`, `unknown`) and
required-field blockers govern editing/submission; `can_create` is feasibility,
not guaranteed write authorization. See [Jira discovery](jira.md) for implemented
sprint follow-up versus unsupported Atlassian Team discovery.

### Current context and cached choices

`Context.jira_projects` preserves YAML project order. Required nullable
`current_project` is the discovered **first configured** project, never a later
available project or hardcoded IDM; it is null with Jira disabled/pending/unavailable
discovery. `current_sprints` contains only active full-pattern matches, sorted by
case-insensitive name then ID for display, not selection priority. Board/options
still return every active/future sprint. Header links set `selectedSprint` on trusted
`board_url`, preserving other query parameters. Overview initially/reset selects an
actual ID only for one match; zero/multiple matches leave an explicit empty/choose
state until deliberate selection. Attention defaults all/no sprint. No synthetic
eligible-sprints filter; deliberate per-tab choices survive refreshes.

`JiraOptions.assignees` uses `JiraAssignee` (Person fields plus required nullable
`email`, exposed upstream only). `statuses` is an always-present array of real
`{id, name}` choices scoped to the selected project/type, deduped by ID and sorted
case-insensitively by name then ID. With no selected type, teams/statuses are empty
and creation blocked; no first-type fallback. Workflow suggestions remain in metadata,
not initially selected editor status. Every editor starts Unassigned/No sprint/no
transition, including legacy configured `assignee: me`.

Normal Jira refresh fully discovers choices for every allowed type/project and
publishes them atomically with cards and effective policy in ephemeral SQLite.
Options/drafts/create never repeat group, per-user, metadata or status discovery.
An active refresh leaves old complete choices usable. Failure retains the complete
same-scope last-good generation with stale Jira `SourceHealth`; without an initial
usable snapshot, fail with readiness/discovery errors. Group/policy changes apply
at the next successful refresh. Final create uses the latest committed cache,
independent of the last query; current Jira permissions govern live writes.

Create `status` remains an exact uniquely resolving cached **name**, not ID/free text;
unknown/ambiguous names fail `422 invalid_choice` before creation. Null means Jira
default/no transition. After confirmed live creation, already at target is a no-op;
otherwise attempt at most one direct transition to its real ID. Inventory does not
prove reachability. Missing/rejected/ambiguous transitions return created success
with a nonblocking warning, never rollback, retry, multi-hop or second create.
Simulated actions perform zero writes and cannot prove transition availability.

### Error and outcome handling

Errors share `ErrorResponse`: 400 malformed/unknown fields, 403 origin/header/host
rejection, 404 missing cached resource/route, 409 conflict, 422 invalid value/project,
429 backoff, 502 sanitized upstream failure, 503 source not ready. Match stable
snake_case `error.code` rather than parsing display messages. After an uncertain
Jira creation outcome, inspect Jira before retrying.

Readiness is `503 jira_not_ready`; discovery/permission/pagination failures use
`502 jira_discovery_failed`, ambiguity `502 jira_metadata_ambiguous` (rate limits
retain 429/backoff). Submitted disallowed choices use `422 invalid_choice`, known
required/capability blockers `422 create_blocked`. A failed read is not empty success.
Known unavailable defaults/unsupported mechanisms can return 200 with diagnostics.
The frozen API examples include an unsupported-sprint scenario; this is not a
claim that every missing-create-Sprint case is unsupported: the backend advertises
`follow_up` only when its implemented mechanism's discovery prerequisites are met.

Simulated creation returns 200 and `issue: null`, not a fake key.
Simulations validate cached choices but cause no upstream writes/cache publication or
write-triggered refresh. Post-create failures return the real issue with warnings,
not a duplicate-create invitation.
If Jira accepted creation but returned no safely usable key, 200 `outcome: "created"`
may have `issue: null` **only with at least one nonblank warning**. Treat it as accepted
success, show the message/warnings without an issue link, skip key-dependent follow-ups
and never retry creation automatically. No key/URL/cache entry is fabricated; the
normal live-write refresh still applies. Existing single-pair many-to-many linking
and duplicate-link idempotency are unchanged; no batch/unlink endpoint is added.
Contract tests use the shared JSON schema against backend responses.
