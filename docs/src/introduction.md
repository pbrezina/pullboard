# Introduction

Pullboard connects upstream engineering work to the JIRA Cloud sprint where
you track it. It is a single-user Angular application running locally through
`ng serve`. No separate server or OAuth application is required.

The default **Pull requests** tab brings together requested reviews, past reviews,
unanswered mentions and your own open PRs. Waiting ages help prioritize replies;
action-needed and Accepted badges make important PRs stand out.

In **Synchronize to JIRA**, the left column aggregates your open pull requests. The right column shows
your assigned, unfinished issues in an active or future sprint, or without any
sprint assignment. Both lists support instant title filtering. Create a new story with
reviewed AI suggestions, link a PR to existing work, or explicitly finish a
ticket whose tracked PRs have closed.

Both tabs load together when the page opens and preserve data when switching.
Manual refresh updates only the current tab; neither polls. The PR tab also
creates unassigned review tickets with editable AI suggestions and a fixed
`Review PR owner/repo#123` title prefix. Whole-project JIRA link scans are disabled.

**The forges are always read-only.** Pullboard never edits, comments on, labels,
merges, closes, or otherwise changes a pull request. Only JIRA is writable in
normal mode. Run `npm run start:test` to read real data and simulate all changes
without writing to JIRA. See [Test Mode](user/test-mode.md).

This book contains quick-start instructions in `quickstart/`, user guides in
`user/`, API reference in `api/`, and developer documentation in `developer/`.
Lifecycle chapters live under `user/lifecycle/`. Build it with `mdbook build docs/`;
no additional preprocessors are required.
