# Full Live-Gateway Test Run

Run date: 2026-07-02, Europe/Dublin.

## Source State

Pulled latest `origin/main` before the run:

```text
git pull --ff-only origin main -> Already up to date
repo_head=1f42fbd
origin_head=1f42fbd
branch=main
cf-controlplane_head=a8f786b7f
evidence=.integration/mcp-context-forge/reports/full-test-run-20260702T065847Z/
```

After the run, `origin/main` was pulled again and was already current at `b7b36d4`. The evidence below remains labeled with the SHA that actually produced it.

## Stack State

Stack refreshed with:

```bash
scripts/cf-integration.sh up
```

Running services include:

```text
cf-integration-fast_time_server-1  ghcr.io/ibm/fast-time-server:latest       healthy
cf-integration-fast_test_server-1  mcpgateway/fast-test-server:latest        healthy
cf-integration-gateway-1           mcpgateway/mcpgateway:latest              healthy
cf-integration-cf-dataplane-1      ghcr.io/contextforge-gateway-rs/contextforge-gateway-rs:0.1.0
cf-integration-nginx-1             mcpgateway/nginx-cache:latest             healthy
```

Actual running image digests:

```text
fast_time_server  sha256:a23c569e05507294a4200196f7c1a26239fafa58cd82dcc194d7e42af4f74410
fast_test_server  sha256:d74d26d7396ddf5cfba6c5fe4f4718f67b6862b08a4539e1d78c289b82180b15
cf-dataplane      sha256:7f8abb166c176502d24564cd52f91e9a1bb5805f7ca132ee63e9701983e75e3b
cf-controlplane   sha256:efd0e78d27589a8a213e66ace07e1805425190525207e1a399065b2e55be5581
nginx             sha256:9dfdbad545d2300e0cae4efb0d1856d709f2a4f4272b99af94f17ddb696c3bbf
```

Fast gateway registrations:

```text
fast_test|http://fast_test_server:8880/mcp|STREAMABLEHTTP|true|active
fast_time|http://fast_time_server:8080/mcp|STREAMABLEHTTP|true|active
fast_time_sse|http://fast_time_server:8080/sse|SSE|true|active

b8e3f1a2c4d5e6f7a1b2c3d4e5f6a7b8|Fast Test Server|6
9779b6698cbd4b4995ee04a4fab38737|Fast Time Server|6
a88e2c3f5d7b4a9e8f1c6d2e3b4a5f6e|Fast Time SSE Server|6
18 tools total
```

## Result Summary

```text
probe     PASS exit 0
smoke     PASS exit 0, 68 requests, 0 failures
live-all  FAIL exit 2, 64 failed, 84 passed, 75 skipped, 5 xfailed, 42 errors
cf-locust FAIL exit 1, 17,015 requests, 10,675 failures
```

This is an improvement over the previous full run. The old `/sse` setup failure is gone, and the `fast-test-*` tool-call fixture failures are gone.

## Probe

Command:

```bash
scripts/cf-integration.sh probe
```

Result:

```text
auth_negative=PASS status=401
initialize=PASS status=200 session=f1841f45-116e-4da9-befa-d57ba451f85f
tools_list=PASS count=6
tool=938b4cbeb3b541c7a013496bcc069f13-convert_time
tool=938b4cbeb3b541c7a013496bcc069f13-echo
tool=938b4cbeb3b541c7a013496bcc069f13-get_stats
tool=938b4cbeb3b541c7a013496bcc069f13-get_system_time
tool=938b4cbeb3b541c7a013496bcc069f13-schema_error
tool=938b4cbeb3b541c7a013496bcc069f13-schema_success
tool_call=PASS tool=938b4cbeb3b541c7a013496bcc069f13-echo
```

The public nginx to dataplane route is healthy for the Fast Time virtual server.

## Smoke

Command:

```bash
scripts/cf-integration.sh smoke
```

Result:

```text
POST MCP initialize                  1   0(0.00%)
POST MCP ping                       12   0(0.00%)
POST MCP tools/call echo            18   0(0.00%)
POST MCP tools/call get_system_time 20   0(0.00%)
POST MCP tools/list                 17   0(0.00%)
Aggregated                          68   0(0.00%)  avg 19ms  max 46ms
```

The harness streamable HTTP client remains green.

## Full CF Locust

Command:

```bash
LOCUST_LOCUSTFILE=locustfile.py scripts/cf-integration.sh locust
```

Settings:

```text
locust_file=locustfile.py
locust_users=100
locust_spawn_rate=10
locust_run_time=5m
evidence=.integration/mcp-context-forge/reports/full-cf-locust-20260702T070453Z/
```

Result:

