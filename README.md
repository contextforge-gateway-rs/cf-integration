# cf-integration

Make- and script-based integration harness for `cf-controlplane` and
`cf-dataplane`.

The public routing contract is fixed:

- `/servers/{virtual_host_id}/mcp` routes through `cf-dataplane` as
  `/contextforge-rs/servers/{virtual_host_id}/mcp`.
- Raw `/mcp`, UI traffic, and API traffic stay on `cf-controlplane`.

The repository contains no compiled CLI. Make provides the public command
surface; Bash owns orchestration, and small standard-library Python programs
handle JWTs, MCP HTTP, credential-safe proxying, validation, and reporting.
Generated checkouts, build output, workload artifacts, and runtime state stay
under `.integration/` or `CF_INTEGRATION_DIR`. The generated conformance
comparison defaults to the tracked `reports/` directory.

## Requirements

- GNU Make
- Bash 3.2 or newer (the macOS system Bash is supported)
- Python 3.10 or newer
- Docker Engine with Docker Compose v2
- Git and curl
- Node.js 22.7.5 or newer with `npx` for conformance and Inspector
- The control-plane development prerequisites (`uv`, pytest, Make, and
  Playwright where required) for upstream live tests

## Quick start

```bash
cp .env.example .env
make help
make up TOPOLOGY=dataplane
```

Open <http://localhost:8080/admin> and log in with the upstream development
credentials (`admin@example.com` / `changeme`). Stop both projects with:

```bash
make down TOPOLOGY=all
```

Local configuration is read from `.env`. Copy `.env.example` to `.env`; values
passed to Make or exported by the caller override the file.

`make up` clones or updates the required source checkouts automatically.
`make checkout` is available when you want to prepare them without starting
Docker services.

## Make command surface

Run `make help` for the authoritative list.

| Goal | Command |
| --- | --- |
| Start a stack | `make up TOPOLOGY=dataplane` |
| Stop all stacks | `make down` |
| Stop stacks and delete volumes | `make reset` |
| Check the public MCP route | `make probe TOPOLOGY=dataplane` |
| Run a short load smoke test | `make smoke TOPOLOGY=dataplane` |
| Run upstream live tests | `make live-all TOPOLOGY=dataplane` |
| Run official conformance | `make conformance` |
| Inspect an MCP method | `make inspect METHOD=tools/list` |
| Verify the harness | `make test` |

`probe`, `load`, `smoke`, `live*`, `conformance`, and `inspect` are managed
workflows: they start the required topology and stop it during cleanup, even
when the workload fails. Use `make up`, `make status`, `make logs`, and
`make down` for a stack you want to keep running while debugging manually.

### Migrating from the removed CLI

<!-- markdownlint-disable MD013 -->

| Previous command | Make replacement |
| --- | --- |
| `cf-integration stack up --topology dataplane` | `make up TOPOLOGY=dataplane` |
| `cf-integration stack up --topology controlplane --fresh` | `make up TOPOLOGY=controlplane FRESH=1` |
| `cf-integration stack down --topology all --volumes` | `make reset` |
| `cf-integration stack status --topology dataplane` | `make status TOPOLOGY=dataplane` |
| `cf-integration stack logs --topology dataplane nginx` | `make logs TOPOLOGY=dataplane SERVICES=nginx` |
| `cf-integration stack config --topology dataplane` | `make config TOPOLOGY=dataplane` |
| `cf-integration probe --topology dataplane` | `make probe TOPOLOGY=dataplane` |
| `cf-integration load --topology dataplane --smoke` | `make smoke TOPOLOGY=dataplane` |
| `cf-integration load --users 20 --spawn-rate 5 --run-time 2m` | `make load USERS=20 SPAWN_RATE=5 RUN_TIME=2m` |
| `cf-integration live --group rbac` | `make live-rbac` |
| `cf-integration conformance run --lane dataplane` | `make conformance LANES=dataplane` |
| `cf-integration conformance report` | `make conformance-report` |
| `cf-integration debug inspect --method tools/list` | `make inspect METHOD=tools/list` |
| `cf-integration debug token --kind scoped --server-id ID` | `make token TOKEN_KIND=scoped SERVER_ID=ID` |

