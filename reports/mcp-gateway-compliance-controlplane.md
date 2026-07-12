# MCP Gateway Compliance: controlplane

- Specification: `2025-11-25`

| Status | Cases |
|---|---:|
| passed | 20 |
| failed | 4 |
| not applicable | 9 |
| fixture failure | 0 |

| Case | Category | Status | Specification | Detail |
|---|---|---|---|---|
| federation.duplicate-upstream-name | Federation | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | current live fixture does not register two upstream tools with the same name |
| federation.exposed-name-uniqueness | Federation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | federated catalog exposes unique tool names |
| federation.prompts-aggregation | Federation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/prompts) | advertised prompts capability returned a prompts array |
| federation.resources-aggregation | Federation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/resources) | advertised resources capability returned a resources array |
| preservation.cancellation-progress | Gateway preservation | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | current live fixture exposes no cancellable progress-emitting operation |
| preservation.tool-result | Gateway preservation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | tool result shape preserved for "fast_time_get_system_time" |
| preservation.tool-schema-stability | Gateway preservation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | repeated tools/list preserved exact tool definitions |
| preservation.tools-list | Gateway preservation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | validated 27 tool definitions |
| protocol.capability-negotiation | Protocol negotiation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | initialize advertised the tools capability required by the live fixture |
| protocol.initialize | Protocol negotiation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | initialize returned a valid JSON-RPC response |
| protocol.initialize-result | Protocol negotiation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | initialize returned an object result |
| protocol.initialized-notification | Protocol negotiation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | initialized notification returned HTTP 202 with no body |
| protocol.ping | Utilities | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/ping) | ping returned an empty object result |
| protocol.server-info | Protocol negotiation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | initialize advertised serverInfo with non-empty name and version |
| protocol.version-negotiation | Protocol negotiation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | server selected the requested protocol version |
| security.authentication-required | Security | failed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization#error-handling) | expected HTTP 401, got 403 |
| security.authorization-wrong-server | Security | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization#token-handling) | raw control-plane /mcp is not scoped by a virtual server path |
| security.invalid-origin | Security | failed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#security-warning) | expected HTTP 403, got 200 |
| security.tenant-isolation | Security | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#security-warning) | current live fixture provisions one tenant and cannot establish cross-tenant isolation |
| security.virtual-server-isolation | Security | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#security-warning) | control-plane mode uses the raw /mcp route |
| session.creation | Session handling | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | initialize returned a non-empty MCP session header |
| session.delete | Session handling | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | DELETE returned HTTP 200 |
| session.deleted-session | Session handling | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | deleted session was rejected with HTTP 404 |
| session.expired-session | Session handling | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | fixture does not expose a deterministic short session TTL |
| session.invalid-session | Session handling | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | invalid session returned HTTP 404 |
| session.reuse | Session handling | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | post-initialize ping reused the assigned session |
| transport.get-behaviour | HTTP transport | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | GET returned an HTTP 200 SSE stream |
| transport.invalid-protocol-version | HTTP transport | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | invalid MCP-Protocol-Version returned HTTP 400 |
| transport.malformed-json | HTTP transport | failed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | expected HTTP 400 or JSON-RPC Parse Error -32700, got HTTP 500 |
| transport.malformed-jsonrpc | HTTP transport | failed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | malformed JSON-RPC returned HTTP 200 without HTTP 400 or a valid -32600 Invalid Request envelope |
| virtualization.a2a-to-mcp | Virtualization | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | live fixture advertises no A2A-generated MCP tool |
| virtualization.grpc-to-mcp | Virtualization | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | live fixture advertises no gRPC-generated MCP tool |
| virtualization.rest-to-mcp | Virtualization | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | live fixture advertises no REST-generated MCP tool |

## Failure evidence

### security.authentication-required

| Field | Value |
|---|---|
| Stack mode | controlplane |
| Protocol version | 2025-11-25 |
| Request availability | captured |
| Request method | POST |
| Request URL | http://127.0.0.1:8080/mcp |
| Request headers | {"accept":"application/json, text/event-stream","content-type":"application/json"} |
| Request body | {"id":90,"jsonrpc":"2.0","method":"initialize","params":{"capabilities":{},"clientInfo":{"name":"cf-integration","version":"1.0"},"protocolVersion":"2025-11-25"}} |
| Response availability | captured |
| Response status | 403 |
| Response headers | {"connection":"keep-alive","content-length":"63","content-type":"application/json","date":"Sun, 12 Jul 2026 16:44:08 GMT","server":"nginx","x-accel-buffering":"no"} |
| Response body | {"detail":"CSRF validation failed","code":"CSRF_TOKEN_INVALID"} |

### security.invalid-origin

