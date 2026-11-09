# Ticket creation, linking and AI

The steps below describe **PR** drafting. The **[Upstream issues](upstream-issues.md)**
tab reuses this editable editor and the same Jira options, outcomes and Test/Live
guards, but always makes a deterministic implementation copy without AI. Its final
explicit create adds the upstream issue's web link; no comments/attachments or
clone-state/duplicate search are included.

1. Select a cached PR and choose an **implementation** or **review** ticket.
2. After Jira discovery is ready, the backend resolves purpose defaults against
   discovered types/choices and prepares AI text or a deterministic template. This
   creates **no Jira issue**. Missing discovery is an error, not fake configured data.
   Review titles start as `Review <repo>/<prid>: <PR title>`, for example
   `Review widget/42: Improve caching`, with the repository name (without its owner)
   and local PR/MR number. This applies with or without AI, is limited to 255
   characters, and remains editable.
3. Edit title, plain-text description, project, issue type, points, assignee,
   sprint, team and status. Project/type changes reload cached capabilities and
   eligible choices. All new PR/review/upstream editors start **Unassigned**, **No
   sprint** and **Jira default — no transition**, even with legacy `assignee: me`
   or workflow mappings. Task is the built-in type suggestion; an unavailable type
   remains unselected with guidance. Review choices exclude self. Team is a
   discovered select choice, not free text; team and sprint stay independently editable.
   All returned active/future sprints remain offered, regardless of name pattern or
   origin board, grouped Active/Future. Optional blanks are normal, not warnings;
   Jira-required values need explicit input. Valid edits and deliberate nulls survive
   option refreshes. Status is a discovered project/type dropdown, not free text;
   ambiguous names cannot be selected. Workflow suggestions remain metadata, not
   initially selected values.
4. Submit explicitly. Only this final action can create a Jira issue. A real result
   returns its actual key/URL when available, then attempts any supported submitted
   sprint follow-up, PR weblink and explicitly submitted transition. Inspect warnings;
   never repeat creation just because a follow-up failed. If Jira accepts creation
   but its success response has no safely usable key, this is still accepted success:
   `outcome: "created"`, `issue: null`, with an explicit warning and no issue link.
   No key, URL or cache entry is invented and key-dependent follow-ups are skipped.
   Inspect Jira, **do not resubmit creation**; the normal live-write refresh still
   applies. A disconnected/unknown creation outcome also blocks duplicate submission
   and requires checking Jira first.

Per-type points/team/sprint capabilities explain whether a value is sent on create,
assigned afterward (`follow_up`), unsupported or unknown. Only create/follow_up modes
accept non-null values; required-field blockers prevent submission. The supported
[sprint follow-up](jira.md) omits Sprint from the create body when absent from that
screen, then assigns the real issue only after confirmed creation. Atlassian Team
UUID discovery is unsupported, distinct from supported AssignedTeam select options.
Group union/project assignability permissions, refresh-time choice publication and
request costs are described in [Jira](jira.md). A retained stale cache remains usable,
with a quiet note and **Refresh Jira choices** action; no initial discovery means
explicit not-ready/failure, not fabricated empty choices.

The assignee control is one searchable accessible combobox with a virtualized list.
It loads the full cached eligible pool before enabling selection, then filters name,
username and exposed email locally without a request per keystroke. Email is neither
inferred nor shown if upstream hides it. **Unassigned** remains reachable even when
there are no matching people. Arrow keys navigate, Enter selects and Escape closes
the list; typing a query alone does not clear the selected person.

