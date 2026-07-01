# Upstream Parity Run

Run date: 2026-07-01, Europe/Dublin.

## Scope

Aligns the harness stack with stock upstream `IBM/mcp-context-forge` so the only intentional differences are:

1. nginx routes `/servers/{id}/mcp` to `cf-dataplane` (the routing split under test)
2. `DATAPLANE_PUBLISHER=true` on the gateway (feeds configs to `cf-dataplane`)
3. Harness test tooling (probe, spec-compliant locustfile, JWT helper)

Goal: every remaining failure measures a `cf-dataplane` gap, not stack configuration drift.

## Divergences Removed

- **Fast Time image**: reverted from `ghcr.io/ibm/cfex-mcp-fast-time-server:latest` (`/mcp`-only) to the upstream default `ghcr.io/ibm/fast-time-server:latest` running `-transport=dual` (`/http` + `/sse`), exactly as upstream configures it.
- **Fast Time registration**: deleted the harness `register-fast-time.py`; the stock upstream `register_fast_time` and `register_fast_time_sse` jobs now run unchanged. They register the same fixed virtual server ids the harness targets (`9779b6698cbd4b4995ee04a4fab38737` streamable, `a88e2c3f5d7b4a9e8f1c6d2e3b4a5f6e` SSE), so no harness-side registration remains.
- **Gateway sizing**: removed the overlay's `GUNICORN_WORKERS`/CPU/memory `deploy` fork. The harness now sets upstream's own sizing env knobs (`GATEWAY_REPLICAS`, `GATEWAY_CPU_LIMIT`, ...) scaled to the local Docker engine, because upstream defaults (3 replicas x 8 CPUs, 24 workers) exceed a local Docker VM.
- **Warm-up Redis publish**: gone with the custom register script. The probe now retries `initialize` for up to `CF_PROBE_CONFIG_TIMEOUT` (120s) to cover the publisher's 60s cycle after a fresh `up`.

Retained intentionally: the spec-compliant harness locustfile (the upstream `locustfile_mcp_protocol.py` sends `Accept: application/json`, which violates the streamable-HTTP negotiation requirement and would produce a false 406 signal against the strict Rust dataplane; the upstream file stays selectable via `LOCUST_LOCUSTFILE`).

## Results (fresh `down` + `up`, stock registrations)

```text
probe          PASS  initialize/tools/list/tools/call via nginx -> dataplane -> fast_time /http
smoke          PASS  70 requests, 0 failures
live-mcp       PASS  19 passed, 3 skipped
live-rbac      37 passed, 3 failed   (was: total setup failure on /sse before parity)
live-protocol  15 passed, 4 failed, 14 errors, 2 skipped (gateway targets)
live-all       84 passed, 64 failed, 42 errors, 75 skipped (was 70/65/55)
```

`live-mcp` skips are upstream-legitimate: ambiguous `timezone://info` across the two stock Fast Time registrations (upstream registers both too), no optional-argument prompt fixture, Rust public transport not mounted.

## Remaining Failures — All Dataplane Signal

### live-rbac: 3 failed / 37 passed

Every failure is a per-server endpoint through the dataplane route:

```text
test_admin_sees_public_and_team_via_http     400 at /servers/{runtime-created-id}/mcp/
test_public_token_accesses_public_server     400 at /servers/{runtime-created-id}/mcp/
test_team_member_accesses_team_server        400 at /servers/{runtime-created-id}/mcp/
```

These use virtual servers and users created at test runtime. The dataplane returns 400 for them — the publisher/dataplane pair does not yet cover configs for runtime-created public/team users and their servers. The same tests pass on the raw `/mcp` control-plane path (37 passing include the multi-transport and negative-auth matrix).

Secondary observation: for the outsider-denial cases the dataplane responds `400` where the suite expects an authorization-shaped rejection; error-code parity is part of the same gap. Dataplane observability is also thin here: `RUST_LOG=info` logs do not say which of "config key missing / decode failed / subject unknown" produced the 400.

### live-protocol (gateway targets): 14 errors, 4 failed

All 14 errors are `gateway_virtual` setup failures:

```text
401 Unauthorized at /servers/{id}/mcp/
```

Cause (validated in [2026-07-01-next-fixes-applied.md](2026-07-01-next-fixes-applied.md)): the suite's `admin_jwt` fixture mints JWTs without a `scopes` claim; `cf-controlplane` accepts them, `cf-dataplane` rejects them. Token-contract parity gap.

The 4 failures (`pagination stub names`, `subscriptions roundtrip`, `ping via connect`, `cancellation`) are `gateway_virtual`-only MCP behaviors to verify once the token gap is closed.

### live-all: known upstream suite artifact

Running the whole `tests/live_gateway` tree in one pytest session breaks protocol-compliance fixtures identically on every target — including `reference-stdio`, which never touches this stack (19 failed + 14 errors per target, same tests). Standalone runs of the same files pass on reference (33/33). This is an upstream test-suite runner interaction, not stack configuration and not dataplane behavior. Use the individual lanes (`live-mcp`, `live-rbac`, `live-protocol`) for signal; `live-all` additionally includes unconfigured SSO/runtime lanes (75 skips).

## Dataplane Work Items Distilled

1. Publish/accept configs for runtime-created users and virtual servers (public/team RBAC paths) — fixes the 3 `live-rbac` failures.
2. Accept control-plane admin tokens minted without a `scopes` claim (or define the contract and fix upstream's fixture) — unblocks 14 `gateway_virtual` compliance tests.
3. Error-code parity: authorization failures currently surface as `400`; align with control-plane semantics (`401`/`403`/`404`).
4. Config-load observability: log the concrete reason (missing key, decode failure, unknown subject) at `info`.
5. After 1–2 land: verify pagination, subscriptions, ping-via-connect, cancellation on `gateway_virtual`.
