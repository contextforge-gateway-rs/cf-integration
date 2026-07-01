# AGENTS.md

This repo is the reusable integration harness for `cf-controlplane` plus the Rust `cf-dataplane`.

Scope:

- Keep this repo focused on Docker Compose overlays, nginx routing, reusable test orchestration, and local test backends.
- Do not add dataplane implementation code here.
- Do not add control-plane implementation code here.
- Keep generated checkout/build/runtime state under `.integration/` or `CF_INTEGRATION_DIR`.
- Preserve the public routing contract: `/servers/{virtual_host_id}/mcp` goes to `cf-dataplane`; raw `/mcp` and UI/API traffic go to `cf-controlplane`.
- Use published `cf-dataplane` images by default. Local builds should be explicit overrides.
