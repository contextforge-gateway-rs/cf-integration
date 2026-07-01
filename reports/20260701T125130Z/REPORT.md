# cf-controlplane + cf-dataplane Integration Test Report

Run date: 2026-07-01  
Harness commit: `40e3936`  
Control-plane checkout: `70c31bd6d`  
Dataplane image: `ghcr.io/contextforge-gateway-rs/contextforge-gateway-rs:0.1.0`  
Raw logs: `/tmp/cf-integration-report-20260701T125130Z`

## Executive Summary

The harness wiring is mostly correct:

- Compose stack starts.
- nginx routes `/health` and raw `/mcp` to `cf-controlplane`.
- nginx routes `/servers/{id}/mcp` and `/servers/{id}/mcp/` to `cf-dataplane`.
- `cf-dataplane` stays running under full Locust load.

The current blocker is not an nginx issue. Virtual-server MCP traffic reaches Rust and then fails before MCP fanout because Rust cannot retrieve/load the user config for the JWT subject:

```text
HTTP/1.1 500 Internal Server Error
Problem occurred retrieving the configuration
```

Until the control-plane publisher/schema contract is fixed, these tests cannot measure real dataplane MCP behavior behind a virtual server.

## Command Results

| Command | Exit | Result |
| --- | ---: | --- |
| routing probes | 0 | nginx split works; virtual path reaches Rust and fails config lookup |
| `smoke` | 1 | 67/67 MCP requests failed |
| `locust` | 1 | 276,779/276,779 MCP requests failed |
| `live-mcp` | 2 | 15 passed, 4 failed, 3 skipped |
| `live-rbac` | 2 | 37 passed, 3 failed |
| `live-protocol` | 2 | 15 passed, 4 failed, 14 errors, 2 skipped, 29 xfailed, 2 xpassed |
| `live-all` | 2 | 81 passed, 68 failed, 42 errors, 74 skipped |

## Preflight And Routing

Observed:

- `GET /health`: `200 OK`, control-plane health response.
- `POST /mcp`: `403 CSRF validation failed`, proving raw `/mcp` stays on control-plane.
- `POST /servers/9779b6698cbd4b4995ee04a4fab38737/mcp`: `500 Problem occurred retrieving the configuration`.
- `POST /servers/9779b6698cbd4b4995ee04a4fab38737/mcp/`: same `500`.
- `GET /servers` with the harness JWT: `403 Access denied`.

Interpretation:

- nginx routing is correct.
- The harness JWT is enough for Rust auth but not enough for control-plane admin API discovery.
- The virtual-server path is blocked by Rust config retrieval, not by routing.

## Locust

Smoke:

```text
Total Requests: 67
Total Failures: 67 (100.00%)
Failures: initialize, tools/list, tools/call, resources/list, prompts/list, ping all HTTP 500
```

Full run:

```text
Total Requests: 276,779
Total Failures: 276,779 (100.00%)
Requests/sec: ~923
p50: 1 ms
p99: 8 ms
```

Failure distribution:

- Most requests are HTTP 500 from Rust config retrieval.
- Session churn paths also show HTTP 403, likely because churn requests hit auth/session paths after failed initialization.
- One important positive signal: the Rust process did not crash under the failed-request load.

Classification: setup/control-plane contract blocker first, dataplane behavior unmeasured.

## `live-mcp`

This target exercises raw `/mcp`, so it is mostly control-plane, not Rust dataplane.

Result:

```text
4 failed, 15 passed, 3 skipped
```

Failures:

- `TestToolCalls::test_echo`: `Tool not found: fast-test-echo`
- `TestToolCalls::test_get_stats`: `Tool not found: fast-test-get-stats`
- schema error/success tests: `fast-test-schema-*` tools not registered

Classification: setup/control-plane fixture mismatch. The test log explicitly says to rebuild `fast_test_server` and restart compose so `register_fast_test` picks up schema fixtures.

## `live-rbac`

Result:

```text
3 failed, 37 passed
```

Failures:

- `test_admin_sees_public_and_team_via_http`
  - Admin can see a private server through HTTP API.
  - Classification: control-plane RBAC/API behavior, not Rust dataplane.

- `test_public_token_accesses_public_server`
  - `/servers/{public_id}/mcp/` returns HTTP 400.

- `test_team_member_accesses_team_server`
  - `/servers/{team_id}/mcp/` returns HTTP 400.

Classification:

- The two per-server endpoint failures are dataplane-path failures, but they fail before MCP semantics. They look like the same config/user publication problem as Locust.
- This also shows the publisher/config path must support non-admin test users and RBAC-scoped virtual servers, not only the admin subject.