<!-- markdownlint-enable MD013 -->

The removed Rust-only Goose load engine has no replacement. `make load` and
`make smoke` use the existing Locust workload for both topologies.

### Stack lifecycle

```bash
make checkout
make up TOPOLOGY=dataplane
make up TOPOLOGY=controlplane FRESH=1
make status TOPOLOGY=dataplane
make logs TOPOLOGY=dataplane SERVICES="nginx cf-dataplane"
make config TOPOLOGY=dataplane
make down
make reset
```

`FRESH=1` removes the selected topology's volumes before startup. `make reset`
stops both Compose projects and removes their volumes. Other stack commands
preserve volumes.

The default topology is `CF_MCP_STACK_MODE`, then `dataplane`. The two
topologies share host ports, so the harness refuses to start one while the
other is running.

### Probe and load

```bash
make probe TOPOLOGY=dataplane
make smoke TOPOLOGY=dataplane
make load TOPOLOGY=dataplane USERS=20 SPAWN_RATE=5 RUN_TIME=2m
```

Probe and load targets start the selected stack, wait for its public MCP route,
run the workload, and stop the stack even when the workload fails.

The probe checks unauthenticated rejection, initialization,
`notifications/initialized`, session reuse, `tools/list`, and one allowlisted
fixture tool call. Locust uses `scripts/locustfile_mcp.py` for both topologies.
Reports are written below `.integration/reports/load/` and scanned for bearer
credential leakage before the target returns.

The Fast Time backend defaults to virtual server
`9779b6698cbd4b4995ee04a4fab38737`; set `MCP_SERVER_ID` to target another
server.

### Upstream live tests

```bash
make live-mcp TOPOLOGY=dataplane
make live-rbac TOPOLOGY=dataplane
make live-protocol TOPOLOGY=dataplane
make live-all TOPOLOGY=dataplane

# Equivalent generic form
make live TOPOLOGY=dataplane GROUP=mcp
```

The `mcp` and `all` groups start the upstream profile-gated
`fast_test_server`, run its registration job synchronously, and wait for its
fixed virtual server to appear in the dataplane publisher snapshot. The base
stack remains free of Fast Test containers for unrelated workflows.

`live-all` runs the upstream suites that are compatible with the standard
stack. It excludes `tests/live_gateway/plugins`, whose E2E tests require a
dedicated gateway restart with a different enforce configuration for each
plugin. Dataplane-only virtual-server capability gaps are reported as explicit
XFAILs; unexpected failures still make the target fail.

### Official MCP conformance

The official client is pinned to
`@modelcontextprotocol/conformance@0.2.0-alpha.9`. Its TypeScript fixture is
built from source revision `794dcab99ed1ef2b89607be9999574140ea5c96e`.

```bash
# All three independently measured lanes
make conformance

# Exact lanes and client/server protocol eras
make conformance \
  LANES="fixture-direct dataplane" \
  CLIENT_VERSION=2025-11-25 \
  SERVER_ERA=legacy

# Rebuild Markdown from existing artifacts
make conformance-report
```

Supported client versions are `2025-06-18`, `2025-11-25`, and `2026-07-28`.
Server eras are `legacy`, `modern`, and `dual`. The fixture-direct lane is an
oracle baseline; controlplane and dataplane lanes provision the same fixed
fixture through the gateway. Routed runner traffic uses a random-path loopback
proxy that injects authentication without putting the bearer token in the
child process arguments. Dataplane conformance also rejects responses marked
as control-plane fallback.

