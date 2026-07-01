#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

default_dataplane_image() {
  printf 'ghcr.io/contextforge-gateway-rs/contextforge-gateway-rs:%s\n' "${CF_DATAPLANE_VERSION:-0.1.0}"
}

INTEGRATION_DIR="${CF_INTEGRATION_DIR:-"$ROOT/.integration"}"
CF_CONTROLPLANE_DIR="${CF_CONTROLPLANE_DIR:-"$INTEGRATION_DIR/mcp-context-forge"}"
CF_CONTROLPLANE_REPO="${CF_CONTROLPLANE_REPO:-https://github.com/IBM/mcp-context-forge.git}"
CF_CONTROLPLANE_REF="${CF_CONTROLPLANE_REF:-main}"
PROJECT="${CF_INTEGRATION_PROJECT:-cf-integration}"
JWT_SECRET_KEY="${JWT_SECRET_KEY:-my-test-key-but-now-longer-than-32-bytes}"
ADMIN_SUBJECT="${MCP_JWT_SUBJECT:-admin@example.com}"
CF_DATAPLANE_IMAGE="${CF_DATAPLANE_IMAGE:-$(default_dataplane_image)}"
CF_DATAPLANE_PLATFORM="${CF_DATAPLANE_PLATFORM:-linux/amd64}"
CF_COMPOSE_BUILD="${CF_COMPOSE_BUILD:-false}"
FAST_TIME_SERVER_ID="${CF_FAST_TIME_SERVER_ID:-9779b6698cbd4b4995ee04a4fab38737}"

export CF_INTEGRATION_ROOT="$ROOT"
export CF_DATAPLANE_IMAGE
export CF_DATAPLANE_PLATFORM
export CF_CONTROLPLANE_DIR
export CF_INTEGRATION_DIR="$INTEGRATION_DIR"
export JWT_SECRET_KEY
export MCP_CLI_BASE_URL="${MCP_CLI_BASE_URL:-http://127.0.0.1:${NGINX_PORT:-8080}}"
export PLATFORM_ADMIN_EMAIL="${PLATFORM_ADMIN_EMAIL:-$ADMIN_SUBJECT}"

compose_args=(
  -p "$PROJECT"
  -f "$CF_CONTROLPLANE_DIR/docker-compose.yml"
  -f "$ROOT/docker/docker-compose.cf-dataplane.yaml"
  -f "$ROOT/docker/docker-compose.cf-integration.yaml"
)

usage() {
  cat <<EOF
Usage: $0 <command>

Commands:
  checkout       Clone/update cf-controlplane into $CF_CONTROLPLANE_DIR
  up             Checkout cf-controlplane and start cf-controlplane + nginx + cf-dataplane + integration MCP backend
  down           Stop the integration stack
  ps             Show compose services
  logs [svc...]  Follow compose logs
  config         Render merged compose config
  token          Print an HS256 JWT for $ADMIN_SUBJECT
  probe          Verify the nginx -> cf-dataplane MCP route (init/tools/call + 401 negative)
  locust         Run the harness Locust test against /servers/\$MCP_VIRTUAL_SERVER_ID/mcp
  smoke          Same as locust with 1 user for 10s
  live-mcp       Run cf-controlplane live MCP protocol E2E tests
  live-rbac      Run cf-controlplane live MCP RBAC/multi-transport tests
  live-protocol  Run cf-controlplane live protocol-compliance tests
  live-all       Run cf-controlplane's full tests/live_gateway suite

MCP_VIRTUAL_SERVER_ID defaults to the auto-registered Fast Time server:
  $FAST_TIME_SERVER_ID

UI:
  http://localhost:\${NGINX_PORT:-8080}/admin
  admin@example.com / changeme

CF-dataplane image:
  $CF_DATAPLANE_IMAGE
  platform: $CF_DATAPLANE_PLATFORM

Integration MCP backend URL to add in cf-controlplane UI:
  http://cf-integration-mcp-counter:5555/mcp
EOF
}

