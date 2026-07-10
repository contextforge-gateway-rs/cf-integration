# cf-integration

Rust 1.97 integration harness for `cf-controlplane` and the Rust
`cf-dataplane`.

The public routing contract is fixed:

- `/servers/{virtual_host_id}/mcp` routes through `cf-dataplane` as
  `/contextforge-rs/servers/{virtual_host_id}/mcp`.
- Raw `/mcp`, UI traffic, and API traffic stay on `cf-controlplane`.

The stack is the stock upstream `docker-compose.yml` with a guarded Fast Time
contract plus exactly two intentional runtime differences: the nginx routing
split above, and `DATAPLANE_PUBLISHER=true` on the gateway so virtual-server
configs reach `cf-dataplane` through Redis. Startup fails if Fast Time is not
using `ghcr.io/ibm/cfex-mcp-fast-time-server:latest`, legacy Fast Time/Fast Test
images appear in the rendered Compose config, or Fast Test services are active
in the base integration stack.

## Requirements

- Rust 1.97 or newer and Cargo
- Docker Engine with Docker Compose v2
- Git
- Node.js 22.7.5 or newer with `npx` for official MCP conformance and Inspector
  commands
- The control-plane development prerequisites (`uv`, pytest, and Make) for
  upstream live tests

Python remains only where Locust imports its user adapter in-process.
Host-side orchestration, JWT generation, MCP parsing, probing, result handling,
and Goose load generation are Rust.

The checked-in `rust-toolchain.toml` makes rustup select Rust 1.97.0 with
rustfmt and Clippy automatically. Install that toolchain once, then build the
locked workspace:

```bash
rustup toolchain install 1.97.0 --profile minimal -c clippy -c rustfmt
cargo build --locked
.integration/cargo-target/debug/cf-integration --help
```

Cargo output is configured under `.integration/cargo-target`; the repository
does not create a root `target/` directory. The examples below use
`cargo run --locked --`, which can be replaced by the built binary.

## Quick start

Start the full dataplane topology and probe its public route:

```bash
cargo run --locked -- stack up --mode dataplane
cargo run --locked -- test probe --mode dataplane
```

Open <http://localhost:8080/admin> and log in with:

```text
admin@example.com / changeme
```

`stack up` synchronizes the required source checkout, fresh-bootstraps both
Compose projects by default, uses the upstream local-build image for the
control plane, and pulls the published dataplane image. Set
`CF_FRESH_STACK=false` only when retaining database state is intentional. A
matching, already-running dataplane stack is reused unless
`CF_FORCE_FRESH_STACK=true` is set.

Generated checkouts under `.integration/` are reset to the fetched remote ref,
so stale local branches cannot drive a run. Explicit checkout directories
outside the integration-state directory are preserved and use a plain Git
checkout. A fetch failure is a warning when the requested ref is already
available locally.

The Fast Time backend is registered as virtual server
`9779b6698cbd4b4995ee04a4fab38737`, so probes and load tests need no manual UI
setup. It runs in dual-transport mode (`/mcp` streamable HTTP and `/sse`). The
publisher exports streamable-HTTP backends to the dataplane; SSE-backed virtual
servers remain on the control-plane route.

## Modes

`--mode controlplane` targets the stock control-plane topology and raw `/mcp`.
`--mode dataplane` targets the nginx-to-dataplane topology and the virtual
server route. Single-stack commands default to `CF_MCP_STACK_MODE`, then
`dataplane`; the environment value must be `controlplane` or `dataplane`.

Cleanup, suite, and compliance commands also accept `--mode all`, which runs
the two topologies sequentially. `stack down` and `stack reset` default to
`all`; `compliance all` also defaults to `all`.

## Command reference

Clap rejects unknown commands, invalid values, and unexpected positional
arguments. Use `--help` at any level for the authoritative interface.

```bash
# Source checkout management
cargo run --locked -- sync

# Compose lifecycle
cargo run --locked -- stack up --mode dataplane
cargo run --locked -- stack down --mode all
cargo run --locked -- stack reset --mode all
cargo run --locked -- stack status --mode dataplane
cargo run --locked -- stack logs --mode dataplane nginx cf-dataplane
cargo run --locked -- stack config --mode dataplane

# Tokens
cargo run --locked -- token --kind scoped --server-id <virtual-server-id>
cargo run --locked -- token --kind admin

# Functional and upstream live tests
cargo run --locked -- test probe --mode dataplane
cargo run --locked -- test live --mode dataplane --group mcp
cargo run --locked -- test live --mode dataplane --group rbac
cargo run --locked -- test live --mode dataplane --group protocol
cargo run --locked -- test live --mode dataplane --group all
cargo run --locked -- test suite --mode all --start --exclude-plugins
```

