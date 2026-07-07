# Dataplane E2E Report — 2026-07-07

Current open issues only; everything previously fixed is in this file's
git history and the linked PRs.

## Stack under test

`scripts/cf-integration.sh test-all-up-no-plugins` at 12:36 UTC on a
**fresh stack** (log: `.integration/test-logs/cf-tests-20260707T123627Z.log`):

- **Harness** — nginx replays dataplane 400/404 on `/servers/{id}/mcp`
  to the control plane, SSE registration runs stock, publisher interval
  2s, dataplane config cache disabled, two-pass `live-all` lane. New
  `test-all-up-no-plugins` command deselects
  `tests/live_gateway/plugins` (`CF_TEST_PLUGINS=false`): those suites
  need a gateway booted with a plugin enforce config, which this stack
  intentionally does not run, so under plain `test-all` their failures
  stay visible as the honest signal.
- **cf-controlplane** built from `user/luca/dataplane-integration-fixes`
  — all open control-plane PRs:
  [#5482](https://github.com/IBM/mcp-context-forge/pull/5482),
  [#5510](https://github.com/IBM/mcp-context-forge/pull/5510),
  [#5515](https://github.com/IBM/mcp-context-forge/pull/5515),
  [#5517](https://github.com/IBM/mcp-context-forge/pull/5517),
  [#5519](https://github.com/IBM/mcp-context-forge/pull/5519),
  [#5523](https://github.com/IBM/mcp-context-forge/pull/5523).
  [#5514](https://github.com/IBM/mcp-context-forge/pull/5514) is the
  draft combined-diff view.
- **cf-dataplane** `cf-dataplane:pr54` — local arm64 build of
  [contextforge-gateway-rs #54](https://github.com/contextforge-gateway-rs/contextforge-gateway-rs/pull/54).

## Lane summary

| Lane          | Result | Detail |
|---------------|--------|--------|
| probe         | PASS   | 401 negative, initialize, tools/list, tools/call via nginx→dataplane |
| smoke         | PASS   | 1-user locust, 10s, streamable HTTP, 0 failures |
| live-mcp      | PASS   | 19 passed, 3 skipped |
| live-rbac     | PASS   | 40 passed |
| live-protocol | **PASS** | 28 passed / 4 skipped / 0 failed |
| live-all      | FAIL   | pass 1: **1 failed** / 111 passed · pass 2: 40 passed |

**One failure remains in the entire suite.**

Notable: the previous run's `gateway_virtual` tool-naming failures
(`test_required_tools_advertised`, pagination stubs, the drift probe)
**do not reproduce on a fresh database** — they were artifacts of
long-lived stack state (repeated compliance-fixture register/delete
cycles leave renamed tool rows whose published `allowed_tool_names` no
longer match the upstream's advertised names). On a clean stack the
dataplane serves the compliance virtual server fully.

## Open issues

### 1. `test_templated_resource_registered_and_resolves[gateway_proxy-http]` (control plane, triage)

The sole failing test. Deterministic: reading the templated resource
`reference://users/7` through the **gateway proxy** returns content
whose text is not the upstream JSON payload (`json.loads` fails at
char 0). Control-plane resource federation path, unrelated to the
dataplane. Needs root-cause work before a fix PR.

### 2. Stale-state tool renames break dataplane filtering (control plane, low priority)

The reframed remainder of the old "tool naming" issue: after many
register/delete cycles of the same gateway on a long-lived database,
published `allowed_tool_names` can stop matching the upstream's
advertised tool names and the dataplane then filters everything out
(sessions fine, 0 tools). Does not affect fresh deployments; worth a
look at how tool `original_name` survives re-registration conflicts.

### 3. D1 — sliding-TTL user-config cache (dataplane)

`lru_time_cache::get_mut` renews the TTL on every hit; steady traffic
pins stale config indefinitely. Harness disables the cache; real fix is
insertion-based TTL upstream.

### 4. D6 — backend-unavailable swallowed as empty success (dataplane)

All-streamable-backends-down still yields `tools/list` 200 with an
empty list; clients cannot distinguish it from a tool-less server.

### 5. Remaining publisher design gaps (control plane)

No event-driven publish (new entities wait up to one snapshot interval)
and the publish loop shares the serving event loop (timers fire late
under load). Design changes, not patches.

### 6. No single token works on both planes (cross-plane contract)

The dataplane accepts control-plane admin tokens (rs#51 + rs#54); the
control-plane admin REST still rejects dataplane-scoped tokens — the
reason `cf-jwt.py` keeps its `--admin` flag.

## Expected next state

- Open PR set (IBM #5482/#5510/#5515/#5517/#5519/#5523 + rs#54) merges
  → these numbers on stock image pulls.
- Issue 1 root-caused and fixed → **full suite green** under
  `test-all-up-no-plugins`.
