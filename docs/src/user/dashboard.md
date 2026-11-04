# Dashboard

Pullboard has two keyboard-accessible tabs: **Pull requests** (the default) and
**Synchronize to JIRA**. Both use a responsive two-column workbench and load
concurrently when the page opens. Switching tabs preserves data, filters and
sprint selection without reloading. There is **no automatic polling**.
Use **Refresh board** to update only the current tab. The last-refreshed timestamp and relative
PR activity ages are shown; hover an age to see its exact local timestamp.
Ages are snapshots of the last refresh. JIRA actions still validate relevant data
and update their local operation results, without reloading the entire board.

## Pull requests

This tab loads PRs from your forge accounts independently of the JIRA board.
JIRA loading and outages cannot block the PR lists. AI is called only when creating a ticket.
It includes open PRs, including drafts, across your configured accounts and repositories.

### Requested reviews from me

- **Review outstanding** (blue): an outstanding review request, including team
  requests matched by GitHub's `review-requested` search.
- **Review given** (muted): PRs you already reviewed remain visible, even after
  GitHub removes your pending request. A re-request changes the badge back to
  **Review outstanding**.
- **Mentioned** (purple): a direct `@username` mention you have not answered in
  that discussion thread. Team mentions are not included. PRs with both a review
  and an unanswered mention appear once with both badges.

An unanswered mention on one of your own PRs appears in this column as well as
in **My opened pull requests**, so it cannot be missed just because you authored it.

PRs with the exact label **Blocked** or **Deferred**, ignoring case, are excluded
from this entire column, including mentions and completed reviews. They remain
visible under **My opened pull requests** if you authored them. Counts exclude
these hidden reviews; filtering cannot bring them back.

Inline review discussions are separate threads: replying elsewhere does not
clear a mention. The PR description, general conversation comments and review
summaries share the top-level conversation; a later comment or nonempty review
summary by you answers mentions there. A later mention in the same thread starts
waiting again. Pushing a commit does not count as answering a mention.

Outstanding review requests and unanswered mentions sort **before completed reviews**.
Within the pending group, cards sort by the **oldest outstanding request or unanswered mention**. Unrelated
commits, labels and comments do not reset this waiting age. If nothing is pending
(for example, a review already given), the PR's last activity supplies the sort
key instead. Missing request times also fall back to last activity, with a visible
note. Direct request dates come from GitHub's timeline; team request dates may
be unavailable without additional membership information.

Cards older than **30 elapsed days** have an amber accent and an explicit badge.
Pending reviews/mentions use their oldest waiting time (**Waiting over 30 days**).
Completed reviews, and requests with unknown request dates, use last activity
(**Inactive over 30 days**). A recent mention is not marked old solely because
the PR itself was created long ago. The threshold is checked at refresh time.

### My opened pull requests

PRs that **need your action sort first**. Within the action-needed and remaining
groups, PRs sort by **oldest last activity first**. **Needs your action** (amber) marks changes requested
or unanswered reviewer/conversation feedback. A later reply in the relevant
thread or a newer commit clears that feedback's highlight. New feedback can
highlight it again. Approvals alone do not trigger the highlight.

### Inspect and filter

Both columns filter locally by title, repository, PR number or label. Every card
shows **Draft** or **Ready for review**, labels sorted by name ignoring case,
author, reviewers and assignees. The exact label name **Accepted**, ignoring case,
has a prominent green badge and green card edge. These draft/ready and label
treatments also appear in **Synchronize to JIRA**. Colors always have text badges.
The metadata line beside the repository and PR number shows **Created: DATE,
Last activity: AGE**, and **Waiting since: AGE** for pending requests/mentions
in the requested-review column. Only values are bold; the text uses the same
12px size as author/reviewer details, without backgrounds or borders. Creation
dates use `yyyy-MM-dd`; ages use full words such as `155 days` or `2 hours`.
Hover a date/age for its exact timestamp. Missing creation/request times show
**Unknown**, not a guessed date. The line wraps naturally on narrow screens.

### Linked JIRA tickets

Known ticket links come from local receipts and Web Links of tickets already
loaded in the selected sprint/no-sprint view. The whole JIRA project is never
scanned. Missing links do not mean no ticket exists. Locally created review
tickets are shared between the PR columns and shown separately on synchronization
cards; they do not by themselves prevent creation of an implementation ticket.

### Create a review ticket

