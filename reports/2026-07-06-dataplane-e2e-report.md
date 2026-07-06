# Dataplane E2E Report — 2026-07-06

Current open issues only; resolved findings live in this file's git
history and the linked PRs.

## Stack under test

Latest full `test-all` (log: `.integration/test-logs/`, 16:0x UTC run):

- **cf-controlplane** built from `user/luca/dataplane-integration-fixes`
  — the consolidated feature branch of all open control-plane PRs
  ([#5482](https://github.com/IBM/mcp-context-forge/pull/5482) publisher
  interval + allow-path wait,
  [#5510](https://github.com/IBM/mcp-context-forge/pull/5510)
  original_name in allowed_tool_names + select fix,
  [#5515](https://github.com/IBM/mcp-context-forge/pull/5515) compliance
  fixture convergence wait,
  [#5516](https://github.com/IBM/mcp-context-forge/pull/5516) non-empty
  allow-path asserts,
  [#5517](https://github.com/IBM/mcp-context-forge/pull/5517) per-worker
  lock id + TTL margin,
  [#5519](https://github.com/IBM/mcp-context-forge/pull/5519)
  streamable-HTTP-only publishing).
  [#5514](https://github.com/IBM/mcp-context-forge/pull/5514) is the
  draft combined-diff view.
- **cf-dataplane** `cf-dataplane:pr54` — local arm64 build of
  [contextforge-gateway-rs #54](https://github.com/contextforge-gateway-rs/contextforge-gateway-rs/pull/54).
- **Harness**: publisher interval 2s, dataplane config cache disabled,
  and nginx now **replays dataplane 400/404 responses on the control
  plane** for `/servers/{id}/mcp` — combined with #5519, SSE-backed
  virtual servers (SSE is deprecated; removed in the 2026-07-28 MCP
  protocol update) are absent from dataplane config and served fully by
  the control plane. Verified live: the Fast Time SSE server initializes
  and lists tools through the fallback, and the RBAC allow-path tests
  pass **with non-empty tool lists**.
- The `live-all` lane runs upstream `tests/live_gateway/` directly in
  two pytest passes (asyncio suites with `-p no:playwright`, then the
  two playwright-dependent suites), replacing `make test-live-gateway`,
  whose single `-p playwright` pass broke every asyncio test
  (~103 × `Runner.run() cannot be called from a running event loop`).
  Harness-side only; upstream Makefile intentionally unchanged.

## Lane summary

| Lane          | Result | Detail |
|---------------|--------|--------|
| probe         | PASS   | 401 negative, initialize, tools/list, tools/call via nginx→dataplane |
| smoke         | PASS   | 1-user locust, 10s, streamable HTTP, 0 failures |
| live-mcp      | PASS   | 19 passed, 3 skipped |
| live-rbac     | FAIL   | 1 failed / 39 passed — visibility contract mismatch only |
| live-protocol | FAIL   | 2 failed / 23 passed / 14 skipped / **0 errors** (fixture race gone) |
| live-all      | FAIL   | 4 failed / 105+ passed / 36 errors — errors are the plugin fixtures |

## Open issues

### 1. D7 (remaining half) — dataplane exposes backend-UUID tool names (dataplane)

Publisher side fixed (#5510 + select fix): filters match and the probe
lists **and calls** tools through the dataplane. The dataplane still
advertises tools as `<backend-uuid>-<name>` instead of the control
plane's naming, so the compliance rows `test_required_tools_advertised`
and the pagination stub checks fail and ~14 tests skip with "tool not
advertised" despite live sessions. Fix in contextforge-gateway-rs
`list_tools` response mapping. Largest remaining item.

### 2. Visibility contract mismatch in `/servers` listing (control plane)

`test_admin_sees_public_and_team_via_http` — the REST listing does not
honor the PR #4341 visibility contract for admin sessions. Reproduces on
the stock control-plane-only stack; needs its own upstream PR. Last
failure in live-rbac.

### 3. Plugin E2E fixtures unusable under compose (upstream tests)

The 36 live-all errors: `tests/live_gateway/plugins` fixtures
self-register the gateway at `http://localhost:8080/mcp`, unreachable
from inside the gateway container (502), and require
`PLUGINS_CONFIG_FILE` at stack boot. Broken in any compose-based run;
needs upstream fixture parameterization.

### 4. Two live-all failures to triage (upstream)

Now that the lane is meaningful, two failures beyond D7 need triage:
`test_drift.py::test_drift_add_call` and
`test_resources.py::test_templated_resource_registered_and_resolves[gateway_proxy-http]`.
Not yet root-caused; may be genuine gateway-proxy gaps or test-order
artifacts.

### 5. D1 — sliding-TTL user-config cache (dataplane)

`lru_time_cache::get_mut` renews TTL on hit; steady traffic pins stale
config. Harness works around with cache=0; real fix is insertion-based
TTL.

### 6. D6 — backend-unavailable swallowed as empty success (dataplane)

All-backends-down still yields `tools/list` 200 + `[]`. Less pressing
now that unsupported transports are excluded at publish time, but the
failure mode remains for genuinely down streamable backends.

### 7. Remaining publisher gaps (control plane)

After #5482/#5517/#5519: no event-driven publish (new entities wait for
the next snapshot) and the publish loop shares the serving event loop
(timers fire late under load). Both are design changes beyond the
current PR set.

### 8. No single token works on both planes (cross-plane contract)

Dataplane accepts control-plane admin tokens (rs#51 + rs#54); the
inverse still fails (dataplane-scoped token rejected by control-plane
admin REST) — why `cf-jwt.py` keeps `--admin`.

## Expected next state

- Open PR set merges (+ rs#54 image) → these numbers on stock pulls.
- Issue 1 (dataplane naming) → live-protocol green; live-all failures
  down to issues 3/4.
- Issue 2 fix → live-rbac green.
