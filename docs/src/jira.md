# Jira discovery, defaults and workflows

## Select policy; discover metadata

`jira.email` and the token provide Basic authentication. The authenticated
`GET /rest/api/3/myself` discovers the authoritative account ID/display name;
do not configure either. `projects: [DEV]` selects project keys, not names or type
inventories. Supply required ordered `board_ids`, semantic `team` and ordered `sprint_name_patterns`;
none has a default. Example names/numbers are sanitized illustrations, not tenant facts.

Configured projects restrict assigned-issue discovery, metadata, ticket creation
and explicit PR linking. The separate description/comment mention search spans all
accessible projects without expanding those permissions or the project inventory;
see [provider queries and limits](providers.md#jira).

Refresh verifies identity once and fetches ticket/display prerequisites first.
Independent assigned/mention searches and configured-board sprint reads use bounded
concurrency, then the complete deduplicated inventory is hydrated. Complete Jira
tickets, board/current sprints and the first YAML project's real key/name publish
early, without waiting for creation choices or substituting a later project. Its
issue-type list may remain empty until options metadata commits; project visibility
is not creation readiness.

Creation discovery may overlap Jira ticket work, reusing visible fields from ticket
preparation and reading selected projects, fully paginated create types and per-type
create metadata, project/type status inventories, primary-board configuration
and project permissions.
Project names and supported issue types come from those reads. It identifies Sprint
by the `gh-sprint` schema and numeric Story Points by the **first board's** estimation field,
not by guessed field IDs. Missing permissions, incomplete pages and ambiguous field
or type names fail explicitly, never as a successful empty inventory.

### AssignedTeam is not Atlassian Team

The exact semantic field name defaults to **`AssignedTeam`**. Optional
`team_field_name` selects a different exact semantic name; raw field IDs are rejected.
A supported select field uses its selected project/type create `allowedValues`;
the backend resolves choices to typed option objects `{id: discovered_option_id}`.
Missing/incomplete allowed values fail discovery; there is no admin-only field-context
or option API fallback. Duplicate fields are an ambiguity error. A missing or
ambiguous configured default option produces null, not a guess; it produces a
default diagnostic only when the team field is required.

The editor receives opaque choice keys and labels, not arbitrary free-text team
values. Separate **Atlassian Team UUID/string discovery is unsupported**: selecting
that field does not turn a select option ID into a Team UUID. Unsupported optional
fields may be left null; unsupported required fields block creation.

## Ordered boards and sprint patterns

Configure 1–100 unique positive `board_ids` and 1–100 unique `sprint_name_patterns`,
both in deliberate order. The first board is **My board**; the first pattern is
**My sprint pattern**, just as the first configured project is yours. These lists
are independent, not positional pairs. Neither has a default. The removed singular
keys are rejected; see [migration](configuration.md#migrate-singular-sprint-settings).

At runtime PullBoard fully paginates
`GET /rest/agile/1.0/board/{board_id}/sprint?state=active,future` for **every configured
board** under the existing shared transport bounds. Duplicate sprint IDs prefer the
earliest configured board returning that sprint, independent of request completion
order. Foreign-origin sprints remain included; PullBoard never follows `originBoardId`.
Estimation metadata comes only from the first board. Runtime does not enumerate a
board inventory; the [setup wizard](setup.md#jira-cloud) can help select boards.

Board filters offer **every collected active/future sprint**, grouped **Active** and
**Future**, plus no sprint/all sprints. Neither patterns nor ticket team restrict those
filters, ticket discovery, existing assignments or Attention. Queried `board_id` and
`board_url` show provenance, not ownership; sprint `team` is null unless ownership is
reliably discovered, never inferred from the configured team or name.

`default_eligible` means **active AND returned by the first board AND matching the
first Rust regex**. A plain `SSSD-IDENTITY` searches anywhere; matching is case-sensitive
unless inline flags change it, and anchors are optional. There are no percent-token
substitutions; see [configuration](configuration.md). Who-am-I shows only these current
matches, their available end dates and links on **My board**. No match and multiple-match
ambiguity are explicit; an additional board or pattern is never a current-sprint fallback.
Exactly one match supplies Overview's initial/reset actual sprint ID. Zero or
multiple matches leave its Jira results empty with an explicit no-current/choose
state until the user selects a filter; never choose the first/minimum ID, a union
or all-sprints fallback. Attention defaults all/no sprint. There is no synthetic
eligible-sprints selector. Deliberate per-tab selections survive refreshes; see
[filters and reset](attention.md#tabs-and-filters).

**Creation choices** include only collected active/future sprint names matching **any**
pattern in the committed options generation, grouped **Active** and **Future**. This
applies to implementation, review and upstream cloning, independently of ticket team;
it is not restricted to My board or active current matches. Unmatched sprints stay in
board filters and existing assignments, not creation choices.

**Ticket creation always starts No sprint**, with no missing/ambiguous-sprint-default
warning, regardless of the number of current matches. Required sprint input still needs
explicit selection. Team and sprint are independently editable: neither filters,
resets or validates the other.

## Assignees: complete candidates, refresh-time checks

`assignee_groups` accepts multiple semantic names. Each is resolved uniquely to a
real group ID and membership is fully paginated. Candidates are the **union**,
deduplicated by accountId, restricted to active human accounts and then checked
individually for project assignability with
`GET /rest/api/3/user/assignable/search?project=...&accountId=...`.
Configured groups do **not** use broad capped assignable-search range pagination or
an all-user directory scan. Repeated group names and overlapping accounts are
deduplicated; each candidate is checked once per project through the refresh's bounded
eligibility queue, not on every editor request.

With absent or empty groups, the backend fully paginates plural
`GET /rest/api/3/users/search`, then applies the same individual checks. It advances
by returned count, including inactive/app users, and continues past short pages to
an empty end page. It does not use singular `/user/search` or infer users from issues.
Broad assignable-search enumeration cannot prove completeness: Jira documents
filtering scanned ranges within the first 1000 users, so a short/empty filtered
range is not a complete candidate inventory.

Missing/ambiguous/forbidden groups, failed pages, repeated/nonprogressing pages,
unexpected individual results and safety bounds fail closed: no partial pool or
unrestricted fallback. The candidate limit is `max_pages * page_size` (10,000 with
defaults), with bounded pagination per group. These are failure limits, not truncation.
Empty individual results exclude that candidate only; permission failures reject
the whole pool.
The browser loads the complete eligible pool and instantly filters it locally by
display name, username and email **only if upstream exposes it**; it never infers
emails or sends an options request per keystroke. Review excludes self; implementation
keeps eligible self available but unselected. Final create validates against the
latest committed pool and policy, regardless of the last search query.

Directory/group visibility and Browse users and groups permissions may be needed;
the absent/empty-group path requires broader all-user enumeration access. Project
assignment permissions must also be available. Request cost can be high: group or
directory pages **plus one request per candidate per project at refresh**. Options,
drafts and create do not repeat those group/user checks. Rate limits and page/response
bounds are real operational limits. A small observed group does not prove pagination
or permissions universally.

After one verified Jira identity, tickets, creation metadata/statuses and groups may
load concurrently. Eligibility waits **only for complete deduplicated membership
candidates**, or complete human inventory without groups, not metadata or tickets.
One owned/cancellable bounded project/account queue allows at most one eligibility
request/item in flight while any configured card source is queued, preparing,
discovering, fetching or publishing. It dynamically expands to existing configured
item concurrency after all card attempts terminate, including failure/skipped backoff.
Metadata/groups may compete; this is not preemption or a fixed-latency promise, and
HTTP cap 1 naturally serializes reads. The existing shared per-origin HTTP cap
(default 4, configurable in YAML), item bounds, pagination, response/time limits and
backoff remain authoritative. No new setting, multiplied/raised cap, hybrid/bulk API
or measured speedup is part of this change.

Inside **Source health**, Jira's compact group/project rows share one options phase
and elapsed time; there is no separate metadata progress panel. Group membership
stays indeterminate until full pagination and deduplication complete, then shows
unique member totals. Per-project eligibility
counts finished checks, **both eligible and ineligible**, not people retained. Totals
are unknown until the complete candidate inventory is known. A project finishing its
checks does not expose a partial pool or make the full options generation ready.
Options preparing/discovering may overlap project checks or wait for metadata after
all projects complete. Fetching means metadata/groups complete with checks outstanding,
not a card barrier. See
[options progress](security-cache.md#ticket-favoring-concurrency-and-options-progress).

## Refresh-time choices and freshness

Normal Jira refresh fully discovers every configured project's permitted assignees,
all allowed types' metadata/capabilities/team choices and statuses, and all configured
boards' active/future sprints. Choices, their **own complete sprint snapshot** and effective group/default/pattern
policy publish as one complete atomic options generation in ephemeral SQLite,
independently of Jira cards, possibly before tickets. Verified prerequisites/sprint
reads are reused without waiting for ticket hydration. Validation never merges newer
board sprints or live YAML pattern policy with an old pool. Creation inclusion and
final validation use that same generation's complete sprint snapshot and regex policy.
Opening/searching a dialog,
drafting and create validation consume that cache, never start metadata/group/user
discovery. AI generation and explicit live write/follow-up requests retain their
separate purposes.

Failed required reads/pages retain the **complete same-scope last-good generation**
and mark **options**, not Jira tickets, stale/error/backoff; no partial, mixed-generation
or unrestricted pool is published. An active refresh leaves previous complete choices
usable until commit; expected assignability is never accepted optimistically. Before
the first verified complete committed cache, creation stays blocked and
options/drafts/create fail explicitly with readiness or discovery errors, not
successful empty results. A genuinely fully discovered empty
pool is different. Principal/service/config scope changes cannot expose another
account's private choices; compatible restored snapshots remain quarantined until
identity verification.

`SyncStatus.jira_options` supplies independent `ready`, persisted `health` and
process-local `progress`. A stale complete scoped generation remains ready; absent,
unverified or quarantined choices do not. Jira source health covers tickets/sprints
only, so fresh tickets can coexist with failed options, or vice versa when prerequisites
permit. Each domain retains only its own scoped last-good data; scope changes quarantine
incompatible cards **and** choices immediately. The editor shows a quiet stale-choices
note and **Refresh Jira choices**; the creation-options panel carries failure details.
Group and pattern/default policy changes apply only with the next successful atomic options publication,
never to an old pool alone. The job's last-success time requires all ticket sources and
options to succeed; a domain failure cannot roll back another successful commit.
An open editor can keep loaded choices; final validation uses the latest committed
generation and rejects no-longer-permitted values while preserving edits. Jira's
current upstream permissions still determine whether a live write succeeds.

## Purpose defaults and create capabilities

Built-ins are **Task/null for both purposes**. Task is a suggestion, not invented
upstream support; an unavailable type stays null with a diagnostic, not a first-type
fallback. All PR/review/upstream editors start **Unassigned**, **No sprint** and
**Jira default — no transition**. Legacy implementation `assignee: me` is accepted
but never selects self. Null assignee means Unassigned, not Jira auto-assignment;
if that is not permitted, explicitly choose an eligible person before submitting.
Optional blank fields are normal, not a warning wall; required fields remain
explicitly required. Optional global purpose policies and `project_overrides` can
change type/workflow suggestions within
the strict [policy schema](configuration.md), not configure fixed account IDs.

Options reload per project/type and report points/team/sprint modes: `create`,
`follow_up`, `unsupported`, `unknown`, requiredness and blockers. Only the first two
permit non-null values. Unknown/unsupported required fields and unhandled required
fields without a proven default block creation. `can_create` describes metadata
feasibility, not a complete valid form or guaranteed upstream write permission.

When writable Sprint metadata has a supported schema, create uses the discovered
numeric setter. **If Sprint is absent from create metadata, it is omitted from the
create body.** Implemented follow-up support independently requires a discovered
scrum board, visible gh-sprint field and project EDIT_ISSUES + SCHEDULE_ISSUES
permissions. The selected sprint must appear in the latest committed **options
generation's** multi-board inventory and match any of that generation's patterns,
not live YAML or a newer card snapshot; submit does not rediscover it.
After explicit submit and confirmed creation it uses Jira Software Cloud's documented
[Move issues to sprint and rank](https://developer.atlassian.com/cloud/jira/software/rest/api-group-sprint/#api-rest-agile-1-0-sprint-sprintid-issue-post):
`POST /rest/agile/1.0/sprint/{sprintId}/issue`, body `{"issues":["REAL-KEY"]}`,
without ranking fields. This is not a generic follow-up API promise or proof of a
successful live write. Metadata/permissions can change and board filters can reject it.

Creation, sprint assignment, PR weblinks and submitted transitions share the explicit
submission/runtime-mode admission gate. A stale revision fails before upstream calls;
all follow-ups keep the captured mode, so admitted live operations may finish after
switching Test. See [mode and stale-dialog review](tickets-ai.md#runtime-testlive-mode).
Sync/draft/options never perform writes. Workflow mappings remain options metadata
suggestions and do not select an initial status; only a submitted status can cause
a transition.
Post-create sprint/link/transition failure returns the real created issue and warnings:
retry/inspect the failed action in Jira, **not ticket creation**. Unknown creation
outcome likewise requires checking Jira before retrying. Read-only simulations
validate against the committed cache but perform zero upstream writes, fabricate no
IDs/cache data, and do not prove live write support.
Accepted creation without a confirmed key is successful `created` with `issue: null`
and an explicit warning, not a reason to create again; key-dependent follow-ups are
skipped and the normal live-write refresh still applies.

## Discovered statuses, one direct transition

The Workflow status dropdown contains actual statuses for the selected project and
issue type, discovered from `GET /rest/api/3/project/{key}/statuses` during refresh.
It is not free text or a Jira-wide list. Changing project/type changes this inventory.
Choices carry real IDs/names, but submissions use the exact **unique name**, not an
ID. Duplicate-name choices are unavailable with neutral help; choose another unique
name or **Jira default — no transition**. Null keeps Jira's initial state and requests
no transition. Inventory alone does not prove direct reachability or permission.

After confirmed live creation, an explicit status selection checks the actual initial
state. Already at the target is a no-op; otherwise PullBoard attempts **at most one
direct transition** to that status ID. Unavailable/ambiguous transitions, read failures,
missing permissions and rejected writes leave the ticket created and return a
nonblocking warning. No rollback, automatic retry, multi-hop workflow search or second
creation occurs. Persistent dismissible notifications retain the confirmed Jira link
with warnings. Simulation performs zero writes and does not prove reachability.