Status inventory is not a promise of direct reachability. A selected unique status
name requests at most one direct transition **after** live creation; already at the
target is a no-op. Unavailable/rejected transitions keep the created issue and show
a nonblocking warning, never rollback, automatic retry, multi-hop search or another
create. Success/warning notifications stay until dismissed and retain the confirmed
issue link. See [status behavior](jira.md#discovered-statuses-one-direct-transition).

Link existing work by dragging a PR onto a cached Jira issue or using **Link PR**
with the keyboard. The backend takes a cached PR ID and looks up its trusted URL;
it does not accept an arbitrary remote URL. Stable Jira global IDs make duplicate
weblinks idempotent. Forge content is never changed.
Existing many-to-many linking is unchanged: one explicit action links one PR/issue
pair; a PR or issue may have other links. There is no batch or unlink operation.

## Read-only test mode

In runtime **Test mode**, valid explicit create/link actions return ordinary
successful responses with `outcome: "simulated"` and an explicit success notice.
Creation has `issue: null`. **Zero** Jira create/sprint/link/transition writes happen,
no fabricated IDs are returned and no simulated cards or links enter the cache.
No write-triggered refresh is scheduled. Validation uses committed cached choices,
not new metadata/group/assignability discovery. Normal refresh still performs upstream
reads. Drafting may contact AI if enabled for a PR; upstream-issue drafting never calls AI.
“Read-only” does not mean offline.

A simulated success does not verify live write permissions or sprint assignment.
There is no hidden sync-driven write path. Forge adapters remain read-only in both modes.

## Runtime Test/Live mode

YAML `read_only_test_mode` initializes one **backend-global** mode on every startup
(default `true`). The persistent Test/Live strip shows the current mode. Choose
**Switch to Live**, review the confirmation and select **Enable Live writes** only
when ready; switching to Test also has a confirmation. All tabs and local API clients
share this setting. The UI polls it about every two seconds; this is not instantaneous
cross-tab messaging. The override never edits YAML or persists in SQLite, localStorage
or display preferences. Restart restores the YAML value.

The backend checks the action's captured mode revision and snapshots its mode
atomically **at admission**, before upstream calls or business validation. The
admitted mode governs the whole action, including follow-ups. An admitted simulation
stays zero-write after switching Live. **An already admitted live operation may
finish creating/linking/transitioning after switching to Test.** The switch controls
new admissions, not cancellation or draining of in-flight actions. Toggling alone
does not contact upstream services or trigger a refresh.

### Stale dialogs and gestures

Ticket/link dialogs capture the mode and revision when opened; dragging captures
the pair at pointer-down. Upstream issue URL entry captures it on Enter/button
submission, before lookup and editor opening. Polling never silently upgrades a
simulated action to Live or replaces its token. A toggle in any tab, a toggle back
to the same mode, or a server restart invalidates that old revision. A known
mismatch or unverifiable mode blocks submission; the backend also rejects stale
requests with `409 mode_conflict`
before any upstream calls. Nothing is automatically retried.

Edits and selections remain intact. Select **Review current mode**, confirm **Use
reviewed mode**, then separately submit the create/link action again if intended.
Reviewing does not submit it. A stale mode-switch confirmation must be cancelled
and reopened. After a stale drag, use Link PR to review and submit deliberately.
If mode lookup fails, use **Check mode** before proceeding. None of these review
steps permits replaying a live create with an unknown outcome; inspect Jira first.

## AI providers

AI is disabled by default. The provider/model defaults are **`openai` /
`gpt-6-luna`**, and the exact model string is passed unchanged. This does not
guarantee your provider/account offers that model. There is no silent substitution.

| Provider | Default base URL | Wire endpoint |
| --- | --- | --- |
| `openai` | `https://api.openai.com/v1` | `/chat/completions` with JSON response format |
| `anthropic` | `https://api.anthropic.com/v1` | `/messages` |
| `ollama` | `http://127.0.0.1:11434` | `/api/chat`, nonstreaming JSON |

Configure `enabled`, `provider`, `model`, optional `base_url` and `token` together
in YAML. Cloud providers require a literal/keyring token. Choose an actual model
ID for Anthropic or an installed Ollama model; changing provider does not guess a
compatible model. Ollama may use `token: null`. In a container, `127.0.0.1` refers
to **that container**, not the host. Use a deliberately reachable approved model
service URL and appropriate network isolation; do not expose PullBoard itself.

Only selected PR title, description, repository, URL and labels plus requested
purpose go to AI—not the whole board, credentials or other issues. PR content is
explicitly untrusted. The AI has no tools and cannot submit tickets. Remote AI
still receives that selected private content: check your organization's policy.

Disabled/unavailable models, rate limits, invalid responses and request failures
return a safe warning and an **editable deterministic template**. Configured
keyring resolution failures remain explicit configuration/startup errors; they are
not silently ignored. No AI-generated text is a claim that changes were verified.
