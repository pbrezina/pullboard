# Credentials and native keyrings

Every secret has exactly one YAML representation:

```yaml
token: {value: "literal-token-entered-in-a-private-editor"}
```

or:

```yaml
token:
  keyring:
    service: pullboard
    account: work-github
```

These examples replace the relevant integration's `token` field. Neither is an
environment variable reference. Do not use `env`, `${TOKEN}`, `.env`, API-token
environment flags or command-line secret arguments. A keyring lookup failure is
explicit; the server never silently substitutes plaintext. File-based secrets
remain a supported **deliberate** choice: protect the whole YAML with mode 600,
avoid unsafe backups, and never commit or send it with a bug report.

Keyring `account` is only the secret lookup key. It is not a forge username or
Jira account ID. No upstream identity/display name is configured; authenticated
`/user` (forges) or `/rest/api/3/myself` (Jira) discovers it at runtime. Jira `email`
remains required for Basic authentication, not as a substitute discovered identity.

On native Linux the implementation uses the Secret Service; on native macOS,
Keychain; on native Windows, Credential Manager. Availability depends on an
unlocked service/session and permissions. A Linux container does not magically
inherit a macOS or Windows host keyring.

## Seed Linux Secret Service without shell-history tokens

Use `secret-tool` on your host with a working, unlocked Secret Service. The Rust
`keyring` 3.6 implementation searches attributes **`service`, `username`, `target`**,
not `account`. YAML's `account` maps to the `username` attribute. For the example:

```sh
secret-tool store --label='PullBoard work GitHub' \
  service pullboard username work-github target default
```

Enter the token at the prompt; do not append it to the command or pipe an `echo`
containing it. Avoid duplicate entries with matching attributes. Do not use
`secret-tool lookup` in logs: it prints the secret. `target default` matches
`Entry::new(service, account)`; the crate also has a legacy no-target fallback,
but use explicit attributes for new entries. See the upstream
[keyring Secret Service implementation](https://docs.rs/keyring/3.6.3/keyring/secret_service/index.html).

```sh
cargo run --manifest-path backend/Cargo.toml --locked -- \
  --config config/local.yaml --check-config
```

This checks YAML and resolves configured credentials, not token scope, identity,
upstream connectivity or AI model availability. Credentials are resolved into the
process; changing a stored value requires a server restart. The diagnostic output
does not print them. Use the [restricted bridge](keyring-container.md) only if you
accept the additional container trust boundary.