| Field | Value |
|---|---|
| Stack mode | controlplane |
| Protocol version | 2025-11-25 |
| Request availability | captured |
| Request method | POST |
| Request URL | http://127.0.0.1:8080/mcp |
| Request headers | {"accept":"application/json, text/event-stream","authorization":"&lt;redacted&gt;","content-type":"application/json","origin":"https://attacker.invalid"} |
| Request body | {"id":91,"jsonrpc":"2.0","method":"initialize","params":{"capabilities":{},"clientInfo":{"name":"cf-integration","version":"1.0"},"protocolVersion":"2025-11-25"}} |
| Response availability | captured |
| Response status | 200 |
| Response headers | {"access-control-allow-credentials":"true","access-control-expose-headers":"Content-Length, X-Request-ID, X-Password-Change-Required","cache-control":"no-store, private","connection":"keep-alive","content-security-policy":"default-src 'self'; script-src-elem 'self' 'nonce-PsRbQwT3uLqvwPiVQOZuyQ' https://cdnjs.cloudflare.com https://cdn.jsdelivr.net https://unpkg.com; script-src-attr 'unsafe-inline'; script-src 'self' 'unsafe-eval'; style-src 'self' 'unsafe-inline' https://cdnjs.cloudflare.com https://cdn.jsdelivr.net; img-src 'self' data: https:; font-src 'self' data: https://cdnjs.cloudflare.com; connect-src 'self' ws: wss: https:; frame-ancestors 'none';","content-type":"application/json","date":"Sun, 12 Jul 2026 16:44:08 GMT","expires":"0","mcp-session-id":"&lt;redacted&gt;","pragma":"no-cache","referrer-policy":"strict-origin-when-cross-origin","server":"nginx","transfer-encoding":"chunked","vary":"Authorization","x-accel-buffering":"no","x-content-type-options":"nosniff","x-contextforge-mcp-affinity-core":"python","x-contextforge-mcp-live-stream-core":"python","x-contextforge-mcp-resume-core":"python","x-contextforge-mcp-runtime":"python","x-contextforge-mcp-session-auth-reuse":"python","x-contextforge-mcp-session-core":"python","x-download-options":"noopen","x-frame-options":"DENY","x-xss-protection":"0"} |
| Response body | {"jsonrpc":"2.0","id":91,"result":{"protocolVersion":"2025-11-25","capabilities":{"experimental":{},"logging":{},"prompts":{"listChanged":false},"resources":{"subscribe":false,"listChanged":false},"tools":{"listChanged":false},"completions":{}},"serverInfo":{"name":"mcp-streamable-http","version":"1.28.1"}}} |

### transport.malformed-json

| Field | Value |
|---|---|
| Stack mode | controlplane |
| Protocol version | 2025-11-25 |
| Request availability | captured |
| Request method | POST |
| Request URL | http://127.0.0.1:8080/mcp |
| Request headers | {"accept":"application/json, text/event-stream","authorization":"&lt;redacted&gt;","content-type":"application/json","mcp-protocol-version":"2025-11-25","mcp-session-id":"&lt;redacted&gt;"} |
| Request body | {not-json |
| Response availability | captured |
| Response status | 500 |
| Response headers | {"cache-control":"no-store, private","connection":"keep-alive","content-length":"37","content-security-policy":"default-src 'self'; script-src-elem 'self' 'nonce-FcEXRRS70v3GB3p44OdGNA' https://cdnjs.cloudflare.com https://cdn.jsdelivr.net https://unpkg.com; script-src-attr 'unsafe-inline'; script-src 'self' 'unsafe-eval'; style-src 'self' 'unsafe-inline' https://cdnjs.cloudflare.com https://cdn.jsdelivr.net; img-src 'self' data: https:; font-src 'self' data: https://cdnjs.cloudflare.com; connect-src 'self' ws: wss: https:; frame-ancestors 'none';","content-type":"application/json","date":"Sun, 12 Jul 2026 16:44:09 GMT","expires":"0","pragma":"no-cache","referrer-policy":"strict-origin-when-cross-origin","server":"nginx","vary":"Authorization","x-accel-buffering":"no","x-content-type-options":"nosniff","x-contextforge-mcp-affinity-core":"python","x-contextforge-mcp-live-stream-core":"python","x-contextforge-mcp-resume-core":"python","x-contextforge-mcp-runtime":"python","x-contextforge-mcp-session-auth-reuse":"python","x-contextforge-mcp-session-core":"python","x-download-options":"noopen","x-frame-options":"DENY","x-xss-protection":"0"} |
| Response body | {"error":"Internal forwarding error"} |

### transport.malformed-jsonrpc

| Field | Value |
|---|---|
| Stack mode | controlplane |
| Protocol version | 2025-11-25 |
| Request availability | captured |
| Request method | POST |
| Request URL | http://127.0.0.1:8080/mcp |
| Request headers | {"accept":"application/json, text/event-stream","authorization":"&lt;redacted&gt;","content-type":"application/json","mcp-protocol-version":"2025-11-25","mcp-session-id":"&lt;redacted&gt;"} |
| Request body | {"id":22,"method":"ping"} |
| Response availability | captured |
| Response status | 200 |
| Response headers | {"cache-control":"no-store, private","connection":"keep-alive","content-length":"77","content-security-policy":"default-src 'self'; script-src-elem 'self' 'nonce-LYshaFVp9j801E_Pn3igvw' https://cdnjs.cloudflare.com https://cdn.jsdelivr.net https://unpkg.com; script-src-attr 'unsafe-inline'; script-src 'self' 'unsafe-eval'; style-src 'self' 'unsafe-inline' https://cdnjs.cloudflare.com https://cdn.jsdelivr.net; img-src 'self' data: https:; font-src 'self' data: https://cdnjs.cloudflare.com; connect-src 'self' ws: wss: https:; frame-ancestors 'none';","content-type":"application/json","date":"Sun, 12 Jul 2026 16:44:09 GMT","expires":"0","mcp-session-id":"&lt;redacted&gt;","pragma":"no-cache","referrer-policy":"strict-origin-when-cross-origin","server":"nginx","vary":"Authorization","x-accel-buffering":"no","x-content-type-options":"nosniff","x-contextforge-mcp-affinity-core":"python","x-contextforge-mcp-live-stream-core":"python","x-contextforge-mcp-resume-core":"python","x-contextforge-mcp-runtime":"python","x-contextforge-mcp-session-auth-reuse":"python","x-contextforge-mcp-session-core":"python","x-download-options":"noopen","x-frame-options":"DENY","x-xss-protection":"0"} |
| Response body | {"jsonrpc":"2.0","error":{"code":-32600,"message":"Invalid Request"},"id":22} |
