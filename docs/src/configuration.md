# One YAML configuration

The server reads exactly one file selected by `--config PATH` (default
`config/pullboard.yaml`). Paths in it are relative to the **server's working
directory**, not the YAML's directory. Unknown keys, ambiguous secret sources and
unsupported values are rejected. There is no `.env`, environment interpolation,
per-provider secret file or automatic config merging.

## Safe starting points

- `config/pullboard.yaml`: native loopback, empty integrations, production assets
  at `frontend/dist/frontend/browser`.
- `config/container.yaml`: empty integrations, container bind `0.0.0.0`, baked
  assets `/app/frontend`, disposable database `/data/pullboard-cache.sqlite`.
  The server listens on 8080 inside the container; publish `127.0.0.1:3333:8080`.
  Its allowed browser origins use host port 3333, unlike the native defaults.
- `config/example.yaml`: comprehensive **placeholder** reference, not runnable
  real-account credentials. Copy to ignored `config/local.yaml` and edit.

Minimal native file (the omitted sections use safe defaults):

```yaml
read_only_test_mode: true
jira: null
forges: []
ai:
  enabled: false
```

## Controls

| Section/key | Meaning/default |
| --- | --- |
| `server.bind`, `port` | `127.0.0.1`, `8080`; unspecified address only for explicitly loopback-published containers |
| `server.allowed_origins` | Exact loopback HTTP(S) browser origins; defaults localhost and 127.0.0.1 on 8080 |
| `server.static_directory` | Built Angular browser directory |
| `server.database` | Disposable SQLite file, parent must be writable |
| `read_only_test_mode` | Startup value, default `true`; initializes shared runtime Test mode with zero Jira writes |
| `refresh_interval_seconds` | `3600`, allowed 1–604800 |
| `upstream.timeout_seconds` | `30`, allowed 1–120 |
| `upstream.max_pages` | `100`, allowed 1–1000; exceeding a bound fails that source refresh, retaining old data |
| `upstream.page_size` | `100`, allowed 1–100 |
| `upstream.max_response_bytes` | `8388608`, allowed 1024–33554432 |
| `upstream.max_concurrent_requests` | `4`, allowed 1–16; shared in-flight Jira/forge HTTP request limit per origin, across configured instances and credentials |
| `upstream.max_concurrent_items` | `4`, allowed 1–32; active item hydration limit per source for Jira and every forge |
| `upstream.github_batch_size` | `5`, allowed 1–10; requested maximum PRs per aliased GitHub GraphQL batch, also capped by `max_concurrent_items` |
| `jira` | Optional single Jira service; selected project keys and discovery/default policy, not upstream metadata |
| `forges` | Up to 64 account/instance entries; unique stable IDs, separate credentials |
| `ai` | Disabled by default; provider `openai`, exact model `gpt-6-luna` |

The three concurrency/batch knobs are positive YAML integers (Rust `usize`), not
strings or fractions. They use these defaults when omitted and are configured only
in this one YAML, not through environment overrides. Restart after changing them.
The HTTP origin is scheme, host and effective port: entries with different paths
or independent credentials on the same origin deliberately share a conservative
request budget and rate-limit state; different origins have independent budgets.
Source identities, snapshots and health remain separate.
Explicitly submitted Jira writes also use this admission budget; it never initiates
or automatically retries writes.

