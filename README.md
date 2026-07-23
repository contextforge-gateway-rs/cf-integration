# cf-integration

Rust 1.97 integration harness for `cf-controlplane` and the Rust
`cf-dataplane`.

The public routing contract is fixed:

- `/servers/{virtual_host_id}/mcp` routes through `cf-dataplane` as
  `/contextforge-rs/servers/{virtual_host_id}/mcp`.
- Raw `/mcp`, UI traffic, and API traffic stay on `cf-controlplane`.

The harness owns Docker Compose overlays, nginx routing, reproducible stack
lifecycle, public-route probes, Locust and Goose load tests, and official MCP
conformance orchestration. Generated checkout, build, and runtime state stays
under `.integration/` or `CF_INTEGRATION_DIR`.

## Requirements

- Rust 1.97 or newer and Cargo
- Docker Engine with Docker Compose v2
- Git
- Node.js 22.7.5 or newer with `npx`
- Python and Locust dependencies from the control-plane checkout when using
  the Locust load engine
- The control-plane development prerequisites (`uv`, pytest, Make, and
  Playwright where required) when running upstream live tests

The checked-in `rust-toolchain.toml` selects Rust 1.97.0 with rustfmt and
Clippy. Install the locked CLI from this checkout with:

```bash
rustup toolchain install 1.97.0 --profile minimal -c clippy -c rustfmt
cargo install --path . --locked
cf-integration --help
```

Cargo places the executable in `$CARGO_HOME/bin` (normally `~/.cargo/bin`).
Re-run the install command after updating the checkout.

## Workspace

The workspace has one application package and four internal libraries:

- `cf-integration`: CLI and workflow composition
- `cf-integration-platform`: configuration, processes, checkouts, Compose, and
  stack lifecycle
- `cf-integration-mcp`: MCP messages, HTTP transport, authentication proxy,
  gateway endpoints, and probes
- `cf-integration-compliance`: the official conformance fixture, result parser,
  and three-lane comparison report
- `cf-integration-load`: Locust and Goose load engines

The official TypeScript fixture is exclusively for conformance. Fast Time
remains the ordinary probe and load fixture; upstream live MCP tests also start
and register the profile-gated Fast Test server on demand.

## Lanes and protocol versions

Probe, load, live, and Inspector use the same target options:
`--lane controlplane|dataplane` and `--protocol-version YYYY-MM-DD`.
`controlplane` targets the stock control-plane topology and raw `/mcp`;
`dataplane` targets nginx, the Rust dataplane, and the virtual-server route.

Single-lane commands resolve their lane in this order:

1. explicit `--lane`;
2. `CF_MCP_STACK_MODE`;
3. `dataplane`.

They resolve the protocol version from explicit `--protocol-version`, then
`MCP_PROTOCOL_VERSION`, then `2025-11-25`. Live protocol tests and conformance
also accept `fixture-direct`; other workflows reject it because they have no
direct-fixture execution path. Conformance defaults to all three lanes and its
pinned `2026-07-28` protocol version.

`--topology` remains a compatibility alias for `--lane` on workflows.
Conformance also retains `--client-version` and `--spec-version` as aliases for
`--protocol-version`. Stack lifecycle commands continue to use `--topology`
because they operate on physical stacks, not test lanes.

## Quick start

Probe the dataplane public MCP route:

```bash
cf-integration probe --lane dataplane --protocol-version 2025-11-25
```

`stack up` synchronizes the required source checkouts, validates the Compose
contract, resolves local builds or published images, starts the selected
topology, and waits for its public endpoint. It preserves existing volumes by
default. Use `--fresh` when state must be discarded.

Probe, load, routed live-test, and Inspector commands start their selected
stack, wait for the fixture to be ready, and stop the stack when the command
succeeds or fails. The direct live fixture lane does not start a stack.
Explicit `stack` commands remain available when a persistent environment is
needed.

The Fast Time backend is registered as virtual server
`9779b6698cbd4b4995ee04a4fab38737`, so probe and load commands need no manual
UI setup.

## CLI

The public CLI contains only distinct workflows:

```text
cf-integration
├── stack
│   ├── up
│   ├── down
│   ├── status
│   ├── logs
│   └── config
├── probe
├── load
├── live
├── conformance
│   ├── run
│   └── report
└── debug
    ├── inspect
    └── token
```

Use `--help` at any level for the authoritative flags.

### Stack lifecycle

