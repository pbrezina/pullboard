# Architecture

```text
App (Material tab navigation + persistent lazy Workspace shell)
    |-- PullRequests (signals + Material)
    |       |-- ForgeAggregatorService -> IForgeProvider.getPullRequestInbox()
    |       |-- PrJiraLinks (shared local association registry; no HTTP)
    |       `-- ReviewTickets -> LlmService + SyncDialog + JiraService
    `-- Dashboard / Synchronize to JIRA (signals + Material + CDK)
            |-- ConfigService -> /assets/config.json
            |-- ForgeAggregatorService -> provider factory registry -> IForgeProvider
            |       `-- GithubProviderService (one per account, GET-only)
            |-- LlmService -> configured provider proxy -> validated suggestion
            `-- SyncService -> JiraService -> JIRA proxy
                                    `-- runtime board/field/workflow discovery
```

`UnifiedPullRequest` is the UI contract; no GitHub response types cross the
strategy boundary. `IForgeProvider` exposes only authenticated username lookup,
authored open-PR enumeration, a review/mention inbox, supported URL detection and individual PR status. `FORGE_PROVIDER_FACTORIES` is a DI
registry mapping config `type` to per-entry provider construction.

`ConfigService` validates JSON and caches the result, avoiding repeated token
loads. No configuration tokens are stored in browser persistence. A failed
load can be retried on reload. `ForgeAggregatorService` aggregates all providers
and reports individual failures without hiding working accounts.

`JiraService` owns every mutation. It paginates discovery/search endpoints,
converts descriptions to Atlassian Document Format, assigns by verified account
ID, finds a numeric create-screen story-points field, and selects workflow
transitions by their destination status (not their user-facing action label).
Already-correct statuses are a no-op; required transition fields are not guessed.
Account discovery is separate from sprint discovery/assignment. Active and future
sprints are listed, while only an unambiguous active sprint supplies the default.
Ticket search accepts a sprint ID or explicit no-sprint selection (`sprint IS
EMPTY`), always scoped to the project, assignee and unfinished status. Descriptions
are flattened from ADF to untrusted plain text, never rendered as HTML.

`SyncService` coordinates a non-transactional workflow. Created tickets are
reported immediately via callback. Subsequent steps use `PartialSyncError`
with that receipt so the dashboard can safely resume. Remote links use stable
global IDs. Issue creation has no automatic retry: JIRA does not provide
an idempotency guarantee for this operation.
Sprint assignment is opt-in. A partial receipt saves the exact sprint ID so
resumption does not accidentally use a different active sprint.

Neither view polls. Both load when the browser page opens and stay mounted
inside `Workspace`; route changes only toggle visibility. Manual refresh affects
one view, and sprint selection loads the selected tickets. `PrJiraLinks` shares
known receipts without any remote lookup. Whole-project Web Link scanning is
disabled; arbitrary remote links are not reverse-searchable through native JQL.

`ReviewTickets` invokes AI only after an explicit action, shows editable
confirmation with a fixed `Review PR owner/repo#123` prefix, and creates an
explicitly unassigned ticket in JIRA's initial status. It adds a Web Link and
optionally a sprint, but never invokes the implementation-ticket transitions.
Multiple review tickets are allowed after confirmation. Keys and uncertainty
markers are persisted before subsequent requests, so partial retries cannot
repeat creation. Review receipts remain separate from implementation receipts.

`PullRequestWorkItem` extends the
shared card data with normalized review status, outstanding request/mention times,
action-needed state and partial-activity warnings. `github-activity.ts` classifies
paginated REST activity without exposing GitHub response types to components;
`pull-request-order.ts` owns action-first grouping, waiting-time/last-activity
sorting, Blocked/Deferred exclusion and the 30-day aging threshold. A small worker pool bounds
parallel link/status requests. Unreadable statuses stay unknown, not closed.
Closure is an explicit action and rechecks all recognized linked PRs. Drag/drop
and the accessible link menu invoke the same linking method.

`npm run start:test` uses an Angular file replacement to set `TEST_MODE` at
build time. Services simulate mutations and return explicitly marked receipts;
the dashboard never restores/persists normal receipts in this mode. Simulated
creations have no ticket URL, and simulated closure leaves real card state
unchanged. A JIRA HTTP interceptor fails closed on any non-GET request. The
test-mode Vite proxy independently rejects non-GET JIRA/forge requests before
forwarding. AI POST requests are deliberately unaffected. Tests exercise both
normal and simulated services, browser boundaries, storage isolation and proxy
guards without remote requests.

## Boundaries

- No backend, OAuth, webhooks, background daemon, or upstream mutation.
- Loopback-only dev server; proxying bypasses browser CORS, not API security.
- Fine-grained read-only forge credentials plus a non-GET HTTP interceptor.
- Local receipts prevent common accidental repeats, not multi-tab distributed
  locking or cross-device issue deduplication.
- Partial state persists, but external changes (deleted tickets, altered
  workflows, closed sprints) require user reconciliation.
- Model text is untrusted, never executed or rendered as HTML.

The proxy generator runs before `ng serve`, not as a backend server. Production
assets exclude the credential-bearing config. This is a local developer tool,
not a deployment architecture.
