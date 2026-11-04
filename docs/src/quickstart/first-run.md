# First Run

1. Open <http://127.0.0.1:4200> after running `npm start`, or use
   `npm run start:test` to rehearse without JIRA writes.
2. The **Pull requests** tab opens first. Inspect requested reviews, past reviews,
   unanswered mentions and your own open PRs. Resolve inline forge API warnings.
3. Open **Synchronize to JIRA**. Verify the header's forge/JIRA identities,
   project and intended sprint.
4. Confirm that the PR list belongs to the account behind each forge token,
   and that JIRA tickets belong to your configured email/current API user.
5. Click **Sync to JIRA** on one PR. Its title and description are sent to the
   configured AI provider only at this point, not during refresh or drag/drop.
6. Review the title, description and points. **Add to sprint** starts unchecked;
   check it and select a sprint only if desired. Cancel to make no JIRA changes,
   or confirm to create (simulate in test mode).
7. In normal mode, follow **Open PROJECT-123** and verify the assignee, optional
   sprint, estimate, Web Link and workflow state in JIRA. In test mode, confirm
   the inline simulated result instead; no new ticket exists.

Use the JIRA sprint selector to inspect active/future sprints or **No sprint
assigned**. Use each column's title filter to narrow work locally. JIRA cards
include description snippets and linked PRs from Web Links. See
[Dashboard](../user/dashboard.md).

Draft PRs target In Progress. Ready PRs target Review. Different project names
can be configured under `jira.statuses`; transition IDs are always discovered.

Both tabs load on page open and retain data when switching. Use **Refresh board**
to update only the current tab; there is no polling. The app never automatically
closes tickets. **Create review ticket** on either PR column drafts a review
ticket with a fixed `Review PR owner/repo#123` prefix; it remains unassigned and
keeps JIRA's initial status. Confirm the editable description/estimate and optional
sprint before creation. The PR becomes a Web Link; no project-wide scan runs.
