# Quickstart and container operation

## Recommended: setup wizard

For Linux x86-64 with rootless Podman, Quadlet and user systemd already installed:

```sh
curl --fail --location --proto '=https' --proto-redir '=https' \
  https://github.com/pbrezina/pullboard/releases/latest/download/pullboard-setup.sh | sh
```

The wizard guides you one prompt at a time, with token instructions included;
Enter accepts defaults and arrow keys select options. The [setup guide](setup.md)
is an optional reference. The disposable launcher downloads a checksum-verified static executable,
restores terminal stdin and removes it on exit. You are trusting the GitHub
publisher/HTTPS, not an independently authenticated checksum. Inspect code before
executing if preferred. No installer is copied to `~/.local/bin`.

Setup writes owner-only YAML and a rootless graphical-session Quadlet after final
confirmation. **Start in Test mode?** defaults to **No** for new setup (Live);
choose Yes for simulated submissions. Editing preserves the existing mode. Install
only still runs `systemctl --user daemon-reload`, but does not pull or start an
image. Default access is <http://localhost:3333>. No upstream writes occur during setup.

The manual and source-build paths below remain available without the wizard.

## Empty, safe start

From the repository root, install GNU Make and either rootless Podman (preferred)
or Docker. Source builds require network access to image and package registries.

```sh
make
make run
```

Visit <http://localhost:3333>. No upstream credentials are needed. The baked
`config/container.yaml` has `jira: null`, `forges: []`, AI disabled and
`read_only_test_mode: true`. The cache disappears on exit. Ctrl+C stops this
foreground container. The initial compile can take several minutes.

For Docker, add `CONTAINER_ENGINE=docker` to both commands. `make` builds
`localhost/pullboard:latest`; use `IMAGE=localhost/pullboard:my-test` consistently
to select another image. No prepublished image is required.

Equivalent hardened Podman invocation:

```sh
podman run --rm --read-only --cap-drop=ALL --security-opt=no-new-privileges \
  --userns=keep-id --user "$(id -u):$(id -g)" \
  --tmpfs /data:rw,nosuid,nodev,noexec,size=64m,mode=0700,U \
  -p 127.0.0.1:3333:8080 localhost/pullboard:latest
```

Podman's `U` sets ownership of this **tmpfs only** to the selected container user;
it does not recursively change a host directory. For Docker omit
`--userns=keep-id`, retain `--user UID:GID`, and replace that tmpfs option with
`--tmpfs "/data:rw,nosuid,nodev,noexec,size=64m,uid=$(id -u),gid=$(id -g),mode=0700"`.
The Make helper selects the engine-specific syntax. Linux Docker user-namespace-remapping installations may
need an administrator-reviewed ownership mapping; do not solve permission errors
by making a credential file world-readable. Docker Desktop and remote engines
have different bind-mount/UID semantics; these examples target a local Linux engine.

## Published images

Images are available at `ghcr.io/pbrezina/pullboard`. `:latest` and version tags
come from published releases; `:nightly` tracks smoke-tested pushes to `master`
and may contain unreleased changes. To use a published image, replace
`localhost/pullboard:latest` in the invocation above with
`ghcr.io/pbrezina/pullboard:latest` or `ghcr.io/pbrezina/pullboard:nightly`, and add
`--pull=always` to fetch updates.

## Manual YAML and container

Create `config/local.yaml` in your editor with the configuration below. Replace
the Jira URL, email, project, board, team, sprint pattern and tokens with your
own values. Keep test mode on while setting things up: Jira writes are simulated,
not sent. Use literal tokens or a supported OS keyring reference, never environment
variables, and do not commit secrets. `config/local.yaml` is Git-ignored.

```sh
umask 077
mkdir -p config
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
```

