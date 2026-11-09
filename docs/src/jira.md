# Jira discovery, defaults and workflows

## Select policy; discover metadata

`jira.email` and the token provide Basic authentication. The authenticated
`GET /rest/api/3/myself` discovers the authoritative account ID/display name;
do not configure either. `projects: [DEV]` selects project keys, not names or type
inventories. Supply required `board_id`, semantic `team` and `sprint_name_pattern`;
none has a default. Example names/numbers are sanitized illustrations, not tenant facts.

Configured projects restrict assigned-issue discovery, metadata, ticket creation
and explicit PR linking. The separate description/comment mention search spans all
accessible projects without expanding those permissions or the project inventory;
see [provider queries and limits](providers.md#jira).

During normal refresh the backend reads visible fields, selected projects, fully
paginated create types and per-type create metadata, project/type status inventories,
configured-board configuration and project permissions.
Project names and supported issue types come from those reads. It identifies Sprint
by the `gh-sprint` schema and numeric Story Points by the board's estimation field,
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

## One configured board, all its active/future sprints

Choose a real positive board ID from your approved Jira board URL. PullBoard does
not browse a board inventory. It fully paginates only
`GET /rest/agile/1.0/board/{board_id}/sprint?state=active,future` and reads configuration
for that same board. Foreign-origin sprints returned by it remain available; it
never follows `originBoardId` to query another board.

All returned active/future sprints and no sprint are offered independently of team
and name pattern, grouped **Active** and **Future** in selectors. Queried board links
do not prove ownership; current discovery returns sprint `team: null`, not a team
inferred from configuration or name. Who-am-I shows only pattern-matching active
sprints, their available end dates and links to the actual sprint on the queried
board. No match and multiple-match ambiguity are explicit.

`default_eligible` means **active AND full-name pattern match**. `%YEAR` is four
ASCII digits, `%NUMBER` one or more, with escaped literals and whole-name anchoring;
see [configuration](configuration.md). Future/nonmatching sprints remain selectable.
Exactly one match supplies Overview's initial/reset actual sprint ID. Zero or
multiple matches leave its Jira results empty with an explicit no-current/choose
state until the user selects a filter; never choose the first/minimum ID, a union
or all-sprints fallback. Attention defaults all/no sprint. There is no synthetic
eligible-sprints selector. Deliberate per-tab selections survive refreshes; see
[filters and reset](attention.md#tabs-and-filters).

**Ticket creation always starts No sprint**, with no missing/ambiguous-sprint-default
warning, regardless of the number of matches. Required sprint input still needs
explicit selection. Patterns do not restrict discovery or attention. Team and
sprint are independently editable: neither filters, resets or validates the other.

## Assignees: complete candidates, refresh-time checks

`assignee_groups` accepts multiple semantic names. Each is resolved uniquely to a
real group ID and membership is fully paginated. Candidates are the **union**,
deduplicated by accountId, restricted to active human accounts and then checked
individually for project assignability with
`GET /rest/api/3/user/assignable/search?project=...&accountId=...`.
Configured groups do **not** use broad capped assignable-search range pagination or
an all-user directory scan. Each deduplicated candidate is checked once per project
while preparing the refresh's choice snapshot, not on every editor request.

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

## Refresh-time choices and freshness

Normal Jira refresh fully discovers every configured project's permitted assignees,
all allowed types' metadata/capabilities/team choices and statuses, and all board
active/future sprints. Choices, effective group/default policy, identities and Jira
cards publish atomically in the ephemeral SQLite snapshot. Opening/searching a dialog,
drafting and create validation consume that cache, never start metadata/group/user
discovery. AI generation and explicit live write/follow-up requests retain their
separate purposes.

Failed required reads/pages retain the **complete same-scope last-good generation**
and mark Jira stale/error/backoff; no partial, mixed-generation or unrestricted pool
is published. An active refresh leaves previous choices usable until commit. Before
the first usable cache, ticket options/drafts/create fail explicitly with readiness
or discovery errors, not successful empty results. A genuinely fully discovered empty
pool is different. Principal/service/config scope changes cannot expose another
account's private choices; compatible restored snapshots remain quarantined until
identity verification.

Jira source health covers choices too: `last_success_at` is their last successful
publication. The editor shows a quiet stale-choices note and **Refresh Jira choices**;
the normal source-health strip carries failure details. Group and policy changes
apply only with the next successful atomic refresh, never to an old pool alone.
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
permissions. The selected sprint must appear in the latest committed configured-board
inventory; submit does not rediscover it.
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
