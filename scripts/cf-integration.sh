#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

default_dataplane_image() {
  # GHCR currently publishes only 0.1.0 (no latest tag); keep the default pinned.
  printf 'ghcr.io/contextforge-gateway-rs/contextforge-gateway-rs:%s\n' "${CF_DATAPLANE_VERSION:-0.1.0}"
}

default_controlplane_image() {
  printf 'mcpgateway/mcpgateway:%s\n' "${CF_CONTROLPLANE_VERSION:-latest}"
}

INTEGRATION_DIR="${CF_INTEGRATION_DIR:-"$ROOT/.integration"}"
CF_CONTROLPLANE_DIR="${CF_CONTROLPLANE_DIR:-"$INTEGRATION_DIR/mcp-context-forge"}"
CF_CONTROLPLANE_REPO="${CF_CONTROLPLANE_REPO:-https://github.com/IBM/mcp-context-forge.git}"
CF_CONTROLPLANE_REF="${CF_CONTROLPLANE_REF:-main}"
PROJECT="${CF_INTEGRATION_PROJECT:-cf-integration}"
CONTROLPLANE_PROJECT="${CF_CONTROLPLANE_PROJECT:-cf-controlplane-only}"
JWT_SECRET_KEY="${JWT_SECRET_KEY:-my-test-key-but-now-longer-than-32-bytes}"
ADMIN_SUBJECT="${MCP_JWT_SUBJECT:-admin@example.com}"
CF_CONTROLPLANE_IMAGE_WAS_SET="${CF_CONTROLPLANE_IMAGE+x}"
IMAGE_LOCAL_WAS_SET="${IMAGE_LOCAL+x}"
CF_CONTROLPLANE_IMAGE="${CF_CONTROLPLANE_IMAGE:-${IMAGE_LOCAL:-$(default_controlplane_image)}}"
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
export CF_CONTROLPLANE_IMAGE
export CF_DATAPLANE_IMAGE
export CF_DATAPLANE_PLATFORM
export CF_CONTROLPLANE_DIR
export CF_INTEGRATION_DIR="$INTEGRATION_DIR"
export IMAGE_LOCAL="$CF_CONTROLPLANE_IMAGE"
export JWT_SECRET_KEY
export MCP_CLI_BASE_URL="${MCP_CLI_BASE_URL:-http://127.0.0.1:${NGINX_PORT:-8080}}"
export PLATFORM_ADMIN_EMAIL="${PLATFORM_ADMIN_EMAIL:-$ADMIN_SUBJECT}"
export KEY_FILE_PASSWORD="${KEY_FILE_PASSWORD:-}"

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
all_stack_profiles=(--profile testing --profile inspector --profile sso)

if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
  bold=$'\033[1m'
  header=$'\033[1;36m'
  green=$'\033[32m'
  red=$'\033[31m'
  grey=$'\033[90m'
  reset=$'\033[0m'
else
  bold=""
  header=""
  green=""
  red=""
  grey=""
  reset=""
fi

section_rule="================================================================================"
lane_rule="--------------------------------------------------------------------------------"

print_section() {
  printf '\n%s%s\n==> %s\n%s%s\n' "$header" "$section_rule" "$1" "$section_rule" "$reset"
}

print_detail() {
  printf '    %s\n' "$1"
}

print_header_detail() {
  local text="$1"
  while IFS= read -r line; do
    printf '    %s\n' "$line"
  done <<<"$text"
}

print_lane_header() {
  printf '\n%s%s\n[%s/%s] %s\n' "$header" "$lane_rule" "$1" "$2" "$3"
  print_header_detail "$4"
  print_header_detail "Command: $5"
  print_header_detail "Streaming summary here; full output goes to the log."
  printf '%s%s\n' "$lane_rule" "$reset"
}

