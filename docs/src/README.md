# Your next reply, in one place

PullBoard runs on your own machine, brings together account-relevant PRs and Jira
issues, and makes unanswered conversations visible. It is not a forge client with
write permissions, a shared hosted service, or an autonomous Jira agent.

**Needs Attention** is an oldest-first queue of upstream conversations needing
your response. **Upstream Pull Requests** is a title-sorted view of assigned Jira
work and your open PRs. Individual threads—not commits, local reads or
dismissals—determine whether attention remains.

The header shows the first configured Jira project's discovered key/name and only
pattern-matching active sprints. Overview starts on a sprint only when exactly one
matches; zero/multiple matches require a deliberate filter choice. Attention starts
with all sprint and no-sprint work. Ticket editors start Unassigned, No sprint and
Jira default/no transition, using complete choices cached during normal refresh.
See [dashboard defaults](attention.md) and [Jira choices](jira.md).

**[Upstream issues](upstream-issues.md)** is a separate compact list of open issues
assigned to you on configured GitHub, GitLab, Codeberg and Forgejo instances, with
independent forge/repository/title filters. Clone a row or enter a configured-instance
issue URL to open a deterministic editable Jira draft. Closed/unassigned URL lookups
are allowed; nothing is created until explicit submission. This path uses no AI and
does not copy comments/attachments or track clones/search Jira for duplicates.

Start with the [empty local board](quickstart.md), then configure only the accounts
you need. There is no runtime sample-data mode: empty means empty. Keep read-only
test mode on while learning the explicit ticket and link flows. Simulated actions
succeed visibly but do not invent records or IDs.

All application settings live in one YAML per run. `docs/book.toml`, Cargo
manifests, npm metadata, Make variables and CI YAML configure **build tools**, not
alternative application configuration sources. Secrets are never read from
environment variables. The sole runtime override is the shared Test/Live action mode:
YAML initializes it, the UI confirms changes, and restart resets it. It is never persisted.

During refresh, each source has its own progress panel above the board; real failures
and backoff also stay prominent. Healthy idle collapses completed counts/durations
into Source health Details. Successful refreshes are green even with neutral coverage
notes; attention-history completeness is separate. The old cards remain visible until
atomic publication. Equal 300px-high Jira/PR cards show useful summaries in Upstream
Pull Requests and concise attention summaries before excerpts, with full
keyboard-accessible details preserved. Upstream issues instead use full-width compact rows.

Read [provider coverage](providers.md) before relying on an empty attention queue.
Automated tests exercise recorded/synthetic upstream fixtures and mocked browser
APIs, not live accounts. Container Secret Service interoperability is documented
as opt-in and must be verified on your host; it is not assumed deployment-proven.

PullBoard is GPL-3.0-or-later, without warranty. The repository's `LICENSE` contains
the full GPL v3 text; the image includes it under
`/usr/share/licenses/pullboard/LICENSE`.