Save with owner-only permissions, then run the published image with rootless
Podman on Linux. Replace `:latest` with `:nightly` for development builds; see the
[GHCR package](https://github.com/pbrezina/pullboard/pkgs/container/pullboard)
for available versions.

```sh
chmod 600 config/local.yaml
podman run --pull=always --rm --read-only --cap-drop=ALL --security-opt=no-new-privileges \
  --userns=keep-id --user "$(id -u):$(id -g)" \
  --tmpfs /data:rw,nosuid,nodev,noexec,size=64m,mode=0700,U \
  -p 127.0.0.1:3333:8080 \
  -v "$PWD/config/local.yaml:/app/config.yaml:ro,Z" \
  ghcr.io/pbrezina/pullboard:latest
```

Open <http://localhost:3333>. Access is loopback-only and the cache is temporary;
Ctrl+C stops the container. See the [configuration guide](configuration.md) for
all settings and credentials. Manual container keyring use requires the
[restricted Secret Service bridge](keyring-container.md).

## Real accounts

```sh
umask 077
cp config/example.yaml config/local.yaml
chmod 600 config/local.yaml
```

Replace placeholders and delete unused account entries. `config/local*.yaml` is
Git-ignored and excluded from the allowlisted image context. For container use,
edit the existing `server` section to retain `port: 8080`, but set
`bind: 0.0.0.0`, `static_directory: /app/frontend`, and
`database: /data/pullboard-cache.sqlite`. Set `allowed_origins` to
`[http://localhost:3333, http://127.0.0.1:3333]`: browser origins use the **host**
port, not the internal container port. Native paths from the comprehensive example
do not work inside the image.

```sh
make run CONFIG=config/local.yaml
```

The helper mounts only that file at `/app/config.yaml:ro,Z`. `:Z` privately labels
this file on SELinux systems; it does not relabel your home directory. Running as
your mapped user is important: the image's default UID 10001 normally **cannot**
read your host-owned mode-600 config. Do not mount your entire config directory.

To check configuration without starting the server, append CLI arguments to a
container command (after its image): `--config /app/config.yaml --check-config`.
Include the same UID and config/bridge mounts as the normal run. This resolves
secrets but makes no upstream connectivity assertion.

For a manual learning setup, keep YAML `read_only_test_mode: true` as the safe
startup value (unlike the wizard's explicit new-install Live default). The persistent mode
strip offers **Switch to Live**, with confirmation, and **Switch to Test**. This
backend-wide override affects all tabs, never edits YAML and resets on restart.
Switching to Test does not cancel already admitted live actions. Read
[runtime mode and stale-action review](tickets-ai.md#runtime-testlive-mode) before
enabling live writes; a simulated success does not test write permissions.

## Graphical-session service (Podman)

The repository provides `deploy/systemd/pullboard.container`, a rootless user
Quadlet intended for `~/.config/containers/systemd/pullboard.container`. It expects
an existing owner-only (mode 600) `~/.config/pullboard/config.yaml`, mounted
read-only at `/app/config.yaml`. Use the container server settings above; a missing
file fails startup. Prepare credentials as literal YAML values: this unit does
**not** expose a keyring bridge or the desktop session bus.

The unit is attached to `graphical-session.target`, not the ordinary user login
target: it starts with the graphical session, not an SSH/terminal-only login, and
stops when that graphical session ends. Your desktop must manage this systemd
target. It does not launch a browser; open <http://localhost:3333> yourself. On
failure it restarts after ten seconds; explicitly stopping it does not trigger
restart. It retains the foreground helper's hardening and temporary 64 MiB cache.

Each service start, including restarts, checks
`ghcr.io/pbrezina/pullboard:latest` using `Pull=newer`. A changed image is downloaded
before the container starts. If the registry is unavailable, Podman can use its
cached image; the first start needs a successful pull if no image is cached.
Startup allows up to fifteen minutes for pulling. No update timer is needed, and
there are no periodic update checks or update-driven restarts while it runs.

## Ports, storage and updates

Container access defaults to host port **3333**, mapped to internal port **8080**;
native server defaults and the Angular development proxy still use 8080.
Existing account YAML files are not rewritten: update their `allowed_origins` to
port 3333, or keep the old host mapping with `make run PORT=8080`.
`make run PORT=8090` maps host loopback 8090 to container 8080. Before using browser
POSTs, add `http://localhost:8090` and `http://127.0.0.1:8090` to the selected YAML
`server.allowed_origins`. They must be exact, without trailing slashes. If changing
the **container** port in YAML too, use a direct invocation with a matching right
side of `-p`; the Make helper intentionally targets container 8080. Ports must be
1024–65535. Never publish `0.0.0.0:PORT`, use host networking, or expose this
unauthenticated service through a shared reverse proxy.

`/data` is a 64 MiB tmpfs in the helper, not persistent storage. Size it to your
account data if necessary. You may explicitly mount a private writable cache
directory at `/data` with correct ownership and SELinux labels, but it remains
disposable: no migrations, backup or durable application state is required.
Never persist credentials there. Stop the old foreground run, rebuild with `make`,
and run again to update. Nothing prunes unrelated images or containers.
