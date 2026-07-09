#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

load_env_file() {
  local env_file="$ROOT/.env"
  local line assignment key value

  [[ -f "$env_file" ]] || return 0

  while IFS= read -r line || [[ -n "$line" ]]; do
    line="${line#"${line%%[![:space:]]*}"}"
    case "$line" in
      ""|\#*) continue ;;
      export\ *) assignment="${line#export }" ;;
      *) assignment="$line" ;;
    esac

    case "$assignment" in
      *=*) ;;
      *)
        printf 'warning: ignoring invalid .env line: %s\n' "$line" >&2
        continue
        ;;
    esac

    key="${assignment%%=*}"
    value="${assignment#*=}"
    if [[ ! "$key" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
      printf 'warning: ignoring invalid .env key: %s\n' "$key" >&2
      continue
    fi
    if [[ -n "${!key+x}" ]]; then
      continue
    fi

    case "$value" in
      \"*\") value="${value#\"}"; value="${value%\"}" ;;
      \'*\') value="${value#\'}"; value="${value%\'}" ;;
    esac
    export "$key=$value"
    export "CF_ENV_FILE_${key}=1"
  done <"$env_file"
}

load_env_file

default_dataplane_image() {
  if [[ -n "${CF_DATAPLANE_REF:-}" ]]; then
    printf '%s\n' "${CF_DATAPLANE_LOCAL_IMAGE:-contextforge-gateway-rs/contextforge-gateway-rs:local}"
  else
    # GHCR currently publishes only 0.1.0 (no latest tag); keep the default pinned.
    printf 'ghcr.io/contextforge-gateway-rs/contextforge-gateway-rs:%s\n' "${CF_DATAPLANE_VERSION:-0.1.0}"
  fi
}

default_controlplane_image() {
  printf 'mcpgateway/mcpgateway:%s\n' "${CF_CONTROLPLANE_VERSION:-latest}"
}

docker_server_platform() {
  local platform

  platform="$(docker version --format '{{.Server.Os}}/{{.Server.Arch}}' 2>/dev/null || true)"
  if [[ -n "$platform" ]]; then
    printf '%s\n' "$platform"
  else
    printf 'linux/amd64\n'
  fi
}

default_dataplane_platform() {
  if [[ -n "${CF_DATAPLANE_REF:-}" ]]; then
    docker_server_platform
  else
    printf 'linux/amd64\n'
  fi
}

absolute_path() {
  case "$1" in
    /*) printf '%s\n' "$1" ;;
    *) printf '%s/%s\n' "$ROOT" "$1" ;;
  esac
}

INTEGRATION_DIR="$(absolute_path "${CF_INTEGRATION_DIR:-"$ROOT/.integration"}")"
CF_CONTROLPLANE_DIR="$(absolute_path "${CF_CONTROLPLANE_DIR:-"$INTEGRATION_DIR/mcp-context-forge"}")"
CF_CONTROLPLANE_REPO="${CF_CONTROLPLANE_REPO:-https://github.com/IBM/mcp-context-forge.git}"
CF_CONTROLPLANE_REF="${CF_CONTROLPLANE_REF:-main}"
CF_DATAPLANE_DIR="$(absolute_path "${CF_DATAPLANE_DIR:-"$INTEGRATION_DIR/contextforge-gateway-rs"}")"
CF_DATAPLANE_REPO="${CF_DATAPLANE_REPO:-https://github.com/contextforge-gateway-rs/contextforge-gateway-rs.git}"
CF_DATAPLANE_REF="${CF_DATAPLANE_REF:-}"
PROJECT="${CF_INTEGRATION_PROJECT:-cf-integration}"
CONTROLPLANE_PROJECT="${CF_CONTROLPLANE_PROJECT:-cf-controlplane-only}"
JWT_SECRET_KEY="${JWT_SECRET_KEY:-my-test-key-but-now-longer-than-32-bytes}"
ADMIN_SUBJECT="${MCP_JWT_SUBJECT:-admin@example.com}"
if [[ -n "${CF_CONTROLPLANE_IMAGE+x}" && "${CF_INTERNAL_CONTROLPLANE_IMAGE:-}" != "1" ]]; then
  CF_CONTROLPLANE_IMAGE_WAS_SET=1
else
  CF_CONTROLPLANE_IMAGE_WAS_SET=""
fi
if [[ -n "${IMAGE_LOCAL+x}" && "${CF_INTERNAL_IMAGE_LOCAL:-}" != "1" ]]; then
  IMAGE_LOCAL_WAS_SET=1
else
  IMAGE_LOCAL_WAS_SET=""
fi
if [[ -n "${CF_DATAPLANE_IMAGE+x}" && "${CF_INTERNAL_DATAPLANE_IMAGE:-}" != "1" ]]; then
  CF_DATAPLANE_IMAGE_WAS_SET=1
else
  CF_DATAPLANE_IMAGE_WAS_SET=""
fi
CF_CONTROLPLANE_IMAGE="${CF_CONTROLPLANE_IMAGE:-${IMAGE_LOCAL:-$(default_controlplane_image)}}"
CF_DATAPLANE_IMAGE="${CF_DATAPLANE_IMAGE:-$(default_dataplane_image)}"
CF_DATAPLANE_PLATFORM="${CF_DATAPLANE_PLATFORM:-auto}"
if [[ "$CF_DATAPLANE_PLATFORM" == "auto" ]]; then
  CF_DATAPLANE_PLATFORM="$(default_dataplane_platform)"
fi
CF_COMPOSE_BUILD="${CF_COMPOSE_BUILD:-auto}"
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
if [[ -z "$CF_CONTROLPLANE_IMAGE_WAS_SET" ]]; then
  export CF_INTERNAL_CONTROLPLANE_IMAGE=1
fi
export CF_DATAPLANE_IMAGE
if [[ -z "$CF_DATAPLANE_IMAGE_WAS_SET" ]]; then
  export CF_INTERNAL_DATAPLANE_IMAGE=1
fi
export CF_DATAPLANE_PLATFORM
export CF_CONTROLPLANE_DIR
export CF_DATAPLANE_DIR
export CF_INTEGRATION_DIR="$INTEGRATION_DIR"
export IMAGE_LOCAL="$CF_CONTROLPLANE_IMAGE"
if [[ -z "$IMAGE_LOCAL_WAS_SET" ]]; then
  export CF_INTERNAL_IMAGE_LOCAL=1
fi
export JWT_SECRET_KEY
export MCP_CLI_BASE_URL="${MCP_CLI_BASE_URL:-http://127.0.0.1:${NGINX_PORT:-8080}}"
export PLATFORM_ADMIN_EMAIL="${PLATFORM_ADMIN_EMAIL:-$ADMIN_SUBJECT}"
export KEY_FILE_PASSWORD="${KEY_FILE_PASSWORD:-}"

compose_args=(
  -p "$PROJECT"
  -f "$CF_CONTROLPLANE_DIR/docker-compose.yml"
  -f "$ROOT/docker/docker-compose.cf-controlplane-build-labels.yaml"
  -f "$ROOT/docker/docker-compose.cf-dataplane.yaml"
  -f "$ROOT/docker/docker-compose.cf-integration.yaml"
)

if [[ -n "$CF_DATAPLANE_REF" ]]; then
  compose_args+=(-f "$ROOT/docker/docker-compose.cf-dataplane-build.yaml")
fi

controlplane_compose_args=(
  -p "$CONTROLPLANE_PROJECT"
  -f "$CF_CONTROLPLANE_DIR/docker-compose.yml"
  -f "$ROOT/docker/docker-compose.cf-controlplane-build-labels.yaml"
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

env_file_set() {
  local marker="CF_ENV_FILE_$1"
  [[ "${!marker:-}" == "1" ]]
}

env_value_for_lane() {
  local lane="$1"
  local key="$2"
  local default_value="$3"

  if [[ "$lane" == "smoke" ]] && { [[ -z "${!key+x}" ]] || env_file_set "$key"; }; then
    printf '%s\n' "$default_value"
  else
    printf '%s\n' "${!key:-$default_value}"
  fi
}

set_default_if_unset_or_env_file() {
  local key="$1"
  local value="$2"

  if [[ -z "${!key+x}" ]] || env_file_set "$key"; then
    export "$key=$value"
  fi
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
Settings: users=$(env_value_for_lane smoke LOCUST_USERS 1), spawn_rate=$(env_value_for_lane smoke LOCUST_SPAWN_RATE 1), run_time=$(env_value_for_lane smoke LOCUST_RUN_TIME 10s). Flow: initialize, tools/list, ping, tools/call.
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
    PASS|XFAIL|XPASS)
      printf '    %s%-5s%s [%7s] %s\n' "$green" "$status" "$reset" "$duration" "$name"
      ;;
    FAIL|ERROR)
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
  local printed=0 line name reqs fails status users spawn_rate run_time
  local locust_row_re='^POST[[:space:]]+(.+)[[:space:]]+([0-9]+)[[:space:]]+([0-9]+)\([^)]+\)[[:space:]]+\|'

  users="$(env_value_for_lane "$lane" LOCUST_USERS 1)"
  spawn_rate="$(env_value_for_lane "$lane" LOCUST_SPAWN_RATE 1)"
  run_time="$(env_value_for_lane "$lane" LOCUST_RUN_TIME 10s)"
  if [[ "$lane" == "locust" ]]; then
    users="$(env_value_for_lane "$lane" LOCUST_USERS 100)"
    spawn_rate="$(env_value_for_lane "$lane" LOCUST_SPAWN_RATE 10)"
    run_time="$(env_value_for_lane "$lane" LOCUST_RUN_TIME 5m)"
  fi

  if [[ "$rc" -eq 0 ]]; then
    print_result_line "PASS" "-" "$lane/config users=$users spawn_rate=$spawn_rate run_time=$run_time server=${MCP_SERVER_ID:-${MCP_VIRTUAL_SERVER_ID:-$FAST_TIME_SERVER_ID}}"
  else
    print_result_line "FAIL" "-" "$lane/config users=$users spawn_rate=$spawn_rate run_time=$run_time server=${MCP_SERVER_ID:-${MCP_VIRTUAL_SERVER_ID:-$FAST_TIME_SERVER_ID}}"
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
  checkout       Clone/update cf-controlplane and configured cf-dataplane source checkouts
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
  source ref: ${CF_DATAPLANE_REF:-published image mode}

CF-controlplane image:
  $CF_CONTROLPLANE_IMAGE

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

dataplane_source_enabled() {
  [[ -n "$CF_DATAPLANE_REF" ]]
}

ensure_dataplane_checkout() {
  dataplane_source_enabled || return 0

  mkdir -p "$INTEGRATION_DIR"
  if [[ ! -d "$CF_DATAPLANE_DIR/.git" ]]; then
    git clone -q "$CF_DATAPLANE_REPO" "$CF_DATAPLANE_DIR"
  fi
  local fetched=1
  if ! git -C "$CF_DATAPLANE_DIR" fetch -q --prune --tags --force origin; then
    fetched=0
    echo "warning: fetch from $CF_DATAPLANE_REPO failed; using existing dataplane checkout" >&2
  fi
  git -C "$CF_DATAPLANE_DIR" checkout -q "$CF_DATAPLANE_REF"
  if [[ "$CF_DATAPLANE_REF" == "main" && $fetched -eq 1 ]]; then
    git -C "$CF_DATAPLANE_DIR" pull -q --ff-only origin main
  fi
}

ensure_source_checkouts() {
  ensure_checkout
  ensure_dataplane_checkout
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
  if dataplane_source_enabled; then
    print_detail "cf-dataplane image: $CF_DATAPLANE_IMAGE"
    print_detail "cf-dataplane sha check skipped; source checkout build image"
  else
    pull_image_if_digest_changed "cf-dataplane" "$CF_DATAPLANE_IMAGE" "$CF_DATAPLANE_PLATFORM"
  fi
}

integration_stack_running() {
  docker ps --format '{{.Names}}' | grep -q "^${PROJECT}-"
}

compose_service_container_id() {
  local project="$1"
  local service="$2"

  docker ps -q \
    --filter "label=com.docker.compose.project=$project" \
    --filter "label=com.docker.compose.service=$service" \
    | head -n 1
}

compose_service_container_id_all() {
  local project="$1"
  local service="$2"

  docker ps -aq \
    --filter "label=com.docker.compose.project=$project" \
    --filter "label=com.docker.compose.service=$service" \
    | head -n 1
}

compose_service_uses_image() {
  local project="$1"
  local service="$2"
  local expected_image="$3"
  local container_id config_image running_image_id expected_image_id

  container_id="$(compose_service_container_id "$project" "$service")"
  [[ -n "$container_id" ]] || return 1

  config_image="$(docker inspect "$container_id" --format '{{.Config.Image}}' 2>/dev/null || true)"
  [[ "$config_image" == "$expected_image" ]] || return 1

  running_image_id="$(docker inspect "$container_id" --format '{{.Image}}' 2>/dev/null || true)"
  expected_image_id="$(docker image inspect "$expected_image" --format '{{.Id}}' 2>/dev/null || true)"
  [[ -n "$running_image_id" && "$running_image_id" == "$expected_image_id" ]]
}

compose_service_image_label() {
  local project="$1"
  local service="$2"
  local label="$3"
  local container_id

  container_id="$(compose_service_container_id "$project" "$service")"
  [[ -n "$container_id" ]] || return 1

  docker inspect "$container_id" --format "{{ index .Config.Labels \"$label\" }}" 2>/dev/null || true
}

compose_service_completed_successfully() {
  local project="$1"
  local service="$2"
  local container_id status exit_code

  container_id="$(compose_service_container_id_all "$project" "$service")"
  [[ -n "$container_id" ]] || return 1

  status="$(docker inspect "$container_id" --format '{{.State.Status}}' 2>/dev/null || true)"
  exit_code="$(docker inspect "$container_id" --format '{{.State.ExitCode}}' 2>/dev/null || true)"
  [[ "$status" == "exited" && "$exit_code" == "0" ]]
}

integration_stack_current() {
  local service checkout_head image_revision
  local required_running_services=(
    gateway
    cf-dataplane
    nginx
    postgres
    pgbouncer
    redis
    fast_time_server
    fast_test_server
  )
  local required_completed_services=(
    migration
    register_fast_time
    register_fast_time_sse
    register_fast_test
  )

  for service in "${required_running_services[@]}"; do
    if [[ -z "$(compose_service_container_id "$PROJECT" "$service")" ]]; then
      print_detail "Integration stack not current; service is not running: $service"
      return 1
    fi
  done

  for service in "${required_completed_services[@]}"; do
    if ! compose_service_completed_successfully "$PROJECT" "$service"; then
      print_detail "Integration stack not current; setup service did not complete successfully: $service"
      return 1
    fi
  done

  if ! compose_service_uses_image "$PROJECT" gateway "$CF_CONTROLPLANE_IMAGE"; then
    print_detail "Integration stack not current; cf-controlplane image differs."
    return 1
  fi

  if ! compose_service_uses_image "$PROJECT" cf-dataplane "$CF_DATAPLANE_IMAGE"; then
    print_detail "Integration stack not current; cf-dataplane image differs."
    return 1
  fi

  if [[ -z "$CF_CONTROLPLANE_IMAGE_WAS_SET" && -z "$IMAGE_LOCAL_WAS_SET" ]]; then
    checkout_head="$(git -C "$CF_CONTROLPLANE_DIR" rev-parse HEAD)"
    image_revision="$(compose_service_image_label "$PROJECT" gateway "org.opencontainers.image.revision")"
    if [[ "$image_revision" != "$checkout_head" ]]; then
      print_detail "Integration stack not current; cf-controlplane branch revision differs."
      return 1
    fi
  fi

  if dataplane_source_enabled; then
    checkout_head="$(git -C "$CF_DATAPLANE_DIR" rev-parse HEAD)"
    image_revision="$(compose_service_image_label "$PROJECT" cf-dataplane "org.opencontainers.image.revision")"
    if [[ "$image_revision" != "$checkout_head" ]]; then
      print_detail "Integration stack not current; cf-dataplane branch revision differs."
      return 1
    fi
  fi

  return 0
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

force_fresh_stack_enabled() {
  case "${CF_FORCE_FRESH_STACK:-false}" in
    true|1) return 0 ;;
    *) return 1 ;;
  esac
}

compose_build_enabled() {
  case "$CF_COMPOSE_BUILD" in
    true|1) return 0 ;;
    *) return 1 ;;
  esac
}

short_revision() {
  local revision="$1"

  if [[ -z "$revision" ]]; then
    printf 'unknown\n'
  else
    printf '%s\n' "${revision:0:12}"
  fi
}

controlplane_checkout_summary() {
  local branch revision

  branch="$(git -C "$CF_CONTROLPLANE_DIR" symbolic-ref --quiet --short HEAD 2>/dev/null || true)"
  revision="$(git -C "$CF_CONTROLPLANE_DIR" rev-parse HEAD 2>/dev/null || true)"

  if [[ -n "$branch" ]]; then
    printf '%s @ %s\n' "$branch" "$(short_revision "$revision")"
  else
    printf 'detached @ %s\n' "$(short_revision "$revision")"
  fi
}

export_controlplane_checkout_env() {
  local branch revision

  branch="$(git -C "$CF_CONTROLPLANE_DIR" symbolic-ref --quiet --short HEAD 2>/dev/null || true)"
  revision="$(git -C "$CF_CONTROLPLANE_DIR" rev-parse HEAD 2>/dev/null || true)"
  export CF_CONTROLPLANE_CHECKOUT_REVISION="$revision"
  export CF_CONTROLPLANE_CHECKOUT_REF="${branch:-$CF_CONTROLPLANE_REF}"
}

dataplane_checkout_summary() {
  local branch revision

  if ! dataplane_source_enabled; then
    printf 'disabled; published image mode\n'
    return 0
  fi

  branch="$(git -C "$CF_DATAPLANE_DIR" symbolic-ref --quiet --short HEAD 2>/dev/null || true)"
  revision="$(git -C "$CF_DATAPLANE_DIR" rev-parse HEAD 2>/dev/null || true)"

  if [[ -n "$branch" ]]; then
    printf '%s @ %s\n' "$branch" "$(short_revision "$revision")"
  else
    printf 'detached @ %s\n' "$(short_revision "$revision")"
  fi
}

export_dataplane_checkout_env() {
  local branch revision

  dataplane_source_enabled || return 0

  branch="$(git -C "$CF_DATAPLANE_DIR" symbolic-ref --quiet --short HEAD 2>/dev/null || true)"
  revision="$(git -C "$CF_DATAPLANE_DIR" rev-parse HEAD 2>/dev/null || true)"
  export CF_DATAPLANE_CHECKOUT_REVISION="$revision"
  export CF_DATAPLANE_CHECKOUT_REF="${branch:-$CF_DATAPLANE_REF}"
}

controlplane_runtime_summary() {
  local project="$1"
  local container_id image image_ref revision version checkout_head revision_status

  checkout_head="$(git -C "$CF_CONTROLPLANE_DIR" rev-parse HEAD 2>/dev/null || true)"
  container_id="$(docker ps -q \
    --filter "label=com.docker.compose.project=$project" \
    --filter "label=com.docker.compose.service=gateway" \
    | head -n 1)"

  if [[ -z "$container_id" ]]; then
    printf 'CF-controlplane runtime: gateway container not running\n'
    return 0
  fi

  image="$(docker inspect "$container_id" --format '{{.Config.Image}}' 2>/dev/null || true)"
  revision="$(docker inspect "$container_id" --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}' 2>/dev/null || true)"
  image_ref="$(docker inspect "$container_id" --format '{{ index .Config.Labels "org.opencontainers.image.ref.name" }}' 2>/dev/null || true)"
  version="$(docker inspect "$container_id" --format '{{ index .Config.Labels "org.opencontainers.image.version" }}' 2>/dev/null || true)"

  if [[ -z "$revision" ]]; then
    revision_status="unknown; image has no revision label"
  elif [[ "$revision" == "$checkout_head" ]]; then
    revision_status="matches checkout"
  else
    revision_status="MISMATCH; checkout $(short_revision "$checkout_head")"
  fi

  printf 'CF-controlplane checkout: %s\n' "$(controlplane_checkout_summary)"
  printf 'CF-controlplane image: %s\n' "${image:-unknown}"
  if [[ -n "$image_ref" ]]; then
    printf 'CF-controlplane image ref: %s\n' "$image_ref"
  fi
  printf 'CF-controlplane image revision: %s (%s)\n' "$(short_revision "$revision")" "$revision_status"
  if [[ -n "$version" ]]; then
    printf 'CF-controlplane image version: %s\n' "$version"
  fi
  printf 'CF_COMPOSE_BUILD resolved: %s\n' "$CF_COMPOSE_BUILD"
}

dataplane_runtime_summary() {
  local project="$1"
  local container_id image image_ref revision version checkout_head revision_status

  container_id="$(docker ps -q \
    --filter "label=com.docker.compose.project=$project" \
    --filter "label=com.docker.compose.service=cf-dataplane" \
    | head -n 1)"

  if [[ -z "$container_id" ]]; then
    printf 'CF-dataplane runtime: cf-dataplane container not running\n'
    return 0
  fi

  image="$(docker inspect "$container_id" --format '{{.Config.Image}}' 2>/dev/null || true)"
  revision="$(docker inspect "$container_id" --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}' 2>/dev/null || true)"
  image_ref="$(docker inspect "$container_id" --format '{{ index .Config.Labels "org.opencontainers.image.ref.name" }}' 2>/dev/null || true)"
  version="$(docker inspect "$container_id" --format '{{ index .Config.Labels "org.opencontainers.image.version" }}' 2>/dev/null || true)"

  if dataplane_source_enabled; then
    checkout_head="$(git -C "$CF_DATAPLANE_DIR" rev-parse HEAD 2>/dev/null || true)"
    if [[ -z "$revision" ]]; then
      revision_status="unknown; image has no revision label"
    elif [[ "$revision" == "$checkout_head" ]]; then
      revision_status="matches checkout"
    else
      revision_status="MISMATCH; checkout $(short_revision "$checkout_head")"
    fi
  else
    revision_status="published image mode"
  fi

  printf 'CF-dataplane checkout: %s\n' "$(dataplane_checkout_summary)"
  printf 'CF-dataplane image: %s\n' "${image:-unknown}"
  printf 'CF-dataplane platform: %s\n' "$CF_DATAPLANE_PLATFORM"
  if [[ -n "$image_ref" ]]; then
    printf 'CF-dataplane image ref: %s\n' "$image_ref"
  fi
  if [[ -n "$revision" || dataplane_source_enabled ]]; then
    printf 'CF-dataplane image revision: %s (%s)\n' "$(short_revision "$revision")" "$revision_status"
  fi
  if [[ -n "$version" ]]; then
    printf 'CF-dataplane image version: %s\n' "$version"
  fi
}

resolve_compose_build_mode() {
  local include_dataplane="${1:-false}"
  local checkout_head image_revision

  export_controlplane_checkout_env
  if [[ "$include_dataplane" == "true" ]]; then
    export_dataplane_checkout_env
  fi

  case "$CF_COMPOSE_BUILD" in
    true|1|false|0) return 0 ;;
    auto) ;;
    *)
      printf 'Invalid CF_COMPOSE_BUILD=%s; use auto, true, or false.\n' "$CF_COMPOSE_BUILD" >&2
      return 2
      ;;
  esac

  if [[ -n "$CF_CONTROLPLANE_IMAGE_WAS_SET" || -n "$IMAGE_LOCAL_WAS_SET" ]]; then
    print_detail "CF_COMPOSE_BUILD=auto; explicit cf-controlplane image set, build disabled."
  else
    checkout_head="$(git -C "$CF_CONTROLPLANE_DIR" rev-parse HEAD)"
    if ! docker image inspect "$CF_CONTROLPLANE_IMAGE" >/dev/null 2>&1; then
      CF_COMPOSE_BUILD=true
      print_detail "CF_COMPOSE_BUILD=auto; cf-controlplane image missing, build enabled: $CF_CONTROLPLANE_IMAGE"
    else
      image_revision="$(docker image inspect "$CF_CONTROLPLANE_IMAGE" \
        --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}' 2>/dev/null || true)"
      if [[ "$image_revision" == "$checkout_head" ]]; then
        print_detail "CF_COMPOSE_BUILD=auto; cf-controlplane image matches checkout, build disabled: $checkout_head"
      else
        CF_COMPOSE_BUILD=true
        print_detail "CF_COMPOSE_BUILD=auto; cf-controlplane image is stale, build enabled: image=${image_revision:-unknown} checkout=$checkout_head"
      fi
    fi
  fi

  if [[ "$include_dataplane" == "true" ]] && dataplane_source_enabled; then
    checkout_head="$(git -C "$CF_DATAPLANE_DIR" rev-parse HEAD)"
    if ! docker image inspect "$CF_DATAPLANE_IMAGE" >/dev/null 2>&1; then
      CF_COMPOSE_BUILD=true
      print_detail "CF_COMPOSE_BUILD=auto; cf-dataplane image missing, build enabled: $CF_DATAPLANE_IMAGE"
    else
      image_revision="$(docker image inspect "$CF_DATAPLANE_IMAGE" \
        --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}' 2>/dev/null || true)"
      if [[ "$image_revision" == "$checkout_head" ]]; then
        print_detail "CF_COMPOSE_BUILD=auto; cf-dataplane image matches checkout, build disabled: $checkout_head"
      else
        CF_COMPOSE_BUILD=true
        print_detail "CF_COMPOSE_BUILD=auto; cf-dataplane image is stale, build enabled: image=${image_revision:-unknown} checkout=$checkout_head"
      fi
    fi
  fi

  if [[ "$CF_COMPOSE_BUILD" == "auto" ]]; then
    CF_COMPOSE_BUILD=false
  fi
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
  local container_ids network_ids volume_ids resource_id

  container_ids="$(docker ps -aq --filter "label=com.docker.compose.project=$project")" || return $?
  if [[ -n "$container_ids" ]]; then
    while IFS= read -r resource_id; do
      [[ -n "$resource_id" ]] || continue
      docker rm -f "$resource_id" >/dev/null
    done <<<"$container_ids"
  fi

  network_ids="$(docker network ls -q --filter "label=com.docker.compose.project=$project")" || return $?
  if [[ -n "$network_ids" ]]; then
    while IFS= read -r resource_id; do
      [[ -n "$resource_id" ]] || continue
      docker network rm "$resource_id" >/dev/null 2>&1 || true
    done <<<"$network_ids"
  fi

  if [[ "$remove_volumes" == "true" ]]; then
    volume_ids="$(docker volume ls -q --filter "label=com.docker.compose.project=$project")" || return $?
    if [[ -n "$volume_ids" ]]; then
      while IFS= read -r resource_id; do
        [[ -n "$resource_id" ]] || continue
        docker volume rm "$resource_id" >/dev/null
      done <<<"$volume_ids"
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
  log_dir="$(cd "$log_dir" && pwd)"
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
    print_detail "Command: CF_FORCE_FRESH_STACK=true $0 up"
  else
    print_section "Step 1/2: start or update integration stack (CF_FRESH_STACK=false)"
    print_detail "Command: $0 up"
  fi
  if fresh_stack_enabled; then
    CF_FORCE_FRESH_STACK=true "$0" up
  else
    "$0" up
  fi
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
  ensure_source_checkouts
  resolve_compose_build_mode true
  local up_args=(-d)
  if compose_build_enabled; then
    up_args+=(--build)
  fi
  pull_stack_images
  if ! force_fresh_stack_enabled && ! compose_build_enabled && integration_stack_current; then
    print_detail "Integration stack already current; skipping docker compose up."
    print_info_box "Integration stack already current." "$(cat <<EOF
UI: http://localhost:${NGINX_PORT:-8080}/admin
Login: admin@example.com / changeme
$(controlplane_runtime_summary "$PROJECT")
$(dataplane_runtime_summary "$PROJECT")
EOF
)"
    return 0
  fi
  bootstrap_fresh_database "integration stack"
  ensure_no_controlplane_stack
  compose up "${up_args[@]}"
  print_info_box "Integration stack started." "$(cat <<EOF
UI: http://localhost:${NGINX_PORT:-8080}/admin
Login: admin@example.com / changeme
$(controlplane_runtime_summary "$PROJECT")
$(dataplane_runtime_summary "$PROJECT")
EOF
)"
}

run_controlplane_up() {
  ensure_checkout
  bootstrap_fresh_database "control-plane-only stack"
  ensure_no_integration_stack
  resolve_compose_build_mode
  mkdir -p "$CF_CONTROLPLANE_DIR/reports"
  export HOST_UID="${HOST_UID:-$(id -u 2>/dev/null || echo 1000)}"
  export HOST_GID="${HOST_GID:-$(id -g 2>/dev/null || echo 1000)}"
  export LOCUST_EXPECT_WORKERS="${LOCUST_EXPECT_WORKERS:-${CONTROLPLANE_LOCUST_WORKERS:-1}}"
  export PASSWORD_CHANGE_ENFORCEMENT_ENABLED="${PASSWORD_CHANGE_ENFORCEMENT_ENABLED:-false}"
  export ADMIN_REQUIRE_PASSWORD_CHANGE_ON_BOOTSTRAP="${ADMIN_REQUIRE_PASSWORD_CHANGE_ON_BOOTSTRAP:-false}"
  export REQUIRE_PASSWORD_CHANGE_FOR_DEFAULT_PASSWORD="${REQUIRE_PASSWORD_CHANGE_FOR_DEFAULT_PASSWORD:-false}"

  local up_args=("${controlplane_profiles[@]}" up -d)
  if compose_build_enabled; then
    up_args+=(--build)
  fi
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
$(controlplane_runtime_summary "$CONTROLPLANE_PROJECT")
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
    ensure_source_checkouts
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
    ensure_source_checkouts
    export_controlplane_checkout_env
    export_dataplane_checkout_env
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
    set_default_if_unset_or_env_file LOCUST_USERS 1
    set_default_if_unset_or_env_file LOCUST_SPAWN_RATE 1
    set_default_if_unset_or_env_file LOCUST_RUN_TIME 10s
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
    export_controlplane_checkout_env
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
