# Number 1 Test Report: Header Passthrough

Run date: 2026-07-01, Europe/Dublin.

## Executive Summary

The header-passthrough fix is verified on the real nginx -> `cf-dataplane` route. It is not the reason the current full run is red.

The remaining red results are useful, but they point elsewhere:

1. `scripts/cf-integration.sh smoke` fails because the locust MCP client sends `Accept: application/json` on a streamable HTTP endpoint that requires `text/event-stream` to be acceptable.
2. `scripts/cf-integration.sh live-all` is too broad for this harness as-is. It includes suites that expect unregistered `fast-test-*` tools, Keycloak/SSO services, Rust runtime modes that are booted off, and protocol-compliance async fixtures that error before gateway behavior is tested.
3. The direct dataplane path works with auth, session propagation, tool listing, and tool calls.

So: number 1 is fixed. The next work is not more nginx header passthrough work. It is test-client compatibility and live-suite scope/configuration.

## Environment

- `cf-integration`: `40e3936`
- `cf-controlplane`: `a8f786b7f`
- `cf-dataplane`: `ghcr.io/contextforge-gateway-rs/contextforge-gateway-rs:0.1.0`, image id `7f8abb166c17`
- `nginx`: `mcpgateway/nginx-cache:latest`, image id `9dfdbad545d2`
- `cf-integration-mcp-counter`: `cf-integration-mcp-counter:local`, image id `958e590b5ded`
- Public test URL: `http://127.0.0.1:8080`

Started with:

```bash
scripts/cf-integration.sh up
```

Result: pass. `/health` returned healthy. Running stack after tests:

- `cf-integration-nginx-1`: healthy
- `cf-integration-gateway-1`: healthy
- `cf-integration-postgres-1`: healthy
- `cf-integration-pgbouncer-1`: healthy
- `cf-integration-fast_time_server-1`: healthy
- `cf-integration-redis-1`: healthy
- `cf-integration-cf-dataplane-1`: running
- `cf-integration-cf-integration-mcp-counter-1`: running

Registered virtual servers:

- `9779b6698cbd4b4995ee04a4fab38737`: `Fast Time Server`
- `a88e2c3f5d7b4a9e8f1c6d2e3b4a5f6e`: `Fast Time SSE Server`

Registered tools are from `fast_time` and `fast_time_sse`; no `fast-test-*` tools were registered in this run.

## What Was Proven

The repo nginx config forwards these headers on the dataplane route:

- `Authorization`
- `Mcp-Session-Id`
- `Mcp-Protocol-Version`
- `Host`
- `X-Forwarded-Proto`
- `X-Forwarded-Host`

The route rewrite also works:

```text
/servers/{virtual_host_id}/mcp
-> /contextforge-rs/servers/{virtual_host_id}/mcp
```

### Real Dataplane Initialize

Command shape:

```bash
curl -i \
  -H "Authorization: Bearer <generated-token>" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -H "Mcp-Protocol-Version: 2025-06-18" \
  --data '{"jsonrpc":"2.0","id":"init-1","method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"cf-integration-report","version":"1.0"}}}' \
  http://127.0.0.1:8080/servers/9779b6698cbd4b4995ee04a4fab38737/mcp
```

Result:

```text
HTTP/1.1 200 OK
Content-Type: text/event-stream
mcp-session-id: <returned>
"protocolVersion":"2025-06-18"
"serverInfo":{"name":"rust-conformance-server","version":"0.1.0"}
```

### Auth Negative Control

Same request without `Authorization`:

```text
HTTP/1.1 401 Unauthorized
```

This matters because it proves the passing initialize was not anonymous fallback behavior. The dataplane saw auth state.

### Session Follow-Up

After initialize, a `tools/list` call with the returned `Mcp-Session-Id` succeeded through nginx.

Result:

```text
PASS real session follow-up
tools/list returned 6 tools
```

Returned tools included:

