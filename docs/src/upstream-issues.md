# Upstream issues to Jira

## Assigned work in compact rows

The **Upstream issues** tab lists open issues assigned to your verified account on
configured **GitHub, GitLab, Codeberg and Forgejo** instances. Pull requests and
merge requests are excluded. Accessible private repositories are included only
when credentials and configured repository policy allow them. This tab does not
add items to Needs Attention.

Rows span the list width instead of using the fixed Jira/PR card grid. Each shows
the title, namespace/repository, source and issue number, description snippet,
reporter and labels, with a **Clone to Jira** button. An unavailable reporter is
shown as unavailable, not an invented identity. Titles sort case-insensitively.

Forge-instance, repository and case-insensitive title filters are independent of
the other tabs' filters. The list shows filtered and total counts; **Reset filters**
clears this tab's text and selects all forges/repositories only. Only display
preferences are persisted—not pasted URLs,
resolved issue content, editor drafts, action mode tokens or Jira results.

Assignment-scoped discovery fully paginates; there is no broad all-issues fallback
or silent truncated success. Include/exclude repository globs apply to discoveries
and explicit URL lookups; empty include permits all discovered repositories and
exclusions win. See [provider permissions and limits](providers.md#assigned-upstream-issue-discovery).
PRs and issues share their source's identity, health, progress and atomic snapshot:
a required read failure in either collection retains both last-good collections
as stale for the same verified account. A successful empty discovery clears the
old issue list. Check source health before treating an empty or stale list as current.

## Open an issue URL

Paste a canonical issue URL into **Issue URL**, then press **Enter** or choose
**Clone to Jira**. This reads the issue and opens an editable deterministic
draft; it does **not** create a Jira issue. A verified forge identity is required;
full assigned-issue discovery need not have completed. Jira discovery must be ready
to prepare the editor. The URL lookup API itself needs neither Jira readiness nor
a mode revision.

Only configured forge instances are accepted. **Lookup instance (optional)**
normally stays at **Detect from URL**. If several configured sources match the
same web origin/path (for example, two accounts on one host), select the intended
source explicitly. PullBoard does not guess an account or try several credentials.
The source selection must match the URL and its repository policy.

Unlike automatic discovery, explicit lookup allows **unassigned and closed**
issues. Closed issues show a warning after lookup and in the editor; the warning
does not prohibit creating a ticket. Manual lookup does not add the issue to the
assigned list or change the board revision, filters, sync health or progress.

### Canonical URLs only

Use HTTP(S) with the configured instance's exact web origin (scheme, host and
effective port) and base path, followed by one of these paths:

| Forge | Path relative to the instance's web base |
| --- | --- |
| GitHub | `/owner/repository/issues/7` |
| GitLab | `/namespace/repository/-/issues/7`; nested namespace groups are allowed |
| Codeberg / Forgejo | `/owner/repository/issues/7` |

Issue numbers must be positive decimal numbers without leading zeroes. Remove
query strings, fragments and trailing slashes. URLs containing credentials,
backslashes, traversal/dot segments or encoded path separators are rejected, as
are PR/MR routes and PR payloads served by an issue route. URLs are limited to
4,096 characters. Unknown hosts, ports and mismatched instance paths are rejected
before any upstream request.

GitHub's configured `https://api.github.com/` maps only to the browser base
`https://github.com/`. An Enterprise API base ending in `/api/v3` maps to the same
origin with that suffix removed; other GitHub bases keep their configured
origin/path. GitLab, Codeberg and Forgejo use their configured base unchanged.
This is not an arbitrary URL proxy: authenticated API reads are constructed from
validated segments using the configured transport, not sent to the pasted URL.
Redirects are not followed or used to forward credentials. After a repository
transfer, use its new canonical URL on a configured, permitted instance.

## Review the deterministic copy

**Clone to Jira** and URL entry use the same implementation-ticket editor:

- The initial project is the forge's configured `default_project`; if omitted,
  it is the first configured Jira project in YAML order. There is no hardcoded
  `IDM` project. You can select another configured project in the editor.
- The title is copied exactly. The normalized plain-text description is followed
  by a blank line and `Upstream issue: <canonical URL>`, then `Labels: <names>` if
  labels exist. An empty description starts with the upstream URL line. Labels
  remain text, not Jira label values.
- Type and team use cached discovered **implementation** options and configured
  defaults. Task is the built-in type suggestion only when available. Every new
  clone starts **Unassigned**, **No sprint** and **Jira default — no transition**;
  story points start empty too. Legacy `assignee: me` and workflow suggestions do
  not preselect those values. Optional blanks are normal; required inputs need
  explicit selection. All board active/future sprints remain available independently
  of team, and status choices belong to the selected project/type.
- Review and edit all fields before submission. Oversized text is not silently
  truncated: edit titles over 255 characters or descriptions over 100,000 characters.

This path **never calls AI**, even when PR AI drafting is enabled. It does not
copy comments or attachments, paraphrase the issue, or write to the forge.

Only explicit submission creates Jira from your edited values and then adds a
Jira web link using the server-trusted upstream URL/title. Supported sprint/status
follow-ups apply only when explicitly selected. A follow-up failure after accepted
creation is success with warnings, not a reason to create again. Only confirmed
Jira keys and URLs are shown. If creation was accepted without a usable key, or its
outcome is uncertain, inspect Jira before any further creation; never replay it
automatically. See [ticket outcomes and capabilities](tickets-ai.md).

Assignee search filters the full cached list locally by name/username/exposed email;
it makes no request per keystroke. Options/draft/create do not rediscover metadata
or group membership. Retained stale choices are labeled through Jira source health;
policy changes apply at the next successful refresh. An explicit status requests at
most one direct post-create transition, with no-op at the target and success plus
warning if unavailable/rejected. Dismissible persistent notices keep the real issue
link. These are the same [editor rules](tickets-ai.md) as PR tickets.

There is **no Jira duplicate search, automatic deduplication, clone history,
cloned-status filter or Jira-wide scan** for this flow. Creating a ticket does not
remove the upstream issue or mark it cloned. Repeating a live submission can create
another Jira issue. Existing PR-to-Jira linking remains PR-only; there is no new
standalone upstream-issue linking action.

## Test/Live mode and recovery

In **Test** mode, valid explicit submissions succeed as **simulated** with zero
Jira or forge writes, no fake IDs, no cache mutation and no write-triggered refresh.
Reads and validation still occur. **Live** mode permits the explicitly submitted
create and web link. The action captures mode/revision at Clone or URL submission,
**before** asynchronous resolution; polling cannot silently change the editor's
action mode. Stale actions preserve edits and require deliberate mode review and
a separate submission. Already admitted live work may finish after switching to
Test. See [shared runtime mode](tickets-ai.md#runtime-testlive-mode).

Manual selections live only in the running backend, separately from SQLite and
the board. The store holds at most **128 entries and 8 MiB of serialized issue
content** in total, evicting older entries to stay within both bounds. One issue
larger than 8 MiB is rejected. There is no YAML capacity setting or durable clone
state. A manual selection survives ordinary board refreshes; re-resolving its URL
explicitly replaces it, and it takes precedence over an older board copy.

Eviction, restart or a changed source/account/service/repository-policy scope can
make a selection unavailable (`404 upstream_issue_not_found`). Keep the edited
dialog open, or copy your edits before closing it: drafts are not saved. Explicitly
resolve the canonical URL again (another tab can do this while the original editor
stays open), review current mode/options and submit deliberately only if intended.
There is no automatic re-resolution, draft restoration or create replay. If a
previous create might have succeeded, inspect Jira instead of submitting again.
