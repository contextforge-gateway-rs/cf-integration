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
CONTROLPLANE_PROJECT="${CF_CONTROLPLANE_PROJECT:-cf-controlplane-only}"
JWT_SECRET_KEY="${JWT_SECRET_KEY:-my-test-key-but-now-longer-than-32-bytes}"
ADMIN_SUBJECT="${MCP_JWT_SUBJECT:-admin@example.com}"
CF_DATAPLANE_IMAGE="${CF_DATAPLANE_IMAGE:-$(default_dataplane_image)}"
CF_DATAPLANE_PLATFORM="${CF_DATAPLANE_PLATFORM:-linux/amd64}"
CF_COMPOSE_BUILD="${CF_COMPOSE_BUILD:-false}"
FAST_TIME_SERVER_ID="${CF_FAST_TIME_SERVER_ID:-9779b6698cbd4b4995ee04a4fab38737}"

# Scale the upstream gateway sizing knobs to the local Docker engine;
# upstream defaults assume a large CI host (3 replicas x 8 CPUs, 24 workers).
DOCKER_CPUS="$(docker info --format '{{.NCPU}}' 2>/dev/null || echo 4)"
export GATEWAY_REPLICAS="${GATEWAY_REPLICAS:-1}"
export GATEWAY_CPU_LIMIT="${GATEWAY_CPU_LIMIT:-$DOCKER_CPUS}"
export GATEWAY_CPU_RESERVATION="${GATEWAY_CPU_RESERVATION:-1}"
export GATEWAY_MEM_LIMIT="${GATEWAY_MEM_LIMIT:-2G}"
export GATEWAY_MEM_RESERVATION="${GATEWAY_MEM_RESERVATION:-512M}"
export GUNICORN_WORKERS="${GUNICORN_WORKERS:-$DOCKER_CPUS}"

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

controlplane_compose_args=(
  -p "$CONTROLPLANE_PROJECT"
  -f "$CF_CONTROLPLANE_DIR/docker-compose.yml"
)

controlplane_profiles=(--profile testing --profile inspector)
case "${CONTROLPLANE_ENABLE_SSO:-false}" in
  true|1) controlplane_profiles+=(--profile sso) ;;
esac

if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
  bold=$'\033[1m'
  green=$'\033[32m'
  red=$'\033[31m'
  cyan=$'\033[36m'
  reset=$'\033[0m'
else
  bold=""
  green=""
  red=""
  cyan=""
  reset=""
fi

print_section() {
  printf '\n%s==> %s%s\n' "$bold" "$1" "$reset"
}

print_detail() {
  printf '    %s\n' "$1"
}

lane_description() {
  case "$1" in
    probe)
      printf 'Route check: unauthenticated 401, initialize, tools/list, tools/call through nginx -> cf-dataplane.\n'
      ;;
    smoke)
      printf 'Small locust run: 1 user for 10s against the Fast Time virtual server.\n'
      ;;
    live-mcp)
      printf 'Control-plane live MCP protocol E2E suite against the running stack.\n'
      ;;
    live-rbac)
      printf 'Control-plane live MCP RBAC and per-server transport suite.\n'
      ;;
    live-protocol)
      printf 'Protocol compliance gateway target suite, including gateway_virtual-http rows.\n'
      ;;
    live-all)
      printf 'Full upstream tests/live_gateway suite. Noisy, but kept for parity with the report.\n'
      ;;
    locust)
      printf 'Full locust load lane using the harness streamable-HTTP locustfile.\n'
      ;;
    *)
      printf 'Run %s.\n' "$1"
      ;;
  esac
}

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
  test-all       Run every lane (probe smoke live-mcp live-rbac live-protocol live-all)
                 and log all output + per-lane PASS/FAIL to a timestamped log file;
                 CF_TEST_ALL_LOCUST=true appends the full locust load run
  test-all-up    Start/update the integration stack, then run test-all without locust
  test-all-up-load
                 Start/update the integration stack, then run test-all with full locust
  controlplane-up        Start stock cf-controlplane testing stack without cf-dataplane overlays
  controlplane-down      Stop the stock cf-controlplane-only stack
  controlplane-ps        Show stock cf-controlplane-only services
  controlplane-logs      Follow stock cf-controlplane-only logs
  controlplane-config    Render stock cf-controlplane-only compose config
  controlplane-live-core Run non-UI, non-SSO live gateway checks against controlplane-only stack
  controlplane-live-all  Run upstream tests/live_gateway against controlplane-only stack, including SSO/playwright
  controlplane-locust    Run upstream control-plane Locust file against controlplane-only stack
  controlplane-test-all  Run controlplane-up, controlplane-live-core, and controlplane-locust with one log

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
  git -C "$CF_CONTROLPLANE_DIR" fetch -q --prune --tags origin
  git -C "$CF_CONTROLPLANE_DIR" checkout -q "$CF_CONTROLPLANE_REF"
  if [[ "$CF_CONTROLPLANE_REF" == "main" ]]; then
    git -C "$CF_CONTROLPLANE_DIR" pull -q --ff-only origin main
  fi
}