print_lane_summary() {
  local status="$1"
  local lane="$2"
  local summary_part="$3"
  local duration="$4"
  local color="$green"
  [[ "$status" == "FAIL" ]] && color="$red"
  printf '\n%s%s\n%s summary: %s%s (%ss)\n%s%s\n' "$color" "$lane_rule" "$status" "$lane" "$summary_part" "$duration" "$lane_rule" "$reset"
}

print_log_footer() {
  printf '\n%s%s\nLog: %s\n%s%s\n' "$header" "$section_rule" "$1" "$section_rule" "$reset"
}

print_info_box() {
  local title="$1"
  local body="$2"
  printf '\n%s%s\n%s\n' "$header" "$section_rule" "$title"
  print_header_detail "$body"
  printf '%s%s\n' "$section_rule" "$reset"
}

lane_description() {
  case "$1" in
    probe)
      cat <<'EOF'
Dataplane route probe against /servers/{virtual_host_id}/mcp.
Checks: unauthenticated initialize -> 401, authenticated initialize -> session, tools/list -> tools, tools/call -> successful tool result.
EOF
      ;;
    smoke)
      cat <<EOF
Locust smoke against /servers/{virtual_host_id}/mcp on the Fast Time virtual server.
Settings: users=${LOCUST_USERS:-1}, spawn_rate=${LOCUST_SPAWN_RATE:-1}, run_time=${LOCUST_RUN_TIME:-10s}. Flow: initialize, tools/list, ping, tools/call.
EOF
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
      printf 'Full upstream tests/live_gateway suite in two passes: asyncio suites without pytest-playwright, then the playwright-dependent suites (sso, RBAC transport).\n'
      ;;
    locust)
      printf 'Full locust load lane using the harness streamable-HTTP locustfile.\n'
      ;;
    *)
      printf 'Run %s.\n' "$1"
      ;;
  esac
}

format_test_summary() {
  local lane_log="$1"
  local summary
  # Lanes may run more than one pytest pass (live-all); join all summaries.
  summary="$(grep -E '^=+ .* (failed|passed|error|errors|skipped|xfailed|xpassed|warnings|deselected).* =+$' "$lane_log" | sed -E 's/^=+ +//; s/ +=+$//' | paste -sd '|' - || true)"
  if [[ -n "$summary" ]]; then
    printf '%s\n' "$summary" | sed 's/|/ | /g'
  fi
}

is_pytest_lane() {
  case "$1" in
    live-mcp|live-rbac|live-protocol|live-all)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

print_result_line() {
  local status="$1"
  local duration="$2"
  local name="$3"
  case "$status" in
    PASS|XFAIL)
      printf '    %s%-5s%s [%7s] %s\n' "$green" "$status" "$reset" "$duration" "$name"
      ;;
    FAIL|ERROR|XPASS)
      printf '    %s%-5s%s [%7s] %s\n' "$red" "$status" "$reset" "$duration" "$name"
      ;;
    SKIP)
      printf '    %s%-5s%s [%7s] %s\n' "$grey" "$status" "$reset" "$duration" "$name"
      ;;
    *)
      printf '    %-5s [%7s] %s\n' "$status" "$duration" "$name"
      ;;
  esac
}

print_recorded_results() {
  local result_file="$1"
  local printed=0 status duration name
  if [[ ! -s "$result_file" ]]; then
    return 1
  fi

  while IFS=$'\t' read -r status duration name; do
    [[ -n "$status" && -n "$name" ]] || continue
    print_result_line "$status" "$duration" "$name"
    printed=1
  done <"$result_file"

  [[ $printed -eq 1 ]]
}

