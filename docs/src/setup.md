# Terminal setup wizard

`pullboard-setup` is a disposable **Linux x86-64** terminal wizard, not a background
updater. It configures one YAML file and a rootless Podman Quadlet. No macOS or
ARM64 setup executable is supplied. The [manual container path](quickstart.md)
remains available; setup does not provision packages, use sudo, enable lingering,
install a keyring bridge or write to Jira/forges.

Setup asks one question at a time in your normal terminal—no full-screen interface
or function keys. It explains token creation and permissions as you go. On reruns,
saved answers are the defaults; Enter keeps a saved token without displaying it.

## Before starting

Already install these using your distribution's instructions:

- Rootless local Podman, cgroup v2 and the Podman **Quadlet generator**. Confirm
  `podman --remote=false info` works as your ordinary user; a remote engine is not supported.
- systemd with a working user manager: `systemctl --user status` must reach it.
  A graphical desktop that manages `graphical-session.target` is needed for verified
  graphical-login autostart. SSH/terminal-only sessions are not graphical sessions.
- An interactive terminal, `curl`, `gzip`,
  `sha256sum`, `mktemp` and standard POSIX shell utilities for the launcher.
- HTTPS access to GitHub Releases, GHCR and the configured providers. No Rust,
  Node, SQLite server or compiler is required to run the downloaded static binary.

Missing runtime prerequisites produce actionable errors; setup does not install
them for you. Choose a free unprivileged host port (default 3333). Access stays on
`127.0.0.1:PORT`; there is no authentication UI and it must not be exposed to a LAN.

## Download and launch

Stable releases use the latest versioned release's setup executable and select the
`:latest` image:

```sh
curl --fail --location --proto '=https' --proto-redir '=https' \
  https://github.com/pbrezina/pullboard/releases/latest/download/pullboard-setup.sh | sh
```

For the rolling nightly prerelease (unreleased development changes):

```sh
curl --fail --location --proto '=https' --proto-redir '=https' \
  https://github.com/pbrezina/pullboard/releases/download/nightly/pullboard-setup.sh | sh -s -- --nightly
```

`--nightly` both selects nightly **setup assets** and forwards that choice to the
wizard to preselect the **nightly container image**; Runtime can change the image
choice. `--no-color` or `NO_COLOR=1` disables color. `sh … --help` prints launcher
help without downloading a binary; the executable also supports `--help`.

The launcher downloads into a private `mktemp` directory, verifies the fixed
compressed asset against `SHA256SUMS` **before** decompression/chmod/execution,
restores `/dev/tty` stdin for the wizard, forwards quoted arguments and returns its
exit status. It removes its downloads on exit, including handled cancellation.
Neither it nor the binary is copied into `~/.local/bin`. An uncatchable kill or
power loss can leave the private temporary directory; remove that specific leftover
directory after confirming no setup is running. Do not run setup as root.

Piping a launcher to `sh` executes code from this publisher. HTTPS and your trust in
the GitHub account/release are the trust boundary: a checksum from the same release
detects corruption, **not independent publisher authenticity**. Review the launcher
or download assets yourself if preferred. Release assets have predictable names:
`pullboard-setup.sh`, `pullboard-setup-linux-x86_64.gz`, `SHA256SUMS`, and
`setup-sizes.txt` (measured raw/compressed byte counts). After downloading all four
from one versioned release into a private directory, verify before executing:

```sh
sha256sum -c SHA256SUMS &&
  gzip -dc pullboard-setup-linux-x86_64.gz > pullboard-setup &&
  chmod 700 pullboard-setup &&
  ./pullboard-setup
```

Delete the downloaded files afterward.
A versioned wizard download does not pin the service image: Stable tracks `latest`.
No fixed download-size claim is made; each build logs its actual sizes.

### Share team Jira settings, not credentials

A team command can supply the shared Jira settings while each user enters their
own email and masked token. Replace these example names and IDs with your tenant's
real values:

```sh
curl --fail --location --proto '=https' --proto-redir '=https' \
  https://github.com/pbrezina/pullboard/releases/latest/download/pullboard-setup.sh | \
  bash -s -- \
    --jira-url=https://example.atlassian.net \
    --jira-projects=DEV,OPS \
    --jira-board-id=42 \
    --jira-board-id=43 \
    --jira-groups=engineering,reviewers \
    --jira-team-field=AssignedTeam \
    --jira-team='Platform Team' \
    --jira-sprint-name-pattern='^Platform ' \
    --jira-sprint-name-pattern='^Other[0-9]{1,3}$'
```

The launcher forwards these arguments unchanged to the setup executable; they are
**setup inputs**, not a second application configuration source. The verified
settings are eventually saved in the same single YAML file. Both `--flag=value`
and `--flag value` work in the launcher and directly in `pullboard-setup`.

| Parameter | Meaning and verification |
| --- | --- |
| `--jira-url URL` | HTTPS Jira Cloud tenant root only; no credentials, port, API/gateway path, query or fragment. Skips the URL prompt. |
| `--jira-projects KEY1,KEY2` | 1..100 unique project keys, verified individually for the current account. The first key is the primary/header project; input order is preserved. |
| `--jira-board-id ID` | Repeat for 1–100 unique positive board IDs, verified through Jira. Supplied order is preserved; first is **My board**. Do not comma-separate IDs. |
| `--jira-groups GROUP1,GROUP2` | At least one exact assignee-group name; verify every name and complete membership-read access. Never fall back to all users. |
| `--jira-team-field NAME` | Exact unique supported semantic team field name, not a raw field ID such as `customfield_123`. Included because the interactive wizard explicitly asks for a field even when only one is available. |
| `--jira-team NAME` | Exact team option name, verified against selected-project/type create metadata. It need not be observed in active-sprint board issues. |
| `--jira-sprint-name-pattern REGEX` | Repeat for 1–100 unique bounded Rust regex searches, at most 4096 UTF-8 bytes each; first is **My sprint pattern**. Never comma-split: commas in quantifiers such as `{1,3}` remain part of one regex. Quote regexes; case-sensitive unless inline flags change it. |

Every flag is optional individually: provided settings bypass their pickers, not
verification; omitted settings remain interactive with existing values as defaults
on reruns. Supplying any of these flags enters the Jira section without asking
whether to enable it. Only `--jira-projects` and `--jira-groups` use commas as separators,
trim surrounding whitespace, reject empty entries and deduplicate in first-occurrence
order. Names containing a literal comma cannot be represented by those two flags;
select them interactively. Only board-ID and sprint-pattern flags are repeatable;
their values must be unique and remain in supplied order. Repeating other flags or
supplying malformed values is rejected. Email, token, issue-type policies and
per-project workflow overrides have no command-line parameters. There are no
credential environment inputs.

Explicit projects, boards, groups and team do not require recent-item, own-group,
board-issue team discovery or broad inventory scans. They still require identity,
semantic fields, scoped create metadata and active/future sprint verification.
Failed supplied-setting verification offers **Retry / Correct this setting
interactively / Cancel setup**; an argument rejected for correction is never
silently reapplied. Network/metadata failures retain the normal connection recovery
flow. Zero or multiple active matches on My board using My pattern still require
explicit acknowledgement or correction. All collected sprints remain in board filters;
creation choices include only names matching any configured pattern.

On an existing configuration, setup displays old → proposed supplied values and
asks before applying them. Unspecified settings remain unchanged unless edited in
their normal prompts. Project removal separately acknowledges orphaned overrides
and affected forge default-project links. Changing tenants **never** authorizes
sending a saved token to the new destination: separate credential consent or token
reentry is required before any Jira request.

A complete team command needs no individual Jira setting prompts after credentials
except warnings that require acknowledgement. It leaves advanced policies at their
defaults (or preserves saved ones). A token-free Jira review offers interactive
editing before continuing; the final installation review still applies. Forge,
AI, runtime, Test-mode and installation questions remain interactive and unchanged.
This is not unattended setup, and setup never writes upstream.

### Legacy editable URL prefill

For example, a team can share this **nonsecret** tenant hint:

```sh
curl --fail --location --proto '=https' --proto-redir '=https' \
  https://github.com/pbrezina/pullboard/releases/latest/download/pullboard-setup.sh | \
  sh -s -- --jira-cloud-url=https://redhat.atlassian.net
```

