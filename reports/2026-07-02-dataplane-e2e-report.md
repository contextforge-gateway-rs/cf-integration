# Dataplane E2E Report — 2026-07-02

## Scope and setup

Full `scripts/cf-integration.sh test-all` against the dataplane stack:
stock upstream compose + nginx routing split + `DATAPLANE_PUBLISHER=true`,
`cf-dataplane` image
`ghcr.io/contextforge-gateway-rs/contextforge-gateway-rs:0.1.0`. UI/SSO
tests out of scope. The current harness leaves the dataplane user-config
cache at the image default.

Timeline of the runs this report is based on:

1. **12:18 UTC — full `test-all`** on the pre-PR-#49 image
   (log: `.integration/test-logs/cf-tests-20260702T121837Z.log`).
2. **12:20 UTC — [contextforge-gateway-rs #49](https://github.com/contextforge-gateway-rs/contextforge-gateway-rs/pull/49) merged**
   (configurable user-config cache expiry); CI republished the `0.1.0` tag
   (digest `8e063c0a`).
3. **12:47–13:01 UTC — targeted re-verification** of the config-staleness
   defect on the new image, by hand (curl MCP handshakes, Redis
   inspection, controlled idle/traffic windows).
4. **13:03–13:07 UTC — A/B locust benchmark** (cache 60s vs cache 0);
   no throughput or latency regression with cache disabled.
5. **13:57 UTC — new `0.1.0` image pulled** and stack recreated with
   cache expiry 60 (digest `802bab9c`).
6. **13:59–14:04 UTC — targeted user-config cache re-verification** on
   digest `802bab9c`: a server created under continuous 10s traffic failed
   until 50s, then initialized and served `tools/list` at 60s while traffic
   continued. Dataplane logs moved from `virtual_hosts = 3` to
   `virtual_hosts = 4`. An idle-subject check also passed after 80s
   (`virtual_hosts = 5`).
7. **14:05 UTC — full `test-all`** on digest `802bab9c`
   (log: `.integration/test-logs/cf-tests-20260702T140505Z.log`).
8. **14:16 UTC — harness overlay cache override removed**; stack
   recreated with no `CONTEXTFORGE_GATEWAY_RS_USER_CONFIG_CACHE_EXPIRY_SECONDS`
   env, `probe` passed, and a runtime-created server became usable under
   continuous traffic after 30s.

Reference baseline: the control-plane-only stack was fully green on the
same suites earlier today (`controlplane-only-20260702T085914Z.log`,
90 passed / 0 failed protocol compliance including `gateway_virtual`
rows), so `gateway_virtual` deltas below are attributable to
`cf-dataplane`, not stack drift.

## Lane summary (latest full `test-all`, digest `802bab9c`, cache 60s)

| Lane          | Result | Detail |
|---------------|--------|--------|
| probe         | PASS   | 401 negative, initialize, session reuse, tools/list, tools/call via nginx→dataplane |
| smoke         | PASS   | 1-user locust, 10s, streamable HTTP, 0 failures |
| live-mcp      | PASS   | 19 passed, 3 skipped |
| live-rbac     | FAIL   | 3 failed / 37 passed |
| live-protocol | FAIL   | 4 failed + 14 errors — every `gateway_virtual-http` row still 401s |
| live-all      | FAIL   | 64 failed + 78 errors — mostly harness noise, see "Not dataplane" |

The steady-state dataplane path (pre-registered Fast Time virtual server)
is healthy under both functional and load traffic. Every dataplane failure
involves **entities created at runtime** (virtual servers, users) or the
**token contract**.

## Dataplane findings

All verified by hand against the live stack, not inferred from test output.

### D2 — Failed initialize returns HTTP 200 + dead session (open)

When a vhost is missing from the subject's config, the dataplane logs
`ERROR ... Failed to serve session: initialize failed: -32002: No
configuration` but still answers the initialize POST with **HTTP 200**
and a `Mcp-Session-Id`. The session is never registered, so the next
request gets `404 Session not found`. Clients observe a phantom-success
handshake that dies one step later with a misleading error; this also
masked the cache-staleness diagnosis (a plain-status probe of initialize
reads as success).

Fix (cf-dataplane): map initialize failure to a real HTTP error
(404/409 with the JSON-RPC error body) instead of fabricating a session.

### D3 — Tokens without a `scopes` claim are rejected 401 (open)

The dataplane 401s any JWT lacking a `scopes` claim, regardless of
`token_use`. Verified matrix against a live vhost:

| Token | Result |
|---|---|
| `scopes` claim present (harness `cf-jwt.py` default) | 200 |
| no `scopes` claim (upstream `make_test_jwt` admin token) | 401 |
| `scopes` claim + `token_use=session` | 200 |

Upstream's compliance harness mints one admin JWT (no `scopes`) and uses
it for both control-plane REST and `/servers/{id}/mcp`. The control plane
accepts it; the dataplane rejects it → all 18 `gateway_virtual-http`
failures/errors in live-protocol. The inverse also holds: the
dataplane-scoped token is rejected by control-plane admin REST
("Access denied"), so **no single token works across both planes** (this
is why `cf-jwt.py` grew an `--admin` flag).

Fix options: (a) dataplane treats a missing `scopes` claim as
unrestricted for platform-admin tokens, matching control-plane semantics;
(b) control plane embeds `scopes` in admin/API tokens; (c) upstream tests
mint dataplane-compatible tokens. Only (a) keeps stock upstream clients
working unmodified.

### D4 — Unknown subject → bare 400 (publisher latency remains)

A token whose subject has no `UserConfig` key in Redis gets a bare
`400 Bad Request` ("Problem occurred retrieving the configuration").
This is what failed `test_public_token_accesses_public_server` and
`test_team_member_accesses_team_server` in live-rbac: the fixtures create
users + servers and exercise them within seconds, inside the publisher
window. The user-config cache fix closes stale existing-subject configs,
but it does not close this first-publish gap for brand-new subjects. A
clearer 403 body ("no dataplane config for subject") would make this
diagnosable from the client side; an on-miss Redis fetch before rejecting
would close the gap entirely.

## Failures NOT attributable to cf-dataplane

- **live-rbac `test_admin_sees_public_and_team_via_http`** — the
  control-plane `/servers` REST listing shows admin another user's
  private server, violating the PR #4341 visibility contract. Pure
  control-plane RBAC regression; identical code path in the stock stack.
- **live-all mass failures (64F/78E in 17s)** — `make test-live-gateway`
  runs `pytest -p playwright` over the whole tree; pytest-playwright's
  `pytest_runtest_call` hook collides with pytest-asyncio →
  `RuntimeError: Runner.run() cannot be called from a running event loop`
  on every async test in **all three targets including reference-stdio**
  (which passed in the baseline via the `-p no:playwright` path). Harness
  incompatibility, not the gateway. The same log also contains the real
  D3 `gateway_virtual` 401s.
- **live-all plugins suite (pii_filter / sql_sanitizer)** — fixtures
  self-register the gateway at `http://localhost:8080/mcp`; inside the
  gateway container localhost:8080 is unreachable → control plane returns
  502 "All connection attempts failed". Also requires
  `PLUGINS_CONFIG_FILE=plugins/plugin_parity_config.yaml` at stack boot.
  Environment/upstream-fixture issue in any compose-based run.

## Status and next steps

Remaining, in priority order:

1. **cf-dataplane: honest initialize errors** (D2) — cheap; removes the
   phantom-200 that makes every config problem look like a session bug.
2. **cf-dataplane: accept admin tokens without `scopes`** (D3) — unblocks
   all upstream `gateway_virtual` compliance rows with no test changes.
3. **cf-dataplane: on-miss Redis fetch for unknown subjects** (D4) —
   closes the remaining ≤60s window for brand-new users.
4. **Harness/upstream:** split live-all into `-p no:playwright` and
   playwright invocations so real regressions aren't drowned in
   event-loop noise; report the private-server visibility regression
   upstream.

Expected test-lane picture once D3–D4 land: live-protocol
`gateway_virtual` rows green; live-rbac green except the control-plane
visibility regression; live-all still needs the pytest split before its
numbers mean anything.
