# Development, testing and CI

## Toolchain and native run

Use Rust 1.99.1 (edition 2024, manifest minimum 1.99), Node 22.23.1 and npm 10.9.8.
The lockfile fixes actual Angular 22 dependencies. Python 3 is used by browser test
serving and image smoke tests. GNU Make is optional command orchestration.

```sh
make install
make build
make check-config
make dev-backend
```

Run from repository root so the native default static path resolves to
`frontend/dist/frontend/browser`. On hosts without Cargo but with a development
Toolbox, use `CARGO='toolbox run -c tb-devel cargo'` on Make commands. This is a
developer transport for a build tool, not application configuration.

For live reload copy/edit the native YAML, add exact `http://localhost:4200` and/or
`http://127.0.0.1:4200` to `server.allowed_origins`, and run these separately:

```sh
make dev-backend CONFIG=config/local.yaml
make dev
```

Angular binds loopback and proxies `/api` to `127.0.0.1:8080`. The browser's actual
origin is still 4200: neither the native 8080 nor container-host 3333 allowlist
alone permits these POSTs. Coordinate changed dev ports or backend ports with YAML
and proxy config. Never add permissive CORS or disable request-header checks to
bypass this.

## Verification commands

```sh
make test-backend  # cargo test --locked, including mock upstream and contract tests
make test-core    # shared strict YAML/policy and bounded read transport, default features
make test-setup   # wizard/discovery/installer tests, no real account or user service
make test-container  # helper mappings and Quadlet policy; no services/credentials
make test-scripts  # all offline launcher/packaging/container/delivery tests
make check        # rustfmt/clippy warnings denied, TS typecheck and Prettier check
make test-frontend
make e2e-install  # npx playwright install --with-deps chromium (may need sudo)
make e2e          # npm pretest:e2e builds first, then Playwright including axe checks
make e2e-server   # real Rust + production Angular browser smoke, no account credentials
make docs
make
make smoke
```

`npm ci` must precede frontend commands; the image and CI do it automatically.
Do not replace the lockfile install with an unconstrained npm install in delivery.
Native Linux builds need a C toolchain, CMake/pkg-config and platform development
dependencies for Rust dependencies. The Alpine build includes build-base, cmake,
pkgconf and sqlite-dev. TLS uses Rust cryptography; do not add OpenSSL to runtime
without evidence that it is needed.

### Setup executable and shared core

`backend/`, `setup/` and `crates/pullboard-core/` have independent Cargo manifests,
lockfiles and target directories, not a root workspace. Shared core defaults have
no server, SQLite or native keyring dependencies; setup must not enable backend-only
features or forge SDKs. Normal refresh behavior remains in the backend.