compose() {
  docker compose "${compose_args[@]}" "$@"
}

controlplane_compose() {
  docker compose "${controlplane_compose_args[@]}" "$@"
}

integration_stack_running() {
  docker ps --format '{{.Names}}' | grep -q "^${PROJECT}-"
}

ensure_no_integration_stack() {
  if integration_stack_running; then
    cat >&2 <<EOF
The $PROJECT dataplane integration stack is running and uses the same host ports.
Stop it first:
  $0 down
Then start the control-plane-only stack:
  $0 controlplane-up
EOF
    return 2
  fi
}

make_token() {
  "$ROOT/scripts/cf-jwt.py" \
    --secret "$JWT_SECRET_KEY" \
    --subject "$ADMIN_SUBJECT" \
    "$@"
}

# The dataplane locustfile needs the scoped token (cf-dataplane rejects
# tokens without a scopes claim); upstream locustfiles exercise admin/RBAC
# control-plane surfaces that reject scoped tokens, so they get the same
# admin token upstream's locust_token service would mint.
export_locust_token() {
  if [[ -z "${MCPGATEWAY_BEARER_TOKEN:-}" ]]; then
    export MCPGATEWAY_BEARER_TOKEN
    if [[ "${LOCUST_LOCUSTFILE:-locustfile_cf_dataplane.py}" == "locustfile_cf_dataplane.py" ]]; then
      MCPGATEWAY_BEARER_TOKEN="$(make_token)"
    else
      MCPGATEWAY_BEARER_TOKEN="$(make_token --admin)"
    fi
  fi
}

run_cf_controlplane_make() {
  ensure_checkout
  make -C "$CF_CONTROLPLANE_DIR" "$@"
}

run_cf_controlplane_only_make() {
  ensure_checkout
  COMPOSE_PROJECT_NAME="$CONTROLPLANE_PROJECT" \
  PASSWORD_CHANGE_ENFORCEMENT_ENABLED="${PASSWORD_CHANGE_ENFORCEMENT_ENABLED:-false}" \
  ADMIN_REQUIRE_PASSWORD_CHANGE_ON_BOOTSTRAP="${ADMIN_REQUIRE_PASSWORD_CHANGE_ON_BOOTSTRAP:-false}" \
  REQUIRE_PASSWORD_CHANGE_FOR_DEFAULT_PASSWORD="${REQUIRE_PASSWORD_CHANGE_FOR_DEFAULT_PASSWORD:-false}" \
  MCP_CLI_BASE_URL="${MCP_CLI_BASE_URL:-http://127.0.0.1:${NGINX_PORT:-8080}}" \
    make -C "$CF_CONTROLPLANE_DIR" "$@"
}

