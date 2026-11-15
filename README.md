# PullBoard

**Your work from all forges at one place.**

PullBoard brings your pull requests from all forges and Jira work into one local
dashboard. See which discussions or pull requests need your attention, review
your assigned Jira issues, link pull requests to JIRA tickets and turn a
selected pull request or upstream issue into an editable Jira ticket. Forge data
is read-only; Jira changes happen only when you review and submit them.

PullBoard is one container: a Rust backend serves the API and the Angular app from
its built-in HTTP server. No separate frontend service is needed.

The header shows your first configured Jira project and matching active sprints.
Overview (Upstream Pull Requests) selects a sprint automatically only when exactly
one matches; attention starts broad, including unscheduled work. New tickets start **Unassigned**,
**No sprint** and **Jira default — no transition**. See the
[dashboard](docs/src/attention.md) and [ticket guide](docs/src/tickets-ai.md).

## Supported forges

- GitHub
- GitLab, including self-hosted instances
- Codeberg
- Forgejo, including self-hosted instances

## Quick start

On **Linux x86-64**, with rootless Podman, Quadlet and a working systemd user
session already installed, run the disposable terminal wizard:

```sh
curl --fail --location --proto '=https' --proto-redir '=https' \
  https://github.com/pbrezina/pullboard/releases/latest/download/pullboard-setup.sh | sh
```

This executes publisher-provided code over HTTPS; inspect the launcher first if
preferred. It verifies the compressed executable's SHA256 before running it, then
removes the download. Checksums detect corruption, not independent authenticity.
The wizard asks one question at a time to configure Jira, forges, optional AI and
a loopback-only service at <http://localhost:3333>. Use arrows to select and Enter
to accept defaults. Enter tokens only in its masked prompts, never CLI/env.
New setup asks **Start in Test mode?** (default **No**, therefore **Live**); choose
Yes to simulate Jira submissions. Setup itself never writes upstream. Existing
mode settings are preserved on edit.

See the [setup guide](docs/src/setup.md) for prerequisites, token permissions,
`--nightly`, [sharing team Jira parameters](docs/src/setup.md#share-team-jira-settings-not-credentials),
the legacy editable `--jira-cloud-url` prefill, protected backups and recovery.
The launcher/binary are not kept in `~/.local/bin`; rerun setup to edit settings.

### Manual YAML and container option

Create `config/local.yaml` in your editor and replace the example Jira URL, email,
project, board, team, sprint pattern, and token values with yours. Keep test mode
on while setting things up: Jira writes are simulated, not sent. Use literal token
values or a supported OS keyring reference; never put secrets in environment
variables or commit them. This file is Git-ignored.

```sh
mkdir -p config
umask 077
```

```yaml
server:
  bind: 0.0.0.0
  port: 8080
  allowed_origins: [http://localhost:3333, http://127.0.0.1:3333]
  static_directory: /app/frontend
  database: /data/pullboard-cache.sqlite

read_only_test_mode: true

jira:
  base_url: https://your-company.atlassian.net
  email: you@example.com
  token: {value: REPLACE_WITH_JIRA_TOKEN}
  projects: [PROJECT_KEY]
  board_id: 1
  team: Your team
  sprint_name_pattern: 'TEAM'

forges:
  - id: github
    kind: github
    base_url: https://api.github.com
    token: {value: REPLACE_WITH_READ_ONLY_GITHUB_TOKEN}
  - id: codeberg
    kind: codeberg
    base_url: https://codeberg.org
    token: {value: REPLACE_WITH_READ_ONLY_CODEBERG_TOKEN}
```

Save the file with owner-only permissions, then run the published image with
Podman on Linux. This pulls `latest` from published releases. To track development
builds from pushes to `master`, replace `:latest` with `:nightly`. See the
[GHCR package](https://github.com/pbrezina/pullboard/pkgs/container/pullboard)
for available versions.

```sh
umask 077
chmod 600 config/local.yaml
podman run --pull=always --rm --read-only --cap-drop=ALL --security-opt=no-new-privileges \
  --userns=keep-id --user "$(id -u):$(id -g)" \
  --tmpfs /data:rw,nosuid,nodev,noexec,size=64m,mode=0700,U \
  -p 127.0.0.1:3333:8080 \
  -v "$PWD/config/local.yaml:/app/config.yaml:ro,Z" \
  ghcr.io/pbrezina/pullboard:latest
```

Open <http://localhost:3333>. The app is published on loopback only, and its
cache is temporary. Read the [configuration guide](docs/src/configuration.md)
before connecting accounts; it covers all settings, credentials, and container
paths. See the [quickstart and container guide](docs/src/quickstart.md) for more
run options. A rootless Podman [Quadlet](deploy/systemd/pullboard.container) is also
provided for [graphical-session startup](docs/src/quickstart.md#graphical-session-service-podman).
It checks for a newer `latest` image at each service start and mounts your existing
`~/.config/pullboard/config.yaml` read-only. It has no keyring integration.
Manual container keyring use requires the
[restricted Secret Service bridge](docs/src/keyring-container.md).

## Development

PullBoard is developed with AI assistance, and AI-assisted contributions are
welcome. The repository bundles development skills in [`.agents/skills`](.agents/skills)
and project-specific instructions in [`AGENTS.md`](AGENTS.md). Start there and use
the relevant skill when working on Rust, Angular, design, accessibility, or docs.

Build the image locally, then run it with your configured account details (Podman
is the default; set `CONTAINER_ENGINE=docker` to use Docker):

```sh
make IMAGE=localhost/pullboard:dev
make run IMAGE=localhost/pullboard:dev CONFIG=config/local.yaml
```

See the [development guide](docs/src/development.md) for tests and native
development, and the [user and operator guide](docs/src/README.md) for product
details.

## License

PullBoard is licensed under [GPL-3.0-or-later](LICENSE).