print_summary_results() {
  local lane_log="$1"
  local printed=0 line status name

  while IFS= read -r line; do
    case "$line" in
      PASSED\ *)
        status="PASS"
        name="${line#PASSED }"
        ;;
      FAILED\ *)
        status="FAIL"
        name="${line#FAILED }"
        name="${name%% - *}"
        ;;
      ERROR\ *)
        status="ERROR"
        name="${line#ERROR }"
        name="${name%% - *}"
        ;;
      XPASS\ *)
        status="XPASS"
        name="${line#XPASS }"
        name="${name%% - *}"
        ;;
      XFAIL\ *)
        status="XFAIL"
        name="${line#XFAIL }"
        name="${name%% - *}"
        ;;
      SKIPPED\ *)
        status="SKIP"
        name="${line#SKIPPED }"
        ;;
      *)
        continue
        ;;
    esac
    print_result_line "$status" "-" "$name"
    printed=1
  done < <(grep -E '^(PASSED|FAILED|ERROR|XPASS|XFAIL|SKIPPED) ' "$lane_log" || true)

  [[ $printed -eq 1 ]]
}

print_probe_results() {
  local lane_log="$1"
  local printed=0 line step status detail

  while IFS= read -r line; do
    case "$line" in
      auth_negative=PASS*|auth_negative=FAIL*|initialize=PASS*|initialize=FAIL*|tools_list=PASS*|tools_list=FAIL*|tool_call=PASS*|tool_call=FAIL*|tool_call=SKIP*)
        step="${line%%=*}"
        detail="${line#*=}"
        status="${detail%% *}"
        detail="${detail#"$status"}"
        detail="${detail# }"
        print_result_line "$status" "-" "probe/$step${detail:+ $detail}"
        printed=1
        ;;
    esac
  done <"$lane_log"

  [[ $printed -eq 1 ]]
}

print_locust_results() {
  local lane="$1"
  local lane_log="$2"
  local rc="$3"
  local printed=0 line name reqs fails status
  local locust_row_re='^POST[[:space:]]+(.+)[[:space:]]+([0-9]+)[[:space:]]+([0-9]+)\([^)]+\)[[:space:]]+\|'

  if [[ "$rc" -eq 0 ]]; then
    print_result_line "PASS" "-" "$lane/config users=${LOCUST_USERS:-1} spawn_rate=${LOCUST_SPAWN_RATE:-1} run_time=${LOCUST_RUN_TIME:-10s} server=${MCP_SERVER_ID:-${MCP_VIRTUAL_SERVER_ID:-$FAST_TIME_SERVER_ID}}"
  else
    print_result_line "FAIL" "-" "$lane/config users=${LOCUST_USERS:-1} spawn_rate=${LOCUST_SPAWN_RATE:-1} run_time=${LOCUST_RUN_TIME:-10s} server=${MCP_SERVER_ID:-${MCP_VIRTUAL_SERVER_ID:-$FAST_TIME_SERVER_ID}}"
  fi
  printed=1

  while IFS= read -r line; do
    if [[ "$line" =~ $locust_row_re ]]; then
      name="${BASH_REMATCH[1]}"
      reqs="${BASH_REMATCH[2]}"
      fails="${BASH_REMATCH[3]}"
      name="$(printf '%s' "$name" | sed -E 's/[[:space:]]+$//')"
      status="PASS"
      [[ "$fails" != "0" ]] && status="FAIL"
      print_result_line "$status" "-" "$lane/$name reqs=$reqs fails=$fails"
    fi
  done <"$lane_log"

  [[ $printed -eq 1 ]]
}

print_lane_results() {
  local lane="$1"
  local lane_log="$2"
  local rc="$3"

  case "$lane" in
    probe)
      print_probe_results "$lane_log"
      ;;
    smoke|locust)
      print_locust_results "$lane" "$lane_log" "$rc"
      ;;
    *)
      return 1
      ;;
  esac
}