run_cf_controlplane_only_pytest() {
  ensure_checkout
  (
    cd "$CF_CONTROLPLANE_DIR"
    COMPOSE_PROJECT_NAME="$CONTROLPLANE_PROJECT" \
    PASSWORD_CHANGE_ENFORCEMENT_ENABLED="${PASSWORD_CHANGE_ENFORCEMENT_ENABLED:-false}" \
    ADMIN_REQUIRE_PASSWORD_CHANGE_ON_BOOTSTRAP="${ADMIN_REQUIRE_PASSWORD_CHANGE_ON_BOOTSTRAP:-false}" \
    REQUIRE_PASSWORD_CHANGE_FOR_DEFAULT_PASSWORD="${REQUIRE_PASSWORD_CHANGE_FOR_DEFAULT_PASSWORD:-false}" \
    MCP_CLI_BASE_URL="${MCP_CLI_BASE_URL:-http://127.0.0.1:${NGINX_PORT:-8080}}" \
      uv run --extra plugins pytest -p no:playwright "$@"
  )
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

run_test_all() {
  local log_dir="${CF_TEST_LOG_DIR:-$INTEGRATION_DIR/test-logs}"
  mkdir -p "$log_dir"
  local log_file="$log_dir/cf-tests-$(date -u +%Y%m%dT%H%M%SZ).log"
  local lanes=(probe smoke live-mcp live-rbac live-protocol live-all)
  case "${CF_TEST_ALL_LOCUST:-false}" in
    true|1) lanes+=(locust) ;;
  esac
  local results=() rc lane failed=0
  local total_lanes="${#lanes[@]}"

  print_section "Dataplane integration test run"
  print_detail "Project: $PROJECT"
  print_detail "Base URL: $MCP_CLI_BASE_URL"
  print_detail "Dataplane image: $CF_DATAPLANE_IMAGE"
  print_detail "Full output log: $log_file"
  print_detail "Lanes: ${lanes[*]}"

  local idx=0 started_at finished_at duration
  for lane in "${lanes[@]}"; do
    idx=$((idx + 1))
    started_at="$(date +%s)"
    printf '\n%s[%d/%d] %s%s\n' "$cyan" "$idx" "$total_lanes" "$lane" "$reset"
    print_detail "$(lane_description "$lane")"
    print_detail "Command: $0 $lane"
    print_detail "Streaming summary here; full output goes to the log."
    printf '===== BEGIN %s %s =====\n' "$lane" "$(date -u +%FT%TZ)" >>"$log_file"
    rc=0
    "$0" "$lane" >>"$log_file" 2>&1 || rc=$?
    finished_at="$(date +%s)"
    duration=$((finished_at - started_at))
    if [[ $rc -eq 0 ]]; then
      results+=("PASS $lane")
      printf '    %sPASS%s %s (%ss)\n' "$green" "$reset" "$lane" "$duration"
    else
      results+=("FAIL $lane exit=$rc")
      printf '    %sFAIL%s %s exit=%s (%ss)\n' "$red" "$reset" "$lane" "$rc" "$duration"
      failed=1
    fi
    printf '===== END %s =====\n\n' "$lane" >>"$log_file"
  done

  print_section "Summary"
  {
    echo "===== SUMMARY $(date -u +%FT%TZ) ====="
    printf '%s\n' "${results[@]}"
  } | tee -a "$log_file"
  printf 'Log: %s\n' "$log_file"
  return "$failed"
}

run_test_all_up() {
  print_section "Step 1/2: start or update integration stack"
  print_detail "Command: $0 up"
  "$0" up
  print_section "Step 2/2: run report lanes without full locust"
  print_detail "Command: CF_TEST_ALL_LOCUST=false $0 test-all"
  CF_TEST_ALL_LOCUST=false "$0" test-all
}

run_test_all_up_load() {
  print_section "Step 1/2: start or update integration stack"
  print_detail "Command: $0 up"
  "$0" up
  print_section "Step 2/2: run report lanes with full locust"
  print_detail "Command: CF_TEST_ALL_LOCUST=true $0 test-all"
  CF_TEST_ALL_LOCUST=true "$0" test-all
}

