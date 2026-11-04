# Link Existing Work

Drag a PR card onto an existing JIRA ticket, or choose **Link existing** and
select a ticket with the keyboard. Both paths execute the same service method.

Pullboard rechecks the upstream PR, writes its URL to the issue's JIRA Web Links,
and discovers a transition to In Progress for Draft or Review for Ready. It
does not change summary, assignee, estimate, or sprint of an existing issue.
The original PR card keeps its description, reviewers, assignees and actions.
A green tint and a compact **Linked to: DEV-1, DEV-2, …** row show its associations.
One PR can link to multiple issues, and each issue can track multiple PRs. Use
drag/drop or **Link existing** again to add another issue; existing pairs are
disabled to prevent duplicates. Hovering a drop target changes only its color,
not its height. **Sync to JIRA** is disabled once tracked to avoid duplicate creation.

Link creation and transition are separate requests. A transition failure keeps
the JIRA link and offers a resumable partial result. A stable JIRA remote-link
global ID avoids adding another identical link when resumed.

The loaded tickets follow the JIRA sprint/no-sprint selection. Linked PRs are
shown on JIRA cards using Web Links, with live status when readable. In
[test mode](../test-mode.md), linking only reports simulated success and never
writes a Web Link or changes the ticket's status.
