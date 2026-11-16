# Providers and permissions

Configure **accounts**, not a compulsory repository inventory. Each forge entry
has its own stable `id`, `kind`, `base_url` and `token`; optional `name` defaults to `id`.
Two GitLab instances (or two accounts on one host) are independent sources. The
authenticated `/user` discovers the real stable account ID and display identity;
there is no configured username/account metadata or mismatch setting. Private
repositories are included only when the credential and organization/server policy
allow access.

## Forge capabilities

All adapters are read-only by interface and transport design in both Test and Live
modes. Do not grant write permissions just because the dashboard can create Jira tickets.

| Kind | Base URL | Discovery and practical credential guidance |
| --- | --- | --- |
| `github` | `https://api.github.com` (API base, not github.com) | REST and read-only GraphQL; authored PRs, direct review requests, mentions via search. Fine-grained tokens need access to relevant repos and read access to PRs/issues/metadata; organization approval/SSO may be required. Classic private-repo tokens can require broad `repo` scope, which also grants upstream write rights: prefer fine-grained read-only tokens where possible. |
| `gitlab` | `https://gitlab.com` or instance root | REST; authored, assigned and reviewer MRs plus accessible account to-dos. Personal token `read_api` is the usual read-only starting point; scopes and project membership vary by server. |
| `codeberg` | `https://codeberg.org` | Forgejo API; global account-created, assigned, mentioned and review-requested PR search. Choose read-only repository/issue/user scopes supported by that deployment. |
| `forgejo` | Your instance root | Same adapter as Codeberg, version-dependent API support. No `/user/repos`-only discovery limitation; public contributions outside joined repositories can be found by global account searches. |

All four kinds use the shared bounded Reqwest transport, with separate credentials
and conditional-response caches per configured instance. Client construction makes
no upstream requests. Redirects are disabled; GitHub GraphQL POSTs carry queries
only. Removing forge SDKs does not expand supported API coverage or permissions.

Repository include/exclude globs narrow the discovered result, not upstream scope.
For enterprise API URLs and older self-hosted versions, inspect source health and
test against the actual instance; there is no claim of universal version support.

## Assigned upstream issue discovery

All four forge kinds also discover **open non-PR/MR issues assigned to the verified
account** for the [Upstream issues tab](upstream-issues.md), not every repository's
issue inventory. The implementation uses these assignment-scoped API reads:

| Kind | Assigned-issue discovery |
| --- | --- |
| GitHub | Account `/issues` with `filter=assigned`, `state=open`, avoiding the PR-search 1,000-result ceiling for this issue collection |
| GitLab | `/api/v4/issues` with `scope=assigned_to_me`, verified `assignee_id` and `state=opened` |
| Codeberg / Forgejo | `/api/v1/repos/issues/search` with `assigned=true`, `type=issues`, `state=open` |

Complete pagination is required. Candidates are hydrated and checked again for
open/assigned/non-PR state; repository include/exclude policy applies before
retention. There is no date cutoff, silent result cap, broad scan or fallback to
unfiltered issues. A required read/page or unsupported assignment API fails the
source, retaining both last-good PR and issue collections for that verified account.
The configured page/response/time limits still apply; they fail rather than publish
truncated success. Progress counts include both collections.

Credentials need issue read access and repository visibility, including any
organization approval/SSO or private project membership. GitHub fine-grained issue
read permission is needed in addition to PR read permission; GitLab `read_api` and
Forgejo/Codeberg read-only issue/repository/user permissions are starting points,
not guarantees across deployments. Assignment filters and pagination depend on
the server API version. Unsupported/forbidden reads are not treated as an empty list.

Manual URL lookup uses the same configured instance, credential, repository policy
and read budget, but may fetch closed/unassigned issues; it does not broaden the
assigned discovery query. It never fetches arbitrary hosts or follows redirects.
No forge write scope is needed to clone into Jira. This feature does not search
Jira for existing clones or change Jira's assigned/mention discovery.

## Bounded reads and GitHub batches