usage() {
  cat <<EOF
Usage: $0 <command>

Commands:
  checkout       Clone/update cf-controlplane into $CF_CONTROLPLANE_DIR
  up             Fresh-bootstrap and start cf-controlplane + nginx + cf-dataplane + integration MCP backend
  up controlplane
                 Fresh-bootstrap and start stock cf-controlplane testing stack without cf-dataplane overlays
  down           Stop integration and control-plane-only stacks
  reset          Stop both stacks and remove volumes (fresh database)
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
                 with lane sections, per-test result rows, and full output logs;
                 CF_TEST_ALL_LOCUST=true appends the full locust load run
  test-all-up    Reset stack state (fresh database; CF_FRESH_STACK=false to keep it),
                 start the integration stack, then run test-all without locust
  test-all-up-load
                 Fresh-bootstrap the integration stack, then run test-all with full locust
  test-all-up-no-plugins
                 Same as test-all-up but deselects tests/live_gateway/plugins:
                 those suites need a gateway booted with a plugin enforce
                 config, which this stack does not run (CF_TEST_PLUGINS=false)
  controlplane-ps        Show stock cf-controlplane-only services
  controlplane-logs      Follow stock cf-controlplane-only logs
  controlplane-config    Render stock cf-controlplane-only compose config
  controlplane-live-core Run non-UI, non-SSO live gateway checks against controlplane-only stack
  controlplane-live-all  Run upstream tests/live_gateway against controlplane-only stack, including SSO/playwright
  controlplane-locust    Run upstream control-plane Locust file against controlplane-only stack
  controlplane-test-all  Run up controlplane, controlplane-live-core, and controlplane-locust with one log

MCP_VIRTUAL_SERVER_ID defaults to the auto-registered Fast Time server:
  $FAST_TIME_SERVER_ID

UI:
  http://localhost:\${NGINX_PORT:-8080}/admin
  admin@example.com / changeme

CF-dataplane image:
  $CF_DATAPLANE_IMAGE
  platform: $CF_DATAPLANE_PLATFORM

CF-controlplane image:
  $CF_CONTROLPLANE_IMAGE

Integration MCP backend URL to add in cf-controlplane UI:
  http://cf-integration-mcp-counter:5555/mcp

Fresh bootstrap:
  up and up controlplane reset compose volumes by default.
  Set CF_FRESH_STACK=false to keep existing database state.
EOF
}

ensure_checkout() {
  mkdir -p "$INTEGRATION_DIR"
  if [[ ! -d "$CF_CONTROLPLANE_DIR/.git" ]]; then
    git clone -q "$CF_CONTROLPLANE_REPO" "$CF_CONTROLPLANE_DIR"
  fi
  # --force: upstream occasionally re-points release tags; a scratch checkout
  # should follow them instead of aborting on "would clobber existing tag".
  # A failed fetch (e.g. offline) is tolerated when the ref already exists
  # locally; the checkout below still fails hard on a truly unknown ref.
  local fetched=1
  if ! git -C "$CF_CONTROLPLANE_DIR" fetch -q --prune --tags --force origin; then
    fetched=0
    echo "warning: fetch from $CF_CONTROLPLANE_REPO failed; using existing checkout" >&2
  fi
  git -C "$CF_CONTROLPLANE_DIR" checkout -q "$CF_CONTROLPLANE_REF"
  if [[ "$CF_CONTROLPLANE_REF" == "main" && $fetched -eq 1 ]]; then
    git -C "$CF_CONTROLPLANE_DIR" pull -q --ff-only origin main
  fi
}

compose() {
  docker compose "${compose_args[@]}" "$@"
}

controlplane_compose() {
  docker compose "${controlplane_compose_args[@]}" "$@"
}

remote_image_digest() {
  docker buildx imagetools inspect "$1" --format '{{.Manifest.Digest}}' 2>/dev/null
}

local_image_has_digest() {
  local image="$1"
  local digest="$2"

  docker image inspect "$image" --format '{{range .RepoDigests}}{{println .}}{{end}}' 2>/dev/null \
    | grep -Fq "@$digest"
}