run_controlplane_up() {
  ensure_checkout
  ensure_no_integration_stack
  mkdir -p "$CF_CONTROLPLANE_DIR/reports"
  export HOST_UID="${HOST_UID:-$(id -u 2>/dev/null || echo 1000)}"
  export HOST_GID="${HOST_GID:-$(id -g 2>/dev/null || echo 1000)}"
  export LOCUST_EXPECT_WORKERS="${LOCUST_EXPECT_WORKERS:-${CONTROLPLANE_LOCUST_WORKERS:-1}}"
  export PASSWORD_CHANGE_ENFORCEMENT_ENABLED="${PASSWORD_CHANGE_ENFORCEMENT_ENABLED:-false}"
  export ADMIN_REQUIRE_PASSWORD_CHANGE_ON_BOOTSTRAP="${ADMIN_REQUIRE_PASSWORD_CHANGE_ON_BOOTSTRAP:-false}"
  export REQUIRE_PASSWORD_CHANGE_FOR_DEFAULT_PASSWORD="${REQUIRE_PASSWORD_CHANGE_FOR_DEFAULT_PASSWORD:-false}"

  local up_args=("${controlplane_profiles[@]}" up -d)
  case "${CONTROLPLANE_START_LOCUST_UI:-false}" in
    true|1)
      up_args+=(--scale "locust_worker=${CONTROLPLANE_LOCUST_WORKERS:-1}")
      ;;
    *)
      up_args+=(--scale locust=0 --scale locust_worker=0)
      ;;
  esac

  controlplane_compose "${up_args[@]}"
  cat <<EOF
Control-plane-only stack started.
Project: $CONTROLPLANE_PROJECT
UI: http://localhost:${NGINX_PORT:-8080}/admin
Login: admin@example.com / changeme
No cf-dataplane service, no dataplane nginx routing override, no DATAPLANE_PUBLISHER overlay.

Run:
  $0 controlplane-live-core
  $0 controlplane-live-all
  $0 controlplane-locust
  $0 controlplane-test-all
EOF
}

run_controlplane_live_core() {
  ensure_no_integration_stack
  run_cf_controlplane_only_make test-mcp-protocol-e2e
  run_cf_controlplane_only_pytest tests/live_gateway/protocol_compliance -v --tb=short
}

run_controlplane_locust() {
  ensure_checkout
  ensure_no_integration_stack
  export HOST_UID="${HOST_UID:-$(id -u 2>/dev/null || echo 1000)}"
  export HOST_GID="${HOST_GID:-$(id -g 2>/dev/null || echo 1000)}"
  export LOCUST_MODE="${LOCUST_MODE:-headless}"
  export LOCUST_LOCUSTFILE="${LOCUST_LOCUSTFILE:-locustfile.py}"
  export LOCUST_USERS="${LOCUST_USERS:-100}"
  export LOCUST_SPAWN_RATE="${LOCUST_SPAWN_RATE:-10}"
  export LOCUST_RUN_TIME="${LOCUST_RUN_TIME:-5m}"
  export CONTROLPLANE_LOCUST_CLASSES="${CONTROLPLANE_LOCUST_CLASSES:-HealthCheckUser FastTimeUser FastTestEchoUser FastTestTimeUser VersionMetaUser}"
  export PASSWORD_CHANGE_ENFORCEMENT_ENABLED="${PASSWORD_CHANGE_ENFORCEMENT_ENABLED:-false}"
  export ADMIN_REQUIRE_PASSWORD_CHANGE_ON_BOOTSTRAP="${ADMIN_REQUIRE_PASSWORD_CHANGE_ON_BOOTSTRAP:-false}"
  export REQUIRE_PASSWORD_CHANGE_FOR_DEFAULT_PASSWORD="${REQUIRE_PASSWORD_CHANGE_FOR_DEFAULT_PASSWORD:-false}"
  local locust_token
  locust_token="$(make_token --admin)"
  controlplane_compose --profile testing run --rm \
    -e MCPGATEWAY_BEARER_TOKEN="$locust_token" \
    --entrypoint /bin/sh \
    locust_token -c 'set -eu; printf "%s" "$MCPGATEWAY_BEARER_TOKEN" > /tokens/gateway.jwt; echo "✅ Token written to /tokens/gateway.jwt"' >/dev/null
  controlplane_compose --profile testing run --rm --no-deps \
    -e CONTROLPLANE_LOCUST_CLASSES="$CONTROLPLANE_LOCUST_CLASSES" \
    --entrypoint /bin/sh \
    locust -c '
set -eu
while [ ! -s /tokens/gateway.jwt ]; do echo "Waiting for gateway JWT..."; sleep 0.5; done
export MCPGATEWAY_BEARER_TOKEN="$(cat /tokens/gateway.jwt)"
set -- \
  -f "/mnt/locust/${LOCUST_LOCUSTFILE:-locustfile.py}" \
  --host=http://nginx:80 \
  --users="${LOCUST_USERS:-100}" \
  --spawn-rate="${LOCUST_SPAWN_RATE:-10}" \
  --run-time="${LOCUST_RUN_TIME:-5m}" \
  --headless \
  --html=/mnt/reports/locust_report.html \
  --csv=/mnt/reports/locust \
  --only-summary
if [ "${CONTROLPLANE_LOCUST_CLASSES:-}" != "all" ] && [ -n "${CONTROLPLANE_LOCUST_CLASSES:-}" ]; then
  set -- "$@" ${CONTROLPLANE_LOCUST_CLASSES}
fi
exec locust "$@"
'
}

