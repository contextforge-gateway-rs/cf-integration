# Number 1 Test Report: GHCR Fast Time Dataplane Run

Run date: 2026-07-01, Europe/Dublin.

Repo HEAD: `5d9a0eb`

## Scope

This report replaces the prior header-passthrough report. It is based on a fresh run against the current stack after switching Fast Time to the published GHCR image:

```text
ghcr.io/ibm/cfex-mcp-fast-time-server:latest
sha256:65b5977ba69bfb12fab5c71c445c8328934911430f1cedd9b07eadbe30fa57c0
created: 2026-06-29T13:45:21.322469287Z
```

Evidence files from the run are under:

```text
.integration/mcp-context-forge/reports/ghcr-fast-time-run/
```

## Verdict

The nginx to `cf-dataplane` route works for the real Fast Time server on `/servers/{virtual_host_id}/mcp`.

Verified through nginx:

- `Authorization` reached the dataplane.
- `Mcp-Session-Id` worked on follow-up requests.
- Streamable HTTP content negotiation worked when the client accepted `text/event-stream`.
- `initialize`, `tools/list`, and `tools/call` all passed.

The full stack is still red. The new failures are not the old number 1 header-passthrough issue:

- `smoke` fails because the Locust MCP client sends `Accept: application/json` against a streamable HTTP endpoint that returns SSE.
- `live-all` fails because the suite still assumes `/sse`, `fast-test-*` fixtures, optional SSO/runtime services, and protocol-compliance async fixtures that do not match this harness run.

## Current Stack State

Command:

```bash
scripts/cf-integration.sh ps
```

Relevant services:

```text
cf-integration-nginx-1              mcpgateway/nginx-cache:latest                         Up, healthy
cf-integration-gateway-1            mcpgateway/mcpgateway:latest                          Up, healthy
cf-integration-cf-dataplane-1       ghcr.io/contextforge-gateway-rs/contextforge-gateway-rs:0.1.0 Up
cf-integration-fast_time_server-1   ghcr.io/ibm/cfex-mcp-fast-time-server:latest          Up, healthy
cf-integration-postgres-1           postgres:18                                           Up, healthy
cf-integration-redis-1              redis:latest                                          Up, healthy
```

The current Fast Time registration is STREAMABLEHTTP on `/mcp`; the stale `/sse` registration from the old report is not present.

DB evidence:

```text
fast_time|http://fast_time_server:8080/mcp|STREAMABLEHTTP|true|active
9779b6698cbd4b4995ee04a4fab38737|Fast Time Server|6
```

The virtual server under test is:

```text
9779b6698cbd4b4995ee04a4fab38737
```

## Direct Dataplane Probe

Command shape:

```bash
TOKEN=$(scripts/cf-integration.sh token)
SERVER_ID=9779b6698cbd4b4995ee04a4fab38737
URL="http://127.0.0.1:8080/servers/${SERVER_ID}/mcp"

curl \
  -H "Authorization: Bearer ${TOKEN}" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -H "Mcp-Protocol-Version: 2025-06-18" \
  --data '{"jsonrpc":"2.0","id":"init","method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"cf-integration-report","version":"2.0"}}}' \
  "$URL"
```

Fresh result after the broad test run:

```text
auth_negative=PASS status=401
initialize=PASS status=200 session=c54f1adb-3533-4970-a35f-de9bc575fcdb
tools_list=PASS count=6
tool=e3c164982fd04edf835cd1e0ef3223da-convert_time
tool=e3c164982fd04edf835cd1e0ef3223da-echo
tool=e3c164982fd04edf835cd1e0ef3223da-get_stats
tool=e3c164982fd04edf835cd1e0ef3223da-get_system_time
tool=e3c164982fd04edf835cd1e0ef3223da-schema_error
tool=e3c164982fd04edf835cd1e0ef3223da-schema_success
echo_call=PASS
```

The `401` negative control used the same initialize request without `Authorization`. The passing initialize used the bearer token.

This is the main pass signal. It exercises the real public route:

```text
nginx :8080
-> /servers/9779b6698cbd4b4995ee04a4fab38737/mcp
-> cf-dataplane
-> ghcr.io/ibm/cfex-mcp-fast-time-server:latest /mcp
```

## Smoke Run

Command:

