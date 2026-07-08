# cf-integration

Reusable integration harness for wiring `cf-controlplane` to the Rust `cf-dataplane`.

Nginx routes only `/servers/{virtual_host_id}/mcp` to `cf-dataplane` as `/contextforge-rs/servers/{virtual_host_id}/mcp`. Raw `/mcp` and all UI/API traffic stay on `cf-controlplane`.

The stack is the stock upstream `docker-compose.yml` (including its Fast Time image, registrations, and fast-test fixtures) with exactly two intentional differences: the nginx routing split above, and `DATAPLANE_PUBLISHER=true` on the gateway so virtual server configs reach `cf-dataplane` via Redis. Any test failure should therefore be attributable to `cf-dataplane` behavior, not stack configuration drift.

## Quick Start

```bash
scripts/cf-integration.sh up
```

The script checks out `cf-controlplane` under `.integration/mcp-context-forge`, fresh-bootstraps the compose projects (`reset`, including volumes, for a clean database), uses the upstream local build image for `cf-controlplane`, pulls the published `cf-dataplane` image, starts the control-plane compose stack with the dataplane/nginx overlay, and starts a local MCP counter backend for UI-created virtual servers. The default control-plane image is rebuilt automatically when its revision label does not match the checked-out commit. Published image pulls are digest-aware: the script pulls only when the remote digest is missing locally or has changed.

Local configuration is read from `.env` when present. Copy `.env.example` to `.env` and edit it for branch/image choices; `.env` is git-ignored and shell variables override it.

Stack-starting commands fresh-bootstrap by default, including `up`, `up controlplane`, `test-all-up*`, and `controlplane-test-all`. A repeated `up` skips `docker compose up` when the running integration stack already matches the requested control-plane checkout and image tags. Set `CF_FRESH_STACK=false` when you intentionally want to keep existing database state while changing or restarting the stack.

Fast Time runs the upstream default image in dual-transport mode (`/http` streamable HTTP + `/sse`), and the stock upstream registration jobs register both gateways and their fixed virtual servers unchanged. The dataplane will not implement SSE upstreams (the transport is deprecated and is removed in the 2026-07-28 MCP protocol update): the control-plane publisher exports streamable-HTTP backends only, SSE-backed virtual servers are therefore absent from dataplane config, and nginx replays their `/servers/{id}/mcp` requests on the control plane — SSE stays fully functional through the control-plane path. Requires a control-plane image with the streamable-only publisher change; older images publish SSE backends and the dataplane answers them with empty tool lists instead of falling back.

Open `http://localhost:8080/admin` and log in with:

```text
admin@example.com / changeme
```

Add this MCP backend in the UI:

```text
http://cf-integration-mcp-counter:5555/mcp
```

Optionally create a virtual server from that backend's tools in the UI. The overlay enables `DATAPLANE_PUBLISHER`, so `cf-controlplane` publishes the virtual server config to Redis for `cf-dataplane`. The publisher runs every 60 seconds.

The Fast Time backend is registered automatically as virtual server `9779b6698cbd4b4995ee04a4fab38737`, so `probe`, `smoke`, and `locust` work with no manual UI step.

## Probe

Verify the public nginx -> `cf-dataplane` route end to end (401 negative, `initialize`, session reuse, `tools/list`, `tools/call`):

```bash
scripts/cf-integration.sh probe
```

## Locust

Run the harness Locust file (`scripts/locustfile_cf_dataplane.py`, streamable-HTTP aware) against the public nginx URL:

```bash
scripts/cf-integration.sh smoke    # 1 user, 10s
scripts/cf-integration.sh locust   # full load run
```

Both default to the auto-registered Fast Time virtual server; set `MCP_VIRTUAL_SERVER_ID=<id>` to target a UI-created one.

Load settings:

```bash
LOCUST_USERS=20 LOCUST_SPAWN_RATE=5 LOCUST_RUN_TIME=2m \
MCP_TOOL_NAMES=<comma-separated-listed-tool-names> \
scripts/cf-integration.sh locust
```

Locust HTML/CSV output is written under `.integration/mcp-context-forge/reports/`. Curated run reports live in `reports/` in this repo, named `YYYY-MM-DD-<topic>.md`.

## Live Tests

Run control-plane live test targets against the same running stack:

```bash
scripts/cf-integration.sh live-mcp
scripts/cf-integration.sh live-rbac
scripts/cf-integration.sh live-protocol
scripts/cf-integration.sh live-all
```

