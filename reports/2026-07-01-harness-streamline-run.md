# Harness Streamline Run

Run date: 2026-07-01, Europe/Dublin.

Baseline: `bb9e383` plus the harness streamline changes (verified from the working tree before commit).

## Scope

Follow-up to [2026-07-01-ghcr-fast-time-run.md](2026-07-01-ghcr-fast-time-run.md). That report left two red lanes: `smoke` (HTTP 406 content negotiation) and `live-all` (upstream fixture mismatches). This run verifies the streamlined harness:

- `probe` command codifying the direct dataplane probe
- harness-owned `scripts/locustfile_cf_dataplane.py` with `Accept: application/json, text/event-stream`
- `MCP_VIRTUAL_SERVER_ID` defaulting to the auto-registered Fast Time server
- `live-core` curated lane (MCP protocol E2E minus fixture-bound `TestToolCalls`)
- collapsed nginx dataplane location (`^/servers/([^/]+)/mcp/?$`)
- deduplicated JWT minting (`cf_jwt.make_token` shared by CLI, probe, register job)

## Verdict

All harness-owned lanes are green with zero manual steps after `up`:

```text
scripts/cf-integration.sh probe      PASS (exit 0)
scripts/cf-integration.sh smoke      PASS (69 requests, 0 failures)
scripts/cf-integration.sh live-core  PASS (12 passed, 3 skipped, 7 deselected)
scripts/cf-integration.sh live-all   RED  (unchanged upstream fixture mismatches)
```

## Probe

```bash
scripts/cf-integration.sh probe
```

```text
probe url: http://127.0.0.1:8080/servers/9779b6698cbd4b4995ee04a4fab38737/mcp
auth_negative=PASS status=401
initialize=PASS status=200 session=043433f6-0fab-4cd5-86df-4a752822d55a
tools_list=PASS count=6
tool_call=PASS tool=76b29617e21c4bd69834e4424a0f7d08-echo
```

This exercises the full public route through the collapsed nginx location:
`nginx :8080 -> /servers/{id}/mcp -> cf-dataplane -> fast_time_server /mcp`.
The trailing-slash variant `/servers/{id}/mcp/` was separately confirmed to
reach the dataplane (401 without auth).

Note: the dataplane requires the `scopes` claim in the JWT; a token without it
is rejected with 401. `cf-jwt.py`, `cf-probe.py`, and the report probe all mint
tokens with `permissions: [servers.use, tools.read, tools.call]`.

## Smoke

```bash
scripts/cf-integration.sh smoke
```

Exit code: `0`. Previously 19/19 failures (HTTP 406).

```text
POST MCP initialize                  1   0(0.00%)
POST MCP ping                       10   0(0.00%)
POST MCP tools/call echo            25   0(0.00%)
POST MCP tools/call get_system_time 15   0(0.00%)
POST MCP tools/list                 18   0(0.00%)
Aggregated                          69   0(0.00%)  avg 18ms  max 41ms
```

The 406s were caused by the upstream `locustfile_mcp_protocol.py` sending
`Accept: application/json` to the streamable HTTP endpoint. The harness now
ships `scripts/locustfile_cf_dataplane.py`, which negotiates
`application/json, text/event-stream` and parses both response forms. The
upstream file remains available via `LOCUST_LOCUSTFILE=locustfile_mcp_protocol.py`.

## Live Core

```bash
scripts/cf-integration.sh live-core
```

Exit code: `0`.

```text
12 passed, 3 skipped, 7 deselected in 2.53s
```

- Deselected: `TestToolCalls` (7 tests) — hard-coded `fast-time-*`/`fast-test-*`
  tool names; this stack registers gateway-id-prefixed tools.
- Skipped: no resources/prompts registered; Rust public transport not active.

## Live All

```bash
scripts/cf-integration.sh live-all
```

Exit code: non-zero.

```text
65 failed, 70 passed, 75 skipped, 5 xfailed, 55 errors, 7 rerun in 28.27s
```

Identical failure buckets to the previous report; none are harness routing
failures:

1. RBAC transport tests register `http://fast_time_server:8080/sse`; the GHCR
   Fast Time image serves `/mcp` only.
2. Tool-call tests expect `fast-test-*` fixture tools that this stack does not
   register.
3. Protocol-compliance async fixture error (`Runner.run() cannot be called from
   a running event loop`) reproduces on `reference-stdio`, i.e. independent of
   the gateway.
4. Optional SSO/runtime lanes are not configured (Azure, Keycloak, boot mode).

These need upstream `cf-controlplane` suite/config changes and are out of scope
for this harness.

## Final Status

The harness green path is now `up` -> `probe` -> `smoke` -> `live-core`, all
passing with defaults and no UI interaction. `live-all` remains the known-red
upstream lane and is documented as such in the README.
