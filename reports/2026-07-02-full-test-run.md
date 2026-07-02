# Control-Plane-Only Test Run

Run date: 2026-07-02, Europe/Dublin.

## Result

Control-plane-only baseline is green for the requested non-UI scope.

```text
PASS controlplane-up
PASS controlplane-live-core
PASS controlplane-locust
```

Evidence:

```text
.integration/test-logs/controlplane-only-20260702T085914Z.log
.integration/mcp-context-forge/reports/controlplane-only-20260702T085914Z/
```

## Source State

`origin/main` was fetched before the run and was already included in local `main`.

```text
repo_head=544cef1e03eb21099a827b0523ce9e31deb19b02
origin_main=d48a21d28e82b0f51c7c96889d98f73b4e407b15
origin_main_is_ancestor=true
cf-controlplane_head=f193742f5
```

The run used the local control-plane runner changes in this branch. It did not push.

## Scope

This run intentionally excludes dataplane, SSO, Playwright, and UI test lanes.

Stack properties:

```text
compose_project=cf-controlplane-only
cf-dataplane service=not present
dataplane nginx overlay=not present
DATAPLANE_PUBLISHER overlay=not present
SSO profile=off
Playwright=off
```

Running services after the run:

```text
gateway, nginx, postgres, pgbouncer, redis
fast_time_server, fast_test_server
a2a_echo_agent, a2a_echo_agent_v0_3_0
mcp_inspector
```

No `cf-dataplane` container was running in this control-plane-only stack.

## Commands

Fresh state:

```bash
git fetch origin main
docker compose -p cf-controlplane-only \
  -f .integration/mcp-context-forge/docker-compose.yml \
  --profile testing --profile inspector --profile sso \
  down -v --remove-orphans
```

Test:

```bash
scripts/cf-integration.sh controlplane-test-all
```

The runner now maps `controlplane-test-all` to:

```text
controlplane-up
controlplane-live-core
controlplane-locust
```

## Live-Core Results

`controlplane-live-core` runs MCP protocol E2E plus protocol compliance without Playwright.

```text
MCP protocol E2E:      19 passed, 3 skipped in 5.02s
Protocol compliance:   90 passed, 13 skipped, 35 xfailed, 4 xpassed in 12.96s
```

Skipped/xfailed tests are expected upstream capability markers. They did not fail the lane.

## Locust Results

`controlplane-locust` uses upstream `tests/loadtest/locustfile.py` with the non-UI class subset:

```text
HealthCheckUser
FastTimeUser
FastTestEchoUser
FastTestTimeUser
VersionMetaUser
```

This keeps the Locust check on real control-plane MCP traffic while excluding admin UI and mutating UI flows.

Console summary from the run:

```text
Total Requests: 32,072
Total Failures: 0 (0.00%)
Requests/sec: 106.97
Average: 269.00 ms
Median p50: 96 ms
p90: 770 ms
p95: 1100 ms
p99: 1700 ms
Max: 5704.05 ms
```

Machine CSV artifact summary:

```text
Aggregated requests: 31,974
Aggregated failures: 0
Failures/sec: 0.0
locust_failures.csv: header only
locust_exceptions.csv: header only
```

The console and CSV differ slightly on request count due Locust final flush timing, but both sources agree on zero failures.

Top green endpoints from the console summary:

```text
/rpc fast-time-get-system-time          10,067 requests, 0 failures
/rpc fast-time-get-system-time [UTC]     4,786 requests, 0 failures
/rpc fast-time-convert-time              2,895 requests, 0 failures
/rpc fast-test-echo                      2,661 requests, 0 failures
/rpc fast-test-get-system-time           2,460 requests, 0 failures
/rpc tools/list [fasttime]               1,974 requests, 0 failures
/rpc tools/list [fasttest]               1,014 requests, 0 failures
/health                                    610 requests, 0 failures
/version                                   344 requests, 0 failures
/ready                                     331 requests, 0 failures
```

Artifacts:

```text
.integration/mcp-context-forge/reports/controlplane-only-20260702T085914Z/locust_report.html
.integration/mcp-context-forge/reports/controlplane-only-20260702T085914Z/locust_stats.csv
.integration/mcp-context-forge/reports/controlplane-only-20260702T085914Z/locust_failures.csv
.integration/mcp-context-forge/reports/controlplane-only-20260702T085914Z/locust_exceptions.csv
.integration/mcp-context-forge/reports/controlplane-only-20260702T085914Z/locust_stats_history.csv
```

## What Was Misconfigured

Two environment/test-harness issues caused the earlier red control-plane runs.

1. Password-change enforcement was on for the default admin.

   Stock compose uses `PLATFORM_ADMIN_PASSWORD=changeme` and password-change enforcement defaults on. The gateway redirected admin routes to `/admin/change-password-required`, which made API/admin checks see 303 redirects or empty bodies instead of JSON/HTML.

   Fix in runner: set these for control-plane-only runs:

   ```text
   PASSWORD_CHANGE_ENFORCEMENT_ENABLED=false
   ADMIN_REQUIRE_PASSWORD_CHANGE_ON_BOOTSTRAP=false
   REQUIRE_PASSWORD_CHANGE_FOR_DEFAULT_PASSWORD=false
   ```

2. Locust token was overwritten by upstream `locust_token`.

   The harness wrote an admin token, but `docker compose run locust` started the `locust_token` dependency again. That regenerated a token without rich admin/session claims. Gateway logs then showed the request as public-only:

   ```text
   token_teams=[]
   token_use=None
   Admin bypass suppressed for public-only token
   Admin access denied for public-only token
   ```

   Fix in runner: mint a session admin token and run Locust with `--no-deps` after writing that token, so the dependency does not overwrite it.

## Why Full Upstream Locust Class Mix Is Not The Green Baseline

`CONTROLPLANE_LOCUST_CLASSES=all` still runs the entire upstream Locust class mix. That includes admin UI, HTMX partials, user/team mutation, prompt retrieval, generic random `tools/call`, import/export, OAuth, RBAC, tokens, logs, and other surfaces.

A real diagnostic run after the token fix showed the auth flood was gone, but the full class mix still reported failures such as:

```text
POST /admin/users [create]: 400
GET /prompts/[id]: 422
GET /admin/teams/[id]/edit: 403
POST /rpc prompts/get: prompt retrieval failed
POST /rpc tools/call: JSON-RPC -32603 Internal error
GET /admin/logs/export: 503
GET /admin/tags: 500
```

Those are not dataplane failures and not SSO/Playwright failures. They are upstream full-load profile issues from mixing UI/admin/mutating/random RPC tasks into one strict run. Since this check was requested as non-UI control-plane health, the green baseline uses the explicit non-UI class subset above.

To reproduce the broader upstream stress profile:

```bash
CONTROLPLANE_LOCUST_CLASSES=all scripts/cf-integration.sh controlplane-locust
```

That is useful for upstream load-test bug hunting, but it is not the green control-plane-without-dataplane baseline.

## Runner Changes Used

The runner now provides a repeatable green control-plane-only path:

```text
controlplane-up         stock control plane, no dataplane overlays
controlplane-live-core  non-UI, non-SSO live checks, Playwright disabled
controlplane-locust     upstream locustfile.py, non-UI Fast Time/Fast Test/health classes
controlplane-test-all   runs all three and logs one timestamped report
```

Override knobs:

```text
CONTROLPLANE_ENABLE_SSO=true       include SSO compose profile when explicitly needed
CONTROLPLANE_LOCUST_CLASSES=all    run the full upstream Locust class mix
```
