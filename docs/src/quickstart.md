# Quickstart and container operation

## Empty, safe start

From the repository root, install GNU Make and either rootless Podman (preferred)
or Docker. Source builds require network access to image and package registries.

```sh
make
make run
```

Visit <http://localhost:8080>. No upstream credentials are needed. The baked
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
  -p 127.0.0.1:8080:8080 localhost/pullboard:latest
```

Podman's `U` sets ownership of this **tmpfs only** to the selected container user;
it does not recursively change a host directory. For Docker omit
`--userns=keep-id`, retain `--user UID:GID`, and replace that tmpfs option with
`--tmpfs "/data:rw,nosuid,nodev,noexec,size=64m,uid=$(id -u),gid=$(id -g),mode=0700"`.
The Make helper selects the engine-specific syntax. Linux Docker user-namespace-remapping installations may
need an administrator-reviewed ownership mapping; do not solve permission errors
by making a credential file world-readable. Docker Desktop and remote engines
have different bind-mount/UID semantics; these examples target a local Linux engine.

## Real accounts

```sh
umask 077
cp config/example.yaml config/local.yaml
chmod 600 config/local.yaml
```

Replace placeholders and delete unused account entries. `config/local*.yaml` is
Git-ignored and excluded from the allowlisted image context. For container use,
edit the existing `server` section to retain `port: 8080` and local allowed origins,
but set `bind: 0.0.0.0`, `static_directory: /app/frontend`, and
`database: /data/pullboard-cache.sqlite`. Native paths from the comprehensive
example do not work inside the image.

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

Keep YAML `read_only_test_mode: true` as the safe startup value. The persistent mode
strip offers **Switch to Live**, with confirmation, and **Switch to Test**. This
backend-wide override affects all tabs, never edits YAML and resets on restart.
Switching to Test does not cancel already admitted live actions. Read
[runtime mode and stale-action review](tickets-ai.md#runtime-testlive-mode) before
enabling live writes; a simulated success does not test write permissions.

## Ports, storage and updates

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
