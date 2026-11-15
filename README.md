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

Requires **Linux x86-64**, rootless Podman, Quadlet and a working systemd user
session. Run either command to start the configuration wizard and install
PullBoard's container as a systemd user service:

**Latest release:**

```sh
curl --fail --location --proto '=https' --proto-redir '=https' \
  https://github.com/pbrezina/pullboard/releases/latest/download/pullboard-setup.sh | sh
```

**Nightly (unreleased development builds):**

```sh
curl --fail --location --proto '=https' --proto-redir '=https' \
  https://github.com/pbrezina/pullboard/releases/download/nightly/pullboard-setup.sh | sh -s -- --nightly
```

Choose **Install and start**, then open <http://localhost:3333> (local access
only). See the [setup guide](docs/src/setup.md) for prerequisites, token
permissions and launcher security details.

### Manage the container manually

The setup wizard installs the `pullboard` container as the `pullboard.service`
systemd user service. Run these commands as your ordinary user (no `sudo`):

```sh
systemctl --user start pullboard.service
systemctl --user stop pullboard.service
systemctl --user status pullboard.service
```

Use `systemctl --user restart pullboard.service` to restart it and
`journalctl --user -u pullboard.service -n 100` to inspect recent logs. Manage the
container through systemd rather than `podman start`/`stop` so service state stays
in sync.

### Manual YAML and container option

See the [manual setup guide](docs/src/quickstart.md#manual-yaml-and-container)
for a minimal YAML configuration and the command to run the container yourself.

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
