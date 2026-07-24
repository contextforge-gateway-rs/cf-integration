# MCP Stateless Streamable HTTP Demo

Use the `tower-mcp` HTTP example with `--features "http,stateless"` running on:

```text
http://127.0.0.1:3000/
```

Start it with:

```sh
cargo run -p tower-mcp --example http_server --features "http,stateless"
```

## Bruno Setup

Select environment:

```text
tower-mcp-stateless
```

Open folder:

```text
Direct July Stateless MCP Sequence
```

## Requests To Run

Run these first:

1. `Server Discover`
2. `List Tools`
3. `Call Add Tool`

Expected:

- `Server Discover` returns capabilities without a session ID.
- `List Tools` includes `add`.
- `Call Add Tool` returns text containing `42`.
- None of the responses returns `MCP-Session-Id`.

## Stream Endpoint Check

Use curl to verify the long-lived stateless stream endpoint:

```sh
curl -N -X POST http://127.0.0.1:3000/ \
  -H 'Content-Type: application/json' \
  -H 'Accept: text/event-stream' \
  -H 'MCP-Protocol-Version: 2026-07-28' \
  -H 'Mcp-Method: messages/listen' \
  -d '{"jsonrpc":"2.0","id":4,"method":"messages/listen","params":{}}'
```

Expected:

- The request stays open.
- No `MCP-Session-Id` is sent.

## Slow Task Producer

Run in Bruno:

```text
Slow Task SSE Producer
```

Expected:

- It completes successfully.
- The response text contains `Completed 4 steps`.
- No `MCP-Session-Id` is returned.

Note: the curl stream is not expected to display chunks for this tower-mcp example. `slow_task` returns normal JSON, not request-scoped SSE chunks. The valid stateless streaming use case is a request returning its own `text/event-stream` response.