The probe checks unauthenticated rejection, initialization, the required
`notifications/initialized` transition, strict session reuse, `tools/list`,
and one known-safe `tools/call`. It targets `/mcp` in controlplane mode and
`/servers/{id}/mcp` in dataplane mode.

The suite runs, per selected mode: probe, a Locust smoke run, MCP live tests,
RBAC live tests, protocol live tests, and the full upstream live suite. Add
full load runs by repeating `--load`:

```bash
cargo run --locked -- test suite --mode dataplane --start \
  --load locust --load goose
```

`--exclude-plugins` affects only the full upstream live lane. Without
`--start`, the suite uses the existing stack. Selected lanes continue after a
failure and the aggregate command returns failure if any lane failed.
Fast Test services remain behind explicit upstream profiles. MCP live lanes
start and register that fixture on demand, then wait for dataplane publication;
the base stack stays free of stale fixture containers and legacy images.

## Locust and Goose

Locust remains available through a minimal framework-required Python adapter.
Goose is a native Rust MCP workload derived from the dataplane harness and
improved for the public integration routes. Both execute the same MCP lifecycle
in controlplane and dataplane modes.

```bash
# One user, one user/second, ten seconds
cargo run --locked -- test load --mode dataplane --engine locust --smoke
cargo run --locked -- test load --mode dataplane --engine goose --smoke

# Explicit full-run settings
cargo run --locked -- test load --mode dataplane --engine goose \
  --users 20 --spawn-rate 5 --run-time 2m
```

Default full-run settings are 100 users, 10 users/second, and five minutes.
CLI options override environment values. Smoke defaults replace built-in and
`.env` values, while explicitly exported `LOCUST_USERS`,
`LOCUST_SPAWN_RATE`, and `LOCUST_RUN_TIME` remain authoritative.
Locust applies `LOCUST_REQUEST_TIMEOUT_SECONDS` (default `60`) to every MCP
POST and session DELETE so an unresponsive endpoint cannot stall a load user.

The harness Locust adapter and Goose workload initialize real MCP sessions, send
`notifications/initialized`, discover tools, call only a finite allowlist of
safe fixture tools, and exercise ping. Goose fails closed on request or
transaction failures and scans its reports for credential leakage. Load
artifacts are generated under `.integration/` (or `CF_INTEGRATION_DIR`).

In dataplane mode, set `MCP_SERVER_ID` or `MCP_VIRTUAL_SERVER_ID` to target a
different virtual server. `MCP_TOOL_NAMES` can restrict the Locust adapter to a
comma-separated set of listed safe tools.

## MCP compliance

