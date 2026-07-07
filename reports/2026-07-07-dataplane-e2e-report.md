# Dataplane E2E Report — 2026-07-07

Current open issues only; everything previously fixed is in this file's
git history and the linked PRs.

## Stack under test

`scripts/cf-integration.sh test-all-up-no-plugins` at 12:56 UTC
(log: `.integration/test-logs/cf-tests-20260707T125640Z.log`):

- **Harness** — `test-all-up*` commands now **reset stack state first**
  (`reset` = `down --volumes`; `CF_FRESH_STACK=false` opts out) so every
  run starts from a fresh database, then wait for a publisher snapshot
  **containing the Fast Time virtual server** before running lanes.
  Key-existence alone is not enough: the gateway's very first snapshot
  on a fresh boot runs before the registration jobs finish and publishes
  an empty config (`virtual_hosts = 0`), which sent the scoped-token
  probe through the nginx fallback to the control plane, where
  `tools/call` is denied. Plus the established pieces: nginx 400/404
  replay to the control plane, stock SSE registration, 2s publisher,
  dataplane cache disabled, two-pass `live-all`, plugin suites
  deselected via `CF_TEST_PLUGINS=false` (their failures stay visible
  under plain `test-all` by design).
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

## Lane summary (fresh stack, reproducible)

| Lane          | Result | Detail |
|---------------|--------|--------|
| probe         | PASS   | 401 negative, initialize, tools/list, tools/call via nginx→dataplane |
| smoke         | PASS   | 1-user locust, 10s, streamable HTTP, 0 failures |
| live-mcp      | PASS   | 19 passed, 3 skipped |
| live-rbac     | PASS   | 40 passed |
| live-protocol | PASS   | 28 passed / 4 skipped / 0 failed |
| live-all      | FAIL   | pass 1 unstable on fresh boots (see issue 1) · pass 2: 40 passed |

The five focused lanes are green and reproducible from a fresh database.
The remaining instability is confined to `live-all`'s full-tree pass.

## Open issues

### 1. live-all pass 1 is unstable on fresh boots (upstream test coupling, triage)

Running the whole `protocol_compliance` tree in one session produces
failures that the same tests do not show in the focused lanes minutes
apart on the same stack:

- **`test_runtime_mode.py`: 9 errors + 2 failures.** The suite's fixture
  gets HTTP 200 with a non-JSON body from `/admin/runtime/mcp-mode`
  (JSON decode error instead of the suite's clean skip). On the
  long-lived pre-reset stack these tests skipped; on fresh boots they
  error. The suite also live-flips the gateway's runtime mode mid-run,
  which is state other rows may observe.
- **`gateway_virtual` rows (`test_required_tools_advertised`, pagination
  stubs) and `test_drift_*`** fail in the full-tree pass while the
  identical rows pass in the `live-protocol` lane in the same run —
  test-session coupling, not a per-test defect. Earlier reports
  attributed these rows to dataplane tool naming and then to stale DB
  state; the full-tree coupling is the consistent explanation for their
  flip-flopping.

Needs upstream triage: isolate what state the full-tree session carries
between suites (runtime-mode flips, shared reference upstream, session
caches) before trusting pass 1 as a signal beyond the focused lanes.

### 2. `test_templated_resource_registered_and_resolves[gateway_proxy-http]` (control plane, triage)

Deterministic in every run: reading the templated resource
`reference://users/7` through the gateway proxy returns content whose
text is not the upstream JSON payload. Control-plane resource
federation path, unrelated to the dataplane.

### 3. D1 — sliding-TTL user-config cache (dataplane)

`lru_time_cache::get_mut` renews the TTL on every hit; steady traffic
pins stale config indefinitely. Harness disables the cache; real fix is
insertion-based TTL upstream.

### 4. D6 — backend-unavailable swallowed as empty success (dataplane)

All-streamable-backends-down still yields `tools/list` 200 with an
empty list; clients cannot distinguish it from a tool-less server.

### 5. Remaining publisher gaps (control plane)

The empty first snapshot on boot (see harness notes above) is the same
family as the other publisher gaps: no event-driven publish, no
registration-aware first snapshot, and the loop shares the serving
event loop. The harness now waits for real content; a product fix
belongs upstream.

### 6. No single token works on both planes (cross-plane contract)

The dataplane accepts control-plane admin tokens (rs#51 + rs#54); the
control-plane admin REST still rejects dataplane-scoped tokens — the
reason `cf-jwt.py` keeps its `--admin` flag.

## Expected next state

- Open PR set (IBM #5482/#5510/#5515/#5517/#5519/#5523 + rs#54) merges
  → the five focused lanes green on stock pulls, reproducibly.
- Issue 2 fixed → focused-lane picture fully green including the
  gateway-proxy resource row.
- Issue 1 triaged → live-all pass 1 becomes a trustworthy superset
  instead of a coupled full-tree run.
