# GHCR Fast Time Rerun After Origin Pull

Run date: 2026-07-01, Europe/Dublin.

Repo state:

```text
branch=main
origin pull=Already up to date
repo_head=6ba9d29
cf-controlplane_head=a8f786b7f
evidence=.integration/mcp-context-forge/reports/ghcr-fast-time-rerun-20260701T214018Z/
```

## Scope

This rewrites the previous GHCR Fast Time report after pulling `origin/main` and rerunning the stack checks. The current harness includes the upstream fixes that moved the reliable green path to:

```text
scripts/cf-integration.sh probe
scripts/cf-integration.sh smoke
scripts/cf-integration.sh live-core
```

`live-all` was rerun too. It is still red for the known broad-suite reasons below.

## Environment

The stack was refreshed with:

```bash
git pull --ff-only origin main
scripts/cf-integration.sh up
```

Fast Time image:

```text
ghcr.io/ibm/cfex-mcp-fast-time-server:latest
sha256:65b5977ba69bfb12fab5c71c445c8328934911430f1cedd9b07eadbe30fa57c0
created: 2026-06-29T13:45:21.322469287Z
```

Other images:

```text
cf-dataplane: ghcr.io/contextforge-gateway-rs/contextforge-gateway-rs:0.1.0
cf-dataplane digest: sha256:7f8abb166c176502d24564cd52f91e9a1bb5805f7ca132ee63e9701983e75e3b
cf-controlplane: mcpgateway/mcpgateway:latest
cf-controlplane digest: sha256:efd0e78d27589a8a213e66ace07e1805425190525207e1a399065b2e55be5581
nginx: mcpgateway/nginx-cache:latest
nginx digest: sha256:9dfdbad545d2300e0cae4efb0d1856d709f2a4f4272b99af94f17ddb696c3bbf
```

Fast Time DB registration:

```text
fast_time|http://fast_time_server:8080/mcp|STREAMABLEHTTP|true|active
9779b6698cbd4b4995ee04a4fab38737|Fast Time Server|6
```

## Verdict

The current harness green path passes from a real run:

```text
probe      PASS exit 0
smoke      PASS exit 0, 65 requests, 0 failures
live-core  PASS exit 0, 12 passed, 3 skipped, 7 deselected
live-all   FAIL exit 2, 65 failed, 70 passed, 75 skipped, 5 xfailed, 55 errors
```

The old number 1 header-passthrough issue remains fixed on the real nginx to `cf-dataplane` route. The old smoke 406 problem is also fixed in the harness-owned Locust file now on `origin/main`.

The full `live-all` lane is still not a green signal for this harness. It runs upstream tests that assume `/sse`, `fast-test-*` fixture tools, optional SSO/runtime services, and protocol-compliance fixtures outside this harness scope.

## Probe

Command:

```bash
scripts/cf-integration.sh probe
```

Exit code: `0`.

Result:

```text
probe url: http://127.0.0.1:8080/servers/9779b6698cbd4b4995ee04a4fab38737/mcp
auth_negative=PASS status=401
initialize=PASS status=200 session=51ad3942-cfed-410e-834b-020bd102ea52
tools_list=PASS count=6
tool=76b29617e21c4bd69834e4424a0f7d08-convert_time
tool=76b29617e21c4bd69834e4424a0f7d08-echo
tool=76b29617e21c4bd69834e4424a0f7d08-get_stats
tool=76b29617e21c4bd69834e4424a0f7d08-get_system_time
tool=76b29617e21c4bd69834e4424a0f7d08-schema_error
tool=76b29617e21c4bd69834e4424a0f7d08-schema_success
tool_call=PASS tool=76b29617e21c4bd69834e4424a0f7d08-echo
```

This checks the real public path:

```text
nginx :8080 -> /servers/{id}/mcp -> cf-dataplane -> fast_time_server /mcp
```

The `401` negative control proves the success path is not anonymous fallback. The passing path uses bearer auth, session propagation, streamable HTTP response parsing, tool listing, and a real tool call.

## Smoke

Command:

```bash
scripts/cf-integration.sh smoke
```

Exit code: `0`.

Result:

```text
POST MCP initialize                  1   0(0.00%)
POST MCP ping                       10   0(0.00%)
POST MCP tools/call echo            12   0(0.00%)
POST MCP tools/call get_system_time 24   0(0.00%)
POST MCP tools/list                 18   0(0.00%)
Aggregated                          65   0(0.00%)  avg 19ms  max 42ms
```

This confirms the latest harness Locust path no longer reproduces the old HTTP 406 failure. The harness now uses `scripts/locustfile_cf_dataplane.py`, which sends:

```text
Accept: application/json, text/event-stream
```

and parses streamable HTTP responses correctly.

## Live Core

Command:

```bash
scripts/cf-integration.sh live-core
```

Exit code: `0`.

Result:

```text
12 passed, 3 skipped, 7 deselected in 2.81s
```

Skipped:

```text
No resources registered on gateway - nothing to read
No prompts registered on gateway - nothing to render
Rust MCP public transport not active at http://127.0.0.1:8080
```

Deselected:

```text
TestToolCalls
```

That deselection is intentional for `live-core`; the full tool-call tests assume fixture-specific tool names that do not match this Fast Time registration.

## Live All

Command:

```bash
scripts/cf-integration.sh live-all
```

Exit code: `2`.

Result:

```text
270 collected
65 failed
70 passed
75 skipped
5 xfailed
109 warnings
55 errors
7 rerun
runtime: 29.51s
```

Failure buckets:

1. RBAC transport tests still register `http://fast_time_server:8080/sse`.

   The GHCR Fast Time image in this harness serves `/mcp`. The broad suite still gets:

   ```text
   Failed to initialize gateway at http://fast_time_server:8080/sse:
   Client error '404 Not Found'
   ```

2. MCP protocol E2E tool-call tests still expect `fast-test-*` tools.

   Examples:

   ```text
   Tool not found: fast-test-echo
   Tool not found: fast-test-get-stats
   Tool 'fast-test-schema-error' is not registered in the gateway
   Tool 'fast-test-schema-success' is not registered in the gateway
   ```

3. Protocol-compliance setup still trips the async fixture error:

   ```text
   RuntimeError: Runner.run() cannot be called from a running event loop
   ```

   This appears on `reference-stdio` as well as gateway targets, so it is not specific to nginx routing or dataplane header passthrough.

4. Optional lanes remain unconfigured for this stack:

   ```text
   Azure credentials not configured
   Keycloak not reachable at http://localhost:8180
   runtime-mode flip refused because boot_mode='off'
   ```

## Final Status

The current `origin/main` harness is green for the supported path:

```text
up -> probe -> smoke -> live-core
```

`live-all` remains a known-red upstream broad suite. Treat it as a source of follow-up work, not as evidence that number 1 header passthrough or the GHCR Fast Time dataplane path regressed.
