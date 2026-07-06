# Dataplane E2E Report — 2026-07-06

Current open issues only. Everything resolved since the 2026-07-02 report
(hollow-200 initialize, scopes/token_use/full_name 401s, publisher-latency
test flakes, frozen-cache staleness workaround) is documented in git
history of this file and in the linked PRs.

## Stack under test

`scripts/cf-integration.sh test-all-up` at 10:28 UTC
(log: `.integration/test-logs/cf-tests-20260706T102814Z.log`):

- **cf-controlplane** built from the
  [PR #5482](https://github.com/IBM/mcp-context-forge/pull/5482) branch
  (publisher interval configurable; harness overlay sets 2s).
- **cf-dataplane** `cf-dataplane:pr54` — local arm64 build of
  [contextforge-gateway-rs #54](https://github.com/contextforge-gateway-rs/contextforge-gateway-rs/pull/54)
  (optional `token_use`/`full_name` claims).
- Harness overlay: publisher interval 2s, dataplane user-config cache
  disabled (`CF_DATAPLANE_USER_CONFIG_CACHE_EXPIRY_SECONDS=0`).

Both PRs are open; once they merge, a stock `test-all-up` with published
images should reproduce these numbers.

## Lane summary

| Lane          | Result | Detail |
|---------------|--------|--------|
| probe         | PASS   | 401 negative, initialize, tools/list, tools/call via nginx→dataplane |
| smoke         | PASS   | 1-user locust, 10s, streamable HTTP, 0 failures |
| live-mcp      | PASS   | 19 passed, 3 skipped |
| live-rbac     | FAIL   | 1 failed / 39 passed — control-plane visibility regression only |
| live-protocol | FAIL   | 2 failed + 2 errors / 21 passed / 11 skipped |
| live-all      | FAIL   | 62 failed + 78 errors — harness noise (playwright collision, plugin fixtures) |

## Open issues

### 1. D7 — allowed_tool_names filtering empties runtime-registered vhosts (dataplane)

The 2 live-protocol failures (`test_list_tools_returns_all_stubs`,
`test_required_tools_advertised`) plus 11 "tool not advertised" skips.
Sessions establish and the dataplane fetches the backend's tools
(`list_tools: backend … completed (136 items)`) but returns **0 tools**
to the client. Published `allowed_tool_names` are control-plane slugs
(`fast-time-echo`) while upstreams advertise bare names (`echo`);
pre-registered Fast Time vhosts filter correctly, the compliance
harness's runtime-registered reference upstream filters to zero. Needs
isolation against a live fixture before teardown; likely in the
dataplane's list_tools prefix/filter mapping.

### 2. Compliance fixtures race the publisher (upstream tests)

The 2 live-protocol errors: the first two `gateway_virtual` rows 404
("Session terminated") at setup because the compliance conftest connects
to the fixture vhost before even the 2s publisher has it in Redis — those
fixtures have no convergence retry. On a stock 60s-publisher stack this
flakes far more often. Fix: apply the same deadline-wait used by PR #5482
to `tests/live_gateway/protocol_compliance/fixtures/gateway_live.py`.

### 3. Private-server visibility regression (control plane)

`test_admin_sees_public_and_team_via_http` — the `/servers` REST listing
shows admin another user's private server, violating the PR #4341
contract. Reproduces identically on the stock control-plane-only stack.

### 4. SSE-backed fixtures in upstream tests (upstream tests; dataplane SSE is won't-fix)

Decision: the dataplane will **not** implement SSE upstreams — the
transport is deprecated and is removed in the 2026-07-28 MCP protocol
update. The harness now profile-gates the stock `register_fast_time_sse`
job off and removed the Fast Time SSE fixed virtual server, so no
SSE-backed vhost reaches the dataplane from the stack itself (verified:
Redis UserConfig contains only `STREAMABLEHTTP` backends; probe green).

Residue: the upstream live-rbac suite registers its **own** SSE gateway
(`mcp-rbac-sse-gw`) inside its fixtures, so its allow-path tests still
pass hollow (0 tools) through the dataplane. Fix belongs upstream: switch
those fixtures to the streamable-HTTP endpoint
(`http://fast_time_server:8080/http`). Until SSE-registered gateways
disappear, the publisher exporting SSE backends into UserConfig remains
misleading — excluding non-supported transports at publish time would
make the config honest.

### 5. D6 — backend-unavailable swallowed as empty success (dataplane)

When all backends of a vhost fail to initialize, `tools/list` returns
success with an empty list. Clients and tests cannot distinguish "no
tools configured" from "all backends down". Fix: JSON-RPC error or
partial-failure indication when zero backends initialized.

### 6. D1 — sliding-TTL user-config cache (dataplane)

`lru_time_cache::get_mut` resets the TTL on every hit, so subjects with
steady <60s traffic never refresh; a client retry loop can pin a stale
config forever. The harness works around it with cache=0; the real fix is
insertion-based TTL. Note the interaction: with the cache disabled, the
publisher's TTL-expiry windows (below) are no longer masked.

### 7. Publisher design gaps (control plane)

PR #5482 makes the interval configurable, but upstream still has:

- **TTL margin**: key TTL = interval + 10s; any publish delay > 10s
  expires every UserConfig key (observed 120–180s cycle slippage under
  load on 2026-07-02 → ~110s total config outages).
- **No event-driven publish**: new entities wait for the next snapshot.
- **`WORKER_ID` computed pre-fork**: all gunicorn workers publish as
  `hostname:1`, defeating the lock's compare-and-swap ownership.
- **Publisher shares the serving event loop**: load delays its timer,
  which is what causes the slippage.

### 8. live-all lane is meaningless (harness + upstream Makefile)

`make test-live-gateway` runs `pytest -p playwright` over the whole tree;
its hook collides with pytest-asyncio (`Runner.run() cannot be called
from a running event loop`) on every async test, and the plugin suites'
fixtures self-register `http://localhost:8080/mcp` which is unreachable
in-container (502). Split into `-p no:playwright` and playwright-only
invocations before reading anything into its numbers.

### 9. No single token works on both planes (cross-plane contract)

The dataplane now accepts control-plane admin tokens (rs#51 + rs#54),
but the inverse still fails: a dataplane-scoped token is rejected by
control-plane admin REST ("Access denied") — why `cf-jwt.py` keeps its
`--admin` flag.

## Expected next state

- rs#54 merge + republished image → live-protocol auth rows stay green on
  stock pulls.
- D7 fix → live-protocol fully green except the fixture race (issue 2).
- Issue 2 fix → live-protocol green.
- Issue 3 fix → live-rbac green.
- Issue 8 split → live-all numbers become meaningful.
