# Dataplane Change List

Run date: 2026-07-02, Europe/Dublin.

## Scope

Verifies that every configuration the control plane holds is correctly exported for the dataplane, then isolates — with direct evidence — the changes `cf-dataplane` needs. Control-plane-side config is now fully correct; no harness or publisher fixes remain.

## Config Parity — Control Plane vs Dataplane

Every virtual server the control plane lists initializes successfully through the public nginx -> dataplane route (after a fresh dataplane config read):

```text
OK 9779b6698cbd4b4995ee04a4fab38737 Fast Time Server        (STREAMABLEHTTP backend)
OK a88e2c3f5d7b4a9e8f1c6d2e3b4a5f6e Fast Time SSE Server    (SSE backend)
OK b8e3f1a2c4d5e6f7a1b2c3d4e5f6a7b8 Fast Test Server        (STREAMABLEHTTP backend)
OK f978768f1d514b3d854c0a8e35685660 propagation-repro       (runtime-created)
```

Notably the SSE-backed virtual server works through the dataplane, so SSE upstream proxying is not a gap.

The publisher exports correctly too. A runtime-created server appeared in the Redis `UserConfig` for its owner within one 60s cycle, with the right backend, tools, and transport:

```text
virtual_hosts: [... 'f978768f1d514b3d854c0a8e35685660']
{'backends': {'3260babb...': {'name': 'fast_test', 'url': 'http://fast_test_server:8880/mcp',
 'transport': 'STREAMABLEHTTP', 'allowed_tool_names': ['fast-test-echo'], ...}}}
```

## Dataplane Change 1 — Reload UserConfig from Redis (highest impact)

Reproduction:

1. Created a new virtual server via the control-plane API.
2. The publisher wrote it into the owner's Redis `UserConfig` within one cycle (verified by decoding the msgpack value).
3. The dataplane kept answering `initialize` with JSON-RPC `-32002 "No configuration"` for **148+ seconds** — more than two publish cycles — while the config sat in Redis.
4. Restarting only the dataplane container made the identical request succeed immediately.

Conclusion: the dataplane loads a subject's `UserConfig` once (binary strings show a `user_store::UserConfig` and a single `loaded user config` path) and never re-reads Redis. The binary exposes no refresh knob — its env surface covers address, TLS, Redis connection, OTel, CPUs, and logging only.

Required change: re-read `UserConfig` on a TTL, on Redis keyspace notification, or per-request with a short cache. This is the root cause of the `live-rbac` runtime-created-server failures: entities created mid-test can never become visible to a running dataplane.

## Dataplane Change 2 — Accept Tokens Without a `scopes` Claim

The control plane accepts admin JWTs without a `scopes` claim (its own `create_jwt_token --admin` and the test suites' `make_test_jwt` mint exactly these). The dataplane rejects them with 401: its claims struct (`ContextForgeClaims`, strict `Scopes` deserialization per the binary) treats `scopes` as required.

Impact: all 14 `gateway_virtual` protocol-compliance setup errors, plus any client using control-plane-issued admin tokens against `/servers/{id}/mcp`.

Required change: treat a missing `scopes` claim per control-plane semantics (admin/unrestricted), or agree the claim is mandatory and change the control-plane token mint — either way the two planes must share one token contract.

## Dataplane Change 3 — Error Semantics Parity

Observed dataplane responses for config/authorization problems:

```text
missing virtual host in loaded config   HTTP 200 + JSON-RPC -32002 "No configuration"
subject with no UserConfig key          HTTP 400
token without scopes claim              HTTP 401
```

The control plane answers the equivalent cases with 401/403/404. The RBAC suite specifically expects authorization-shaped rejections and receives 400. Align status codes and expose distinct causes (unknown subject vs unknown virtual host vs unauthorized).

## Dataplane Change 4 — Config-Load Observability

At `RUST_LOG=info` the dataplane logs nothing actionable when config retrieval fails; the generic strings (`Problem occurred retrieving the configuration`) do not say whether the Redis key was missing, decoding failed, or the virtual host was absent. Log the concrete reason and subject at `info`.

## Deferred Until 1–2 Land

`gateway_virtual` behavioral failures to re-verify once reload + token contract are fixed: pagination stub names, subscribe/unsubscribe roundtrip, ping-via-connect, cancellation propagation.

## Status of Non-Dataplane Failures

- Full CF Locust: fixed in the harness (admin token for upstream locustfiles, [2026-07-02-locust-token-fix.md](2026-07-02-locust-token-fix.md)); residual 4% are upstream control-plane API quirks (`/prompts/{id}` 422, `/resources/{id}` 500).
- `live-all` full-suite protocol-compliance breakage: upstream runner artifact (hits `reference-stdio` identically); use individual lanes.
- SSO/runtime lanes: environmental skips.