pull_image_if_digest_changed() {
  local label="$1"
  local image="$2"
  local platform="${3:-}"
  local remote_digest

  print_detail "$label image: $image"
  if ! remote_digest="$(remote_image_digest "$image")" || [[ -z "$remote_digest" ]]; then
    if docker image inspect "$image" >/dev/null 2>&1; then
      print_detail "$label sha unavailable; nothing pulled: using local image"
      return 0
    fi
    print_detail "$label sha unavailable and local image missing; pulling"
    if [[ -n "$platform" ]]; then
      docker pull --platform "$platform" "$image"
    else
      docker pull "$image"
    fi
    return 0
  fi

  if local_image_has_digest "$image" "$remote_digest"; then
    print_detail "$label sha unchanged; nothing pulled: $remote_digest"
    return 0
  fi

  print_detail "$label sha changed or missing locally; pulling: $remote_digest"
  if [[ -n "$platform" ]]; then
    docker pull --platform "$platform" "$image"
  else
    docker pull "$image"
  fi
}

pull_controlplane_image_if_needed() {
  if [[ -z "$CF_CONTROLPLANE_IMAGE_WAS_SET" && -z "$IMAGE_LOCAL_WAS_SET" ]]; then
    print_detail "cf-controlplane image: $CF_CONTROLPLANE_IMAGE"
    print_detail "cf-controlplane sha check skipped; nothing pulled: default local build image"
  else
    pull_image_if_digest_changed "cf-controlplane" "$CF_CONTROLPLANE_IMAGE"
  fi
}

pull_stack_images() {
  pull_controlplane_image_if_needed
  pull_image_if_digest_changed "cf-dataplane" "$CF_DATAPLANE_IMAGE" "$CF_DATAPLANE_PLATFORM"
}

integration_stack_running() {
  docker ps --format '{{.Names}}' | grep -q "^${PROJECT}-"
}

controlplane_stack_running() {
  docker ps --format '{{.Names}}' | grep -q "^${CONTROLPLANE_PROJECT}-"
}

fresh_stack_enabled() {
  case "${CF_FRESH_STACK:-true}" in
    true|1) return 0 ;;
    *) return 1 ;;
  esac
}

ensure_no_integration_stack() {
  if integration_stack_running; then
    cat >&2 <<EOF
The $PROJECT dataplane integration stack is running and uses the same host ports.
Stop it first:
  $0 down
Then start the control-plane-only stack:
  $0 up controlplane
EOF
    return 2
  fi
}

ensure_no_controlplane_stack() {
  if controlplane_stack_running; then
    cat >&2 <<EOF
The $CONTROLPLANE_PROJECT control-plane-only stack is running and uses the same host ports.
Stop it first:
  $0 down
Then start the integration stack:
  $0 up
EOF
    return 2
  fi
}