`--jira-cloud-url URL` is also accepted. The URL must be an HTTPS tenant root,
without userinfo, query, fragment, gateway or API paths. It prefills an **editable**
field; no discovery or saving happens merely from supplying it. New setup has no
hardcoded tenant. On edit, the existing URL is used unless deliberately changed.
Changing a destination does not authorize sending an existing token there: explicit
credential confirmation/reentry is required first. Never put tokens in command-line
arguments, environment variables, shell history, screenshots or shared commands.
This legacy flag keeps its editable-prefill behavior; it is not an alias for
`--jira-url`. Supplying both URL flags is an error.

## Accounts and permissions

Choose expiry dates and revoke unused tokens on their provider's settings page.
Organization approval, SSO, repository membership and server version can all affect
visibility. A verified identity is **not** proof of complete API or private-repository
access; inspect source health after the first runtime refresh.

### Jira Cloud

Use your Atlassian account **email plus an unscoped API token** from
[Atlassian API-token settings](https://id.atlassian.com/manage-profile/security/api-tokens).
See [token help](https://support.atlassian.com/atlassian-account/docs/manage-api-tokens-for-your-atlassian-account/).
Scoped-token gateway routing (`api.atlassian.com/ex/jira/…`) is not supported by this
tenant-root setup flow; do not paste a scoped gateway token and assume it will work.

The account needs access to configured projects/issues and comments/history; the
chosen Agile boards and active/future sprints, primary-board configuration; visible fields,
create metadata and per-project/type statuses. Group discovery must expose the
selected groups and **their complete memberships** (tenant permissions often require
Browse users and groups or administrator assistance). Runtime also verifies each
candidate's project assignability. Setup requires at least one explicitly selected
assignee group and never silently falls back to all users. Missing metadata/group
permissions can block refresh and ticket options even if `/myself` succeeds.

For later **explicit Live submissions**, your account additionally needs the actual
Jira permissions for issue creation, assignment, remote links and any requested
transition. Sprint updates require applicable Scrum/Sprint support plus Edit Issues
and Schedule Issues. Read verification and simulated success do not prove write
permissions. See [provider requirements](providers.md#jira) and [Jira choices](jira.md).

Project and board selection opens the **recent-item picker immediately** when choices
are available, not a selection-method menu or the tenant's entire inventory. Board
selection supports multiple choices. Accepting it verifies every selected board and
asks **My board:**, then saves that board first. Project selection similarly asks for
a primary project when needed. Press **Ctrl-O More options** in a project, board or
group picker to open its actions menu. Recent lists
are shortcuts, not complete lists of what you can access. When editing, saved
selections absent from those lists are verified directly and remain available;
they are not silently removed.

- Add a project using its key, exact name, or full project URL. Setup verifies it
  through Jira; if an exact name identifies multiple projects, choose the intended
  key explicitly.
- Add a board using its numeric ID or full board URL. Setup verifies that board
  directly. Project and board URLs must belong to the configured Jira tenant;
  entering a URL never redirects your credentials to another service.
- **Board actions** lets you add/remove selections from the recent/verified list,
  add a manual board, retry saved-board verification, or continue and choose **My board**.
  Keep 1–100 unique boards. Saved selections retain their order and remain selected
  even when absent from recents; an unverified board must be retried or explicitly removed.
- Explicit full-lookup actions can load **all projects**, or **all boards relevant
  to the selected projects**. Each requires confirmation after a warning about long
  waits and API limits. Setup never scans every board in the tenant automatically.
  Boards may be shared by multiple projects and are deduplicated.
- Empty or unavailable recent lists still allow manual entry and explicit lookup.
  Recent boards use Jira's undocumented internal API, which may change or become
  unavailable independently of ordinary board access.

Assignee groups default to **groups you belong to**, discovered through Jira's
`/rest/api/3/user/groups` endpoint using your verified account ID. This needs
**Browse users and groups** permission and does not enumerate all tenant groups.
The Ctrl-O group menu offers exact-name lookup, retry, and an explicitly confirmed
all-groups scan. Saved groups outside your memberships remain selected after their
exact identity and member-read access are verified. Unverified saved groups must
be retried or explicitly removed; they are never silently dropped. Setup still
checks complete member-read access for the selected assignee pools.

Manual entries are checked after submission, not on every keystroke. Setup reuses
verified selections and bounded successful reads within the current credential
scope; it defers group and field discovery until after project/board selection.
Errors identify the failed operation and retain safe Jira messages, HTTP status,
timeout or pagination-limit details instead of replacing every failure with a
generic message. Credentials, authorization material, raw response bodies and
unsafe terminal formatting are never printed. Full lookups must complete within
the configured pagination bounds; partial results are not presented as complete.
The default limits remain 30 seconds per request, 10 seconds to connect, 100 pages
and up to 100 requested rows per page; a multi-page lookup can take longer overall.

Choose the intended primary project, **My board** and **My sprint pattern**; each is
saved **first** in its respective ordered list. All selected boards supply a complete
active/future sprint inventory, deduplicated by sprint ID with first-configured-board
provenance, never by following `originBoardId`. My board alone supplies header links
and estimation configuration. Boards and patterns are independent, not paired.

Without a supplied verified `--jira-team-field`, setup explicitly asks
**Team field (semantic name):** even when only one field is
available. The **Team** picker initially offers verified option values observed on
**My board's** issues across **all returned active sprints**. It does not
scan future sprints or history, or filter this discovery by the sprint-name regex.
Observed option IDs and values must match selected-project/type create metadata;
an observed team is **not proof that you personally belong to it**.

These read-only issue lookups use the current Jira Software API
(`rest/software/1.0/board/{boardId}/sprint/{sprintId}/issue`) with bounded token
pagination. Failed or incomplete reads never supply a partial team list. If no
usable list is available, setup opens **Team actions** instead of silently scanning
more broadly. **Ctrl-O More options** in the Team picker also opens those actions:
return to board-derived choices, choose **all verified selected-project/type
create-metadata teams** without another scan, or retry board issue discovery.
A saved team remains available if still verified by that metadata, even when not
observed in current active-sprint issues; setup labels that distinction.

**Regular expression to identify your sprints:** edits the first/saved pattern and
accepts a bounded **Rust regex search**, case-sensitive unless inline flags change
it; anchors are optional. For example, `TEAM` searches anywhere, `^TEAM` matches a
prefix, and `(?i)team` ignores case. Each expression is at most 4096 UTF-8 bytes,
with bounded compiled size and no percent-token substitutions.

**Sprint pattern actions** offers **Add sprint pattern**, **Edit sprint pattern**,
**Remove sprint pattern**, **Choose My sprint pattern**, and **Continue with these
patterns**. Keep 1–100 unique patterns; choosing My pattern moves it first. Saved
additional patterns survive edits. The preview labels each active/future sprint
**Included creation** or **Excluded creation**, shows its queried board, and marks
**My current sprint match** only for active first-board/first-pattern matches.
Zero/multiple current matches require **Continue with no unique current-sprint
default?** acknowledgement or correction. There is no additional-board/pattern fallback.

Any pattern permits a creation choice across all selected boards, independently of
ticket team and purpose. Nonmatches still appear in board filters and existing
assignments; patterns never restrict ticket discovery or Attention. New tickets
start No sprint, Unassigned and no transition; workflow mappings are suggestions only.
Setup writes `board_ids` and `sprint_name_patterns`; old singular YAML keys are
[rejected rather than migrated automatically](configuration.md#migrate-singular-sprint-settings).

The wizard does not ask for issue types or workflow suggestions. New configurations
use Task for implementation and review, with no workflow suggestions. To customize
these, edit the [YAML purpose policies](configuration.md#purpose-policies-and-cached-choices).
Setup preserves saved policies when editing an existing configuration.

### Forges

Walk through saved instances or add another: provider, display name, URL, token and
verification. The editable name defaults to the forge kind, such as `github`;
additional instances of the same kind default to `github-2`, `github-3`, and so on.
Generated names skip names already in use. Adding another instance does not rename
the first. Saved custom names are preserved; editing an unnamed singleton suggests
the kind name instead of its ID.
New instances receive stable IDs automatically; existing IDs are retained, even
when changing the display name.
At any forge prompt, **Esc** discards the current draft and returns to forge
management. Cancelling an edit preserves the existing forge unchanged; cancelling
a new account does not add it. **Ctrl-C** still exits the whole setup.
GitHub uses its API base (`https://api.github.com`); GitLab, Codeberg and Forgejo
use instance roots. Repository include/exclude globs and a forge's default Jira
project are YAML-only settings, preserved when editing an existing configuration
(apart from confirmed cleanup of links to removed Jira projects). New instances
use no repository filters and the primary Jira project. Repository globs narrow
account discovery, not token scope. Private repositories need actual credential access.

| Provider | Token creation/help | Permissions and caveats |
| --- | --- | --- |
| GitHub | [Classic PAT creation](https://github.com/settings/tokens/new), [fine-grained PATs](https://github.com/settings/personal-access-tokens/new), [help](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens) | Setup offers classic PAT guidance. Classic `repo` enables private-repository discovery but is **broad and includes upstream write rights**; PullBoard never uses forge writes. Fine-grained tokens are possible with relevant repositories and read-only Pull requests, Issues and Metadata, but cross-organization/search access, org approval and SSO need checking. |
| GitLab | [GitLab.com tokens](https://gitlab.com/-/user_settings/personal_access_tokens), [help](https://docs.gitlab.com/user/profile/personal_access_tokens/) | Start with `read_api`; use your self-hosted instance's token page when applicable. Membership and server policy still apply. |
| Codeberg | [Token settings](https://codeberg.org/user/settings/applications), [help](https://docs.codeberg.org/advanced/access-token/) | Read-only user/repository/issue scopes, commonly `read:user`, `read:repository`, `read:issue`; availability depends on deployment/version. |
| Forgejo | Your instance's `/user/settings/applications`; [help](https://forgejo.org/docs/latest/user/token-scope/) | The same version-dependent read-only scopes; do not send an instance token to another host's settings/API. |

Forge verification reads identity; full account-relevant PR/issue discovery happens
at runtime. No forge write scope is required by PullBoard itself. See
[provider coverage](providers.md) for search and history limits.

### Optional AI

New setup defaults to **Disabled**. OpenAI, Anthropic and Ollama are optional;
editable deterministic templates remain available when AI is disabled or fails.
Enter provider endpoint/model and credentials only in the wizard. OpenAI defaults
to the exact editable ID `gpt-6-luna`, passed unchanged; model existence/access is
not guaranteed and no substitute is silently chosen.

Enabling remote AI can send the **selected PR's content, including private content**,
to that provider for drafting. Check your organization's policy, provider retention
and billing before enabling it. Setup sends no real PR content to AI. An optional
synthetic inference test requires separate explicit consent and **may incur a charge**;
failure permits saving with a fallback warning. Provider pages:
[OpenAI keys](https://platform.openai.com/api-keys),
[Anthropic keys](https://console.anthropic.com/settings/keys),
[Ollama networking](https://docs.ollama.com/faq#how-do-i-configure-ollama-server).

Ollama must already be reachable **from the container**. Host `localhost` is not
container `localhost`; a successful host-side test does not prove container reachability.
Use an administrator-reviewed endpoint/network arrangement; setup does not install
Ollama, expose its listener, change firewall rules or guarantee `host.containers.internal`
works on every host. See [AI behavior](tickets-ai.md).

## Runtime, review and installation

Press Enter to accept an answer or default, use arrow keys to select an option,
and Space to toggle multiple selections. Type to filter searchable lists. **Ctrl-O
More options** opens discovery actions in project/board/group/Team pickers; Esc also
returns to those menus. These pickers show blue help and cyan focused/selected
choices, with plain output under `--no-color` or `NO_COLOR=1`. Esc at a forge prompt
discards only that forge draft. Ctrl-C always cancels setup. Secrets stay masked
and are never included in completed-answer summaries.

Discovered values are suggested when available. Advanced repository filters and
default Jira project settings are not prompted for; customize them in the
[YAML configuration](configuration.md). Failed connections
offer only **Retry / Change answers / Skip this integration**. There is no
save-unverified path. Unfinished settings stay
in memory, and nothing is installed until you choose a final installation action.

Runtime selects host port and Stable/Nightly image. YAML uses container bind
`0.0.0.0:8080`, `/app/frontend` and `/data/pullboard-cache.sqlite`; the host mapping
is **loopback only** and allowed browser origins use the selected host port.
**Start in Test mode?** defaults to **No** on new installs, writing
`read_only_test_mode: false` (**Live**). Select Yes for zero-upstream-write simulated
submissions. Existing YAML mode is preserved on edit. Live does not mean autonomous
writes: Jira writes still require explicit reviewed submission; sync and setup never write.
Omitting the YAML property manually retains the backend's safer Test default.

Review the token-free summary, destination changes, verification warnings and
replacements, then choose **Install and start** or **Install only**. Ctrl-C cancels
before installation. Installation writes:

- `~/.config/pullboard/config.yaml` — the single configuration, including literal secrets;
- `~/.config/containers/systemd/pullboard.container` — rootless, hardened Quadlet;
- one previous-version `.bak` beside each replaced file. **Config backups contain secrets too.**

Files/backups are owner-only (0600); newly created private directories are 0700.
Comments need not survive serialization. Symlinks, external edits, unrelated custom
units and unverifiable existing containers are refused rather than silently
overwritten. Setup verifies that the service is generated from its supported Quadlet.
Existing systemd drop-ins, including desktop resource policies and user/admin
overrides, are preserved and continue to apply; their presence does not block setup.
No automatic plaintext fallback is provided for existing keyring references;
the disposable setup has no native keyring service support. Use the separate
[restricted bridge guide](keyring-container.md) for a deliberate manual keyring deployment.
Do not weaken permissions or upload YAML/backups to troubleshoot.

**Install and start** pulls the selected image, reloads the user manager and starts
the generated service (or restarts an existing one after confirmation). **Install
only** still performs `systemctl --user daemon-reload` to register the Quadlet, but
does **not pull an image or start/restart** the service. An already running service
is not stopped; new settings apply on its next start/restart. Never `enable` the generated
`pullboard.service`; the Quadlet's Install section supplies its target relationship.
Without an active graphical target installation is allowed, but graphical autostart
is reported as unverified. Your desktop must manage that target; setup does not
turn SSH login into a graphical session or enable lingering.

**Install and start** waits for the newly requested service invocation to become
active (not an old process still running during restart), then for local HTTP
readiness, and returns. It does **not** poll `/api/sync` or wait for upstream source
refresh completion. Success shows the dashboard URL in bold, the configured
**Test (simulated)** or **Live (explicit submissions only)** mode, and a notice that
initial data loads in the background. With no color, the URL is plain text. Open
the dashboard to monitor source health; HTTP readiness does not prove upstream
refresh succeeded. **Install only** reports saved files and configured mode without
claiming service or HTTP readiness.

Actual warnings remain visible. Cancellation/failure reports distinguish saved
files, manager reload, service job request, running state and HTTP readiness, with
recovery instructions. Partially saved files or an already submitted service job
may remain; inspect the reported state before retrying. A start failure retains
configuration. The browser is never opened automatically.

## Manage, update or recover

```sh
systemctl --user daemon-reload
systemctl --user start pullboard.service
systemctl --user status pullboard.service
journalctl --user -u pullboard.service -n 100
systemctl --user restart pullboard.service
systemctl --user stop pullboard.service
```

Open the completion URL, normally <http://localhost:3333>. Review logs locally for
sensitive content before sharing. Rerun the same launcher to edit settings; do not
start a second wizard over an in-progress installation. If an unrelated custom
unit/drop-in blocks setup, review/migrate it manually rather than deleting it blindly.
Fix missing prerequisites, registry access, occupied ports or user-manager errors,
then reload/start as appropriate. A checksum mismatch runs nothing; retry after a
rolling release finishes updating, never bypass verification.

Every service start/restart checks the selected `latest`/`nightly` image with
`Pull=newer`; a changed image is fetched before startup. A cached image can be used
when the registry is unavailable; the first start needs a successful pull. There is
no periodic updater or automatic restart while running. Graphical-session autostart
and logout behavior are described in the [container guide](quickstart.md#graphical-session-service-podman).

The dashboard's **Switch to Live** confirmation is a temporary backend-wide override
and resets on restart; it never changes YAML. For persistent Live startup, rerun
setup and answer No to Test mode, or set `read_only_test_mode: false` in the protected
YAML and restart the service. Set it to `true` and restart for persistent Test startup.
Switching to Test does not cancel actions already admitted in Live mode.