```bash
cf-integration stack up --topology dataplane
cf-integration stack up --topology dataplane --fresh
cf-integration stack down --topology all
cf-integration stack down --topology all --volumes
cf-integration stack status --topology dataplane
cf-integration stack logs --topology dataplane nginx cf-dataplane
cf-integration stack config --topology dataplane
```

`stack down --volumes` is the explicit destructive cleanup operation.
Diagnostic commands use the harness Compose project and overlays so callers do
not need to reconstruct its Compose invocation.

### Probe

```bash
cf-integration probe --lane dataplane --protocol-version 2025-11-25
```

The probe checks unauthenticated rejection, initialization,
`notifications/initialized`, session reuse, `tools/list`, and one known-safe
`tools/call`. It targets `/mcp` in controlplane topology and
`/servers/{id}/mcp` in dataplane topology.

### Locust and Goose

Both load engines exercise the same MCP lifecycle and remain first-class:

```bash
cf-integration load --lane dataplane --protocol-version 2025-11-25 \
  --engine locust --smoke
cf-integration load --lane dataplane --protocol-version 2025-11-25 \
  --engine goose --smoke

cf-integration load --lane dataplane --engine locust \
  --users 20 --spawn-rate 5 --run-time 2m
cf-integration load --lane dataplane --engine goose \
  --users 20 --spawn-rate 5 --run-time 2m
```

Default full-run settings are 100 users, 10 users/second, and five minutes.
CLI settings override `.env`; explicitly exported `LOCUST_USERS`,
`LOCUST_SPAWN_RATE`, and `LOCUST_RUN_TIME` remain authoritative for both
engines. Smoke defaults are one user, one user/second, and ten seconds.

Locust uses the framework-required Python adapter. Goose is the native Rust
runner. Both initialize real MCP sessions, send
`notifications/initialized`, discover tools, call only a finite allowlist of
safe fixture tools, exercise ping, and audit generated artifacts for credential
leakage.

### Upstream live tests

Run the control-plane repository's live gateway tests against either topology:

```bash
cf-integration live --lane dataplane --group mcp
cf-integration live --lane dataplane --group rbac
cf-integration live --lane dataplane --group protocol
cf-integration live --lane dataplane --group all

# Run the upstream protocol suite directly against its reference fixture.
cf-integration live \
  --lane fixture-direct \
  --group protocol \
  --protocol-version 2025-06-18
```

The `mcp` and `all` groups start the upstream profile-gated `fast_test_server`,
run its one-shot registration job, and, for the dataplane topology, wait until
the publisher snapshot contains its fixed virtual server before launching the
tests. The base stack remains unchanged when other workflows run.

`--lane fixture-direct` is valid with `--group protocol` and runs the upstream
`test-protocol-compliance-reference` target without a gateway stack. The
selected date-formatted version is applied to MCP SDK initialization, and the
live run fails with the installed SDK's supported-version list when that SDK
cannot emit it.

## Official MCP conformance

The official runner is pinned to
`@modelcontextprotocol/conformance@0.2.0-alpha.9`. The official TypeScript
fixture is built from matching source revision
`794dcab99ed1ef2b89607be9999574140ea5c96e`.

The default command is intentionally complete and reproducible:

```bash
cf-integration conformance run
```

It always:

- starts fresh stacks owned by the conformance workflow;
- provisions the pinned official fixture;
- runs every applicable official server scenario;
- defaults to MCP `2026-07-28`;
- runs fixture-direct, controlplane, and dataplane lanes;
- passes an empty expected-failure file to the official runner;
- records raw failures without suppression;
- removes temporary API resources, fixture services, and stacks;
- writes a comparison report even when a lane reports protocol failures.

The official runner's protocol version and the upstream fixture's server era
are independent. The fixture defaults to `--server-era dual`, preserving the
existing behavior where it selects the matching lifecycle from the incoming
request.

Run the same-era baselines explicitly:

```bash
cf-integration conformance run \
  --protocol-version 2026-07-28 \
  --server-era modern
cf-integration conformance run \
  --protocol-version 2025-11-25 \
  --server-era legacy
```

Run the two cross-era paths:

```bash
# Modern client-facing traffic against a legacy-only upstream.
cf-integration conformance run \
  --protocol-version 2026-07-28 \
  --server-era legacy

# Legacy client-facing traffic against a modern-only upstream.
cf-integration conformance run \
  --protocol-version 2025-11-25 \
  --server-era modern
```

In a cross-era run, the fixture-direct lane is the expected incompatible
baseline. A routed lane that passes where fixture-direct fails demonstrates
that the gateway adapted the lifecycle across the boundary; the comparison
report records both axes. The official runner emits the selected client era
strictly. It does not itself test a general-purpose SDK client's automatic
dual-era fallback.

The three lanes are:

1. official oracle directly to the official TypeScript fixture;
2. official oracle through the control-plane public MCP route;
3. official oracle through nginx and the Rust dataplane route.

Select exact lanes by repeating `--lane`:

```bash
cf-integration conformance run \
  --lane fixture-direct \
  --lane dataplane
```

Supported client revisions are explicit and use the same pinned runner and
fixture:

```bash
cf-integration conformance run --protocol-version 2025-11-25
cf-integration conformance run --protocol-version 2025-06-18
```

Artifacts default below `CF_INTEGRATION_DIR`. Use `--results-dir` to place them
elsewhere. Regenerate only the official comparison report with:

```bash
cf-integration conformance report
cf-integration conformance report \
  --results-dir /path/to/results \
  --output-dir /path/to/reports
```

The official runner has no bearer-header option. The harness therefore uses a
random-path loopback proxy that injects authorization while keeping tokens out
of process arguments. Automatic fixture provisioning requires a loopback
`MCP_CLI_BASE_URL`.

## Debug utilities

Debug commands are useful for manual diagnosis but are not compliance gates.

```bash
cf-integration debug inspect \
  --lane dataplane \
  --protocol-version 2025-11-25 \
  --method tools/list

cf-integration debug token \
  --kind scoped \
  --server-id <virtual-server-id>

cf-integration debug token --kind admin
```

Inspector is pinned to `@modelcontextprotocol/inspector@0.22.0` and uses the
same loopback authentication proxy as conformance. The proxy applies the
selected protocol version to Inspector's initialize request.

## Configuration

Copy `.env.example` to `.env`. Shell variables override `.env`, and relative
paths resolve from the repository root.

Common settings:

```bash
CF_MCP_STACK_MODE=dataplane
CF_INTEGRATION_DIR=.integration

CF_CONTROLPLANE_REPO=https://github.com/IBM/mcp-context-forge.git
CF_CONTROLPLANE_REF=main
CF_CONTROLPLANE_VERSION=latest

CF_DATAPLANE_REPO=https://github.com/contextforge-gateway-rs/contextforge-gateway-rs.git
CF_DATAPLANE_REF=
CF_DATAPLANE_IMAGE=ghcr.io/contextforge-gateway-rs/contextforge-gateway-rs:0.1.0
CF_DATAPLANE_PLATFORM=auto

CF_COMPOSE_BUILD=auto
CF_FAST_TIME_EXPECTED_IMAGE=ghcr.io/ibm/cfex-mcp-fast-time-server:latest
CF_FAST_TIME_SERVER_ID=9779b6698cbd4b4995ee04a4fab38737

MCP_CLI_BASE_URL=http://127.0.0.1:8080
MCP_PROTOCOL_VERSION=2025-11-25
NGINX_PORT=8080
```

Published dataplane images are the default. Set `CF_DATAPLANE_REF` to build an
explicit local dataplane ref. `CF_COMPOSE_BUILD=auto` rebuilds missing or
revision-stale local images; `true` always builds and `false` never builds.

Token and endpoint overrides used by probe, load, and debug commands:

```bash
JWT_SECRET_KEY=<integration-secret>
MCP_JWT_SUBJECT=admin@example.com
MCPGATEWAY_BEARER_TOKEN=<pre-minted-token>
MCP_SERVER_ID=<virtual-server-id>
MCP_TOOL_NAMES=<comma-separated-safe-tool-names>
```

Conformance ignores caller-managed fixture IDs and tokens so every lane uses
the same official fixture. Never commit `.env` or generated tokens.

## Repository layout

```text
Cargo.toml, Cargo.lock                     Rust workspace
.cargo/config.toml                        Cargo output under .integration/
src/                                      CLI and workflow composition
crates/platform/                          platform orchestration library
crates/mcp/                               MCP transport and probe library
crates/compliance/                        official conformance library
crates/load/                              Locust and Goose library
docker/docker-compose.cf-dataplane.yaml   dataplane service and nginx override
docker/docker-compose.cf-integration.yaml Fast Time and Locust overlay
docker/docker-compose.cf-conformance.yaml official fixture overlay
scripts/locustfile_mcp.py                  Locust MCP adapter
reports/mcp-conformance-comparison.md      tracked three-lane comparison
.integration/                              ignored checkout/build/runtime state
```