Artifacts default below `.integration/conformance/`; the comparison is written
to `reports/mcp-conformance-comparison.md`. Override these with `RESULTS_DIR`
and `OUTPUT_DIR`. The checked-in comparison records the latest retained run and
includes its exact client version, server era, fixture revision, and per-lane
outcomes. A completed run returns nonzero when the official oracle finds a
failure, but it still retains all artifacts and writes the comparison report.

### Debug utilities

```bash
make inspect TOPOLOGY=dataplane METHOD=tools/list
make token TOKEN_KIND=scoped SERVER_ID=<virtual-server-id>
make token TOKEN_KIND=admin
```

Inspector is pinned to `@modelcontextprotocol/inspector@0.22.0` and uses the
same authentication proxy as conformance.

## Configuration

Common settings:

```bash
CF_MCP_STACK_MODE=dataplane
CF_INTEGRATION_DIR=.integration

CF_CONTROLPLANE_REPO=https://github.com/IBM/mcp-context-forge.git
CF_CONTROLPLANE_REF=main
CF_CONTROLPLANE_VERSION=latest

CF_DATAPLANE_REPO=https://github.com/contextforge-gateway-rs/contextforge-gateway-rs.git
CF_DATAPLANE_REF=
CF_DATAPLANE_VERSION=0.1.0
CF_DATAPLANE_PLATFORM=auto

CF_COMPOSE_BUILD=auto
CF_FAST_TIME_EXPECTED_IMAGE=ghcr.io/ibm/cfex-mcp-fast-time-server:latest
CF_FAST_TIME_SERVER_ID=9779b6698cbd4b4995ee04a4fab38737

MCP_CLI_BASE_URL=http://127.0.0.1:8080
MCP_SPEC_VERSION=2025-11-25
MCP_PROTOCOL_VERSION=2025-11-25
JWT_SECRET_KEY=my-test-key-but-now-longer-than-32-bytes
MCP_JWT_SUBJECT=admin@example.com
```

Published dataplane images are the default. Set `CF_DATAPLANE_REF` to build a
local source checkout explicitly. `CF_COMPOSE_BUILD=auto` rebuilds local images
when their revision label differs from the checkout; `true` always builds and
`false` never builds.

Make variables take precedence for a single invocation:

```bash
make up TOPOLOGY=dataplane FRESH=1
make conformance LANES="fixture-direct dataplane" CLIENT_VERSION=2025-11-25
```

Use `.env` for persistent local choices such as repository refs, image tags,
ports, and load defaults. Keep secrets and machine-specific overrides out of
version control.

Never commit `.env` or generated bearer tokens.

## Troubleshooting

- If startup reports that the other topology owns the shared host ports, run
  `make down` and retry.
- If checkout preparation reports uncommitted changes, commit or stash those
  changes in the named checkout under `.integration/`; the harness will not
  overwrite them.
- Use `make config TOPOLOGY=dataplane` to inspect the fully merged Compose
  model before startup.
- Use `make status TOPOLOGY=dataplane` and
  `make logs TOPOLOGY=dataplane SERVICES="nginx cf-dataplane gateway"` to
  diagnose a running stack.
- Use `make reset` only when persistent volumes may be stale; it deletes data
  for both harness Compose projects.

## Verification

Run the non-mutating harness tests with:

```bash
make test
```

They cover the Make command surface, JWT claims, route construction,
authentication injection and dataplane identity enforcement, Compose contract
validation, conformance comparison generation, and the absence of a compiled
workspace.

## Repository layout

```text
Makefile                                  public command surface
scripts/cf-integration.sh                 stack and workflow orchestration
scripts/cf_probe.py                       MCP route probe
scripts/cf_jwt.py                         local token generation
scripts/auth_proxy.py                     credential-injection proxy
scripts/conformance.py                    fixture provisioning, runner, report
scripts/cf_pytest_dataplane.py            dataplane live-suite XFAIL overlay
scripts/locustfile_mcp.py                 Locust MCP workload
docker/                                   Compose overlays and nginx routing
reports/                                  tracked comparison reports
tests/                                    non-mutating Python tests
.integration/                             ignored generated state
```