remove_compose_project_by_label() {
  local project="$1"
  local remove_volumes="${2:-false}"
  local containers=()
  local networks=()
  local volumes=()
  local container_ids network_ids volume_ids

  container_ids="$(docker ps -aq --filter "label=com.docker.compose.project=$project")" || return $?
  if [[ -n "$container_ids" ]]; then
    mapfile -t containers <<<"$container_ids"
  fi
  if ((${#containers[@]} > 0)); then
    docker rm -f "${containers[@]}" >/dev/null
  fi

  network_ids="$(docker network ls -q --filter "label=com.docker.compose.project=$project")" || return $?
  if [[ -n "$network_ids" ]]; then
    mapfile -t networks <<<"$network_ids"
  fi
  if ((${#networks[@]} > 0)); then
    docker network rm "${networks[@]}" >/dev/null 2>&1 || true
  fi

  if [[ "$remove_volumes" == "true" ]]; then
    volume_ids="$(docker volume ls -q --filter "label=com.docker.compose.project=$project")" || return $?
    if [[ -n "$volume_ids" ]]; then
      mapfile -t volumes <<<"$volume_ids"
    fi
    if ((${#volumes[@]} > 0)); then
      docker volume rm "${volumes[@]}" >/dev/null
    fi
  fi
}

run_down() {
  local rc=0

  if [[ -f "$CF_CONTROLPLANE_DIR/docker-compose.yml" ]]; then
    compose "${all_stack_profiles[@]}" down --remove-orphans || rc=$?
    controlplane_compose "${all_stack_profiles[@]}" down --remove-orphans || rc=$?
  else
    print_detail "cf-controlplane checkout missing; stopping known compose projects by label."
  fi

  remove_compose_project_by_label "$PROJECT" false || rc=$?
  remove_compose_project_by_label "$CONTROLPLANE_PROJECT" false || rc=$?
  return "$rc"
}

run_reset() {
  local rc=0

  if [[ -f "$CF_CONTROLPLANE_DIR/docker-compose.yml" ]]; then
    compose "${all_stack_profiles[@]}" down --volumes --remove-orphans || rc=$?
    controlplane_compose "${all_stack_profiles[@]}" down --volumes --remove-orphans || rc=$?
  else
    print_detail "cf-controlplane checkout missing; stopping known compose projects by label."
  fi

  remove_compose_project_by_label "$PROJECT" true || rc=$?
  remove_compose_project_by_label "$CONTROLPLANE_PROJECT" true || rc=$?
  return "$rc"
}

bootstrap_fresh_database() {
  local target="$1"

  if fresh_stack_enabled; then
    print_section "Fresh database bootstrap"
    print_detail "Target: $target"
    print_detail "Command: $0 reset"
    run_reset
  else
    print_detail "CF_FRESH_STACK=false; keeping existing database state."
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

# Two-pass replacement for upstream's `make test-live-gateway`, which runs the
# whole tree under -p playwright: pytest-playwright's runtest hook breaks every
# pytest-asyncio test ("Runner.run() cannot be called from a running event
# loop"), drowning real regressions. Pass 1 runs the asyncio suites without
# the plugin; pass 2 runs the two playwright-dependent suites with it.
run_live_all() {
  ensure_checkout
  local rc=0
  local pass1_ignores=(
    --ignore=tests/live_gateway/sso
    --ignore=tests/live_gateway/mcp/test_mcp_rbac_transport.py
  )
  # The plugin E2E suites need a gateway booted with a plugin enforce
  # config; this stack runs without enabled plugins, so their failures are
  # expected. CF_TEST_PLUGINS=false deselects them (test-all-up-no-plugins).
  case "${CF_TEST_PLUGINS:-true}" in
    false|0) pass1_ignores+=(--ignore=tests/live_gateway/plugins) ;;
  esac
  (
    cd "$CF_CONTROLPLANE_DIR"
    uv run --extra plugins pytest -p no:playwright tests/live_gateway/ \
      "${pass1_ignores[@]}" \
      -v --tb=short
  ) || rc=$?
  (
    cd "$CF_CONTROLPLANE_DIR"
    uv run --extra plugins pytest -p playwright \
      tests/live_gateway/sso \
      tests/live_gateway/mcp/test_mcp_rbac_transport.py \
      -v --tb=short
  ) || rc=$?
  return "$rc"
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
  local started_at finished_at duration lane_log result_file summary summary_part
  local idx=0

  print_section "Dataplane integration test run"
  print_detail "Project: $PROJECT"
  print_detail "Base URL: $MCP_CLI_BASE_URL"
  print_detail "Dataplane image: $CF_DATAPLANE_IMAGE"
  print_detail "Full output log: $log_file"
  print_detail "Lanes: ${lanes[*]}"

  for lane in "${lanes[@]}"; do
    idx=$((idx + 1))
    lane_log="$(mktemp "$log_dir/cf-${lane}.XXXXXX.log")"
    result_file="$(mktemp "$log_dir/cf-${lane}-results.XXXXXX.tsv")"
    print_lane_header "$idx" "$total_lanes" "$lane" "$(lane_description "$lane")" "$0 $lane"
    started_at="$(date +%s)"
    rc=0
    if is_pytest_lane "$lane"; then
      PYTHONPATH="$ROOT/scripts${PYTHONPATH:+:$PYTHONPATH}" \
      PYTEST_ADDOPTS="${PYTEST_ADDOPTS:-} -rA -p cf_pytest_result_recorder" \
      CF_TEST_RESULT_FILE="$result_file" \
        "$0" "$lane" >"$lane_log" 2>&1 || rc=$?
    else
      "$0" "$lane" >"$lane_log" 2>&1 || rc=$?
    fi
    finished_at="$(date +%s)"
    duration=$((finished_at - started_at))
    summary="$(format_test_summary "$lane_log")"
    summary_part="${summary:+ - $summary}"

    {
      printf '===== BEGIN %s %s =====\n' "$lane" "$(date -u +%FT%TZ)"
      printf 'Command: %s %s\n' "$0" "$lane"
      printf 'Description: '
      lane_description "$lane"
      cat "$lane_log"
      if [[ -s "$result_file" ]]; then
        printf 'Recorded results:\n'
        cat "$result_file"
      fi
      printf '===== END %s =====\n\n' "$lane"
    } >>"$log_file"

    if [[ $rc -eq 0 ]]; then
      results+=("PASS $lane$summary_part")
      print_lane_results "$lane" "$lane_log" "$rc" || print_recorded_results "$result_file" || print_summary_results "$lane_log" || print_result_line "PASS" "${duration}s" "$lane"
      print_lane_summary "PASS" "$lane" "$summary_part" "$duration"
    else
      results+=("FAIL $lane exit=$rc$summary_part")
      print_lane_results "$lane" "$lane_log" "$rc" || print_recorded_results "$result_file" || print_summary_results "$lane_log" || print_result_line "FAIL" "${duration}s" "$lane"
      print_lane_summary "FAIL" "$lane" "$summary_part" "$duration"
      failed=1
    fi
    rm -f "$lane_log" "$result_file"
  done

  {
    echo "===== SUMMARY $(date -u +%FT%TZ) ====="
    printf '%s\n' "${results[@]}"
  } >>"$log_file"
  print_log_footer "$log_file"
  return "$failed"
}

wait_for_publisher_snapshot() {
  # On a fresh database the first publisher snapshot and the admin app warmup
  # land seconds after `up` returns; lanes that start immediately race them
  # (the scoped-token probe then lands on the control-plane fallback, which
  # denies tools/call). Wait until dataplane config exists in Redis.
  local timeout="${CF_PUBLISHER_WAIT_SECONDS:-90}"
  local deadline=$((SECONDS + timeout))
  print_detail "Waiting for a publisher snapshot containing the Fast Time virtual server (max ${timeout}s)..."
  # Key existence is not enough: the gateway's very first snapshot on a fresh
  # boot can run before the registration jobs finish and publish an empty
  # config (virtual_hosts = 0). Require the Fast Time vhost id inside the
  # payload so lanes start against real dataplane config.
  while ((SECONDS < deadline)); do
    if docker exec "${PROJECT}-redis-1" redis-cli EVAL \
        "for _,k in ipairs(redis.call('KEYS','*UserConfig*')) do if string.find(redis.call('GET',k), ARGV[1], 1, true) then return 1 end end return 0" \
        0 "$FAST_TIME_SERVER_ID" 2>/dev/null | grep -q '^1$'; then
      print_detail "Dataplane config with Fast Time server present in Redis."
      return 0
    fi
    sleep 2
  done
  print_detail "WARNING: Fast Time server not in dataplane config after ${timeout}s; lanes may hit the control-plane fallback."
  return 0
}

run_stack_up_for_test_all() {
  if fresh_stack_enabled; then
    print_section "Step 1/2: fresh-bootstrap and start integration stack"
  else
    print_section "Step 1/2: start or update integration stack (CF_FRESH_STACK=false)"
  fi
  print_detail "Command: $0 up"
  "$0" up
  wait_for_publisher_snapshot
}

run_test_all_up() {
  run_stack_up_for_test_all
  print_section "Step 2/2: run report lanes without full locust"
  print_detail "Command: CF_TEST_ALL_LOCUST=false $0 test-all"
  CF_TEST_ALL_LOCUST=false "$0" test-all
}

run_test_all_up_load() {
  run_stack_up_for_test_all
  print_section "Step 2/2: run report lanes with full locust"
  print_detail "Command: CF_TEST_ALL_LOCUST=true $0 test-all"
  CF_TEST_ALL_LOCUST=true "$0" test-all
}

run_test_all_up_no_plugins() {
  run_stack_up_for_test_all
  print_section "Step 2/2: run report lanes without locust and without plugin suites"
  print_detail "Command: CF_TEST_ALL_LOCUST=false CF_TEST_PLUGINS=false $0 test-all"
  CF_TEST_ALL_LOCUST=false CF_TEST_PLUGINS=false "$0" test-all
}

run_integration_up() {
  ensure_checkout
  bootstrap_fresh_database "integration stack"
  ensure_no_controlplane_stack
  local up_args=(-d)
  if [[ "$CF_COMPOSE_BUILD" == "true" || "$CF_COMPOSE_BUILD" == "1" ]]; then
    up_args+=(--build)
  fi
  pull_stack_images
  compose up "${up_args[@]}"
  print_info_box "Integration stack started." "$(cat <<EOF
UI: http://localhost:${NGINX_PORT:-8080}/admin
Login: admin@example.com / changeme
CF-controlplane image: $CF_CONTROLPLANE_IMAGE
CF-dataplane image: $CF_DATAPLANE_IMAGE
CF-dataplane platform: $CF_DATAPLANE_PLATFORM
EOF
)"
}

run_controlplane_up() {
  ensure_checkout
  bootstrap_fresh_database "control-plane-only stack"
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

  pull_controlplane_image_if_needed
  controlplane_compose "${up_args[@]}"
  cat <<EOF
Control-plane-only stack started.
Project: $CONTROLPLANE_PROJECT
UI: http://localhost:${NGINX_PORT:-8080}/admin
Login: admin@example.com / changeme
CF-controlplane image: $CF_CONTROLPLANE_IMAGE
No cf-dataplane service, no dataplane nginx routing override, no DATAPLANE_PUBLISHER overlay.

Run:
  $0 down
  $0 controlplane-live-core
  $0 controlplane-live-all
  $0 controlplane-locust
  $0 controlplane-test-all
EOF
}

run_up() {
  local target="${1:-integration}"

  if [[ $# -gt 1 ]]; then
    printf 'Usage: %s up [controlplane]\n' "$0" >&2
    return 2
  fi

  case "$target" in
    integration|dataplane)
      run_integration_up
      ;;
    controlplane|control-plane|baseline)
      run_controlplane_up
      ;;
    *)
      printf 'Unknown up target: %s\n\n' "$target" >&2
      usage >&2
      return 2
      ;;
  esac
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
  local lanes=("up controlplane" controlplane-live-core controlplane-locust)
  local results=() rc lane failed=0

  for lane in "${lanes[@]}"; do
    echo "Running $lane..."
    printf '===== BEGIN %s %s =====\n' "$lane" "$(date -u +%FT%TZ)" >>"$log_file"
    rc=0
    case "$lane" in
      "up controlplane")
        "$0" up controlplane >>"$log_file" 2>&1 || rc=$?
        ;;
      *)
        "$0" "$lane" >>"$log_file" 2>&1 || rc=$?
        ;;
    esac
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
    shift
    run_up "$@"
    ;;
  down)
    run_down
    ;;
  reset)
    run_reset
    ;;
  ps)
    compose ps
    ;;
  logs)
    shift
    if [[ $# -eq 0 ]]; then
      compose logs -f
    else
      services=()
      while IFS= read -r service; do
        services+=("$service")
      done < <(map_compose_services "$@")
      compose logs -f "${services[@]}"
    fi
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
    run_live_all
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
  test-all-up-no-plugins)
    run_test_all_up_no_plugins
    ;;
  controlplane-up)
    # Backward-compatible alias; prefer: up controlplane.
    run_controlplane_up
    ;;
  controlplane-down)
    # Backward-compatible alias; prefer: down.
    run_down
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