ensure_checkout() {
  mkdir -p "$INTEGRATION_DIR"
  if [[ ! -d "$CF_CONTROLPLANE_DIR/.git" ]]; then
    git clone -q "$CF_CONTROLPLANE_REPO" "$CF_CONTROLPLANE_DIR"
  fi
  git -C "$CF_CONTROLPLANE_DIR" fetch -q --tags origin
  git -C "$CF_CONTROLPLANE_DIR" checkout -q "$CF_CONTROLPLANE_REF"
  if [[ "$CF_CONTROLPLANE_REF" == "main" ]]; then
    git -C "$CF_CONTROLPLANE_DIR" pull -q --ff-only origin main
  fi
}

compose() {
  docker compose "${compose_args[@]}" "$@"
}

make_token() {
  "$ROOT/scripts/cf-jwt.py" \
    --secret "$JWT_SECRET_KEY" \
    --subject "$ADMIN_SUBJECT"
}

export_locust_token() {
  if [[ -z "${MCPGATEWAY_BEARER_TOKEN:-}" ]]; then
    export MCPGATEWAY_BEARER_TOKEN
    MCPGATEWAY_BEARER_TOKEN="$(make_token)"
  fi
}

run_cf_controlplane_make() {
  ensure_checkout
  make -C "$CF_CONTROLPLANE_DIR" "$@"
}

map_compose_services() {
  for service in "$@"; do
    case "$service" in
      cf-controlplane)
        printf '%s\n' gateway
        ;;
      *)
        printf '%s\n' "$service"
        ;;
    esac
  done
}

export_server_id() {
  export MCP_SERVER_ID="${MCP_SERVER_ID:-${MCP_VIRTUAL_SERVER_ID:-$FAST_TIME_SERVER_ID}}"
}

# LOCUST_MODE/LOCUST_LOCUSTFILE defaults live in the compose overlay.
run_locust() {
  ensure_checkout
  export_server_id
  export_locust_token
  compose --profile testing run --rm --no-deps locust
}

case "${1:-}" in
  checkout)
    ensure_checkout
    ;;
  up)
    ensure_checkout
    up_args=(-d)
    if [[ "$CF_COMPOSE_BUILD" == "true" || "$CF_COMPOSE_BUILD" == "1" ]]; then
      up_args+=(--build)
    fi
    compose pull cf-dataplane
    compose up "${up_args[@]}"
    cat <<EOF
Integration stack started.
UI: http://localhost:${NGINX_PORT:-8080}/admin
Login: admin@example.com / changeme
CF-dataplane image: $CF_DATAPLANE_IMAGE
CF-dataplane platform: $CF_DATAPLANE_PLATFORM
Add MCP backend URL in cf-controlplane UI: http://cf-integration-mcp-counter:5555/mcp
Fast Time is auto-registered as virtual server $FAST_TIME_SERVER_ID, so these work directly:
  $0 probe
  $0 smoke
  $0 locust
Override with MCP_VIRTUAL_SERVER_ID=<id> to target a UI-created virtual server.
EOF
    ;;
  down)
    compose down --remove-orphans
    ;;
  ps)
    compose ps
    ;;
  logs)
    shift
    mapfile -t services < <(map_compose_services "$@")
    compose logs -f "${services[@]}"
    ;;
  config)
    ensure_checkout
    compose --profile testing config
    ;;
  token)
    make_token
    ;;
  probe)
    export_server_id
    "$ROOT/scripts/cf-probe.py"
    ;;
  locust)
    run_locust
    ;;
  smoke)
    export LOCUST_USERS="${LOCUST_USERS:-1}"
    export LOCUST_SPAWN_RATE="${LOCUST_SPAWN_RATE:-1}"
    export LOCUST_RUN_TIME="${LOCUST_RUN_TIME:-10s}"
    run_locust
    ;;
  live-mcp)
    run_cf_controlplane_make test-mcp-protocol-e2e
    ;;
  live-rbac)
    run_cf_controlplane_make test-mcp-rbac
    ;;
  live-protocol)
    run_cf_controlplane_make test-protocol-compliance-gateway
    ;;
  live-all)
    run_cf_controlplane_make test-live-gateway
    ;;
  ""|-h|--help|help)
    usage
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac
