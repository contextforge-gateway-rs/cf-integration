# live-rbac server access — tracking doc

Tracks the harness-side work needed to make the `live-rbac` lane green without
depending on upstream fixes.

## Current state (2026-07-06 report)

`live-rbac` fails on 1 test:

- `test_admin_sees_public_and_team_via_http` — the `/servers` REST listing
  returns another user's **private** server to admin, violating the PR #4341
  visibility contract.  Reproduces on the stock control-plane-only stack (no
  dataplane involved).

The 39 passing tests include RBAC allow-path cases that reach the dataplane
through SSE-backed fixtures; those tests pass hollow (0 tools) because the
dataplane does not implement SSE upstreams (won't-fix, SSE deprecated
2026-07-28).

## Harness-side items on this branch

### 1. Convergence retry for live-rbac fixtures

`test-mcp-rbac` fixtures that register runtime virtual hosts can race the
publisher (2s interval in the integration stack).  Add a deadline-wait
(same pattern as IBM/mcp-context-forge#5482) in the harness overlay so
`live-rbac` does not flake on slow machines.

_Blocker_: none; pure harness change.

### 2. SSE fixture exclusion from dataplane UserConfig

The publisher exports SSE-backed gateways into `UserConfig`; the dataplane
silently drops them.  The harness overlay should filter non-`STREAMABLEHTTP`
backends from the published config so the config accurately reflects what the
dataplane will serve.

_Upstream fix_: IBM/mcp-context-forge `dataplane_publisher.py` — exclude
backends whose `transport != "STREAMABLEHTTP"` before writing `allowed_tool_names`.

### 3. `allowed_tool_names` — bare names vs slugs

Published `allowed_tool_names` currently contain control-plane slugs
(`compliance-reference-progress-reporter`) while upstream MCP servers advertise
bare names (`progress_reporter`), causing the dataplane to return 0 tools.

_Root cause and fix_: `DataplanePublisherService._build_user_data` in
`mcpgateway/services/dataplane_publisher.py` builds `tool_name_by_id` using
`tool.name` (the slugged hybrid property).  Change to `tool.original_name` so
the allow-list carries the exact name the upstream advertises.

```python
# mcpgateway/services/dataplane_publisher.py  ~line 280
# before:
tool_name_by_id = {tool.id: tool.name for tool in tool_rows ...}
# after:
tool_name_by_id = {tool.id: tool.original_name for tool in tool_rows ...}
```

This one-word change makes IBM/mcp-context-forge PR #55 unnecessary.

## Exit criteria

`scripts/cf-integration.sh live-rbac` exits 0 on the integration stack with
stock published images (no local overrides).
