# Restricted Linux container bridge (opt-in)

This is an **optional Linux-host integration**, not the default run mode. Native
and bridged keyring behavior has not been verified against live user credentials
in this delivery test. Test `--check-config` yourself before trusting the setup.
macOS/Windows keyrings are not reachable through these Linux instructions.

## Trust boundary

The host must already have an unlocked Secret Service and `xdg-dbus-proxy` and
`secret-tool` installed. Do not start or reconfigure your personal keyring from an
untrusted container. Seed the entry on the host as in [credentials](credentials.md).

The proxy restricts **the D-Bus service**, not individual entries. Access to
`org.freedesktop.secrets` can grant access to **other secrets in unlocked
collections**, and potentially write/delete operations, depending on the service's
policy. The bridge is not a per-token vault and should be enabled only for a trusted
image. For stronger isolation use a separately managed collection/service/session
or deliberately choose a mode-600 literal YAML instead. No automatic fallback.

Never mount the full session bus, runtime directory, home directory or host
keyring files. Never use `--privileged` to make keyring access work.

## Start the restricted host proxy

In a dedicated **host** terminal:

```sh
sh backend/scripts/secret-service-bridge.sh
```

Keep it running. It creates the dedicated mode-700 directory
`$XDG_RUNTIME_DIR/pullboard-secret-service` and exposes only its `bus` socket using
`xdg-dbus-proxy --filter --talk=org.freedesktop.secrets`. It refuses to overwrite an
existing socket. Stop the proxy with Ctrl+C when finished; do not remove a socket
owned by a still-running process.

## Mount only the dedicated socket directory

Ensure `config/local.yaml` uses container paths and a matching keyring reference:

```sh
podman run --rm --read-only --cap-drop=ALL --security-opt=no-new-privileges \
  --userns=keep-id --user "$(id -u):$(id -g)" \
  --tmpfs /data:rw,nosuid,nodev,noexec,size=64m,mode=0700,U \
  -p 127.0.0.1:8080:8080 \
  -v "$PWD/config/local.yaml:/app/config.yaml:ro,Z" \
  -v "$XDG_RUNTIME_DIR/pullboard-secret-service:/run/pullboard-secret-service:ro,z" \
  -e DBUS_SESSION_BUS_ADDRESS=unix:path=/run/pullboard-secret-service/bus \
  localhost/pullboard:latest --config /app/config.yaml --check-config
```

After a successful check, omit `--check-config` to run. The environment variable
above selects an **OS IPC transport**, not a credential source or application
setting. There are no app token variables. Read-only mounting a socket directory
does not make the Secret Service protocol read-only.

For Docker, replace `podman` with `docker`, remove `--userns=keep-id`, and replace
the tmpfs option with
`--tmpfs "/data:rw,nosuid,nodev,noexec,size=64m,uid=$(id -u),gid=$(id -g),mode=0700"`.
Keep the numeric host user. D-Bus peer-UID checks and user namespace mappings must match
your host; rootless Podman's `keep-id` is intentional. Docker namespace remapping,
remote engines or Desktop VMs may prevent this bridge entirely.

On SELinux hosts, label **only this dedicated proxy directory** (`:z` above allows
the host proxy and container to use it). Never relabel all of `$XDG_RUNTIME_DIR`.
If local SELinux/D-Bus policy still denies access, inspect the denial first. As a
last resort for this **explicit keyring-enabled invocation only**, adding
`--security-opt=label=disable` weakens SELinux separation; understand the risk.
It is intentionally absent from normal run helpers. Do not disable SELinux or
expose the real session bus globally.

Lookup errors mean stop and fix the bridge, unlock state, attributes or UID
mapping—or explicitly edit the YAML to use a literal secret. Do not copy secret
values into container environment variables while troubleshooting.
