# Dataplane E2E Report — 2026-07-02 (evening run)

Supersedes the morning report from this file. Same stack shape, but three
inputs changed since then, so all findings below were re-verified by hand
against the live stack — nothing is carried over untested.

## Scope and setup

`scripts/cf-integration.sh test-all-up` at 17:16 UTC
(log: `.integration/test-logs/cf-tests-20260702T171620Z.log`):

- **cf-dataplane** `ghcr.io/contextforge-gateway-rs/contextforge-gateway-rs:0.1.0`,
  digest `cb5a64fe`, **rebuilt today 14:59 UTC** (after the morning runs).
- **cf-controlplane** upstream `main` @ `5c22ade5c` (local build image),
  4 gunicorn workers, `DATAPLANE_PUBLISHER=true`.
- Dataplane user-config cache at **image default** (60s sliding TTL) —
  the harness override was removed in commit `e41a079`.
- [PR #5482](https://github.com/IBM/mcp-context-forge/pull/5482)
  (live-RBAC allow-path retries) is **open, not in this checkout**; it was
  additionally tested by hand below.

Run note: the run first failed at `up` because the working tree switched the
dataplane default tag to `latest`, and **GHCR has no `latest` tag for
`contextforge-gateway-rs`** (only `0.1.0`, confirmed via the registry tags
API). Re-run with `CF_DATAPLANE_VERSION=0.1.0`.

## Lane summary

| Lane          | Result | Detail |
|---------------|--------|--------|
| probe         | PASS   | 401 negative, initialize, tools/list, tools/call via nginx→dataplane |
| smoke         | PASS   | 1-user locust, 10s, streamable HTTP, 0 failures |
| live-mcp      | PASS   | 19 passed, 3 skipped |
| live-rbac     | FAIL   | 3 failed / 37 passed |
| live-protocol | FAIL   | 4 failed + 14 errors — all 18 `gateway_virtual-http` rows 401 |
| live-all      | FAIL   | 64 failed + 78 errors — mostly harness noise (see "Not dataplane") |

Steady-state dataplane traffic (pre-registered Fast Time virtual server) is
healthy. Every real failure again involves runtime-created entities or the
token contract — but the root-cause picture has changed materially.

## Fixed since the morning report

**D2 — hollow-200 initialize is FIXED** in the rebuilt 0.1.0. Verified by
hand: initialize against a vhost absent from the subject's config now
returns `404 {"detail":"Server not found"}` (dataplane path via nginx
regex), no phantom `Mcp-Session-Id`, no `-32002` in the logs. This restores
honest client-side failures for every config problem below.

## Root cause: the publisher is a 60-second batch pipeline

This is the load-bearing finding of this run. Everything previously filed
under "publisher latency" (D4) is downstream of the control-plane publisher
design in `mcpgateway/services/dataplane_publisher.py`:

- **Fixed 60s full-snapshot loop.** `REDIS_PUBLISHER_TIME = 60` is a
  hardcoded module constant (not a setting). Every cycle re-reads all
  users/servers/tools from the DB and rewrites every `UserConfig` key.
  There is **no event-driven publish** — creating a user or server does
  nothing until the next cycle fires.
- **Key TTL 70s vs 60s interval = 10s of margin.** Keys are written with
  `ex=PUBLISHER_TTL` (70). Idle sampling of Redis showed the admin
  UserConfig TTL draining to **12s** before the next cycle refreshed it.
- **Under load the cycle slips past the TTL.** Observed publish log
  timestamps during the test-all run: 17:18:12 → 17:21:12 (**180s gap**) →
  17:23:12 (120s) → 17:25:12 (120s) → 17:26:12 (60s, idle again). During a
  120–180s gap every UserConfig key expires, so **all subjects lose
  dataplane config for up to ~110s** — the dataplane's 60s in-memory cache
  partially masks this for active subjects, which means disabling that
  cache (the harness's former overlay override) exposes the outage instead
  of fixing staleness. The slippage correlates with test load; the
  publisher shares the serving event loop, so its timers fire late when
  workers are busy.
- **Lock ownership is broken.** `WORKER_ID = hostname:os.getpid()` is
  computed at module import in the gunicorn master (pid 1) before fork, so
  all 4 workers publish as `…:1`. The compare-and-delete release script
  therefore can't distinguish owners; any worker can release any other's
  lock. Harmless today only because publishes are near-instant.
- **Silence at INFO.** Skipped cycles log at DEBUG only, and the dataplane
  logs nothing at INFO when a config fetch misses — a subject in the
  outage window just gets a bare `400 Bad Request`.

Convergence for a brand-new subject is therefore uniform 0–60s in the best
case and unbounded during load-induced slippage.

## PR #5482 assessment: right direction, insufficient as-is

The PR retries the two live-RBAC allow-path checks **5 × 100ms = 500ms**.
Verified by hand against the live stack:

1. **Patch applied to the checkout, tests run 3×: 6 failures out of 6
   runs** (`400 Bad Request` throughout). A 500ms window against a 60s
   publish cycle passes only if the cycle happens to land inside it
   (~1% chance).
2. **Retry widened locally to 60 × 0.5s (30s):**
   `test_public_token_accesses_public_server` converged after **12.04s**
   (23 attempts); `test_team_member_accesses_team_server` still failed
   after the full 30s — its fixture ran just after a publish cycle, so the
   next chance was ~45s out. The window must cover a **full publisher
   interval plus slippage: ≥75s** (e.g. 1s spacing, 75s deadline) to be
   reliable.
3. **The pass it produces is hollow.** When the public test converged it
   printed `0 tools`. The fixture servers are backed by an **SSE gateway**
   (`http://fast_time_server:8080/sse`); the dataplane opened a
   streamable-HTTP client against it, got `HTTP 405 Method Not Allowed`,
   logged `list_tools: backend … unavailable`, and returned an **empty
   tools list with success status**. The tests assert only that
   `tools/list` succeeds, so they will go green without ever proxying a
   tool. Recommend the PR also assert a non-empty tool list — today that
   assertion would correctly fail and expose the SSE gap below.

## Dataplane findings (current 0.1.0, digest `cb5a64fe`)

### D3 — tokens without a `scopes` claim rejected 401 (still open)

Unchanged. All 18 `gateway_virtual-http` live-protocol rows fail because
upstream's compliance harness mints one admin JWT (no `scopes`) for both
planes; the control plane accepts it, the dataplane 401s it. The inverse
also still holds (dataplane-scoped token rejected by control-plane admin
REST), so no single token works across both planes. Preferred fix remains:
treat a missing `scopes` claim as unrestricted for platform-admin tokens.
Largest single test unlock available.

### D5 — SSE upstream transports not honored (new)

The published `UserConfig` carries `transport` per gateway backend, but the
dataplane initializes every backend with its streamable-HTTP client. For an
SSE gateway this dies with `HTTP 405`:

```
ERROR rmcp::transport::worker: worker quit with fatal: … UnexpectedServerResponse("HTTP 405 Method Not Allowed: ")
WARN  … initialize: Unable to initialize for DownstreamSessionId { … } … context: "send initialize request"
```

Fix: honor the transport field (SSE client for SSE gateways), or have the
publisher exclude non-supported transports so the config is honest.

### D6 — backend-unavailable is silently swallowed (new)

When every backend of a vhost fails to initialize, `tools/list` returns
**success with an empty list** instead of an error. Same
phantom-success philosophy as the now-fixed D2, one layer deeper. Clients
(and tests) cannot distinguish "no tools configured" from "all backends
down". Fix: surface a JSON-RPC error or at minimum a partial-failure
indication when zero backends initialized.

### D4 — unknown subject → bare 400 (reframed)

Still present: a subject with no `UserConfig` key gets a bare
`400 Bad Request` with no INFO-level dataplane log. But given the publisher
findings above, an on-miss Redis fetch would not help — the key genuinely
does not exist until the next batch cycle. The real fixes are on the
publisher (below). What the dataplane should still do: return a clearer
403-style body ("no dataplane config for subject") and log the miss with
the subject at WARN, so outage windows are diagnosable.

### D1 — sliding-TTL user-config cache (still open, now double-edged)

`lru_time_cache::get_mut` still resets the timestamp on every hit, so
subjects with steady <60s traffic never refresh (insertion-based TTL still
the right fix). Note the interaction: this same cache is currently the only
thing masking the publisher's TTL-expiry outages for active subjects, which
is why the harness reverting to the default cache (commit `e41a079`) made
steady-state lanes stable while doing nothing for new-entity latency.

## Control-plane findings

- **Publisher redesign needed** (see root cause). Concretely, in
  ascending effort:
  1. Make `REDIS_PUBLISHER_TIME` / `PUBLISHER_TTL` env-configurable, with
     TTL ≥ 2× interval + slippage margin (today's 70/60 leaves 10s).
  2. Publish immediately at startup and after any mutation that changes a
     UserConfig (user/team/server/tool CRUD) — even a debounced "publish
     soon" flag would cut convergence from 0–60s to sub-second.
  3. Compute `WORKER_ID` post-fork (or in `start()`), restoring lock CAS.
  4. Move the publish loop off the request-serving event loop (thread or
     dedicated process) so load can't starve its timer past the TTL.
- **Private-server visibility regression** (unchanged):
  `test_admin_sees_public_and_team_via_http` — `/servers` REST listing
  shows admin another user's private server, violating the PR #4341
  contract. Pure control-plane bug, identical code path in the stock stack.

## Failures NOT attributable to cf-dataplane

Unchanged from the morning report, re-confirmed in this run's log:

- **live-all mass failures (64F/78E in 17.5s)** — `make test-live-gateway`
  runs `pytest -p playwright` over the whole tree; the plugin's
  `pytest_runtest_call` hook collides with pytest-asyncio → 103×
  `Runner.run() cannot be called from a running event loop`. The same log
  contains the real D3 401 rows.
- **live-all plugins suites** — fixtures self-register the gateway at
  `http://localhost:8080/mcp`, unreachable from inside the container →
  502 "All connection attempts failed"; also need `PLUGINS_CONFIG_FILE`
  at boot. Broken in any compose-based run.

## Improvements

### cf-dataplane (priority order)

1. **Accept admin tokens without `scopes`** (D3) — unblocks all 18
   `gateway_virtual` compliance rows with zero test changes.
2. **Honor backend `transport` / support SSE upstreams** (D5) — without it
   every SSE-backed virtual server is a silent no-op through the dataplane.
3. **Error on zero-backends-initialized instead of empty success** (D6).
4. **Insertion-based TTL for the user-config cache** (D1).
5. **Diagnosable config misses** (D4): clear error body + WARN log with
   subject.

### cf-controlplane

1. Publisher: configurable interval/TTL, mutation-triggered publish,
   post-fork `WORKER_ID`, off-loop scheduling (details above).
2. Fix the private-server visibility regression (PR #4341 contract).

### Harness (this repo)

1. **Fix the broken `latest` default** in the working tree —
   `CF_DATAPLANE_VERSION` defaults to `latest` but GHCR only has `0.1.0`;
   `test-all-up` dies at `up`. Revert to a pinned tag (or pin by digest)
   until upstream publishes `latest`.
2. **Add a publisher-health check to the probe lane**: assert a
   `UserConfig` key exists and its TTL/interval margin is sane; tail the
   gateway's `Published N user configs` lines into the run log. Publish
   gaps were only visible here via manual Redis sampling.
3. **Split live-all** into `-p no:playwright` and playwright-only
   invocations so real regressions aren't drowned in event-loop noise.
4. Keep `CF_DATAPLANE_USER_CONFIG_CACHE_EXPIRY_SECONDS` easy to re-enable
   in the overlay for A/B runs — with the publisher outage windows, cache
   on/off now trades staleness against availability, which is worth
   measuring per image.

### Upstream tests / PR #5482

1. Widen the retry to cover a full publish interval plus slippage
   (≈75s deadline, ~1s spacing), ideally reading the deadline from an env
   var so a future event-driven publisher can shrink it.
2. Assert the converged `tools/list` is **non-empty** — today's assertion
   passes on the hollow empty list produced by the SSE 405 path (D5/D6)
   and would hide it permanently once retries land.

## Expected picture once fixes land

- D3 fix → live-protocol fully green.
- Publisher mutation-triggered publish + PR #5482 (widened) → live-rbac
  green except the control-plane visibility regression.
- D5+D6 → allow-path tests can assert real tool lists.
- live-all numbers remain meaningless until the playwright split.