```bash
MCP_VIRTUAL_SERVER_ID=9779b6698cbd4b4995ee04a4fab38737 scripts/cf-integration.sh smoke
```

Exit code: `1`

Result:

```text
Total Requests: 19
Total Failures: 19 (100.00%)
Requests/sec (RPS): 2.00

Error report
1 POST MCP initialize: HTTP 406
8 POST MCP tools/list: HTTP 406
7 POST MCP prompts/list: HTTP 406
2 POST MCP ping: HTTP 406
1 POST MCP resources/list: HTTP 406
```

Diagnosis:

The load test client sends `Accept: application/json` for MCP streamable HTTP calls:

```text
.integration/mcp-context-forge/tests/loadtest/locustfile_mcp_protocol.py:260
.integration/mcp-context-forge/tests/loadtest/locustfile_mcp_protocol.py:542
.integration/mcp-context-forge/tests/loadtest/locustfile_mcp_protocol.py:918
```

That is incompatible with the endpoint in this run. The successful direct probe used:

```text
Accept: application/json, text/event-stream
```

The smoke failure is therefore a client/content-negotiation bug in the load-test path. It is not evidence that nginx is dropping the number 1 headers.

## Full Live Run

Command:

```bash
MCP_CLI_BASE_URL=http://127.0.0.1:8080 scripts/cf-integration.sh live-all
```

Exit code: `2`

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
runtime: 30.28s
```

Important failure buckets:

1. RBAC transport tests still try to register `http://fast_time_server:8080/sse`.

   The GHCR Fast Time image used by this harness exposes `/mcp`. The suite gets:

   ```text
   Failed to initialize gateway at http://fast_time_server:8080/sse:
   Client error '404 Not Found'
   ```

   This accounts for the RBAC `/sse` setup errors. It is a suite expectation mismatch for this image.

2. MCP protocol E2E tool-call tests expect `fast-test-*` tools.

   Examples from the run:

   ```text
   Tool not found: fast-test-echo
   Tool not found: fast-test-get-stats
   Tool 'fast-test-schema-error' is not registered in the gateway
   Tool 'fast-test-schema-success' is not registered in the gateway
   ```

   The actual registered tools are the six GHCR Fast Time tools listed in the direct probe.

3. Protocol-compliance setup errors occur before gateway behavior is tested.

   The repeated error is:

   ```text
   RuntimeError: Runner.run() cannot be called from a running event loop
   ```

   It appears for `reference-stdio`, `gateway_proxy-http`, and `gateway_virtual-http` targets. That points at the suite fixture/runtime setup, not nginx passthrough.

4. Optional lanes are not configured in this stack.

   Skips include:

   ```text
   Azure credentials not configured
   Keycloak not reachable at http://localhost:8180
   runtime-mode flip refused because boot_mode='off'
   ```

## What This Run Proves

Proven:

- The current harness uses `ghcr.io/ibm/cfex-mcp-fast-time-server:latest`.
- The control plane registers Fast Time at `http://fast_time_server:8080/mcp` with `STREAMABLEHTTP`.
- The dataplane can initialize a session through nginx.
- The dataplane can use the returned `Mcp-Session-Id` through nginx.
- The dataplane can list and call Fast Time tools through nginx.

Not proven green:

- `scripts/cf-integration.sh smoke`
- `scripts/cf-integration.sh live-all`
- `/sse` compatibility for the GHCR Fast Time image
- `fast-test-*` fixture coverage
- optional SSO and runtime-mode rails

## Next Fixes

1. Fix `tests/loadtest/locustfile_mcp_protocol.py` to use `Accept: application/json, text/event-stream` for streamable HTTP and parse SSE responses. Then rerun `scripts/cf-integration.sh smoke`.
2. Update the RBAC transport tests or harness config so this GHCR Fast Time lane does not register `/sse` unless the image actually exposes it.
3. Either register the `fast_test_server` fixture for `live-all`, or exclude tests that require `fast-test-*` tools from this GHCR Fast Time lane.
4. Triage the protocol-compliance async fixture error separately. It reproduces on `reference-stdio`, so it should not be treated as a dataplane routing failure.

## Final Status

Number 1 header passthrough is fixed on the real dataplane route.

The report should not be read as "the whole integration stack is green." It is not. The next work is load-test content negotiation and live-suite scope/config alignment for the GHCR Fast Time image.