```text
exit code: 1
Total Requests:     17,015
Total Failures:     10,675 (62.74%)
Requests/sec (RPS): 56.75
Average:            9.92 ms
Min:                1.05 ms
Max:                740.51 ms
p50:                7.00 ms
p90:                16.00 ms
p95:                21.00 ms
p99:                37.00 ms
```

Artifacts:

```text
.integration/mcp-context-forge/reports/full-cf-locust-20260702T070453Z/locust_report.html
.integration/mcp-context-forge/reports/full-cf-locust-20260702T070453Z/locust_stats.csv
.integration/mcp-context-forge/reports/full-cf-locust-20260702T070453Z/locust_failures.csv
.integration/mcp-context-forge/reports/full-cf-locust-20260702T070453Z/locust_exceptions.csv
.integration/mcp-context-forge/reports/full-cf-locust-20260702T070453Z/locust_stats_history.csv
```

Top high-volume endpoints:

```text
/rpc fast-time-get-system-time        2,311 requests, 2,311 failures
/rpc fast-time-get-system-time [UTC]  1,120 requests, 1,120 failures
/rpc fast-time-convert-time             770 requests,   770 failures
/tools                                  649 requests,     0 failures
/rpc tools/list                         583 requests,     0 failures
/servers                                533 requests,     0 failures
/rpc tools/list [fasttime]              489 requests,     0 failures
/rpc fast-test-echo                     411 requests,   411 failures
/health                                 395 requests,     0 failures
/rpc fast-test-get-system-time          395 requests,   395 failures
```

The setup phase also failed in strict mode:

```text
/gateways -> HTTP 403
/resources -> HTTP 403
/prompts -> HTTP 403
/teams/ -> HTTP 403
/rbac/roles -> HTTP 403
```

Dominant errors are authorization/permission failures:

```text
GET /admin/: Expected [200], got 403
POST /rpc fast-time-get-system-time: JSON-RPC error [-32003]: Access denied - {'method': 'tools/call'}
POST /rpc fast-test-echo: JSON-RPC error [-32003]: Access denied - {'method': 'tools/call'}
POST /rpc prompts/list: JSON-RPC error [-32003]: Access denied - {'method': 'prompts/list'}
GET /gateways: Expected [200], got 403
GET /metrics [api]: Expected [200], got 403
```

Failure cause:

```text
The full CF Locust profile is exercising control-plane admin, RBAC, teams,
resources, prompts, metrics, protocol, and JSON-RPC tool-call surfaces.
The harness-provided bearer token is accepted for basic read paths such as
/tools, /servers, and /rpc tools/list, but it is not authorized for the full
control-plane session/RBAC surface. The run therefore fails strict setup on
protected pool fetches and then records the same authorization denial under load.
```

Failure classification from `locust_failures.csv`:

```text
JSON-RPC -32003 Access denied: 6,708 occurrences
HTTP 403 Forbidden:             3,929 occurrences
Other setup/list 403 wrappers:      9 occurrences
```

This is not a dataplane routing or Fast Time image failure. The green paths in the same run prove the stack is reachable, while the failing paths are permission-gated by the upstream control-plane profile.

## Full Suite

Command:

```bash
scripts/cf-integration.sh live-all
```

Result:

```text
270 collected
84 passed
64 failed
42 errors
75 skipped
5 xfailed
110 warnings
runtime: 18.63s
```

Notable pass/improvement:

```text
tests/live_gateway/mcp/test_mcp_protocol_e2e.py .........s.s.........s
```

That means the upstream MCP protocol E2E lane no longer fails on missing `fast-test-*` tools in this stack.

Remaining failure buckets:

1. RBAC visibility and per-server endpoint regressions:

   ```text
   TestServerVisibilityViaAPI.test_admin_sees_public_and_team_via_http
   TestMcpPerServerEndpoint.test_public_token_accesses_public_server
   TestMcpPerServerEndpoint.test_team_member_accesses_team_server
   ```

   Details:

   ```text
   admin via HTTP must NOT see private server
   HTTP 400 for /servers/{public_server}/mcp/
   HTTP 400 for /servers/{team_server}/mcp/
   ```

2. Protocol-compliance fixture/runtime failures dominate the remaining red:

   ```text
   RuntimeError: Runner.run() cannot be called from a running event loop
   ```

   This appears for `reference-stdio` and gateway targets. It is not specific to nginx or dataplane routing.

3. Optional or environment-bound lanes still skip:

   ```text
   Rust MCP public transport not active
   Langfuse auth not configured
   Keycloak not reachable at http://localhost:8180
   Azure credentials not configured
   runtime-mode flip refused because boot_mode='off'
   ```

## Notes

The current stock-upstream registration path now publishes all three expected fast fixtures:

```text
fast_time
fast_time_sse
fast_test
```

That resolves the earlier `/sse` and missing `fast-test-*` classes of failures. The remaining work is now narrower: RBAC/per-server endpoint behavior and protocol-compliance fixture/runtime cleanup.
