# Dataplane E2E Report — 2026-07-07

Current open issues only; everything previously fixed is in this file's
git history and the linked PRs.

## Stack under test

`scripts/cf-integration.sh test-all-up` at 07:59 UTC
(log: `.integration/test-logs/cf-tests-20260707T075927Z.log`):

- **Harness** at cf-integration `main` — nginx replays dataplane 400/404
  on `/servers/{id}/mcp` to the control plane, SSE registration runs
  stock (SSE-backed servers are served fully via the control-plane
  path), publisher interval 2s, dataplane config cache disabled,
  two-pass `live-all` lane.
- **cf-controlplane** built from `user/luca/dataplane-integration-fixes`
  — all open control-plane PRs:
  [#5482](https://github.com/IBM/mcp-context-forge/pull/5482)
  (publisher interval, allow-path convergence wait, non-empty asserts),
  [#5510](https://github.com/IBM/mcp-context-forge/pull/5510)
  (original_name in allowed_tool_names + select fix),
  [#5515](https://github.com/IBM/mcp-context-forge/pull/5515)
  (compliance fixture convergence wait),
  [#5517](https://github.com/IBM/mcp-context-forge/pull/5517)
  (per-worker lock id, TTL = 2×interval + 10),
  [#5519](https://github.com/IBM/mcp-context-forge/pull/5519)
  (streamable-HTTP-only publishing),
  [#5523](https://github.com/IBM/mcp-context-forge/pull/5523)
  (admin own-private listing test aligned with owner matching —
  the long-standing live-rbac failure was a stale test, not a service
  bug: issue #4694 / commit 8c186c5e0 deliberately made owner-matched
  private rows visible),
  [#5539](https://github.com/IBM/mcp-context-forge/pull/5539)
  (plugin E2E suites skip cleanly when the gateway has no enabled
  plugins).
  [#5514](https://github.com/IBM/mcp-context-forge/pull/5514) is the
  draft combined-diff view.
- **cf-dataplane** `cf-dataplane:pr54` — local arm64 build of
  [contextforge-gateway-rs #54](https://github.com/contextforge-gateway-rs/contextforge-gateway-rs/pull/54)
  (optional `token_use`/`full_name` claims).

## Lane summary

| Lane          | Result | Detail |
|---------------|--------|--------|
| probe         | PASS   | 401 negative, initialize, tools/list, tools/call via nginx→dataplane |
| smoke         | PASS   | 1-user locust, 10s, streamable HTTP, 0 failures |
| live-mcp      | PASS   | 19 passed, 3 skipped |
| live-rbac     | **PASS** | 40 passed — first fully green run |
| live-protocol | FAIL   | 2 failed / 23 passed / 0 errors |
| live-all      | FAIL   | pass 1: 4 failed / 105 passed / 105 skipped / 0 errors · pass 2: 40 passed |

Zero errors anywhere; every failure has a known root cause. Both
remaining failure classes are listed below.

## Open issues

### 1. Dataplane advertises backend-UUID tool names (dataplane — the last blocker)

Root cause of **5 of the 6 remaining failures**:
`test_required_tools_advertised[gateway_virtual-http]` and
`test_list_tools_returns_all_stubs[gateway_virtual-http]` (both lanes)
plus `test_drift_add_call` (gateway_virtual leg reports
`add not advertised`), and ~11 "tool not advertised" skips. The
dataplane serves sessions and filters correctly (#5510) but exposes
tools as `<backend-uuid>-<name>` instead of the control plane's naming.
Fix in contextforge-gateway-rs `list_tools` response mapping.

### 2. `test_templated_resource_registered_and_resolves[gateway_proxy-http]` (control plane, triage)

Deterministic across runs: reading the templated resource
`reference://users/7` through the **gateway proxy** returns content
whose text is not the upstream JSON payload (`json.loads` fails at
char 0). Control-plane resource federation path, unrelated to the
dataplane. Needs root-cause work before a fix PR.

### 3. D1 — sliding-TTL user-config cache (dataplane)

`lru_time_cache::get_mut` renews the TTL on every hit; steady traffic
pins stale config indefinitely. Harness disables the cache; real fix is
insertion-based TTL upstream.

### 4. D6 — backend-unavailable swallowed as empty success (dataplane)

All-streamable-backends-down still yields `tools/list` 200 + `[]`.
Narrower since unsupported transports never reach the dataplane, but
the failure mode remains for genuinely down backends.

### 5. Remaining publisher design gaps (control plane)

Beyond the open PR set: no event-driven publish (new entities wait up
to one snapshot interval) and the publish loop shares the serving event
loop (timers fire late under load). Design changes, not patches.

### 6. No single token works on both planes (cross-plane contract)

The dataplane accepts control-plane admin tokens (rs#51 + rs#54); the
control-plane admin REST still rejects dataplane-scoped tokens — the
reason `cf-jwt.py` keeps its `--admin` flag.

## Expected next state

- Open PR set (IBM #5482/#5510/#5515/#5517/#5519/#5523/#5539 + rs#54)
  merges → these numbers on stock image pulls.
- Issue 1 (dataplane tool naming) → live-protocol green and live-all
  down to issue 2 only.
- Issue 2 root-caused and fixed → full suite green.