Run everything in one shot with neutral lane sections, nextest-style per-test result rows, and full output captured to a timestamped log file (default `.integration/test-logs/`, override with `CF_TEST_LOG_DIR`):

```bash
scripts/cf-integration.sh test-all
```

`CF_TEST_ALL_LOCUST=true` appends the full Locust load run (default 100 users, 5m; `LOCUST_*` variables apply) as a final lane.

To start the stack and run the same report lanes in one command. These
commands use the same fresh-bootstrap path as `up` so runs are reproducible —
long-lived state has produced failures that do not exist on clean deployments;
set `CF_FRESH_STACK=false` to keep existing state, or run
`scripts/cf-integration.sh reset` manually:

```bash
scripts/cf-integration.sh test-all-up            # no full locust lane
scripts/cf-integration.sh test-all-up-load       # includes full locust lane
scripts/cf-integration.sh test-all-up-no-plugins # also deselects tests/live_gateway/plugins
```

These commands stream stack startup output, then print colored section headers
and every recorded pytest result row to the terminal while writing full
pytest/locust output to the timestamped log.

`live-mcp` is the green lane for this harness: `up` starts the upstream fast-test fixture services, so the full MCP protocol E2E suite (including `TestToolCalls`) passes. The stack matches upstream, so remaining failures in the other lanes measure `cf-dataplane` feature gaps (for example, tokens minted without a `scopes` claim are accepted by `cf-controlplane` but rejected with 401 by `cf-dataplane`); see `reports/` for the current classification.

## Control-Plane Baseline

Run the stock upstream `cf-controlplane` testing stack without the `cf-dataplane` service, nginx route override, integration MCP counter, or `DATAPLANE_PUBLISHER` overlay:

```bash
scripts/cf-integration.sh controlplane-test-all
```

This uses a separate compose project (`CF_CONTROLPLANE_PROJECT`, default `cf-controlplane-only`) but the same host ports as the dataplane stack. `controlplane-test-all` fresh-bootstraps first, starts the stock testing stack, runs non-UI live gateway checks without SSO/playwright, then runs upstream `locustfile.py` with the non-UI Fast Time/Fast Test/health class subset. Output is logged under `.integration/test-logs/`.

Useful individual commands:

```bash
scripts/cf-integration.sh up controlplane
scripts/cf-integration.sh controlplane-live-core
scripts/cf-integration.sh controlplane-live-all
scripts/cf-integration.sh controlplane-locust
scripts/cf-integration.sh down
```

Set `CONTROLPLANE_ENABLE_SSO=true` only when explicitly validating SSO-dependent tests.
Set `CONTROLPLANE_LOCUST_CLASSES=all` to run the full upstream Locust class mix, including admin/UI/mutating surfaces.

## Configuration

Useful overrides:

```bash
# Put these in .env, or export them in the shell.
CF_CONTROLPLANE_REPO=<control-plane-git-url>
CF_CONTROLPLANE_REF=main
CF_CONTROLPLANE_IMAGE=mcpgateway/mcpgateway:latest
CF_CONTROLPLANE_VERSION=latest
CF_DATAPLANE_REPO=https://github.com/contextforge-gateway-rs/contextforge-gateway-rs.git
CF_DATAPLANE_REF=
CF_DATAPLANE_DIR=.integration/contextforge-gateway-rs
CF_DATAPLANE_LOCAL_IMAGE=contextforge-gateway-rs/contextforge-gateway-rs:local
CF_DATAPLANE_IMAGE=ghcr.io/contextforge-gateway-rs/contextforge-gateway-rs:<tag>
CF_DATAPLANE_VERSION=0.1.0
CF_DATAPLANE_PLATFORM=auto
CF_COMPOSE_BUILD=auto
CF_INTEGRATION_DIR=.integration
CF_FAST_TIME_SERVER_ID=9779b6698cbd4b4995ee04a4fab38737
CF_DATAPLANE_PUBLISHER_INTERVAL_SECONDS=2
CF_DATAPLANE_USER_CONFIG_CACHE_EXPIRY_SECONDS=0
NGINX_PORT=8080
```