Ticket-favoring concurrent refresh uses these **existing** bounds for searches,
hydration, creation discovery and one project/account eligibility queue. After verified
identity, tickets/metadata/groups may overlap; eligibility waits only for complete
membership candidates. It allows one in-flight request/item while any card attempt is
unfinished, dynamically expanding to `max_concurrent_items` when all terminate,
including failure/backoff. The shared HTTP cap (default 4) remains authoritative;
nested work never multiplies it. There is no new knob, raised cap or hybrid/bulk API.
Metadata/groups may compete, so this is not preemption or a latency guarantee; cap 1
serializes reads. See [refresh scheduling](security-cache.md#ticket-favoring-concurrency-and-options-progress).

For GitHub, effective batch size is
`min(github_batch_size, max_concurrent_items)`; at most
`floor(max_concurrent_items / effective_batch_size)` batches run concurrently.
Thus the defaults request batches of at most four PRs, not five. Neither wider
batches nor independent detail reads create unbounded fanout. Cursor-dependent
pages remain sequential. Limits bound work, not requests per second, and cannot
guarantee that a provider will never throttle. See [bounded read scheduling](architecture.md#bounded-read-scheduling)
and [rate limits](providers.md#shared-request-budgets-and-rate-limits).

### Runtime mode override

The only application setting overridable at runtime is Test/Live action mode.
The UI confirms switching Live; the resulting backend-wide mode is shared by all
tabs/clients and initialized again from YAML on every restart. It never changes the
file or persists in SQLite, localStorage or display preferences. To change the
**startup** mode, edit YAML and restart; keep `true` for a safe restart default.

The mode strip and `/api/runtime-mode` show the current runtime value, not necessarily
the YAML value. Forge adapters remain read-only regardless. Switching to Test does
not cancel live actions already admitted by the server; stale dialogs require
explicit review, not an automatic retry. See [action mode](tickets-ai.md#runtime-testlive-mode).

Forge `id` uses ASCII letters/digits/hyphen/underscore and cannot be `jira`.
Optional forge `name` defaults to `id`; it is a source label, not an account identity.
Do not casually rename IDs: cached PR/issue identities and links are source-qualified.
`include_repositories` and `exclude_repositories` are glob lists applied **after**
account discovery (`owner/repository`); empty include means all discovered, excludes
win. They do not grant access or replace discovery with a hard-coded repo list.
`default_project`, when set, must refer to a configured Jira project. It supplies
the initial project for both PR and [upstream-issue drafts](upstream-issues.md);
when absent, use the first Jira project in YAML order, never a hardcoded `IDM`.
Repository policy also applies to explicit issue-URL lookups, including closed or
unassigned issues; a URL does not bypass exclusions or grant access.

## Jira policy, not an inventory

When `jira` is non-null, supply `base_url`, Basic-auth `email`, `token`, and:

| Key | Rule |
| --- | --- |
| `projects` | 1–100 unique selected project-key strings, e.g. `[DEV]`; first project's discovered key/name supplies Current project; no project objects |
| `board_ids` | Required ordered list of 1–100 unique positive integers, **no default**; first is **My board** |
| `team` | Required nonblank semantic default name, **no default**; not sprint ownership |
| `sprint_name_patterns` | Required ordered list of 1–100 unique nonblank Rust regex strings, **no default**; first is **My sprint pattern**; at most 4096 UTF-8 bytes each |
| `team_field_name` | Optional exact semantic field name; defaults to `AssignedTeam`, never a raw field ID |
| `assignee_groups` | Optional nonblank group names; exact duplicates removed; multiple groups form a union |
| `defaults` | Optional purpose policies; built-ins are Task/null for both implementation and review |
| `project_overrides` | Optional selected-project-key map; values accept only `defaults` |

Each `sprint_name_patterns` entry is a Rust regular expression searched anywhere in the
sprint name. For example, `'SSSD-IDENTITY'` finds that substring, while
`'^[0-9]{4}-SSSD-IDENTITY-S[0-9]+$'` requires the entire name to match. Matching is
case-sensitive unless an inline flag such as `(?i)` changes it. Regex metacharacters
are significant: use `'Team\.v2'` to match a literal dot. Lookaround and backreferences
are unsupported. Blank, invalid or oversized expressions fail validation; compiled
size is bounded and expressions compile once per configuration load. There are no
`%YEAR`/`%NUMBER` substitutions or legacy modes.

For example, `board_ids: [42, 43]` and
`sprint_name_patterns: ['EXAMPLE', 'OTHER']` are **independent ordered lists**, not
board/pattern pairs. All boards supply their complete active/future sprints,
deduplicated by sprint ID with the first configured board returning it preferred.
PullBoard never follows `originBoardId`. The first board supplies header links and
estimation metadata. Current sprints must be **active, returned by the first board,
and match the first pattern**; there is no fallback to an additional board or pattern.
Exactly one current match supplies Overview's initial/reset sprint filter.

Creation choices include sprints from any configured board whose names match **any**
committed pattern, for implementation, review and upstream cloning alike, independently
of the selected ticket team. All collected sprints remain in board filters and existing
assignments; patterns never filter ticket discovery or Attention. Every new editor
starts No sprint. See [sprint behavior](jira.md#ordered-boards-and-sprint-patterns).

### Migrate singular sprint settings

For contract v1.7.0, replace the removed keys with lists, even when keeping only one value:

```yaml
board_ids: [42]
sprint_name_patterns: ['EXAMPLE']
```

Remove `board_id` and `sprint_name_pattern` entirely; they are rejected, not aliases
or compatibility fallbacks. Empty, duplicate or over-100 lists are rejected too.
Keep the existing URL, email, token and unrelated settings unchanged. For additional
boards/patterns, append values and keep your own board/pattern first. Unlike the old
defaults-only pattern, the patterns now also restrict creation sprint choices; add
patterns for every sprint-name family you want offered. Restart after editing.

### Purpose policies and cached choices

Each supplied `defaults.implementation` or `defaults.review` object requires exact
`issue_type` and explicit `assignee`: implementation accepts `me` or null; review
accepts null only. Legacy `assignee: me` is accepted for compatibility but **does
not preselect self** or produce a missing-self warning. Every new PR/review/upstream
editor starts Unassigned, No sprint and Jira default/no transition. Omitted purposes
inherit built-ins. Optional `workflow` requires
both `draft` and `ready`, each a nonblank status suggestion or null. Omit the object
rather than writing `workflow: null` or a null purpose. Built-in workflow is null/null.

Project overrides may only name selected projects and override purpose defaults,
not boards/team/patterns/groups/fields. A supplied purpose replaces type/assignee;
omitted workflow inherits that purpose's global workflow. Explicit null workflow
entries clear suggestions. Empty maps change nothing. Workflow suggestions remain
available in options metadata, not initially selected in the editor. Only an explicit
status selection can request a post-create transition; sync never writes.

Normal Jira refresh discovers complete assignee/type/status/capability choices and
publishes them with their own sprint snapshot and effective policy in one independent
atomic options generation, not with cards. Dialogs, drafts and submissions use that
committed generation, not repeated group/user/metadata discovery or newer board sprints.
Group/default/pattern-policy changes take effect only after successful options publication;
an options failure retains complete same-scope last-good choices as usable but stale
through independent `jira_options.health` and `jira_options.ready`, not Jira ticket health. No verified
complete initial cache means creation fails closed; assignments are never optimistic.
Principal/service/incompatible configuration scope changes quarantine both domains. See
[choice freshness](jira.md#refresh-time-choices-and-freshness).

This is a breaking strict schema: configured forge `account`, Jira `account_id`/
`display_name`, metadata-rich projects/type inventories, `fields`, `teams` mappings,
team IDs, sprint inventories/fixed defaults, singular board/pattern keys and legacy top-level workflows are
rejected, not optional compatibility settings. Keyring `account` remains a secret
lookup key, not an upstream identity. See [Jira discovery](jira.md) for capabilities,
group permissions/request costs and unsupported mechanisms.

## Complete checked-in reference

The following is included directly from the actual example, not a second schema.

```yaml
{{#include ../../config/example.yaml}}
```

Restart the process after editing application configuration; this also discards any
runtime mode override. Browser localStorage contains display preferences only, not
server settings, action mode or credentials.