- `6e74192d84014a9fb8efe7e5822b5be8-echo`
- `6e74192d84014a9fb8efe7e5822b5be8-get_stats`
- `6e74192d84014a9fb8efe7e5822b5be8-schema_success`

### Real Tool Call

Tool call through the same dataplane route:

```text
tools/call name=6e74192d84014a9fb8efe7e5822b5be8-echo
```

Result:

```text
PASS real echo tool call
"text":"header passthrough real run"
"isError":false
```

### Raw Upstream Header Probe

A throwaway Docker network ran this repo's nginx config with a mock `cf-dataplane` upstream that echoed received headers.

Result:

```text
PASS header passthrough
path=/contextforge-rs/servers/header-pass/mcp?probe=1
authorization=Bearer header-pass-token
host=integration.example.test
mcp-protocol-version=2025-06-18
mcp-session-id=session-123
x-forwarded-host=public.example.test
x-forwarded-proto=https
```

This is the direct proof that nginx does not drop the number 1 headers.

## Full Run Results

### `live-all`

Command:

```bash
MCP_CLI_BASE_URL=http://127.0.0.1:8080 scripts/cf-integration.sh live-all
```

Result: fail.

Summary:

```text
270 collected
80 passed
68 failed
42 errors
75 skipped
5 xfailed
4 rerun
```

This is not a header passthrough failure.

Useful triage:

- `tests/live_gateway/mcp/test_mcp_protocol_e2e.py` calls `fast-test-echo`, `fast-test-get-stats`, `fast-test-schema-error`, and `fast-test-schema-success`.
- The actual stack registered only `fast_time` and `fast_time_sse` tool sets. The missing `fast-test-*` fixtures explain the MCP tool-call failures.
- Many protocol-compliance tests error with `RuntimeError: Runner.run() cannot be called from a running event loop`, including `reference-stdio` cases. Those errors happen before any nginx/dataplane header behavior can be inferred.
- SSO tests skipped because Keycloak/Azure credentials were not available.
- Rust-mode tests skipped because the gateway booted with `boot_mode='off'`, so runtime-mode flips were refused.

### `smoke`

Command:

```bash
MCP_VIRTUAL_SERVER_ID=9779b6698cbd4b4995ee04a4fab38737 scripts/cf-integration.sh smoke
```

Result: fail.

Summary:

```text
Total Requests: 14
Total Failures: 14 (100.00%)
1 POST MCP initialize: HTTP 406
9 POST MCP tools/list: HTTP 406
2 POST MCP resources/list: HTTP 406
2 POST MCP prompts/list: HTTP 406
```

Root cause found in the run:

```text
Accept: application/json            -> HTTP 406
Accept: application/json, text/event-stream -> HTTP 200
```

The locust file sends the bad header for MCP discovery and requests:

```python
headers = {"Authorization": f"Bearer {token}", "Accept": "application/json"}
...
"Content-Type": "application/json",
"Accept": "application/json",
```

That matches the failure. It does not implicate nginx header passthrough.

## Recommended Next Fixes

1. Update `tests/loadtest/locustfile_mcp_protocol.py` to send `Accept: application/json, text/event-stream` on streamable HTTP MCP requests, and parse SSE responses instead of assuming `response.json()`.
2. Split `live-all` for this integration harness into supported lanes:
   - dataplane `/servers/{id}/mcp` smoke
   - control-plane `/mcp/` live tests that match registered fixtures
   - optional SSO/Keycloak lane
   - optional Rust runtime mode lane
3. Either register the `fast_test_server` fixture in this harness or stop running tests that require `fast-test-*` tools.
4. Triage the protocol-compliance async fixture error separately, since it reproduces for `reference-stdio` and is not specific to nginx or dataplane.

## Final Status

Number 1 is fixed and verified by real traffic.

The test report should not be read as "the integration stack is green." It is not green. The remaining failures have different causes and need separate fixes.