## `live-protocol`

This target is the useful protocol compliance subset for gateway virtual-server behavior.

Result:

```text
4 failed, 15 passed, 2 skipped, 76 deselected, 29 xfailed, 2 xpassed, 14 errors
```

Main error:

```text
HTTPStatusError: 401 Unauthorized
http://127.0.0.1:8080/servers/1125c73c7e8f4ed283818e5d9b397866/mcp/
```

Affected categories:

- lifecycle
- ping
- prompts
- protocol version
- resources
- tools
- transport semantics

Classification: setup/auth/config. The compliance target's generated token/server setup does not satisfy the Rust dataplane auth/config requirements, so the virtual target cannot initialize. This is not yet evidence of missing Rust MCP features.

Known xfails in control-plane compliance still matter for future dataplane parity:

- notification relay on POST-correlated streams
- progress notifications
- client roots forwarding
- sampling/elicitation broker behavior
- prompt federation
- dropped mutation/cancellation tools
- subscription update streaming

Those are real dataplane parity areas, but this run did not reach Rust far enough to validate them.

## `live-all`

Result:

```text
68 failed, 81 passed, 74 skipped, 5 xfailed, 42 errors
```

This is not clean as a dataplane signal because it mixes:

- raw control-plane `/mcp`
- gateway proxy
- gateway virtual
- reference stdio
- SSO tests
- protocol compliance tests with known xfails

Additional setup/test-runner failures:

- Many protocol compliance errors across `reference-stdio`, `gateway_proxy-http`, and `gateway_virtual-http` are `RuntimeError: Runner.run() cannot be called from a running event loop`.
- This points to test harness/plugin interaction when running the full suite, not Rust dataplane.
- Several raw control-plane failures are the same missing `fast-test-*` fixture tools.

Use `live-all` as a broad regression smoke only. Use the targeted commands above for dataplane diagnosis.

## Failure Classification

### Setup / Control-Plane Contract To Fix First

1. Rust-compatible Redis `UserConfig` publication is not working for the tested subjects.
   - Evidence: every Locust virtual-server request returns config retrieval failure.
   - Evidence: direct virtual-server curl returns config retrieval failure.
   - Likely related to the pending control-plane null/schema changes.

2. Token/user coverage for publisher output is incomplete.
   - Evidence: RBAC-created public/team server per-server endpoints fail on Rust path.
   - Evidence: protocol compliance generated token gets 401 at Rust path.

3. Harness/admin API token cannot call `GET /servers`.
   - Evidence: `403 Access denied`.
   - Fix either the helper JWT scopes or use a control-plane-generated admin token for API discovery.

4. Raw control-plane fast-test fixtures are stale or not rebuilt.
   - Evidence: `fast-test-echo`, `fast-test-get-stats`, and schema tools are not registered.
   - Fix by rebuilding helper images / rerunning registration from the current control-plane checkout.

5. `live-all` has pytest async runner errors across non-Rust targets.
   - Evidence: `Runner.run() cannot be called from a running event loop`.
   - Treat as control-plane test-suite/harness issue.

### Dataplane Gaps We Can Infer, But Not Fully Validate Yet

1. Dataplane config errors need better observability.
   - Current response is generic.
   - Current logs at `RUST_LOG=info` only show tower 500 status, not whether Redis key is missing, MessagePack decode failed, or a schema field failed.

2. Dataplane must support all control-plane-published subjects used by virtual-server tests.
   - Admin, public user, team user, protocol compliance user.

3. Once config load works, validate these likely parity areas:
   - initialize/session lifecycle
   - tools/list and tools/call
   - resources/list and resources/templates/list
   - prompts/list
   - ping
   - pagination
   - POST-correlated stream notifications
   - progress notifications
   - roots forwarding
   - sampling and elicitation
   - cancellation
   - subscriptions/SSE update behavior

## Recommended Next Steps

1. Merge/apply the control-plane publisher/schema fix, especially nullable passthrough/header fields.
2. Add a small Redis inspection command or test helper that confirms the exact Rust MessagePack key/value exists for a JWT subject.
3. Run one direct `initialize` against `/servers/{id}/mcp` with `RUST_LOG=debug` before rerunning full Locust.
4. Fix the harness/admin token so it can list `/servers`; then Locust can auto-select a real virtual server.
5. Rebuild control-plane fast-test fixtures so raw `/mcp` tests stop failing on missing `fast-test-*` tools.
6. Only after config load succeeds, rerun `smoke`, `locust`, `live-rbac`, and `live-protocol` to classify true Rust dataplane MCP parity gaps.
