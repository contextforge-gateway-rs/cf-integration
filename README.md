# cf-integration

Reusable integration harness for wiring `cf-controlplane` to the Rust `cf-dataplane`.

Nginx routes only `/servers/{virtual_host_id}/mcp` to `cf-dataplane` as `/contextforge-rs/servers/{virtual_host_id}/mcp`. Raw `/mcp` and all UI/API traffic stay on `cf-controlplane`.

## Quick Start

```bash
scripts/cf-integration.sh up
```

The script checks out `cf-controlplane` under `.integration/mcp-context-forge`, pulls the published `cf-dataplane` image, starts the control-plane compose stack with the dataplane/nginx overlay, and starts a local MCP counter backend for UI-created virtual servers.

Open `http://localhost:8080/admin` and log in with:

```text
admin@example.com / changeme
```

Add this MCP backend in the UI:

```text
http://cf-integration-mcp-counter:5555/mcp
```

Create a virtual server from that backend's tools. The overlay enables `DATAPLANE_PUBLISHER`, so `cf-controlplane` publishes the virtual server config to Redis for `cf-dataplane`. The publisher runs every 60 seconds.

## Locust

Run the control-plane MCP protocol Locust file against the public nginx URL:

```bash
MCP_VIRTUAL_SERVER_ID=<virtual-server-id-from-ui> scripts/cf-integration.sh smoke
MCP_VIRTUAL_SERVER_ID=<virtual-server-id-from-ui> scripts/cf-integration.sh locust
```

Load settings:

```bash
LOCUST_USERS=20 LOCUST_SPAWN_RATE=5 LOCUST_RUN_TIME=2m \
MCP_TOOL_NAMES=<comma-separated-listed-tool-names> \
MCP_VIRTUAL_SERVER_ID=<virtual-server-id-from-ui> scripts/cf-integration.sh locust
```

Reports are written under `.integration/mcp-context-forge/reports/`.

## Live Tests

Run control-plane live test targets against the same running stack:

```bash
scripts/cf-integration.sh live-mcp
scripts/cf-integration.sh live-rbac
scripts/cf-integration.sh live-protocol
scripts/cf-integration.sh live-all
```

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
NGINX_PORT=8080
```

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
scripts/cf-integration.sh down
```

## Layout

```text
docker/docker-compose.cf-dataplane.yaml      cf-dataplane service and nginx override
docker/docker-compose.cf-integration.yaml    integration MCP backend and Locust override
docker/nginx.cf-dataplane.conf               public routing split
docker/mcp_counter.Dockerfile                local MCP counter backend
scripts/cf-integration.sh                    orchestration wrapper
scripts/cf-jwt.py                            local HS256 JWT helper
```