Both **Requested reviews from me** and **My opened pull requests** offer
**Create review ticket**. AI drafts a summary, review-focused description and
story-point estimate. The confirmation dialog lets you edit the summary,
description and estimate, but fixes the prefix **Review PR owner/repo#123:**.
The full title is limited to 255 characters.

The ticket uses the configured project and issue type, is explicitly unassigned,
and keeps JIRA's initial workflow status. **Add to sprint** is optional and starts
unchecked. The PR URL is added to the ticket's **Web Links**. Unassigned tickets
will not appear in the synchronization tab's assigned-to-you ticket list.

Additional review tickets for the same PR are allowed. The dialog lists review
tickets previously created in this browser and explicitly warns that confirming
creates another. Tickets created elsewhere are not searched for.

If creation succeeds but linking/sprint assignment fails, **Finish review ticket**
retries those steps on the saved ticket, never issue creation. Unknown creation
outcomes require checking JIRA before permitting another attempt. If AI fails,
**Write review ticket manually** is available. Test mode simulates all mutations
without touching real receipts; AI drafting remains live.

### GitHub activity limitations

These are read-only activity indicators, not GitHub's formal review decision.
REST does not expose thread resolution, so resolving a thread without replying
may leave a badge visible. Commit timestamps (and recorded force-push events)
are used as evidence of action; pushing an older pre-existing commit can be
missed because REST does not provide every push's time. Edited mentions may not
have an independently identifiable mention time. Discovery follows GitHub's
search index and token visibility; search indexing can lag, and archived
repositories are excluded. Removed, never-completed requests are not retained.

Searches and all activity collections are paginated. Incomplete searches or
results exceeding GitHub's 1,000-result search cap produce warnings, not silent
truncation. Per-card activity errors leave the PR visible and warn that badges
and ordering may be incomplete. A failed account is reported without blocking
other accounts. If an entire refresh fails, previously displayed cards are
retained with an explicit stale-data warning. See [permissions](authentication.md).

## Synchronize to JIRA

The header shows the authenticated username for each forge account (including
its instance), your JIRA display name/email, the configured project key, and the
currently viewed sprint. JIRA Cloud does not expose legacy usernames; the current
account's display name and email identify you instead.

### Choose JIRA work

The **Sprint** selector lists active and future sprints on the configured Scrum
board. `jira.sprintNamePattern` filters both states; an explicit `jira.sprintId`
remains available even if it does not match the pattern. Closed sprints are not
listed. Exactly one identifiable active sprint is selected initially. If no
unique active sprint is available, Pullboard warns rather than guessing and
defaults to **No sprint assigned**. You can still select a future sprint.

**No sprint assigned** means the Sprint field is empty, not merely that the issue
is outside an active sprint. Tickets assigned only to previous/closed sprints
are not included. Every selection is restricted to unfinished tickets assigned
to your verified JIRA account in the configured project.

The selection survives manual refreshes for the current page session.
Reloading the page restores the active-sprint default. If a selected sprint
closes or disappears, the view falls back to unsprinted tickets with a warning.
Changing this selector never changes a ticket's sprint and does not change the
default sprint in the creation dialog.

### Filter and inspect

- **Filter pull requests** matches titles and PR numbers (including `#42`).
- **Filter tickets** matches summaries and ticket keys (such as `DEV-123`).

Matching is case-insensitive and immediate, without further API calls. Each
column reports its matching/total count; clear a search to see the full list.
Drag/drop targets follow the visible JIRA cards. The **Link existing** menu can
still select from all loaded tickets in the current sprint/no-sprint view.

PR cards emphasize the repository and list assignees and reviewers. Reviewers
include pending requests, requested teams and people who have already reviewed.
A PR author's replies do not make them a reviewer. Both participant lists are
sorted alphabetically, ignoring case.
A lookup failure leaves the card visible with an inline warning. Linked cards
retain their layout and show every known issue in a compact **Linked to:** row.

JIRA cards show the issue type (Task, Epic, etc.), a two-line plain-text description snippet and recognized PRs
from JIRA **Web Links**. Their titles (when available), URLs and live open,
closed, or merged status are displayed; unreadable status remains unknown.
The JIRA Development panel is not queried. Recognition/status checks are limited
to configured forge instances, and credentials are never sent to arbitrary URLs.

Tab switching does not destroy either view or interrupt an in-flight operation.
Drafting dialogs remain open across navigation. A browser reload starts a new
data session; saved partial receipts still allow safe resumption.
