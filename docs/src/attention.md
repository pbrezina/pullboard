# Attention and the dashboard

## What remains unanswered

Needs Attention combines Jira mentions, direct PR review requests/mentions and
feedback on your own PRs. It sorts by the **oldest still-unanswered upstream
timestamp**, not the last fetch or last push.

- Each upstream review conversation is an independent stream. Your own reply or
  upstream resolution clears that stream's earlier events; later events reopen it.
- General flat PR discussion is one separate stream. Replying there does not clear
  unrelated review threads.
- A submitted qualifying review clears the review-request stream; it does not
  automatically answer every discussion.
- Pushing a commit never clears attention. Merely opening PullBoard does not either.
- Bots and your own events do not open attention.
- Jira explicit account mentions in descriptions/comments open issue attention;
  your later comment clears earlier mentions. Only status **Closed** excludes an
  issue: a resolution value or Done status category does not clear attention.
  Mention discovery searches all accessible projects, regardless of assignee and
  without a date cutoff. Search hits need explicit ADF mention/history verification;
  handled mentions, bots and false positives do not enter Needs Attention.

The attention detail lists the unresolved streams and their source links. Upstream
APIs may lack events, resolver identities or usable timestamps. Per-item partial
attention coverage remains visible independently of green source refresh health;
the application does not invent a timer from local discovery time.

## Tabs and filters

Upstream Pull Requests shows only Jira issues assigned to you **in configured
projects** and your own open PRs, sorted by title case-insensitively (stable ID
tiebreaker). Needs Attention can include verified unanswered Jira mentions from any
accessible project, including issues assigned to others or unassigned. This does not expand ticket
creation/link permissions or the configured project inventory. PR reviewers/assignees
exclude you and sort by username; the author/reporter can still be you.

Jira selectors offer every configured-board active/future sprint, grouped **Active**
and **Future**, **No sprint**, and **All sprints & no sprint**. There is no synthetic
eligible-active-sprints option. Needs Attention initially includes all sprint and
no-sprint work, so backlog mentions remain visible. Upstream Pull Requests (Overview)
initially selects an actual sprint ID only when **exactly one** active sprint matches
the configured full-name pattern. Zero matches show **No current sprint**; multiple
matches show **Multiple current sprints — choose a sprint**. Both leave Jira results
empty until you choose a filter—no first-sprint, union or all-sprints fallback.
Nonmatching, foreign-origin and future sprints remain available for explicit choice.
Pattern/default policy never filters upstream discovery or attention.
PR selectors offer all forges or an individual source instance, not merely a
provider type. Each tab remembers its own case-insensitive title filters and
selections. Attention badges retain the **unfiltered total**; filtered results show
“X of Y”, so a filter cannot silently erase the queue count.

Deliberate selections, including all/no sprint, survive refreshes; an unavailable
selected ID is shown rather than silently replaced. An untouched Overview can
initialize once first discovery completes. **Reset filters** clears only the active
tab's text, forge and repository filters, then selects the sole matching current
sprint for Overview (or its explicit no-current/choose state), or all/no sprint for
Attention. Older saved `active` selections migrate to these defaults.

The separate **[Upstream issues](upstream-issues.md)** tab lists open non-PR/MR
issues assigned to you on all configured forge kinds. It has its own instance,
repository and title filters, title ordering and filtered/total count. These items
do not create attention. Compact full-width rows show title, labels, description
snippet, reporter and repository; **Clone to Jira** or Enter in its issue-URL form
opens an editable deterministic editor, never an immediate create.

Equal fixed-size Jira/PR cards, **300px high** with matching grid widths, keep the
desktop-first board dense and aligned. This is the single compact layout, not a
density preference. PR cards in Upstream Pull Requests show repository, author,
reviewers, assignees, Jira link count and a short plain-text description; Jira cards
show assignee, PR link count and a description preview. Attention cards use concise summaries before the
prominent discussion excerpts. Full content remains available in details.

During refresh, separate source progress panels above the board show actual phase,
evaluated items and elapsed time; real failures/backoff remain visible there too.
Healthy idle shows the green Source health summary, with completed counts/durations
in its existing keyboard-expandable Details. Cards stay on the old snapshot until
[atomic publication](security-cache.md#per-source-progress).
Who-am-I shows verified identities and **Current project**: the real key/name of
the first Jira project in YAML order, or **Not available yet** before discovery/with
Jira disabled. A forge's draft project override does not change this header. Only
pattern-matching active sprints appear here, with their end date when available;
links set `selectedSprint` on the trusted configured-board URL, preserving its other
parameters. No matching sprint and multiple-match ambiguity are explicit, not an
automatic choice. Nonmatches remain selectable in filters, not in this header.
The Test/Live strip and source health show mode and refresh state. An empty board
with no sources is valid, not demo data waiting to load; identities may be empty
while source health is pending/error.

## Preferences and keyboard operation

Choose light, dark or system theme. Display preferences live in browser
localStorage, separately from all application settings; no credentials or durable
board cache are stored there. Focusable native controls and dialogs provide
keyboard access. Tab to **Ticket details** or **PR details**, then press Enter or
Space to open full content beyond the visible summaries: full title/description,
long names, repository/people links, linked items, labels (PRs) and timestamps.
The card's reading area scrolls without enlarging the card or shifting
the grid; Tab reaches links inside expanded details. Use **Link PR** as the alternative
to dragging a PR onto a Jira issue. Confirm the explicit action before a link is submitted.

The persistent Test/Live strip shows the application-wide action mode, shared across
tabs through polling, not a saved display preference. Review
[stale dialogs and action mode](tickets-ai.md#runtime-testlive-mode) before submitting.

See [ticket flow](tickets-ai.md) for creation and simulated success behavior, and
[source health](security-cache.md) before assuming stale cards are current.