`CF_COMPOSE_BUILD` defaults to `auto`: when using the default local control-plane image tag, the script compares the image's `org.opencontainers.image.revision` label with the checked-out `CF_CONTROLPLANE_REF` commit and passes `--build` when the image is missing or stale. If `CF_DATAPLANE_REF` is set, the same revision-label check is applied to the local dataplane image. Harness-built images are stamped with checkout revision and ref so the next run can skip the build, and repeated `up` can skip compose entirely when the running stack already matches. Set `CF_COMPOSE_BUILD=false` to force image reuse, or `CF_COMPOSE_BUILD=true` to always build. Explicit `CF_CONTROLPLANE_IMAGE` or `IMAGE_LOCAL` overrides disable control-plane auto-build unless `CF_COMPOSE_BUILD=true` is also set.

The upstream gateway sizing knobs (`GATEWAY_REPLICAS`, `GATEWAY_CPU_LIMIT`, `GATEWAY_CPU_RESERVATION`, `GATEWAY_MEM_LIMIT`, `GATEWAY_MEM_RESERVATION`, `GUNICORN_WORKERS`) are defaulted to fit the local Docker engine (upstream assumes a large CI host); override them to match upstream sizing on bigger hardware.

If `CF_CONTROLPLANE_IMAGE` is not set, the script uses the upstream local build tag:

```text
mcpgateway/mcpgateway:${CF_CONTROLPLANE_VERSION:-latest}
```

To use a published control-plane image instead, set `CF_CONTROLPLANE_IMAGE=ghcr.io/ibm/mcp-context-forge:<tag>`. Use a tag that matches `CF_CONTROLPLANE_REF`; the published `latest` tag can lag `main`.

If `CF_DATAPLANE_IMAGE` is not set, the script uses:

```text
ghcr.io/contextforge-gateway-rs/contextforge-gateway-rs:${CF_DATAPLANE_VERSION:-0.1.0}
```

GHCR currently publishes only `0.1.0` for the dataplane; there is no `latest` tag.

To test a dataplane branch instead of the published image, set `CF_DATAPLANE_REF`:

```bash
CF_DATAPLANE_REF=user/luca/cp-parity-tool-names ./scripts/cf-integration.sh up
```

When `CF_DATAPLANE_REF` is set and `CF_DATAPLANE_IMAGE` is unset, the script checks out `CF_DATAPLANE_REPO` under `CF_DATAPLANE_DIR`, builds `CF_DATAPLANE_LOCAL_IMAGE`, stamps the image with `org.opencontainers.image.ref.name` and `org.opencontainers.image.revision`, and prints both the checkout and image revision in the `up` summary. `CF_DATAPLANE_PLATFORM=auto` uses `linux/amd64` for published images and the Docker server platform for local source builds; set an explicit platform to override that.

Config propagation defaults are tuned for functional runs: the overlay sets the
control-plane publisher snapshot interval to 2s
(`CF_DATAPLANE_PUBLISHER_INTERVAL_SECONDS`, requires a control-plane image with
configurable publisher interval; older images ignore it and publish every 60s)
and disables the dataplane per-subject config cache
(`CF_DATAPLANE_USER_CONFIG_CACHE_EXPIRY_SECONDS=0`, whose sliding TTL can pin
stale configs under steady traffic). For load benchmarks set them back to the
upstream defaults (60 and 60).

## Commands

```bash
scripts/cf-integration.sh checkout
scripts/cf-integration.sh up
scripts/cf-integration.sh up controlplane
scripts/cf-integration.sh ps
scripts/cf-integration.sh logs nginx cf-dataplane cf-controlplane
scripts/cf-integration.sh config
scripts/cf-integration.sh token
scripts/cf-integration.sh probe
scripts/cf-integration.sh test-all-up
scripts/cf-integration.sh test-all-up-load
scripts/cf-integration.sh test-all-up-no-plugins
scripts/cf-integration.sh down
```

## Layout

```text
docker/docker-compose.cf-dataplane.yaml      cf-dataplane service and nginx override
docker/docker-compose.cf-integration.yaml    integration MCP backend and Locust override
docker/nginx.cf-dataplane.conf               public routing split
docker/mcp_counter.Dockerfile                local MCP counter backend
scripts/cf-integration.sh                    orchestration wrapper
scripts/cf_pytest_result_recorder.py         pytest result recorder for test-all display
scripts/cf-jwt.py                            local HS256 JWT helper (CLI + importable make_token)
scripts/cf-probe.py                          end-to-end dataplane route probe
scripts/locustfile_cf_dataplane.py           harness Locust file for the dataplane route
scripts/mcp_http.py                          shared MCP streamable-HTTP helpers
reports/                                     curated run reports (YYYY-MM-DD-<topic>.md)
```
