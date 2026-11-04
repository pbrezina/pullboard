# Close Tracked Work

In [test mode](../test-mode.md), the checks below still read real PRs and Web
Links, but closing only reports simulated success. The real ticket and its
displayed status remain unchanged.

For every issue in the selected sprint/no-sprint view, Pullboard reads JIRA Web Links, asks registered
forge strategies which URLs they recognize, and reads those PR statuses.
Ordinary document links are ignored. Configure the appropriate forge/provider
for every PR you want monitored; unsupported PR URLs are not status-checked.
No JIRA key parsing or PR comments are necessary.

A closed or merged PR gives its ticket an amber border and closure prompt.
**Close JIRA Ticket** is enabled only when every recognized PR is closed/merged
and no Web Link/status request failed. This prevents a ticket with several PRs
from being closed when just one merged. A closed-but-unmerged PR also qualifies;
you decide whether its associated work is actually complete.

Clicking the button re-reads Web Links and upstream statuses immediately before
discovering an available transition to Done or Closed. Unknown status or a
reopened PR prevents the write. Nothing changes upstream and no automatic
closure is performed.

The closed card remains in the UI with its JIRA link until the next refresh;
completed issues are then excluded from the ticket query. There is a
small unavoidable race between remote reads and the JIRA transition; unrelated
APIs cannot provide a shared transaction.
