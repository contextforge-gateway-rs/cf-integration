# cf-integration

Reusable integration harness for wiring `cf-controlplane` to the Rust `cf-dataplane`.

Nginx routes only `/servers/{virtual_host_id}/mcp` to `cf-dataplane` as `/contextforge-rs/servers/{virtual_host_id}/mcp`. Raw `/mcp` and all UI/API traffic stay on `cf-controlplane`.

The stack is the stock upstream `docker-compose.yml` (including its Fast Time image, registrations, and fast-test fixtures) with exactly two intentional differences: the nginx routing split above, and `DATAPLANE_PUBLISHER=true` on the gateway so virtual server configs reach `cf-dataplane` via Redis. Any test failure should therefore be attributable to `cf-dataplane` behavior, not stack configuration drift.

## Quick Start

```bash
scripts/cf-integration.sh up
```

The script checks out `cf-controlplane` under `.integration/mcp-context-forge`, pulls the published `cf-dataplane` image, starts the control-plane compose stack with the dataplane/nginx overlay, and starts a local MCP counter backend for UI-created virtual servers.

Fast Time runs the upstream default image in dual-transport mode (`/http` streamable HTTP + `/sse`), and the stock upstream registration jobs register both gateways and their fixed virtual servers unchanged.

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

Run everything in one shot with per-lane PASS/FAIL and full output captured to a timestamped log file (default `.integration/test-logs/`, override with `CF_TEST_LOG_DIR`):

```bash
scripts/cf-integration.sh test-all
```

`live-mcp` is the green lane for this harness: `up` starts the upstream fast-test fixture services, so the full MCP protocol E2E suite (including `TestToolCalls`) passes. The stack matches upstream, so remaining failures in the other lanes measure `cf-dataplane` feature gaps (for example, tokens minted without a `scopes` claim are accepted by `cf-controlplane` but rejected with 401 by `cf-dataplane`); see `reports/` for the current classification.

## Configuration

Useful overrides:

```bash
CF_CONTROLPLANE_REPO=<control-plane-git-url>
CF_CONTROLPLANE_REF=main
CF_DATAPLANE_IMAGE=ghcr.io/contextforge-gateway-rs/contextforge-gateway-rs:<tag>
CF_DATAPLANE_VERSION=0.1.0
CF_DATAPLANE_PLATFORM=linux/amd64
CF_COMPOSE_BUILD=false
CF_INTEGRATION_DIR=.integration
CF_FAST_TIME_SERVER_ID=9779b6698cbd4b4995ee04a4fab38737
NGINX_PORT=8080
```

`CF_COMPOSE_BUILD` defaults to `false`; published images are used and local builds happen only when an image is missing or `CF_COMPOSE_BUILD=true` forces `--build`.

The upstream gateway sizing knobs (`GATEWAY_REPLICAS`, `GATEWAY_CPU_LIMIT`, `GATEWAY_CPU_RESERVATION`, `GATEWAY_MEM_LIMIT`, `GATEWAY_MEM_RESERVATION`, `GUNICORN_WORKERS`) are defaulted to fit the local Docker engine (upstream assumes a large CI host); override them to match upstream sizing on bigger hardware.

If `CF_DATAPLANE_IMAGE` is not set, the script uses:

```text
ghcr.io/contextforge-gateway-rs/contextforge-gateway-rs:${CF_DATAPLANE_VERSION:-0.1.0}
```

## Commands

```bash
scripts/cf-integration.sh checkout
scripts/cf-integration.sh up
scripts/cf-integration.sh ps
scripts/cf-integration.sh logs nginx cf-dataplane cf-controlplane
scripts/cf-integration.sh config
scripts/cf-integration.sh token
scripts/cf-integration.sh probe
scripts/cf-integration.sh down
```

## Layout

```text
docker/docker-compose.cf-dataplane.yaml      cf-dataplane service and nginx override
docker/docker-compose.cf-integration.yaml    integration MCP backend and Locust override
docker/nginx.cf-dataplane.conf               public routing split
docker/mcp_counter.Dockerfile                local MCP counter backend
scripts/cf-integration.sh                    orchestration wrapper
scripts/cf-jwt.py                            local HS256 JWT helper (CLI + importable make_token)
scripts/cf-probe.py                          end-to-end dataplane route probe
scripts/locustfile_cf_dataplane.py           harness Locust file for the dataplane route
scripts/mcp_http.py                          shared MCP streamable-HTTP helpers
reports/                                     curated run reports (YYYY-MM-DD-<topic>.md)
```
