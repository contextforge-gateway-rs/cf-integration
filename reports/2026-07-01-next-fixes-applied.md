# Next Fixes Validation And Application

Run date: 2026-07-01, Europe/Dublin.

## Scope

Validates the follow-up items from [2026-07-01-ghcr-fast-time-run.md](2026-07-01-ghcr-fast-time-run.md) (failure buckets) and [20260701T125130Z/REPORT.md](20260701T125130Z/REPORT.md) (recommended next steps), and applies the ones that are harness-fixable.

## Verdict

```text
probe     PASS exit 0
smoke     PASS 67 requests, 0 failures
live-mcp  PASS 20 passed, 2 skipped (was 12 passed + 7 deselected via live-core)
```

`live-mcp` is now the full green E2E lane, so the curated `live-core` command was removed.

## Fix-by-Fix Validation

### 1. `fast-test-*` tools not registered — FIXED (harness)

Upstream ships `fast_test_server` + `register_fast_test` in its compose file, gated behind `--profile testing`, which the harness never started. The overlay now resets their profiles so plain `up` includes them:

```yaml
fast_test_server:
  profiles: !reset []
register_fast_test:
  profiles: !reset []
```

Verified: 6 `fast-test-*` tools register, and the full `test_mcp_protocol_e2e.py` suite — including all of `TestToolCalls` — passes:

```text
20 passed, 2 skipped in 3.00s
```

Remaining skips are environmental (no optional-argument prompt fixture; Rust public transport not active).

### 2. Harness token 403 on `GET /servers` — FIXED (harness)

Reproduced: the scoped harness JWT got `403` on `GET /servers`. Tested permission sets against the live gateway:

```text
["servers.read","servers.use","tools.read","tools.call"] -> 200
["admin"]                                                -> 403
```

`servers.read` added to `cf_jwt.DEFAULT_SCOPES`. Probe re-verified against the dataplane route with the extra permission: still PASS.

### 3. RBAC `/sse` registration — NOT HARNESS-FIXABLE (validated)

`test_mcp_rbac_transport.py` hardcodes `http://fast_time_server:8080/sse`. Verified against the GHCR image from inside the network:

```text
GET /sse -> 404 Not Found
GET /mcp -> 405 Method Not Allowed (POST-only endpoint exists)
```

The image entrypoint accepts no transport flags (upstream's locally-built Go image uses `-transport=dual`; the GHCR cfex Rust image serves `/mcp` only). Fixing this means either an upstream test change or switching the Fast Time image — the harness deliberately uses the published GHCR image, so this stays a documented red lane.

### 4. Protocol-compliance async `RuntimeError` — STALE, superseded (validated)

The previously reported `Runner.run() cannot be called from a running event loop` on `reference-stdio` no longer reproduces:

```text
make test-protocol-compliance-reference: 33 passed, 109 deselected
```

The remaining `live-protocol` failures are different and precisely diagnosed:

```text
gateway targets: 4 failed, 15 passed, 2 skipped, 29 xfailed, 2 xpassed, 14 errors
```

All 14 errors are `gateway_virtual` setup failures:

```text
httpx.HTTPStatusError: Client error '401 Unauthorized'
for url 'http://127.0.0.1:8080/servers/{id}/mcp/'
```

Root cause: the suite's `admin_jwt` fixture (`tests/live_gateway/protocol_compliance/fixtures/gateway_live.py`) mints a JWT without a `scopes` claim, and `cf-dataplane` rejects scope-less tokens with 401. This is the same failure mode the harness probe hit and fixed for its own token. The fix belongs upstream (pass `scopes=` in the fixture) or in `cf-dataplane` (accept scope-less admin tokens); it is a control-plane/dataplane token contract gap, out of scope for this harness.

### 5. Redis `UserConfig` publication / config retrieval failures — ALREADY FIXED (validated)

The older report's headline failure (every virtual-server request failing on config retrieval) no longer reproduces: probe initialize/tools/list/tools/call all pass through the dataplane. No action needed.

### 6. Optional SSO/runtime lanes — ENVIRONMENTAL (unchanged)

Azure/Keycloak/runtime-mode skips remain; they need external services this stack does not run.

## Final Status

Green path, all defaults, no manual steps:

```text
up -> probe -> smoke -> live-mcp
```

Remaining red lanes and their owners:

```text
live-rbac      upstream test expects /sse; GHCR Fast Time image is /mcp-only
live-protocol  gateway_virtual 401: admin_jwt fixture lacks the scopes claim cf-dataplane requires
live-all       superset of the above plus SSO/runtime lanes
```
