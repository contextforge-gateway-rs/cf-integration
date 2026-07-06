# Dataplane E2E Report — 2026-07-06

Current open issues only; everything previously fixed is in this file's
git history and the linked PRs.

## Stack under test

`scripts/cf-integration.sh test-all-up` at 15:24 UTC
(log: `.integration/test-logs/cf-tests-20260706T152417Z.log`):

- **Harness** at `6da466e` — nginx replays dataplane 400/404 on
  `/servers/{id}/mcp` to the control plane (404 = vhost absent from
  dataplane config, 400 = subject config not yet published), SSE
  registration runs stock, publisher interval 2s, dataplane config cache
  disabled, and the `live-all` lane runs upstream `tests/live_gateway/`
  in two pytest passes (asyncio suites with `-p no:playwright`, then
  sso + RBAC transport with the plugin) instead of
  `make test-live-gateway`, whose single `-p playwright` pass broke
  every asyncio test.
- **cf-controlplane** built from `user/luca/dataplane-integration-fixes`
  (`4016f2bd3`) — the combination of all open control-plane PRs:
  [#5482](https://github.com/IBM/mcp-context-forge/pull/5482)
  (configurable publisher interval, allow-path convergence wait,
  non-empty tool asserts),
  [#5510](https://github.com/IBM/mcp-context-forge/pull/5510)
  (original_name in allowed_tool_names, incl. select fix),
  [#5515](https://github.com/IBM/mcp-context-forge/pull/5515)
  (compliance fixture convergence wait),
  [#5517](https://github.com/IBM/mcp-context-forge/pull/5517)
  (per-worker lock id, TTL = 2×interval + 10),
  [#5519](https://github.com/IBM/mcp-context-forge/pull/5519)
  (streamable-HTTP-only publishing, backendless vhosts omitted).
  [#5514](https://github.com/IBM/mcp-context-forge/pull/5514) is the
  draft combined-diff view.
- **cf-dataplane** `cf-dataplane:pr54` — local arm64 build of
  [contextforge-gateway-rs #54](https://github.com/contextforge-gateway-rs/contextforge-gateway-rs/pull/54)
  (optional `token_use`/`full_name` claims).

SSE stays fully functional: the dataplane will not implement it
(deprecated; removed in the 2026-07-28 MCP protocol update), so
SSE-backed virtual servers are absent from dataplane config (#5519) and
nginx serves them via the control plane — verified live, including the
RBAC allow-path tests passing with non-empty tool lists.

## Lane summary

| Lane          | Result | Detail |
|---------------|--------|--------|
| probe         | PASS   | 401 negative, initialize, tools/list, tools/call via nginx→dataplane |
| smoke         | PASS   | 1-user locust, 10s, streamable HTTP, 0 failures |
| live-mcp      | PASS   | 19 passed, 3 skipped |
| live-rbac     | FAIL   | 1 failed / 39 passed |
| live-protocol | FAIL   | 2 failed / 23 passed / 11 skipped / 0 errors |
| live-all      | FAIL   | pass 1: 3 failed / 106 passed / 36 errors · pass 2: 1 failed / 39 passed |

Every failure below is accounted for; there is no unexplained noise left
in any lane.

## Open issues

### 1. Dataplane advertises backend-UUID tool names (dataplane — next fix)

`test_required_tools_advertised[gateway_virtual-http]` and
`test_list_tools_returns_all_stubs[gateway_virtual-http]` (both lanes),
plus ~11 "tool not advertised" skips. Sessions establish, the backend
fetch returns all tools, filtering works (#5510), but the dataplane
exposes tools as `<backend-uuid>-<name>` instead of the control plane's
naming, so name-based checks miss. Fix in contextforge-gateway-rs
`list_tools` response mapping. Sole blocker for a green live-protocol.

### 2. Visibility contract mismatch in `/servers` listing (control plane)

`test_admin_sees_public_and_team_via_http` — the REST listing does not
honor the PR #4341 visibility contract for admin sessions. Reproduces
identically on the stock control-plane-only stack. Needs its own
upstream PR. Sole blocker for a green live-rbac.

### 3. Plugin E2E fixtures unusable under compose (upstream tests)

All 36 live-all errors: `tests/live_gateway/plugins` fixtures
self-register the gateway at `http://localhost:8080/mcp`, which is
unreachable from inside the gateway container (502), and require
`PLUGINS_CONFIG_FILE` at stack boot. Needs upstream fixture
parameterization; broken in any compose-based run.

### 4. `test_templated_resource_registered_and_resolves[gateway_proxy-http]` (triage)

Failed in both meaningful live-all runs; not yet root-caused. Runs on
the gateway_proxy target, so it is control-plane behavior, unrelated to
the dataplane. (`test_drift_add_call` failed once and passed on rerun —
watching for flakiness rather than tracking as an issue.)

### 5. D1 — sliding-TTL user-config cache (dataplane)

`lru_time_cache::get_mut` renews the TTL on every hit, so steady
traffic pins stale config indefinitely. The harness disables the cache;
the real fix is insertion-based TTL upstream.

### 6. D6 — backend-unavailable swallowed as empty success (dataplane)

When all of a vhost's streamable backends fail to initialize,
`tools/list` still returns 200 with an empty list. Narrower now that
unsupported transports never reach the dataplane, but the failure mode
remains for genuinely down backends.

### 7. Remaining publisher design gaps (control plane)

Beyond the merged/open PR set: no event-driven publish (new entities
wait up to one snapshot interval) and the publish loop shares the
serving event loop (timers fire late under load). Design changes, not
patches.

### 8. No single token works on both planes (cross-plane contract)

The dataplane accepts control-plane admin tokens (rs#51 + rs#54), but
control-plane admin REST still rejects dataplane-scoped tokens — the
reason `cf-jwt.py` keeps its `--admin` flag.

## Expected next state

- Open PR set (IBM #5482/#5510/#5515/#5517/#5519 + rs#54) merges →
  these numbers on stock image pulls.
- Issue 1 → live-protocol green; live-all failures reduce to issues 3–4.
- Issue 2 → live-rbac green.
