# Create a Ticket

Choose **Sync to JIRA** on an unlinked PR. Pullboard sends only its title and
description (up to 40,000 description characters) to the configured AI provider.
No source files, comments, credentials, or existing JIRA context enter the prompt.
The model suggests title, plain-text description, and story points. Review/edit
all three in the Material dialog. Cancel makes no JIRA writes.

**Add to sprint** is unchecked each time the dialog opens. Leave it unchecked
to create the issue without a sprint. Check it to select an active or future
sprint; the active sprint is preselected when uniquely identifiable, regardless
of which sprint you are viewing. With no active sprint, you can select a future
one explicitly or create without a sprint. Closed sprints are never offered.

Confirming performs these steps:

1. Re-read PR state; refuse a PR that has already closed.
2. Verify the user account, issue type, and create-screen fields. If requested,
   check that the selected sprint is still active or future on the configured board.
3. Create the issue with summary, Atlassian Document Format description,
   story points and your account ID as assignee.
4. Save the returned ticket key locally immediately.
5. Add the PR as a JIRA remote Web Link and, only if opted in, add the issue to
   the selected sprint.
6. Discover available transitions for that issue and move Draft to In Progress
   or Ready to Review (using configured destination status aliases).

The result is a permanent inline success card with a prominent HTML JIRA link.
Receipts remain available after refresh; receipts for PRs no longer open appear
under **Saved JIRA links**.

These requests are not a transaction. A later failure leaves a linked partial
card with **Resume sync**, reusing the saved ticket instead of creating another.
Remote link writes use a stable `globalId` for idempotent retries. If the PR
closes during a partial sync, inspect and complete the existing ticket in JIRA.

An interrupted/unknown creation outcome must be reconciled in JIRA before
allowing a retry. Use one tab/instance; cross-tab or cross-device deduplication
of new issues is not guaranteed. Do not clear local receipts during a partial
operation. LLM failures offer **Write ticket manually** without sending more data.

Partial receipts preserve the exact chosen sprint ID (or no sprint). Resume
never substitutes a newly active sprint; a closed selected sprint requires
manual reconciliation. Older receipts that requested sprint assignment without
saving its ID must be completed in JIRA rather than guessing a destination.

In [test mode](../test-mode.md), creation and every later mutation are simulated.
The inline result is clearly labelled and contains no fake link to a new issue;
no normal receipts or JIRA tickets are changed.