For a Linux x86-64 **static** setup build, install Rust 1.99.1's
`x86_64-unknown-linux-musl` target plus a C toolchain, musl headers/compiler,
CMake, pkg-config and binutils. On Debian/Ubuntu, CI installs `build-essential
musl-tools cmake ninja-build clang pkg-config binutils` (including prerequisites
for rustls' aws-lc C build). Set `CC_x86_64_unknown_linux_musl=musl-gcc` and
`CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc` if not auto-detected.

```sh
make setup
make check-setup-dependencies
cargo +1.99.1 test --manifest-path setup/Cargo.toml --locked --release --target x86_64-unknown-linux-musl
setup/target/x86_64-unknown-linux-musl/release/pullboard-setup --help
make package-setup
```

Use `CARGO='cargo +1.99.1'` to pin Make's Cargo command explicitly. `make setup`
builds only; it does not install files or start services. `make package-setup`
packages the **already tested** `SETUP_BINARY` without rebuilding, writing assets
to `target/setup-release` by default (`SETUP_DIST` overrides that directory).
It uses deterministic gzip headers, records real raw/compressed byte counts and
creates checksums; synthetic packaging tests are not actual release-size measurements.
The launcher tests replace downloads through command mocks, use temporary homes/
directories and PTYs, and never fetch releases or use real credentials/services.

Setup uses inline `inquire` prompts, not a full-screen terminal renderer. Its Rust
tests drive the linear flow through scripted answers and mock discovery. For real
arrow-key/search/multiselect, masked-token and cancellation checks, build the setup
tests with `cargo test --manifest-path setup/Cargo.toml --no-run`, then run
`python3 setup/tests/inline_smoke.py <wizard-test-executable>` using the emitted
`wizard-…` executable in `setup/target/debug/deps/`. This opts into an otherwise
ignored PTY fixture; it uses mock prerequisites/discovery and a private temporary
HOME (preferring `/tmp/opencode` when available). It never installs or contacts providers.

The disposable installer has no persistent updater. See the [setup guide](setup.md)
for CLI, token permissions, final confirmation, partial-state recovery and graphical
session limits. No test result should be represented as a successful real desktop
installation or proof of live Jira permissions.

The browser suite uses a Python-served **production build** at loopback port 4209
and mocks the API. It covers accessible controls and flows, not a live Rust server
or upstream service. It includes successful green sources with neutral coverage,
per-source progress/failure/unknown totals and collapsed healthy idle counters,
multi-tab mode changes, stale dialogs and drag gestures, preserved edits, and compact
Upstream Pull Requests summaries/full details/keyboard access in light/dark
themes with axe checks. The image smoke script separately exercises real Rust health,
static routing/assets and CSP-loader compatibility, JSON API 404, request/origin rejection, refresh and an
empty genuine board. It creates a uniquely named temporary container on a dynamic
loopback port and stops/removes **only that container**. It never needs live tokens.

`make e2e-server` additionally builds the native debug binary and production Angular,
starts a real loopback-only Rust process using a temporary in-memory-cache YAML,
and checks browser boot, CSP/assets, light/dark axe, refresh, origin rejection and
JSON API 404s, confirmed shared mode changes and stale-token rejection across tabs.
It does not mock the local API or call any live provider. Only its
own process and temporary file are cleaned up. Chromium must already be installed.

The sanitized-example parser test reads only tracked `config/{example,pullboard,container}.yaml`
without resolving placeholder credentials. After changing examples, run it directly:

```sh
toolbox run --container tb-devel cargo test --manifest-path backend/Cargo.toml --locked --offline --test config all_sanitized_examples_validate_without_resolving_placeholder_credentials
```

Use native `cargo` instead of Toolbox when available. Parser success proves strict
YAML compatibility, not credential validity, discovery readiness or live writes.
Backend HTTP tests use localhost mock upstreams for account quarantine, metadata,
non-hardcoded typed fields, independent foreign sprints, complete group union and
all-user pagination, refresh-time individual accountId assignability and fail-closed bounds.
They verify create/follow-up bodies and complete zero-write request logs/cache
invariance for simulations. Frontend mocks cover null assignee/sprint/status initial
values, purpose/type defaults, manual/null preservation, async races, capabilities
and persistent linked warnings. None proves live tenant API
permissions, Team UUID discovery or successful sprint assignment. Do not add live
tenant requests/writes to validate a documentation/example migration.

Backend refresh/runtime tests also cover deduplicated evaluated counts versus retained
cards, unknown totals, per-source phase/reset/backoff timing, publication failure and
old-snapshot retention, revision changes on toggle/restart and atomic action admission.
Admitted-mode tests must allow live follow-ups after switching Test while proving
admitted simulations stay zero-write after switching Live. Stale requests must fail
before upstream calls, without automatic replay; preserve the accepted-create/unknown-key
success safeguard and disconnected-outcome duplicate protection.

Contract 1.7 preserves refresh/choice-cache fixtures covering independent atomic source and options
publication, early real first-project/sprint context, initial readiness versus genuinely
empty pools, each domain's complete last-good retention, failed-commit isolation,
principal/service/config quarantine in both domains and policy changes only at successful
options publication. Gated fixtures check overlapping cards/metadata/groups, membership-only
eligibility gating, one in-flight eligibility item while cards are unfinished, and
watch-driven expansion after every card attempt terminates, including queued failures
and backoff. They cover caps 1/2/default 4, options-before-cards publication, completed
project rows awaiting metadata, shared sprint failures, indeterminate full group
pagination, deduplication, per-project positive/negative counts, independent persisted
options backoff and owned cancellation. Options, both
draft flows and both simulated create flows assert zero repeated choice-discovery
requests. Status tests reject
unknown/ambiguous names before create and check destination-ID no-op/direct-transition
behavior, warnings on failure and no duplicate creation. Backend context fixtures
check YAML project order; frontend tests cover the discovered current-project header,
matching-current-sprint links/end dates, unique/zero/
multiple Overview defaults/reset, broad Attention defaults, selection preservation,
legacy preference migration, local virtualized assignee search/keyboard/no-results,
stale choices and lazy editor load failures/late opens. Publication-aware polling tests
cover mid-job and options-only revisions, missed phases, dependent-read failure/retry,
late-response ordering and preserved filters/editor edits/captured mode revisions.
The mocked `frontend/e2e/ticket-first-refresh.spec.ts` exercises independent creation
readiness and accessible concurrent group/project progress in light/dark themes,
preserving the compact Source health disclosure; it is not live tenant or performance
verification.

Ordered-sprint fixtures cover required 1–100 unique board/pattern entries, singular-key
rejection, complete multi-board pagination and deterministic first-board provenance,
first-board/first-pattern current matches without fallback, and any-pattern creation
choices independent of team. Retained options keep their own full sprint snapshot and
pattern policy rather than borrowing live YAML or newer display sprints. Focused
mock checks (not live tenant verification):

```sh
toolbox run -c tb-devel cargo test --manifest-path backend/Cargo.toml --locked --offline --test ordered_sprints
toolbox run -c tb-devel cargo test --manifest-path crates/pullboard-core/Cargo.toml --locked --offline --test ordered_sprints
cd frontend && npx playwright test e2e/ordered-sprints.spec.ts --reporter=dot
```

The offline launcher tests also verify repeated board/pattern flags are forwarded in
order without splitting regex commas or evaluating shell content; other flags remain
single-use. Run `make test-scripts` after changing launcher behavior or help.

Focused cache verification uses localhost mocks, no credentials:

```sh
toolbox run -c tb-devel cargo test --manifest-path backend/Cargo.toml --locked --offline --test jira_choice_cache --test ticket_first
```

For focused refresh/config verification using localhost mocks, run:

```sh
toolbox run -c tb-devel cargo test --manifest-path backend/Cargo.toml --locked --test config
toolbox run -c tb-devel cargo test --manifest-path backend/Cargo.toml --locked --test sync --test refresh_progress --test transport_budget --test adapter_hydration_concurrency --test adapters
```

The concurrency fixtures exercise bounded overlapping reads, source failure
isolation/atomic publication, shared-origin admission and rate-limit handling,
and GitHub batches/connection pagination. They are correctness checks, not live
performance measurements. Keep cursor-dependent pages sequential and preserve
last-good retention, deterministic output and fully evaluated progress counts
when changing scheduling; never replace these with elapsed-time speed assertions.
The existing YAML per-origin HTTP cap defaults to 4 and remains shared through nested
search, hydration, discovery and eligibility work; item concurrency also defaults
to 4. Tests must not justify new settings, raising caps, hybrid eligibility or new bulk
APIs. Metadata/groups may compete, fairness/backoff still apply, and HTTP cap 1
serializes reads; this is not preemption or a fixed-latency guarantee. No measured
speedup is claimed for this scheduling change. For a separately authorized
live read measurement, distinguish source reads, each source commit, time to first
cards, options-ready time and total job duration, and origin-cumulative request
counters from per-job counts; see [safe logs](troubleshooting.md#refresh-timing-and-safe-logs).
No provider writes are needed to measure refresh, and no secrets or raw traces
belong in measurement reports.

### Measured local refresh example

This historical comparison predates contract 1.4's complete refresh-time choice
discovery, v1.5 independent publications, v1.6 ticket-favoring concurrency and v1.7
ordered boards/patterns. It is **not a measurement of the current implementation
or evidence of a speedup from those changes**.
An authorized live comparison on 2026-11-09 used the then-default limits with one Jira
source and one GitHub source: 60 unique Jira candidates and 14 PRs, producing
49 Jira cards, 14 PR cards and 7 sprints.

| Refresh | Previous serial implementation | Bounded concurrent implementation |
|---|---:|---:|
| Startup with an empty ephemeral cache | 87.3 s | 15.3 s |
| Repeat refresh in the same process | 83.7 s | 15.6 s |

On the final repeat run, the GitHub read stage took 9.5 s; its previous source
progress duration was 46.2 s, including final publication. The new refresh made
13 GitHub HTTP requests, including identity, discovery, batches and required
pagination/history. The Jira source made 143 requests. Both sources were healthy;
PR and sprint data matched the baseline, and the only Jira difference was a real
upstream update to one issue's `updated_at` timestamp. No upstream writes were
performed. API schemas and the hardened container smoke checks passed.

These are individual workload/network observations, not a benchmark guarantee or
a suggested rate-limit allowance. In earlier runs of the same optimization, total
refresh took 13–14 s; provider latency also varies between runs. Use source
read-stage logs to separate work from publication waiting in that historical design;
current sources publish independently and options have separate timing.

For offline contract/schema/example validation, with Python `jsonschema` and
`openapi-spec-validator` installed in an isolated environment:

```sh
python api/validate_contract.py
```

This validates the shared wire contract and illustrative fixtures, not live upstream
coverage or the complete running application. Mode tokens are mandatory in create/link
fixtures as well as mode-change requests; do not work around conflicts by retrying.

## Documentation

Install mdBook (tested with 0.4.52), then `make docs` or `make docs-serve` (loopback
preview, default port 3000). Override its executable with `MDBOOK=/path/to/mdbook`.
If absent on the host but installed in the development Toolbox, run
`make docs MDBOOK='toolbox run -c tb-devel mdbook'` from the repository root.
Generated output is `docs/book/` and ignored. The config chapter includes the actual
sanitized example through the built-in `links` preprocessor. Missing chapters fail
the build. `docs/book.toml` is documentation tool metadata, not app configuration.
The HTML build is not a comprehensive link checker: also check local Markdown paths,
include targets and rendered internal links/fragments. External URLs and live provider
behavior require separate validation; do not contact live accounts for a docs gate.

## GitHub Actions

PR CI runs independent backend Rust formatting/clippy/tests; shared-core checks and
setup formatting/clippy, musl release build/tests, executable help/static-linkage
checks and offline delivery tests; locked npm typecheck,
formatting, unit tests, production build and Chromium/axe browser tests; mdBook
build; and a three-stage Docker build with real image smoke. No app credentials are
used, including in forks. Workflow default permissions are read-only.

Image/setup publication share one workflow, reusing the same setup build as PRs.
Package-write permission is scoped to the image publishing job; content-write
permission is scoped separately to the setup-asset publication job. Pushes to
`master` build and publish `:nightly`; published **non-prerelease** releases whose
tags start with `v` publish their version tag and `:latest`.
Nightly builds never update `:latest`. Superseded master runs are canceled, and
publication checks the current master commit so rerunning an older workflow
cannot replace nightly with an outdated image.

Only after the corresponding image publication succeeds and the setup checks pass,
the exact tested executable is attached compressed, with launcher, checksums and
measured size report, to that versioned release or the rolling `nightly` prerelease.
Nightly publication rechecks master, moves the `nightly` source tag to the built
commit, and marks the release prerelease/not latest. `GITHUB_TOKEN` release creation
does not recursively trigger a new workflow; the image job independently excludes
the non-`v` nightly tag and all prereleases. Asset uploads/registry tags are not one
atomic transaction: during rolling updates checksum mismatches fail closed and users
can retry. A failed build/publication can leave the previous setup assets in place;
job success is not evidence of a newly published asset unless upload completed.

Merely pushing a tag does not publish. The registry path lowercases
owner/repository. PRs and pushes to other branches never publish, and no workflow
dispatch bypass exists. Every candidate passes image smoke before registry
login/publication; the exact tested image is tagged and pushed, not rebuilt after
verification. `GITHUB_TOKEN` authenticates CI registry operations; it is not an
application secret mechanism and requires no `.env`.
Publish corresponding versioned source with binary images to meet GPL obligations.

Automated success does not imply universal upstream API support, live credential
verification, native OS keyring verification or a functional host Secret Service
bridge on every deployment. Report exact commands/results and these limits.