Jira and all forge adapters hydrate a bounded number of items concurrently per
source, using `upstream.max_concurrent_items`; this is not a total item cap.
Independent details/history reads can overlap;
pages that require a previous response's cursor remain sequential. The shared
origin HTTP budget also applies, so item concurrency is not a separate allowance
for unlimited requests. Existing conditional reads remain in use where supported.

GitHub uses aliased GraphQL batches for PR metadata, comments, reviews, commits,
assignees, labels, `reviewRequests` and `reviewThreads`, including PRs from different
repositories. Every required connection is fully drained within the configured
page/response bounds, including nested review-thread comments. A first page is not
treated as complete history. REST discovery remains; REST timeline history is read
only when needed for the actor and upstream timestamp of a currently pending
self-review request, rather than substituting PR update time.

Only explicit GraphQL resource/complexity errors allow splitting a batch into
smaller batches. Authentication, permission, required-read and rate-limit failures
do not trigger a REST fallback or publish partial results as success. A failed
required connection fails the source and preserves verified same-scope last-good
data. Batch/item sizing is described in [configuration](configuration.md#controls).

## Shared request budgets and rate limits

Concurrency and secondary/abuse backoff are shared across all configured entries
on the same HTTP origin, even with independent credentials. This is deliberately
conservative: accounts remain independent sources, but are not assumed to have
independent upstream quotas. GitHub primary quotas are tracked separately for
core, search and GraphQL; a secondary limit applies across the origin.

The scheduler honors `Retry-After` and numeric Unix or RFC3339 reset times,
including GraphQL reset metadata. Once a limit is known, affected queued reads are
suppressed at admission rather than starting a retry storm. In-flight responses
can still finish; a late success never clears a longer active backoff. Overlapping
quota observations retain the minimum remaining allowance and maximum reset time.
Request permits cover response consumption, not just receipt of headers.

These controls and conditional reads reduce avoidable work, but cannot guarantee
no throttling. Other clients, upstream policy and large histories still matter.
Actual refresh speed depends on the workload, network and provider. Measure your
instance rather than assuming a fixed speedup. Check each source's health and
retry deadline instead of repeatedly
requesting refresh or increasing limits to evade backoff.

## Coverage is deliberately honest

A successful refresh is `ok` (green) for Jira, GitHub, GitLab, Codeberg and Forgejo.
Source `coverage_notes` describe API, indexing, permission visibility and history
limits as **neutral information**, not an amber/partial source state. This does not
claim complete upstream history. Per-item `Attention.coverage` can independently be
`partial`; unknown history is never silently treated as answered. Known limits include:

- GitHub **PR** search indexing, credential visibility and its 1000-result search ceiling
  limit PR discovery; assigned issues use the separate account issue endpoint above.
  Team-only review requests are not guaranteed. Inaccessible or
  deleted review/discussion history cannot be reconstructed.
- GitLab mention discovery depends on retained accessible to-dos. Review-request
  timestamps come from timestamped system notes, not MR `updated_at`. Missing
  history produces no invented request timer.
- Forgejo/Codeberg search filters and review metadata depend on server version.
  Thread grouping uses review IDs/code locations and resolver metadata when
  exposed. Team-only requests, deleted history and bot detection can be incomplete.
  A description mention without a usable revision timestamp is disclosed in
  `coverage_notes` with the PR URL; it is not assigned a local discovery time.
- Every provider may omit deleted events or history hidden by permissions. Bots
  are excluded where upstream identity metadata permits classification.
- Request/page/response bounds are safety limits. Hitting a pagination bound is
  not a successful truncated refresh: that source keeps its last good snapshot.

An empty queue is therefore not proof that every upstream conversation is handled.
Read source-specific **Diagnostics** in Source health, last-success time and
any error/backoff before acting. Failed required reads/pages are real refresh failures,
not neutral coverage notes; they retain last-good data marked stale. See
[refresh progress](security-cache.md#per-source-progress) for counts and publication.

## Jira

The implementation targets Jira Cloud REST API v3 and Agile v1; it is **not** a
claim of Jira Server/Data Center compatibility. Email plus an **unscoped** API token
authenticate tenant-root requests; authenticated `/rest/api/3/myself` discovers
identity for mentions/assignment. The [setup wizard](setup.md#jira-cloud) links token
creation/help and requires explicit assignee groups rather than silently selecting all users.
Use credentials allowed to browse configured projects, issues, comments, changelog,
all configured boards' active/future sprints, primary-board configuration, visible fields, create metadata,
per-type project statuses (`/rest/api/3/project/{key}/statuses`, Browse Projects) and
user/group discovery. Configured groups need complete group lookup/membership access
plus individual project assignability checks. Absent/empty groups need full paginated
`/rest/api/3/users/search` access plus the same checks; directory permissions may be
broader and request cost is pages plus one request per candidate, not one capped
assignable-search scan. After verified identity, tickets/metadata/groups may overlap.
Exact eligibility waits only for complete deduplicated candidates, not metadata or
tickets; one owned bounded project/account queue allows one request/item in flight
while any card source is unfinished, then expands to existing item concurrency once
all card attempts terminate, including failure/backoff. Shared per-origin HTTP bounds
remain authoritative; metadata/groups may compete, with no preemption/latency guarantee.
These checks occur during normal refresh;
options/drafts/create use the committed complete options generation without repeating
them. Discovery/eligibility failures fail options closed without failing committed
Jira tickets; complete same-scope last-good choices stay usable with independent stale
options health. Without a verified complete initial generation, creation stays blocked.
Ticket read failures retain only their own scoped last-good cards/sprints; see [Jira setup](jira.md).
Sprint reads fully paginate every ordered board and deduplicate sprint IDs with
first-configured-board provenance, never following `originBoardId`. Creation choices
match any committed pattern; current sprints require active first-board/first-pattern
matches without fallback. Patterns never narrow ticket discovery or Attention.
For explicit non-simulated actions, Jira permissions must additionally allow issue
creation, assignment, remote links and the selected transition; sprint follow-up
requires discovered scrum/Sprint support and EDIT_ISSUES + SCHEDULE_ISSUES. These reads
do not guarantee a later write succeeds. Scoped-token gateways require different
routing than this tenant-root basic-auth flow and are not supported by setup.

Discovery uses two separate server-side JQL queries through
`/rest/api/3/search/jql` enhanced cursor search:

- Assigned: `project in (<configured project keys>) AND assignee = currentUser()
  AND status != "Closed"`.
- Mention candidates: `status != "Closed" AND (description ~ currentUser() OR
  comment ~ currentUser())`, across **all accessible projects**, with any assignee
  (including unassigned).

Neither query has a date cutoff or excludes issues by resolution or Done status
category. Both fully paginate the filtered results and deduplicate by upstream
issue ID before per-issue comments, description changelog and weblink reads.
Request cost follows search pages and unique matching issues plus their history,
not every non-Closed issue in a project; large matching histories can still be
expensive. Metadata and assignee-option discovery have separate costs described in
[Jira setup](jira.md).

A search hit is only a candidate: explicit Atlassian Document Format (ADF) account
mentions and upstream reply timestamps determine attention. Handled mentions, bots
and text-search false positives do not open attention. Only configured-project
assigned issues or verified unanswered mentions become cards; PR weblink references
come only from these retained, inspected matching issues, not unrelated colleague
tickets.

There is no project-wide scan or broad fallback. Search, page or per-issue read
failure (including safety bounds) fails Jira ticket refresh and preserves its last-good
same-scope cards/sprints, if any, with stale/error ticket health rather than publishing partial
results as success. Indexing, inaccessible projects and deleted history remain
limitations disclosed in source `coverage_notes`. Current description/comment
revision timestamps are used when historical mention edits are unavailable; no
timestamp is fabricated.

No live account credentials are used by the repository's automated tests. Provider
permission advice must be confirmed against your actual tenant and server version.