run_controlplane_test_all() {
  local log_dir="${CF_TEST_LOG_DIR:-$INTEGRATION_DIR/test-logs}"
  mkdir -p "$log_dir"
  local log_file="$log_dir/controlplane-only-$(date -u +%Y%m%dT%H%M%SZ).log"
  local lanes=(controlplane-up controlplane-live-core controlplane-locust)
  local results=() rc lane failed=0

  for lane in "${lanes[@]}"; do
    echo "Running $lane..."
    printf '===== BEGIN %s %s =====\n' "$lane" "$(date -u +%FT%TZ)" >>"$log_file"
    rc=0
    "$0" "$lane" >>"$log_file" 2>&1 || rc=$?
    if [[ $rc -eq 0 ]]; then
      results+=("PASS $lane")
    else
      results+=("FAIL $lane exit=$rc")
      failed=1
    fi
    printf '===== END %s =====\n\n' "$lane" >>"$log_file"
  done

  {
    echo "===== SUMMARY $(date -u +%FT%TZ) ====="
    printf '%s\n' "${results[@]}"
  } | tee -a "$log_file"
  echo "Log: $log_file"
  return "$failed"
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
  test-all)
    run_test_all
    ;;
  test-all-up)
    run_test_all_up
    ;;
  test-all-up-load)
    run_test_all_up_load
    ;;
  controlplane-up)
    run_controlplane_up
    ;;
  controlplane-down)
    ensure_checkout
    controlplane_compose "${controlplane_profiles[@]}" down --remove-orphans
    ;;
  controlplane-ps)
    ensure_checkout
    controlplane_compose "${controlplane_profiles[@]}" ps
    ;;
  controlplane-logs)
    ensure_checkout
    shift
    controlplane_compose "${controlplane_profiles[@]}" logs -f "$@"
    ;;
  controlplane-config)
    ensure_checkout
    controlplane_compose "${controlplane_profiles[@]}" config
    ;;
  controlplane-live-core)
    run_controlplane_live_core
    ;;
  controlplane-live-all)
    ensure_no_integration_stack
    run_cf_controlplane_only_make test-live-gateway
    ;;
  controlplane-locust)
    run_controlplane_locust
    ;;
  controlplane-test-all)
    run_controlplane_test_all
    ;;
  ""|-h|--help|help)
    usage
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac
