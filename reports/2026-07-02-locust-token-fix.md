# Full CF Locust Token Fix

Run date: 2026-07-02, Europe/Dublin.

## Scope

Diagnoses and fixes the dominant failure in [2026-07-02-full-test-run.md](2026-07-02-full-test-run.md): the full CF Locust profile (`LOCUST_LOCUSTFILE=locustfile.py`) failing 10,675 of 17,015 requests (62.74%) with `HTTP 403` and JSON-RPC `-32003 Access denied`.

## Root Cause — Harness Token Divergence

Upstream's locust service reads an admin token minted by its `locust_token` service (`create_jwt_token --admin`, no `scopes` claim). The harness overlay instead injected its own JWT, which carries a `scopes` claim so that `cf-dataplane` accepts it — and that claim *restricts* the token on the control plane.

Verified side by side against the live stack:

```text
                                scoped token   scope-less admin token
GET /gateways                   403            200
GET /rbac/roles                 403            200
POST /rpc tools/call            -32003         result (isError: false)
```

The scoped token reproduces every failure class in the report; the admin token passes all of them. One token cannot serve both surfaces today because `cf-dataplane` rejects tokens without a `scopes` claim — the already-documented token contract gap.

## Fix

`export_locust_token` now picks per locustfile:

- `locustfile_cf_dataplane.py` (default): scoped token — required by `cf-dataplane`
- any upstream locustfile: admin token (`cf-jwt.py --admin`, new flag) — same shape upstream's `locust_token` service mints

An explicit `MCPGATEWAY_BEARER_TOKEN` still overrides both.

## Results After Fix

Full CF profile (20 users, 40s verification run):

```text
before  62.74% failures  (all 403 / -32003 authorization denials)
after    4.03% failures  (720 requests, 29 failures)
```

Remaining failures are control-plane API behavior on the raw path, reproducible on stock upstream with no dataplane involved:

```text
GET /prompts/[id]    Expected [200, 403, 404], got 422
GET /resources/[id]  Expected [200, 403, 404], got 500
```

Dataplane lanes re-verified unchanged with the scoped token:

```text
probe  PASS
smoke  PASS 70 requests, 0 failures
```

## Conclusion

The 62.74% full-CF Locust failure was harness configuration (wrong token class), not a dataplane or upstream regression. After the fix the full CF profile runs with upstream-equivalent credentials; its residual failures are upstream control-plane API quirks, and the dataplane signal remains confined to the previously documented gaps (per-user config coverage, scopes-claim contract, error-code parity).
