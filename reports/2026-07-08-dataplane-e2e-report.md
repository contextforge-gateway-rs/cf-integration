# Dataplane E2E Report — 2026-07-08

Current state only; resolved history lives in this file's git log and
the linked PRs.

## Headline

**The full suite is green and deterministic**: two consecutive fresh
`test-all-up-no-plugins` runs with identical results
(logs: `.integration/test-logs/cf-tests-20260708T090730Z.log` and the
09:12 UTC rerun; every lane PASS, `EXIT=0` twice).

| Lane          | Result | Detail |
|---------------|--------|--------|
| probe         | PASS   | 401 negative, initialize, tools/list, tools/call via nginx→dataplane |
| smoke         | PASS   | 1-user locust, 10s, streamable HTTP, 0 failures |
| live-mcp      | PASS   | 19 passed, 3 skipped |
| live-rbac     | PASS   | 40 passed |
| live-protocol | PASS   | 28 passed, 2 skipped |
| live-all      | PASS   | pass 1: 112 passed · pass 2: 40 passed |

## Stack under test

- **Harness** (cf-integration `main`): fresh-bootstrap `test-all-up*`
  (volume reset + wait for a publisher snapshot containing the Fast
  Time server), nginx 400/404 replay to the control plane, stock SSE
  registration, 2s publisher, dataplane cache disabled, two-pass
  `live-all`, plugin suites deselected (`CF_TEST_PLUGINS=false`;
  plain `test-all` keeps their failures visible by design),
  password-change enforcement disabled for the test admin, and
  `ensure_checkout` now tolerates upstream tag moves and offline
  fetches.
- **cf-controlplane** from `user/luca/dataplane-integration-fixes`:
  open PRs [#5482](https://github.com/IBM/mcp-context-forge/pull/5482),
  [#5515](https://github.com/IBM/mcp-context-forge/pull/5515),
  [#5517](https://github.com/IBM/mcp-context-forge/pull/5517),
  [#5519](https://github.com/IBM/mcp-context-forge/pull/5519),
  [#5553](https://github.com/IBM/mcp-context-forge/pull/5553)
  (#5510 and #5523 merged upstream).
- **cf-dataplane** `cf-dataplane:parity` — rs `main` (incl. merged
  [#54](https://github.com/contextforge-gateway-rs/contextforge-gateway-rs/pull/54))
  plus open
  [#56](https://github.com/contextforge-gateway-rs/contextforge-gateway-rs/pull/56).

## What closed the last failures

1. **Password-change enforcement on fresh databases** (harness fix,
   committed): the bootstrap admin's `password_change_required` made
   every `/admin/*` route 303-redirect to an HTML page. This single
   cause produced the runtime-mode suite's 9 errors, the intermittent
   gateway-proxy templated-resource JSON failures, and most of what
   earlier reports called "full-tree test coupling".
2. **Tool-name parity across the planes** (IBM#5553 + rs#56): the
   dataplane namespaced tools as `<gateway-uuid>-<raw_name>`; the
   control plane advertises `<gateway-slug>-<slugified-name>`. Exact
   name checks therefore flip-flopped depending on which plane served
   the request. The publisher now keys backends by gateway slug and the
   dataplane slugifies advertised tool names (reverse-mapping to the
   original on `tools/call`), making names identical on both planes.
   Notably, the dataplane never used `allowed_tool_names` for
   filtering — every historical "filtered to 0 tools" symptom was this
   naming divergence.

## Open items (none currently failing)

1. **Gateway-proxy templated-resource read** — passed in the last four
   runs since the enforcement fix, but it failed intermittently in
   live-all before root-cause was confirmed; watch for recurrence
   before declaring it closed.
2. **D1 — sliding-TTL user-config cache** (dataplane): harness runs
   with the cache disabled; the upstream fix is insertion-based TTL.
3. **D6 — backend-unavailable yields empty tools/list success**
   (dataplane): failure mode still exists for genuinely down backends.
4. **Publisher design gaps** (control plane): first snapshot on boot
   precedes registrations (harness waits for real content as a
   workaround), no event-driven publish, publish loop shares the
   serving event loop.
5. **Cross-plane token contract**: control-plane admin REST still
   rejects dataplane-scoped tokens (`cf-jwt.py --admin` exists for
   this).
6. **Plugin E2E suites** need a plugin-enforce gateway; excluded from
   `test-all-up-no-plugins`, honestly failing under plain `test-all`.

## Expected next state

- Open PR set (IBM #5482/#5515/#5517/#5519/#5553 + rs #56) merges →
  this green, deterministic picture on stock image pulls.
