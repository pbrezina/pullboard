# Troubleshooting

| Symptom | Check |
| --- | --- |
| Empty board on first run | Safe samples configure no sources. Add real accounts to ignored local YAML; there is no fake demo data. |
| Upstream Pull Requests missing a Jira issue | It must be assigned to you in a configured project and match sprint/title filters. Overview initially/reset selects only a unique matching active sprint's actual ID; zero/multiple matches leave Jira results empty until you choose. Explicitly select all, active/future or no sprint; check health. |
| Upstream issue missing from the list | Automatic discovery includes only open non-PR/MR issues assigned to the verified account and permitted by repository policy. Check independent forge/repository/title filters, issue-read permissions and shared source health; explicit URL lookup may read closed/unassigned issues. |
| Issue URL rejected or ambiguous | Use a configured instance's canonical issue URL without query, fragment, trailing slash or credentials. PR/MR routes, redirects and excluded repositories are rejected. Select Lookup instance when multiple sources match; see [URL rules](upstream-issues.md#canonical-urls-only). |
| Manual selection unavailable / `404 upstream_issue_not_found` | The process-local store is bounded to 128 entries/8 MiB; eviction, restart or source/account/service/policy changes can invalidate it. Keep/copy edits, explicitly resolve the URL again and review mode/options before a separate submit. No draft is saved or create replayed; see [recovery](upstream-issues.md#testlive-mode-and-recovery). |
| Upstream issue still shown after cloning | Expected: there is no clone tracking, cloned-status filter or Jira duplicate search. Its open/assigned state determines discovery; repeat live submissions can create duplicates. |
| Attention count exceeds visible cards | Badges are unfiltered. Inspect independent tab filters and the “X of Y” count. |
| Old cards after refresh | A failed verified same-scope source keeps last good data and complete Jira choices. Startup quarantines private data until identity verification; incompatible account/service/config scope cannot expose it. Inspect health/backoff/permissions/bounds. |
| Green source with coverage notes | Expected after a successful refresh. Notes are neutral API/history information, not partial/error source health. Per-item partial attention coverage is independent; green does not prove complete upstream history. |
| Evaluated count exceeds cards | Counts are unique fully fetched/evaluated candidates, including those excluded afterward, not retained cards, pages or attention threads. |
| Completed progress panels disappeared | Expected when healthy and idle: expand Source health Details for completed counts/durations. Active refreshes and real failures/backoff still show separate panels above the board. |
| Total unknown / empty bar | `null` means inventory not yet known, not zero; active work is indeterminate. `0 / 0` means genuinely empty completed work, not a fabricated percentage. |
| Fetch complete but old board visible | `publishing` waits for the combined atomic snapshot. Do not infer publication from a full item bar; check phase and live sync health. |
| Fast source has a long elapsed time | Progress `elapsed_ms` includes waiting for other sources and final publication. Compare the `source refresh reads finished` log to isolate the read stage, not just the completed progress duration. |
| Failed/backoff progress with previously green cards | Live `/api/sync` health can fail before the retained board changes. Inspect the error/retry deadline; old snapshot health must not mask the current failure. |
| Independent accounts back off together | Entries on the same origin deliberately share concurrency and conservative rate-limit state, even with different credentials. GitHub primary core/search/GraphQL buckets are separate; secondary backoff is origin-wide. |
| GitHub batches smaller than configured | Effective size is `min(github_batch_size, max_concurrent_items)`; only explicit resource/complexity errors split further. Auth, permission, read and rate-limit errors fail rather than falling back. |
| No timer for a known discussion | Upstream may lack a valid historical timestamp. Read coverage notes and follow the upstream URL; local discovery time is never invented. |
| API POST is 403 in Angular dev mode | Add the actual 4200 localhost/127 origin to the one YAML and restart; JSON plus `X-PullBoard-Request: 1` are mandatory. |
| API POST is 403 after changing port | Change exact browser `allowed_origins` along with host publishing. No trailing slash or wildcard. |
| Container cannot read config | Mode 600 is correct. Use mapped host UID/GID and Podman `--userns=keep-id`, not image UID 10001. Check SELinux file label. |
| SQLite cannot initialize | Parent directory must exist and be writable. Use `/data` with a correctly owned tmpfs in a read-only container. |
| Legacy YAML rejected | Remove configured identities, fields/IDs, project metadata/inventories and teams mappings. Use project-key strings and required board/team/pattern. Legacy implementation assignee: me alone is still accepted but does not select self. See the sanitized example. |
| Team unavailable | Default field name is AssignedTeam; optional team_field_name is semantic, not an ID. A unique permitted select option is needed. Separate Atlassian Team UUID discovery is unsupported. |
| Points or sprint rejected | Inspect selected-type capabilities and discovery diagnostics, not configured field IDs. Sprint must be in the configured board's active/future response; pattern/team do not restrict explicit selection. Unknown/unsupported values cannot be submitted. |
| Missing Sprint on create screen | It is omitted from create body. Supported scrum/Sprint/permission discovery can enable explicit post-create follow_up; otherwise null/unsupported. Never force an absent create field. |
| No default sprint in ticket editor | Expected for every PR/review/upstream ticket, even with one matching current sprint. No sprint is a normal optional blank, not a warning; select explicitly if desired or required. All board active/future sprints remain available. |
| No current sprint / multiple current sprints in Overview | Header/current context include only pattern-matching active sprints. Zero/multiple matches require an explicit filter choice and initially leave Jira results empty, not an automatic union/all-sprints fallback. Reset reapplies this rule; deliberate selections survive refreshes. |
| Wrong/unavailable Current project | It is the first configured Jira project's discovered key/name, not a forge draft override, hardcoded IDM or later-project fallback. Check YAML order and Jira discovery health. |
| Foreign sprint / unknown ownership | Expected when returned by the configured board. No other board is queried and configured team does not prove ownership. |
| Identity/project list empty after startup | Check pending/error health. Identity comes from /myself or /user, independently of issue discovery. Complete initial Jira metadata is required for ticket actions; no configured account ID is needed. |
| Jira discovery takes many assignee requests | At normal refresh, groups form a paginated union, then one accountId assignability check per candidate/project. Missing/empty groups fully enumerate plural /users/search first. Verify directory/group/project permissions and bounds; no partial or unrestricted fallback. Opening/searching/submitting the editor does not repeat discovery. |
| Choices stale / group-policy change not visible | A failed refresh retains the complete last-good generation and its effective policy. Use Refresh Jira choices or normal refresh; new policy applies only on successful atomic publication. Jira health/last-success covers choices too. |
| Options/draft/create not ready or discovery failed | No complete scoped initial choice snapshot exists. Check Jira health and refresh permissions/pagination; do not treat failure as an empty or unrestricted pool. |
| Reviewer missing / all tickets unassigned | Self is excluded for review; eligible self remains available but unselected for implementation. All editors start Unassigned. People must be active humans, in the configured group union (if any), and project-assignable in the committed cache. Null requires unassigned capability. |
| Assignee email missing / search finds nobody | Only upstream-exposed email is shown/searched; it is never inferred. Search name/username too. Filtering is local to the full cached eligible pool, and typing does not change the selected person. Unassigned remains keyboard-accessible. |
| Status missing or ambiguous | Choices are discovered for the selected project/type, not free text or Jira-wide statuses. Duplicate-name choices cannot be selected; choose a unique name or Jira default/no transition. A listed status does not guarantee a direct transition. |
| Keyring lookup fails | Host service unlocked? Exact `service` + `username` + `target default` attributes? Container restricted bridge and UID mapping available? No automatic plaintext fallback. |
| AI returns template | Expected for disabled/unavailable model, rate limit or invalid output. Edit the template; exact model ID is never substituted. |
| Ollama unavailable in container | Container loopback is not host loopback. Configure a reachable approved service URL in YAML without publishing PullBoard remotely. |
| Simulated success but no Jira issue | Correct: zero create/sprint/link/transition writes, no fake IDs/cache entries or write-triggered refresh. Reads may occur; simulation does not verify live writes. Use the confirmed Switch to Live flow only when ready. |
| Mode changed in another tab / reset on restart | Mode is backend-global, polled about every two seconds. It is never persisted; restart restores YAML `read_only_test_mode` and issues a fresh revision. |
| Stale dialog / `409 mode_conflict` | No upstream calls were admitted for that request and nothing is retried. Edits/selections stay intact: Review current mode → Use reviewed mode → separately submit again. A stale mode-switch confirmation must be cancelled/reopened. A toggle back or restart invalidates tokens too. |
| Mode cannot be verified | Use Check mode; actions stay blocked while verification fails. Do not bypass the guard or silently replace a dialog token. |
| Jira write finishes after switching Test | Already admitted live actions keep their captured mode for all submitted follow-ups. Switching Test gates new admissions, not cancellation of in-flight work. |
| Accepted creation notice without a key/link | Jira accepted the write but returned no safely usable key: `created` with `issue: null` and a warning. Inspect Jira; do not resubmit. No fake ID/cache entry or key-dependent follow-up is made; normal live-write refresh still applies. |
| Ticket created but sprint/link/status missing | Read the persistent dismissible warning and use its real issue link. Only explicit status selection requests at most one direct transition; already at target is a no-op. Unavailable/rejected transitions keep created success; no rollback, automatic retry, multi-hop or duplicate creation. Inspect the failed follow-up in Jira. Unknown creation outcome also requires checking Jira first. |
| Ticket editor could not load | The editor is lazy-loaded. Check the local server/assets connection and explicitly open it again; loading failure creates nothing and must not replay a submission. |
| Static page missing | Build Angular; native path is `frontend/dist/frontend/browser`. Container uses `/app/frontend`. Paths are relative to process cwd. |

Use `--check-config` to separate invalid YAML/missing keyring entries from upstream
access problems; it does not contact providers to validate access. `/api/health`
only checks the local process. `/api/sync` and source-health details carry upstream
status. API errors are sanitized deliberately; never enable verbose authenticated
HTTP logging to work around that protection.

## Refresh timing and safe logs

The server enables application-owned INFO and above, with dependencies at WARN
and above. This is a fixed safe filter, not an environment-controlled HTTP tracing
switch. Diagnostics omit raw upstream URLs, query/payload contents and credentials.

- `source refresh reads finished` records the configured `source_id`,
  `completed_items`, `elapsed_ms` and `succeeded`. Its duration covers preparation,
  discovery and fetching before publication waiting, so it isolates the read stage
  from the progress panel's longer completion duration. It is not pure network
  time: request-budget waits and item evaluation are included. A failed read's
  count is only fully evaluated items, not a successful partial snapshot.
- `upstream request budget idle` means that an origin budget has no active reads
  at that instant, not that the refresh job finished. `bucket` is a static label
  (`general`, `github_core`, `github_search` or `github_graphql`) identifying the
  final request's category. `requests` is cumulative for that shared origin budget
  across **all buckets and configured instances**, not a per-job or per-bucket
  total. `suppressed` is also cumulative. Idle can occur between reads; these
  counters do not reset at each idle message or refresh. Raw origin URLs are not
  logged, so the label alone does not identify a particular host.

Up to four sources can read concurrently, but the old board remains visible until
all source outcomes reach the one atomic publication. Read-stage timing and final
progress `elapsed_ms` therefore answer different questions. Observed performance
depends on the workload and provider; bounded concurrency does not
guarantee no upstream throttling. Honor health/backoff and adjust only the
[single-YAML limits](configuration.md#controls), not credentials or hidden
environment overrides to bypass quotas.

## Cache recovery and reports

For cache corruption, stop the process and remove only its configured disposable
SQLite file (and its associated SQLite sidecars if present), then restart. Do not
delete your config, keyring or unrelated databases. In the default container,
starting a fresh tmpfs run already gives a fresh cache.

For a report include tool versions, sanitized source kind/version, coverage notes,
safe error code and steps. **Do not** attach tokens, your local YAML, full private
board/cache data or raw authenticated traces. State whether the issue was tested
with live providers or only mocks.

For phase/count/timing details see [per-source progress](security-cache.md#per-source-progress);
for concurrency and deliberate reconfirmation see [runtime mode](tickets-ai.md#runtime-testlive-mode).