The [official MCP Conformance Test
Framework](https://github.com/modelcontextprotocol/conformance) server oracle
is pinned to `@modelcontextprotocol/conformance@0.1.16`. The default stable MCP
revision is `2025-11-25`; `--suite active` excludes upstream pending scenarios,
while the default `--suite all` includes every scenario tagged for that
revision.

```bash
# Official framework only
cargo run --locked -- compliance conformance --mode dataplane --start

# Rust gateway-specific live checks only
cargo run --locked -- compliance gateway --mode dataplane --start

# Both layers against control plane and dataplane, then compare them
cargo run --locked -- compliance all --mode all --start

# Rebuild the comparison report from existing result artifacts
cargo run --locked -- compliance report
```

Use `--server-id` for an existing fixture, `--spec-version` to select an
explicit revision, and `--results-dir` to change the generated artifact root.
The harness starts each selected stack when `--start` is present, uses the
configured ID or the auto-registered Fast Time fixture, generates a
mode-appropriate token, runs each topology independently, and preserves the
command's failure status.

The official-only command passes supported dated revisions through to the
pinned framework. Rust gateway cases are currently defined for
`2025-11-25` and reject other revisions instead of producing mislabeled
evidence. The coverage inventory is likewise pinned to `2025-11-25`.

The official framework has no bearer-header option. The harness therefore
binds a random-path loopback proxy with a fixed upstream endpoint and injects
the Authorization header there. Tokens never appear in the `npx` argument
list. The same proxy boundary is used for Inspector.

Expected failures are independent and mode-specific:

- `conformance/baseline-controlplane.yml`
- `conformance/baseline-dataplane.yml`

Each expected failure must name one exact official scenario and include its
specification reference, concrete implementation gap, tracking issue, and
ownership classification. Wildcards and undocumented expected failures are
rejected. `--baseline` can select an explicit rich baseline for an official
run.

Generated runtime artifacts stay under `.integration/`. Tracked reports are:

- `reports/mcp-conformance-comparison.md`, generated from independent
  control-plane and dataplane results
- `reports/mcp-spec-coverage.md`, a page-by-page inventory of normative MCP
  2025-11-25 requirements from the pinned official specification source

`conformance/coverage-overrides.yml` contains reviewed mappings from the
pinned requirement IDs to official scenarios and Rust gateway cases. Every
catalog row receives a conservative role/capability applicability
classification. Missing evidence remains `Not run`; classification alone is
never counted as passing.

## Inspector

[MCP Inspector](https://github.com/modelcontextprotocol/inspector) is an
interactive debugging aid, not a compliance gate. The harness pins
`@modelcontextprotocol/inspector@0.22.0` and runs its official CLI against the
authenticated loopback proxy:

```bash
cargo run --locked -- inspect --mode dataplane --method tools/list
cargo run --locked -- inspect --mode controlplane --method prompts/list
```

Use `--server-id` to override the configured/default fixture virtual server. Use
`compliance conformance`, not Inspector, for protocol conformance claims.

## Configuration

The CLI reads an optional repository-root `.env`. Start from the tracked sample
with `cp .env.example .env`. Process keys take precedence over `.env`; settings
that require a value reject or replace an empty value according to their
documented contract. Relative configured paths are resolved from the
repository root.

Common settings:

```bash
CF_MCP_STACK_MODE=dataplane
CF_INTEGRATION_DIR=.integration

CF_CONTROLPLANE_REPO=https://github.com/IBM/mcp-context-forge.git
CF_CONTROLPLANE_REF=main
CF_CONTROLPLANE_IMAGE=mcpgateway/mcpgateway:latest
CF_CONTROLPLANE_VERSION=latest

CF_DATAPLANE_REPO=https://github.com/contextforge-gateway-rs/contextforge-gateway-rs.git
CF_DATAPLANE_REF=
CF_DATAPLANE_DIR=.integration/contextforge-gateway-rs
CF_DATAPLANE_LOCAL_IMAGE=contextforge-gateway-rs/contextforge-gateway-rs:local
CF_DATAPLANE_IMAGE=ghcr.io/contextforge-gateway-rs/contextforge-gateway-rs:0.1.0
CF_DATAPLANE_VERSION=0.1.0
CF_DATAPLANE_PLATFORM=auto

CF_COMPOSE_BUILD=auto
CF_FRESH_STACK=true
CF_FAST_TIME_EXPECTED_IMAGE=ghcr.io/ibm/cfex-mcp-fast-time-server:latest
CF_FAST_TIME_SERVER_ID=9779b6698cbd4b4995ee04a4fab38737
CF_DATAPLANE_PUBLISHER_INTERVAL_SECONDS=2
CF_DATAPLANE_USER_CONFIG_CACHE_EXPIRY_SECONDS=0

MCP_CLI_BASE_URL=http://127.0.0.1:8080
MCP_SPEC_VERSION=2025-11-25
MCP_PROTOCOL_VERSION=2025-11-25
NGINX_PORT=8080
```

`CF_COMPOSE_BUILD=auto` rebuilds a missing or revision-stale local image.
`true` always builds and `false` reuses the selected image. Configured
`CF_CONTROLPLANE_IMAGE`/`IMAGE_LOCAL` overrides from either the process or
`.env` disable the automatic local control-plane build unless building is
forced.

Published dataplane images are the default. To build an explicit dataplane
source ref locally:

```bash
CF_DATAPLANE_REF=user/luca/cp-parity-tool-names \
  cargo run --locked -- stack up --mode dataplane
```

`CF_DATAPLANE_PLATFORM=auto` uses the Docker server platform for a local source
build and the published-image platform for a registry image. Set an explicit
platform when required.

The publisher interval and dataplane cache defaults above favor deterministic
functional tests. Restore production-like values (normally 60 and 60) for
load benchmarking. Upstream gateway CPU, memory, replica, and worker settings
can also be overridden through `.env` for the local Docker engine.

Token and endpoint overrides:

```bash
JWT_SECRET_KEY=<integration-secret>
MCP_JWT_SUBJECT=admin@example.com
MCPGATEWAY_BEARER_TOKEN=<pre-minted-token>
MCP_SERVER_ID=<virtual-server-id>
MCP_TOOL_NAMES=<comma-separated-safe-tool-names>
CF_PROBE_CONFIG_TIMEOUT=120
CF_PROBE_REQUEST_TIMEOUT=30
```

Never commit `.env` or generated tokens.

## Repository layout

```text
Cargo.toml, Cargo.lock                    Rust 1.97 package and root workspace
.cargo/config.toml                       Cargo output under .integration/
src/                                     Clap CLI and testable Rust workflows
conformance/                             rich baselines and coverage mappings
docker/docker-compose.cf-dataplane.yaml  dataplane service and nginx override
docker/docker-compose.cf-integration.yaml fixture and Locust overlay
docker/nginx.cf-dataplane.conf            public routing split
scripts/locustfile_mcp.py                 framework-required Locust adapter
reports/                                  tracked compliance and curated reports
.integration/                             ignored checkout/build/runtime state
```
