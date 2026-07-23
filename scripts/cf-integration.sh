#!/usr/bin/env bash
set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

load_env_file() {
  local env_file="$ROOT/.env" line assignment key value
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
      *) printf 'warning: ignoring invalid .env line: %s\n' "$line" >&2; continue ;;
    esac
    key="${assignment%%=*}"
    value="${assignment#*=}"
    if [[ ! "$key" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]]; then
      printf 'warning: ignoring invalid .env key: %s\n' "$key" >&2
      continue
    fi
    [[ -n "${!key+x}" ]] && continue
    case "$value" in
      \"*\") value="${value#\"}"; value="${value%\"}" ;;
      \'*\') value="${value#\'}"; value="${value%\'}" ;;
    esac
    export "$key=$value"
  done <"$env_file"
}

absolute_path() {
  case "$1" in
    /*) printf '%s\n' "$1" ;;
    *) printf '%s/%s\n' "$ROOT" "$1" ;;
  esac
}

truthy() {
  case "${1:-}" in true|TRUE|1|yes|YES|on|ON) return 0 ;; *) return 1 ;; esac
}

load_env_file

INTEGRATION_DIR="$(absolute_path "${CF_INTEGRATION_DIR:-.integration}")"
CF_CONTROLPLANE_DIR="$(absolute_path "${CF_CONTROLPLANE_DIR:-$INTEGRATION_DIR/mcp-context-forge}")"
CF_DATAPLANE_DIR="$(absolute_path "${CF_DATAPLANE_DIR:-$INTEGRATION_DIR/contextforge-gateway-rs}")"
CF_CONTROLPLANE_REPO="${CF_CONTROLPLANE_REPO:-https://github.com/IBM/mcp-context-forge.git}"
CF_CONTROLPLANE_REF="${CF_CONTROLPLANE_REF:-main}"
CF_DATAPLANE_REPO="${CF_DATAPLANE_REPO:-https://github.com/contextforge-gateway-rs/contextforge-gateway-rs.git}"
CF_DATAPLANE_REF="${CF_DATAPLANE_REF:-}"
CF_INTEGRATION_PROJECT="${CF_INTEGRATION_PROJECT:-cf-integration}"
CF_CONTROLPLANE_PROJECT="${CF_CONTROLPLANE_PROJECT:-cf-controlplane-only}"
JWT_SECRET_KEY="${JWT_SECRET_KEY:-my-test-key-but-now-longer-than-32-bytes}"
MCP_JWT_SUBJECT="${MCP_JWT_SUBJECT:-admin@example.com}"
CF_FAST_TIME_SERVER_ID="${CF_FAST_TIME_SERVER_ID:-9779b6698cbd4b4995ee04a4fab38737}"
CF_FAST_TIME_EXPECTED_IMAGE="${CF_FAST_TIME_EXPECTED_IMAGE:-${FAST_TIME_IMAGE:-ghcr.io/ibm/cfex-mcp-fast-time-server:latest}}"
MCP_CLI_BASE_URL="${MCP_CLI_BASE_URL:-http://127.0.0.1:${NGINX_PORT:-8080}}"
CF_COMPOSE_BUILD="${CF_COMPOSE_BUILD:-auto}"

if [[ -n "${CF_CONTROLPLANE_IMAGE+x}" || -n "${IMAGE_LOCAL+x}" ]]; then
  CONTROLPLANE_IMAGE_EXPLICIT=1
else
  CONTROLPLANE_IMAGE_EXPLICIT=0
fi
CF_CONTROLPLANE_IMAGE="${CF_CONTROLPLANE_IMAGE:-${IMAGE_LOCAL:-mcpgateway/mcpgateway:${CF_CONTROLPLANE_VERSION:-latest}}}"
if [[ -n "$CF_DATAPLANE_REF" ]]; then
  CF_DATAPLANE_IMAGE="${CF_DATAPLANE_IMAGE:-${CF_DATAPLANE_LOCAL_IMAGE:-contextforge-gateway-rs/contextforge-gateway-rs:local}}"
else
  CF_DATAPLANE_IMAGE="${CF_DATAPLANE_IMAGE:-ghcr.io/contextforge-gateway-rs/contextforge-gateway-rs:${CF_DATAPLANE_VERSION:-0.1.0}}"
fi
CF_DATAPLANE_PLATFORM="${CF_DATAPLANE_PLATFORM:-auto}"
if [[ "$CF_DATAPLANE_PLATFORM" == auto ]]; then
  if [[ -n "$CF_DATAPLANE_REF" ]]; then
    CF_DATAPLANE_PLATFORM="$(docker version --format '{{.Server.Os}}/{{.Server.Arch}}' 2>/dev/null || printf 'linux/amd64')"
  else
    CF_DATAPLANE_PLATFORM="linux/amd64"
  fi
fi

export ROOT CF_INTEGRATION_ROOT="$ROOT" CF_INTEGRATION_DIR="$INTEGRATION_DIR"
export CF_CONTROLPLANE_DIR CF_DATAPLANE_DIR CF_CONTROLPLANE_IMAGE CF_DATAPLANE_IMAGE
export CF_DATAPLANE_PLATFORM JWT_SECRET_KEY MCP_JWT_SUBJECT CF_FAST_TIME_SERVER_ID
export FAST_TIME_IMAGE="$CF_FAST_TIME_EXPECTED_IMAGE" MCP_CLI_BASE_URL
export IMAGE_LOCAL="$CF_CONTROLPLANE_IMAGE"
export PLATFORM_ADMIN_EMAIL="${PLATFORM_ADMIN_EMAIL:-$MCP_JWT_SUBJECT}"
export PASSWORD_CHANGE_ENFORCEMENT_ENABLED="${PASSWORD_CHANGE_ENFORCEMENT_ENABLED:-false}"
export ADMIN_REQUIRE_PASSWORD_CHANGE_ON_BOOTSTRAP="${ADMIN_REQUIRE_PASSWORD_CHANGE_ON_BOOTSTRAP:-false}"
export REQUIRE_PASSWORD_CHANGE_FOR_DEFAULT_PASSWORD="${REQUIRE_PASSWORD_CHANGE_FOR_DEFAULT_PASSWORD:-false}"
export GATEWAY_REPLICAS="${GATEWAY_REPLICAS:-1}"
export GATEWAY_CPU_LIMIT="${GATEWAY_CPU_LIMIT:-$(docker info --format '{{.NCPU}}' 2>/dev/null || printf '4')}"
export GATEWAY_CPU_RESERVATION="${GATEWAY_CPU_RESERVATION:-1}"
export GATEWAY_MEM_LIMIT="${GATEWAY_MEM_LIMIT:-2G}"
export GATEWAY_MEM_RESERVATION="${GATEWAY_MEM_RESERVATION:-512M}"
export GUNICORN_WORKERS="${GUNICORN_WORKERS:-$GATEWAY_CPU_LIMIT}"

topology() {
  local selected="${TOPOLOGY:-${CF_MCP_STACK_MODE:-dataplane}}"
  case "$selected" in controlplane|dataplane) printf '%s\n' "$selected" ;; *) printf 'invalid topology: %s\n' "$selected" >&2; return 2 ;; esac
}

topology_selection() {
  local selected="${TOPOLOGY:-all}"
  case "$selected" in controlplane|dataplane|all) printf '%s\n' "$selected" ;; *) printf 'invalid topology selection: %s\n' "$selected" >&2; return 2 ;; esac
}

integration_compose() {
  local args=(-p "$CF_INTEGRATION_PROJECT" -f "$CF_CONTROLPLANE_DIR/docker-compose.yml" -f "$ROOT/docker/docker-compose.cf-controlplane-build-labels.yaml" -f "$ROOT/docker/docker-compose.cf-dataplane.yaml" -f "$ROOT/docker/docker-compose.cf-integration.yaml")
  [[ -z "$CF_DATAPLANE_REF" ]] || args+=(-f "$ROOT/docker/docker-compose.cf-dataplane-build.yaml")
  docker compose "${args[@]}" "$@"
}

controlplane_compose() {
  local args=(-p "$CF_CONTROLPLANE_PROJECT" -f "$CF_CONTROLPLANE_DIR/docker-compose.yml" -f "$ROOT/docker/docker-compose.cf-controlplane-build-labels.yaml")
  truthy "${CONTROLPLANE_ENABLE_SSO:-false}" && args+=(--profile sso)
  docker compose "${args[@]}" "$@"
}

mode_compose() {
  local mode="$1"
  shift
  if [[ "$mode" == dataplane ]]; then integration_compose "$@"; else controlplane_compose "$@"; fi
}

conformance_compose() {
  local mode="$1"
  shift
  if [[ "$mode" == dataplane ]]; then
    local args=(-p "$CF_INTEGRATION_PROJECT" -f "$CF_CONTROLPLANE_DIR/docker-compose.yml" -f "$ROOT/docker/docker-compose.cf-controlplane-build-labels.yaml" -f "$ROOT/docker/docker-compose.cf-dataplane.yaml" -f "$ROOT/docker/docker-compose.cf-integration.yaml" -f "$ROOT/docker/docker-compose.cf-conformance.yaml")
    [[ -z "$CF_DATAPLANE_REF" ]] || args+=(-f "$ROOT/docker/docker-compose.cf-dataplane-build.yaml")
    docker compose "${args[@]}" --profile conformance "$@"
  else
    local args=(-p "$CF_CONTROLPLANE_PROJECT" -f "$CF_CONTROLPLANE_DIR/docker-compose.yml" -f "$ROOT/docker/docker-compose.cf-controlplane-build-labels.yaml" -f "$ROOT/docker/docker-compose.cf-conformance.yaml")
    truthy "${CONTROLPLANE_ENABLE_SSO:-false}" && args+=(--profile sso)
    docker compose "${args[@]}" --profile conformance "$@"
  fi
}

ensure_checkout_one() {
  local directory="$1" repository="$2" reference="$3" label="$4"
  mkdir -p "$(dirname "$directory")" || return
  if [[ ! -d "$directory/.git" ]]; then
    printf 'Cloning %s into %s\n' "$label" "$directory"
    git clone "$repository" "$directory" || return
  fi
  if ! git -C "$directory" diff --quiet || ! git -C "$directory" diff --cached --quiet; then
    printf '%s checkout has uncommitted changes: %s\n' "$label" "$directory" >&2
    return 1
  fi
  git -C "$directory" fetch --prune --tags --force origin || printf 'warning: %s fetch failed; using local refs\n' "$label" >&2
  if git -C "$directory" show-ref --verify --quiet "refs/remotes/origin/$reference"; then
    (git -C "$directory" checkout -q "$reference" 2>/dev/null || git -C "$directory" checkout -q -B "$reference" "origin/$reference") || return
    git -C "$directory" merge --ff-only "origin/$reference" || return
  else
    git -C "$directory" checkout -q "$reference" || return
  fi
}

export_checkout_metadata() {
  CF_CONTROLPLANE_CHECKOUT_REVISION="$(git -C "$CF_CONTROLPLANE_DIR" rev-parse HEAD)" || return
  CF_CONTROLPLANE_CHECKOUT_REF="$(git -C "$CF_CONTROLPLANE_DIR" symbolic-ref --quiet --short HEAD 2>/dev/null || printf '%s' "$CF_CONTROLPLANE_REF")" || return
  export CF_CONTROLPLANE_CHECKOUT_REVISION CF_CONTROLPLANE_CHECKOUT_REF
  if [[ -n "$CF_DATAPLANE_REF" ]]; then
    CF_DATAPLANE_CHECKOUT_REVISION="$(git -C "$CF_DATAPLANE_DIR" rev-parse HEAD)" || return
    CF_DATAPLANE_CHECKOUT_REF="$(git -C "$CF_DATAPLANE_DIR" symbolic-ref --quiet --short HEAD 2>/dev/null || printf '%s' "$CF_DATAPLANE_REF")" || return
    export CF_DATAPLANE_CHECKOUT_REVISION CF_DATAPLANE_CHECKOUT_REF
  fi
}

ensure_source_checkouts() {
  ensure_checkout_one "$CF_CONTROLPLANE_DIR" "$CF_CONTROLPLANE_REPO" "$CF_CONTROLPLANE_REF" control-plane || return
  if [[ -n "$CF_DATAPLANE_REF" ]]; then
    ensure_checkout_one "$CF_DATAPLANE_DIR" "$CF_DATAPLANE_REPO" "$CF_DATAPLANE_REF" dataplane || return
  fi
  export_checkout_metadata
}

require_checkouts() {
  [[ -f "$CF_CONTROLPLANE_DIR/docker-compose.yml" ]] || { printf 'control-plane checkout is missing; run make checkout\n' >&2; return 1; }
  [[ -z "$CF_DATAPLANE_REF" || -d "$CF_DATAPLANE_DIR/.git" ]] || { printf 'dataplane checkout is missing; run make checkout\n' >&2; return 1; }
  export_checkout_metadata
}

project_running() {
  [[ -n "$(docker ps -q --filter "label=com.docker.compose.project=$1")" ]]
}

ensure_other_stopped() {
  local mode="$1" other label
  if [[ "$mode" == dataplane ]]; then other="$CF_CONTROLPLANE_PROJECT"; label=controlplane; else other="$CF_INTEGRATION_PROJECT"; label=dataplane; fi
  if project_running "$other"; then
    printf '%s stack already uses the shared host ports; run make down first\n' "$label" >&2
    return 1
  fi
}

image_revision() {
  docker image inspect "$1" --format '{{ index .Config.Labels "org.opencontainers.image.revision" }}' 2>/dev/null || true
}

build_arguments() {
  local mode="$1" build=0
  case "$CF_COMPOSE_BUILD" in
    true|1) build=1 ;;
    false|0) build=0 ;;
    auto)
      if [[ "$CONTROLPLANE_IMAGE_EXPLICIT" == 0 && "$(image_revision "$CF_CONTROLPLANE_IMAGE")" != "$CF_CONTROLPLANE_CHECKOUT_REVISION" ]]; then build=1; fi
      if [[ "$mode" == dataplane && -n "$CF_DATAPLANE_REF" && "$(image_revision "$CF_DATAPLANE_IMAGE")" != "$CF_DATAPLANE_CHECKOUT_REVISION" ]]; then build=1; fi
      ;;
    *) printf 'CF_COMPOSE_BUILD must be auto, true, or false\n' >&2; return 2 ;;
  esac
  [[ "$build" == 0 ]] || printf '%s\n' --build
}

pull_images() {
  local mode="$1"
  if [[ "$CONTROLPLANE_IMAGE_EXPLICIT" == 1 ]]; then
    (docker pull "$CF_CONTROLPLANE_IMAGE" || docker image inspect "$CF_CONTROLPLANE_IMAGE" >/dev/null) || return
  fi
  if [[ "$mode" == dataplane && -z "$CF_DATAPLANE_REF" ]]; then
    (docker pull --platform "$CF_DATAPLANE_PLATFORM" "$CF_DATAPLANE_IMAGE" || docker image inspect "$CF_DATAPLANE_IMAGE" >/dev/null) || return
  fi
}

wait_public_endpoint() {
  local mode="$1" endpoint status deadline
  if [[ "$mode" == dataplane ]]; then endpoint="$MCP_CLI_BASE_URL/servers/$CF_FAST_TIME_SERVER_ID/mcp"; else endpoint="$MCP_CLI_BASE_URL/mcp"; fi
  deadline=$((SECONDS + ${CF_STACK_READY_TIMEOUT:-90}))
  printf 'Waiting for %s\n' "$endpoint"
  while (( SECONDS < deadline )); do
    status="$(curl --noproxy '*' --max-time 2 -sS -o /dev/null -w '%{http_code}' -H 'Accept: application/json, text/event-stream' "$endpoint" 2>/dev/null || true)"
    case "$status" in 401|403|405) return 0 ;; esac
    sleep 1
  done
  printf '%s did not become ready (last HTTP status: %s)\n' "$endpoint" "${status:-none}" >&2
  return 1
}

stack_up() {
  local mode="$1" fresh="$2" build_output build_args=()
  ensure_source_checkouts || return
  ensure_other_stopped "$mode" || return
  if truthy "$fresh"; then stack_down_one "$mode" 1 || return; fi
  pull_images "$mode" || return
  build_output="$(build_arguments "$mode")" || return
  [[ -z "$build_output" ]] || build_args+=("$build_output")
  truthy "${CF_FORCE_STACK_RESTART:-false}" && build_args+=(--force-recreate)
  if [[ "$mode" == dataplane ]]; then
    integration_compose config --format json | python3 "$ROOT/scripts/validate_compose.py" || return
    integration_compose up -d --remove-orphans "${build_args[@]+"${build_args[@]}"}" || return
  else
    controlplane_compose up -d --remove-orphans "${build_args[@]+"${build_args[@]}"}" || return
  fi
  wait_public_endpoint "$mode" || return
  printf '%s stack started\n' "$mode"
}

remove_project_by_label() {
  local project="$1" remove_volumes="$2" container_ids network_ids volume_ids id failure=0
  container_ids="$(docker ps -aq --filter "label=com.docker.compose.project=$project")" || return
  while IFS= read -r id; do
    [[ -z "$id" ]] || docker rm -f "$id" || failure=1
  done <<<"$container_ids"
  network_ids="$(docker network ls -q --filter "label=com.docker.compose.project=$project")" || return
  while IFS= read -r id; do
    [[ -z "$id" ]] || docker network rm "$id" >/dev/null 2>&1 || true
  done <<<"$network_ids"
  if truthy "$remove_volumes"; then
    volume_ids="$(docker volume ls -q --filter "label=com.docker.compose.project=$project")" || return
    while IFS= read -r id; do
      [[ -z "$id" ]] || docker volume rm "$id" || failure=1
    done <<<"$volume_ids"
  fi
  return "$failure"
}

stack_down_one() {
  local mode="$1" remove_volumes="$2" args=(down --remove-orphans)
  truthy "$remove_volumes" && args+=(--volumes)
  if [[ -f "$CF_CONTROLPLANE_DIR/docker-compose.yml" ]]; then
    if [[ "$mode" == dataplane ]]; then
      integration_compose --profile testing --profile inspector --profile sso --profile conformance "${args[@]}" || true
    else
      controlplane_compose --profile testing --profile inspector --profile sso --profile conformance "${args[@]}" || true
    fi
  fi
  if [[ "$mode" == dataplane ]]; then remove_project_by_label "$CF_INTEGRATION_PROJECT" "$remove_volumes"; else remove_project_by_label "$CF_CONTROLPLANE_PROJECT" "$remove_volumes"; fi
}

stack_down() {
  local selection="$1" remove_volumes="$2"
  case "$selection" in
    controlplane|dataplane) stack_down_one "$selection" "$remove_volumes" ;;
    all) stack_down_one controlplane "$remove_volumes"; stack_down_one dataplane "$remove_volumes" ;;
  esac
}

run_managed() {
  local mode="$1" operation="$2" primary=0 cleanup=0
  stack_up "$mode" 0 || primary=$?
  if [[ "$primary" == 0 && "$mode" == dataplane ]]; then wait_publisher "$CF_FAST_TIME_SERVER_ID" || primary=$?; fi
  if [[ "$primary" == 0 ]]; then "$operation" "$mode" || primary=$?; fi
  stack_down_one "$mode" 0 || cleanup=$?
  [[ "$primary" != 0 ]] && return "$primary"
  return "$cleanup"
}

token_for() {
  local mode="$1" server_id="$2"
  if [[ -n "${MCPGATEWAY_BEARER_TOKEN:-}" ]]; then printf '%s\n' "$MCPGATEWAY_BEARER_TOKEN"; return; fi
  if [[ "$mode" == controlplane ]]; then python3 "$ROOT/scripts/cf_jwt.py" --kind admin; else python3 "$ROOT/scripts/cf_jwt.py" --kind scoped --server-id "$server_id"; fi
}

validate_load_settings() {
  python3 - "${USERS:-${LOCUST_USERS:-100}}" "${SPAWN_RATE:-${LOCUST_SPAWN_RATE:-10}}" "${RUN_TIME:-${LOCUST_RUN_TIME:-5m}}" <<'PY'
import math
import re
import sys

users, spawn_rate, run_time = sys.argv[1:]
try:
    if int(users) <= 0 or str(int(users)) != users:
        raise ValueError
except ValueError:
    raise SystemExit("USERS must be an integer greater than zero")
try:
    if not math.isfinite(float(spawn_rate)) or float(spawn_rate) <= 0:
        raise ValueError
except ValueError:
    raise SystemExit("SPAWN_RATE must be a finite number greater than zero")
groups = re.findall(r"([0-9]+)(ms|s|m|h|d)", run_time)
if not groups or "".join(amount + unit for amount, unit in groups) != run_time or any(int(amount) == 0 for amount, _ in groups):
    raise SystemExit("RUN_TIME must contain positive integer+unit groups using ms, s, m, h, or d")
PY
}

wait_publisher() {
  local server_id="$1" timeout="${CF_PUBLISHER_WAIT_SECONDS:-90}" redis deadline output lua
  redis="$(docker ps -q --filter "label=com.docker.compose.project=$CF_INTEGRATION_PROJECT" --filter 'label=com.docker.compose.service=redis' | head -1)"
  [[ -n "$redis" ]] || { printf 'dataplane Redis container is not running\n' >&2; return 1; }
  lua="for _,key in ipairs(redis.call('KEYS','*UserConfig*')) do local value=redis.call('GET',key); if value then local ok,config=pcall(cmsgpack.unpack,value); if ok and type(config)=='table' and type(config.virtual_hosts)=='table' and config.virtual_hosts[ARGV[1]]~=nil then return 1 end end end return 0"
  deadline=$((SECONDS + timeout))
  while (( SECONDS < deadline )); do
    output="$(docker exec "$redis" redis-cli EVAL "$lua" 0 "$server_id" 2>/dev/null || true)"
    [[ "$output" == 1 ]] && return 0
    sleep 2
  done
  printf 'publisher snapshot did not contain server %s within %ss\n' "$server_id" "$timeout" >&2
  return 1
}

probe_operation() {
  python3 "$ROOT/scripts/cf_probe.py" --topology "$1"
}

load_operation() {
  local mode="$1" token reports users spawn_rate run_time result=0 audit=0 volume
  users="${USERS:-${LOCUST_USERS:-100}}"
  spawn_rate="${SPAWN_RATE:-${LOCUST_SPAWN_RATE:-10}}"
  run_time="${RUN_TIME:-${LOCUST_RUN_TIME:-5m}}"
  token="$(token_for "$mode" "$CF_FAST_TIME_SERVER_ID")" || return
  reports="$INTEGRATION_DIR/reports/load/$mode/locust"
  mkdir -p "$reports"
  volume="$reports:/mnt/reports"
  export MCPGATEWAY_BEARER_TOKEN="$token" MCP_STACK_MODE="$mode" MCP_SERVER_ID="$CF_FAST_TIME_SERVER_ID"
  export LOCUST_USERS="$users" LOCUST_SPAWN_RATE="$spawn_rate" LOCUST_RUN_TIME="$run_time"
  if [[ "$mode" == dataplane ]]; then
    integration_compose --profile testing run --rm --no-deps --volume "$volume" locust || result=$?
  else
    controlplane_compose run --rm --no-deps --volume "$volume" --volume "$ROOT/scripts/locustfile_mcp.py:/mnt/locust-cf/locustfile_mcp.py:ro" -e MCPGATEWAY_BEARER_TOKEN -e MCP_STACK_MODE -e MCP_PROTOCOL_VERSION -e MCP_TOOL_NAMES -e LOCUST_REQUEST_TIMEOUT_SECONDS --entrypoint locust locust -f /mnt/locust-cf/locustfile_mcp.py --host=http://nginx:80 --users="$users" --spawn-rate="$spawn_rate" --run-time="$run_time" --headless --html=/mnt/reports/locust_report.html --csv=/mnt/reports/locust --only-summary || result=$?
  fi
  AUDIT_TOKEN="$token" python3 "$ROOT/scripts/audit_reports.py" "$reports" || audit=$?
  [[ "$result" != 0 ]] && return "$result"
  return "$audit"
}

ensure_fast_test() {
  local mode="$1"
  mode_compose "$mode" --profile testing up -d --wait --wait-timeout 120 fast_test_server || return
  mode_compose "$mode" --profile testing run --rm --no-deps register_fast_test || return
  [[ "$mode" != dataplane ]] || wait_publisher  b8e3f1a2c4d5e6f7a1b2c3d4e5f6a7b8
}

run_live_pytest() {
  local mode="$1"
  shift
  if [[ "$mode" == dataplane ]]; then
    CF_INTEGRATION_DATAPLANE_EXPECTED_GAPS=1 \
      PYTHONPATH="$ROOT/scripts${PYTHONPATH:+:$PYTHONPATH}" \
      PYTEST_PLUGINS="${PYTEST_PLUGINS:+$PYTEST_PLUGINS,}cf_pytest_dataplane" \
      "$@"
  else
    "$@"
  fi
}

live_operation() {
  local mode="$1" group="${GROUP:-all}" first=0 second=0
  case "$group" in mcp|rbac|protocol|all) ;; *) printf 'GROUP must be mcp, rbac, protocol, or all\n' >&2; return 2 ;; esac
  case "$group" in mcp|all) ensure_fast_test "$mode" ;; esac
  case "$group" in
    mcp) make -C "$CF_CONTROLPLANE_DIR" test-mcp-protocol-e2e ;;
    rbac) make -C "$CF_CONTROLPLANE_DIR" test-mcp-rbac ;;
    protocol) run_live_pytest "$mode" make -C "$CF_CONTROLPLANE_DIR" test-protocol-compliance-gateway ;;
    all)
      (cd "$CF_CONTROLPLANE_DIR" && run_live_pytest "$mode" uv run --extra plugins pytest -p no:playwright tests/live_gateway/ --ignore=tests/live_gateway/plugins --ignore=tests/live_gateway/sso --ignore=tests/live_gateway/mcp/test_mcp_rbac_transport.py -v --tb=short) || first=$?
      (cd "$CF_CONTROLPLANE_DIR" && run_live_pytest "$mode" uv run --extra plugins pytest -p playwright tests/live_gateway/sso tests/live_gateway/mcp/test_mcp_rbac_transport.py -v --tb=short) || second=$?
      [[ "$first" == 0 ]] || return "$first"
      return "$second"
      ;;
  esac
}

start_conformance_service() {
  local mode="$1"
  export CF_CONFORMANCE_SERVER_ERA="${SERVER_ERA:-dual}"
  conformance_compose "$mode" build mcp_conformance_server || return
  conformance_compose "$mode" up -d --wait gateway mcp_conformance_server
}

stop_conformance_service() {
  conformance_compose "$1" rm --stop --force mcp_conformance_server || true
}

direct_fixture_endpoint() {
  local address
  address="$(conformance_compose "$1" port mcp_conformance_server 3000 | head -1)"
  [[ -n "$address" ]] || { printf 'conformance fixture has no published port\n' >&2; return 1; }
  printf 'http://%s/mcp\n' "$address"
}

run_conformance_lane() {
  local lane="$1" endpoint="$2" artifact_root="$3" token="${4:-}"
  if [[ -n "$token" ]]; then
    AUTH_PROXY_UPSTREAM="$endpoint" AUTH_PROXY_TOKEN="$token" python3 "$ROOT/scripts/auth_proxy.py" -- python3 "$ROOT/scripts/conformance.py" run-lane --lane "$lane" --endpoint '{proxy_url}' --spec-version "${CLIENT_VERSION:-2026-07-28}" --server-era "${SERVER_ERA:-dual}" --artifact-root "$artifact_root"
  else
    python3 "$ROOT/scripts/conformance.py" run-lane --lane "$lane" --endpoint "$endpoint" --spec-version "${CLIENT_VERSION:-2026-07-28}" --server-era "${SERVER_ERA:-dual}" --artifact-root "$artifact_root"
  fi
}

conformance_topology() {
  local mode="$1" artifact_root="$2" run_direct="$3" run_routed="$4" failure=0 lane_failure=0 gateway_id="" admin_token endpoint route token
  stack_up "$mode" 1 || failure=$?
  if [[ "$failure" == 0 ]]; then start_conformance_service "$mode" || failure=$?; fi
  if [[ "$failure" == 0 && "$run_direct" == 1 ]]; then
    endpoint="$(direct_fixture_endpoint "$mode")" || failure=$?
    if [[ "$failure" == 0 ]]; then run_conformance_lane fixture-direct "$endpoint" "$artifact_root" || lane_failure=1; fi
  fi
  if [[ "$failure" == 0 && "$run_routed" == 1 ]]; then
    admin_token="$(python3 "$ROOT/scripts/cf_jwt.py" --kind admin)"
    gateway_id="$(MCPGATEWAY_BEARER_TOKEN="$admin_token" python3 "$ROOT/scripts/conformance.py" provision --base-url "$MCP_CLI_BASE_URL")" || failure=$?
    if [[ "$failure" == 0 && "$mode" == dataplane ]]; then wait_publisher 3f33286667d34b65a31c3bafd30e4c21 || failure=$?; fi
    if [[ "$failure" == 0 ]]; then
      if [[ "$mode" == dataplane ]]; then route="$MCP_CLI_BASE_URL/servers/3f33286667d34b65a31c3bafd30e4c21/mcp"; else route="$MCP_CLI_BASE_URL/mcp"; fi
      token="$(token_for "$mode" 3f33286667d34b65a31c3bafd30e4c21)"
      run_conformance_lane "$mode" "$route" "$artifact_root" "$token" || lane_failure=1
    fi
  fi
  if [[ -n "$admin_token" ]]; then
    if [[ -n "$gateway_id" ]]; then
      MCPGATEWAY_BEARER_TOKEN="$admin_token" python3 "$ROOT/scripts/conformance.py" cleanup --base-url "$MCP_CLI_BASE_URL" --gateway-id "$gateway_id" || true
    else
      MCPGATEWAY_BEARER_TOKEN="$admin_token" python3 "$ROOT/scripts/conformance.py" cleanup --base-url "$MCP_CLI_BASE_URL" || true
    fi
  fi
  stop_conformance_service "$mode"
  stack_down_one "$mode" 0
  [[ "$failure" == 0 ]] || return "$failure"
  return "$lane_failure"
}

run_conformance() {
  local artifact_root="${RESULTS_DIR:-$INTEGRATION_DIR}" lanes="${LANES:-fixture-direct controlplane dataplane}" lane
  local fixture=0 controlplane=0 dataplane=0 failure=0 direct_done=0
  case "${CLIENT_VERSION:-2026-07-28}" in 2025-06-18|2025-11-25|2026-07-28) ;; *) printf 'unsupported CLIENT_VERSION\n' >&2; return 2 ;; esac
  case "${SERVER_ERA:-dual}" in dual|legacy|modern) ;; *) printf 'SERVER_ERA must be dual, legacy, or modern\n' >&2; return 2 ;; esac
  for lane in $lanes; do
    case "$lane" in fixture-direct) fixture=1 ;; controlplane) controlplane=1 ;; dataplane) dataplane=1 ;; *) printf 'invalid conformance lane: %s\n' "$lane" >&2; return 2 ;; esac
  done
  python3 "$ROOT/scripts/conformance.py" clear --artifact-root "$artifact_root"
  if [[ "$controlplane" == 1 || "$dataplane" == 0 ]]; then
    conformance_topology controlplane "$artifact_root" "$fixture" "$controlplane" || failure=1
    direct_done="$fixture"
  fi
  if [[ "$dataplane" == 1 ]]; then
    if [[ "$direct_done" == 1 ]]; then fixture=0; fi
    conformance_topology dataplane "$artifact_root" "$fixture" 1 || failure=1
  fi
  python3 "$ROOT/scripts/conformance.py" report --artifact-root "$artifact_root" --output-dir "${OUTPUT_DIR:-$ROOT/reports}" || failure=1
  return "$failure"
}

inspect_operation() {
  local mode="$1" server_id="${SERVER_ID:-$CF_FAST_TIME_SERVER_ID}" token endpoint
  token="$(token_for "$mode" "$server_id")" || return
  if [[ "$mode" == dataplane ]]; then endpoint="$MCP_CLI_BASE_URL/servers/$server_id/mcp"; else endpoint="$MCP_CLI_BASE_URL/mcp"; fi
  AUTH_PROXY_UPSTREAM="$endpoint" AUTH_PROXY_TOKEN="$token" python3 "$ROOT/scripts/auth_proxy.py" -- npx -y @modelcontextprotocol/inspector@0.22.0 --cli '{proxy_url}' --transport http --method "${METHOD:-tools/list}"
}

usage() {
  cat <<'EOF'
Internal script entrypoint. Prefer `make help` and the Make targets.
Commands: checkout up down status logs config probe load live conformance conformance-report inspect token
EOF
}

case "${1:-}" in
  checkout) ensure_source_checkouts ;;
  up) stack_up "$(topology)" "${FRESH:-0}" ;;
  down) stack_down "$(topology_selection)" "${VOLUMES:-0}" ;;
  status) require_checkouts; mode_compose "$(topology)" ps ;;
  logs)
    require_checkouts
    if [[ -n "${SERVICES:-}" ]]; then
      read -r -a log_services <<<"$SERVICES"
      for index in "${!log_services[@]}"; do [[ "${log_services[index]}" != cf-controlplane ]] || log_services[index]=gateway; done
      mode_compose "$(topology)" logs -f "${log_services[@]}"
    else
      mode_compose "$(topology)" logs -f
    fi
    ;;
  config)
    require_checkouts
    selected="$(topology)"
    if [[ "$selected" == dataplane ]]; then integration_compose --profile testing config; else controlplane_compose config; fi
    ;;
  probe) selected="$(topology)"; run_managed "$selected" probe_operation ;;
  load) selected="$(topology)"; validate_load_settings; run_managed "$selected" load_operation ;;
  live) selected="$(topology)"; run_managed "$selected" live_operation ;;
  conformance) run_conformance ;;
  conformance-report) python3 "$ROOT/scripts/conformance.py" report --artifact-root "${RESULTS_DIR:-$INTEGRATION_DIR}" --output-dir "${OUTPUT_DIR:-$ROOT/reports}" ;;
  inspect) selected="$(topology)"; run_managed "$selected" inspect_operation ;;
  token)
    kind="${TOKEN_KIND:-scoped}"
    case "$kind" in
      admin) [[ -z "${SERVER_ID:-}" ]] || { printf 'SERVER_ID is only valid with TOKEN_KIND=scoped\n' >&2; exit 2; }; python3 "$ROOT/scripts/cf_jwt.py" --kind admin ;;
      scoped) python3 "$ROOT/scripts/cf_jwt.py" --kind scoped --server-id "${SERVER_ID:-$CF_FAST_TIME_SERVER_ID}" ;;
      *) printf 'TOKEN_KIND must be scoped or admin\n' >&2; exit 2 ;;
    esac
    ;;
  help|-h|--help|"") usage ;;
  *) usage >&2; exit 2 ;;
esac
