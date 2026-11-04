# WebUI

`App` provides Material tab navigation into a lazy-loaded `Workspace` shell.
The shell mounts `PullRequests` (default `/`) and `Dashboard`
(`/synchronize-to-jira`) together, hiding the inactive view. Both load concurrently
and remain alive across navigation, preserving data, filters and operations.
There is no deactivation guard or reload on tab switching. Each view has two
responsive columns and its own manual refresh. JIRA cannot delay the PR inbox.
Angular Material supplies cards, buttons, progress bars, menus and
dialogs. Tailwind 4 supplies responsive grid/flex utilities; SCSS defines the
workbench's type, color and status treatment. There are no remote fonts.
The compact header identifies forge accounts, the JIRA account, project and
viewed sprint. Each column uses Signal Forms for local case-insensitive title
filtering (plus ticket keys/PR numbers), with matching/total counts. The JIRA
selector loads active/future sprints or tickets with no sprint assignment;
manual refreshes preserve the current selection. Neither tab polls. JIRA cards include
issue types, plain-text two-line ADF description snippets and recognized Web Link PRs.
PR cards emphasize repositories and list reviewers and assignees.

The shared `PrBadges` component renders draft/ready status and sorted labels;
Accepted labels have a strong green treatment in both views. The PR view adds
blue outstanding-review, muted review-given, purple mention and amber action-needed
badges, with matching card accents and text equivalents. Signal Forms filters
match titles, repositories, numbers and labels locally. Pure helpers determine
action-first grouping, waiting-time/last-activity sorting and snapshot relative ages.
Blocked/Deferred labels exclude requested-review cards; reviews older than 30 days
get an amber accent and a waiting/inactivity badge. The shared `PrActivity`
component renders creation date and full-word activity/waiting ages beside the
PR number. Only values are bold, at the same 12px size as participant metadata.
The inline text wraps on narrow screens, without backgrounds or borders. Exact timestamps
are available on hover. Activity classification and API limitations are documented
in the [dashboard guide](../user/dashboard.md).

`PrJiraLinks` is a shared signal registry of locally known receipts, without
HTTP dependencies or project-wide discovery. The synchronization view publishes
associations from its loaded tickets and saved receipts; `ReviewTickets` publishes
newly completed review links. Missing links are not presented as verified absence.

`dashboard.html` demonstrates typed CDK drag/drop with one source list and an
individual destination list on each JIRA card. Dragged PRs are not moved out of
the source array; a successful association adds a compact linked-issue row and
green tint, retaining the rest of the card. A PR can associate with multiple
issues; each issue can associate with multiple PRs. The destination enter
predicate rejects existing pairs. PR placeholders are hidden inside ticket drop
targets so hovering never enlarges a ticket or shifts its neighbors.
The **Link existing** Material menu is the keyboard/touch alternative.

`SyncDialog` uses Angular 22 Signal Forms and Material inputs. Title and
description are required; titles are capped at 255 characters and estimates
at 0–100. Dialog submission returns data; API orchestration remains outside it.
The **Add to sprint** checkbox starts false. Its dependent sprint selector is
disabled until opted in, validates an available active/future sprint, and uses
the context's active default independently of the JIRA viewing selection.
`ReviewTicketActions` uses this dialog through the shared `ReviewTickets` service.
Review creation fixes the title prefix outside the editable suffix, explicitly
sets JIRA's assignee to null and leaves the workflow status unchanged. Additional
review tickets require confirmation with prior local tickets disclosed. Partial
and uncertain operations persist separately under `pullboard:reviews:v1:...`;
resumption only adds the Web Link and optional sprint. AI runs only on demand.

Local receipt state is keyed by JIRA base URL, project and email, then PR URL.
Completed associations are stored separately from the current operation's receipt
so linking another issue cannot overwrite prior links. Legacy single receipts
are still restored. Only receipts are persisted. In-flight creation leaves an uncertainty marker
before the non-idempotent request, so a tab crash does not invite a blind retry.
Partial state retains the requested sprint ID. In test mode, all preview state
stays in memory, normal receipts are ignored and untouched, and fake receipts
never enter saved links. A persistent banner and dialog notice identify test
mode; inline success messages explicitly distinguish simulated actions.

Status is communicated by text as well as color. Errors and success are inline
and announced through live regions; success uses prominent, persistent real links
with `noopener noreferrer`. Focus rings, a skip link, responsive columns, and
reduced-motion styling are included. No snackbar/toast service is used.
