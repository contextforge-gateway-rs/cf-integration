# MCP Gateway Compliance: dataplane

- Specification: `2025-11-25`

| Status | Cases |
|---|---:|
| passed | 17 |
| failed | 7 |
| not applicable | 9 |
| fixture failure | 0 |

| Case | Category | Status | Specification | Detail |
|---|---|---|---|---|
| federation.duplicate-upstream-name | Federation | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | current live fixture does not register two upstream tools with the same name |
| federation.exposed-name-uniqueness | Federation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | federated catalog exposes unique tool names |
| federation.prompts-aggregation | Federation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/prompts) | advertised prompts capability returned a prompts array |
| federation.resources-aggregation | Federation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/resources) | advertised resources capability returned a resources array |
| preservation.cancellation-progress | Gateway preservation | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | current live fixture exposes no cancellable progress-emitting operation |
| preservation.tool-result | Gateway preservation | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | fixture advertises no explicitly safe echo or system-time tool |
| preservation.tool-schema-stability | Gateway preservation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | repeated tools/list preserved exact tool definitions |
| preservation.tools-list | Gateway preservation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | validated 14 tool definitions |
| protocol.capability-negotiation | Protocol negotiation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | initialize advertised the tools capability required by the live fixture |
| protocol.initialize | Protocol negotiation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | initialize returned a valid JSON-RPC response |
| protocol.initialize-result | Protocol negotiation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | initialize returned an object result |
| protocol.initialized-notification | Protocol negotiation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | initialized notification returned HTTP 202 with no body |
| protocol.ping | Utilities | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/ping) | ping returned an empty object result |
| protocol.server-info | Protocol negotiation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | initialize advertised serverInfo with non-empty name and version |
| protocol.version-negotiation | Protocol negotiation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | server selected the requested protocol version |
| security.authentication-required | Security | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization#error-handling) | unauthenticated initialize returned HTTP 401 |
| security.authorization-wrong-server | Security | failed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization#token-handling) | wrong-server token returned HTTP 200 |
| security.invalid-origin | Security | failed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#security-warning) | expected HTTP 403, got 200 |
| security.tenant-isolation | Security | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#security-warning) | current live fixture provisions one tenant and cannot establish cross-tenant isolation |
| security.virtual-server-isolation | Security | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#security-warning) | a second live virtual-server fixture is not provisioned |
| session.creation | Session handling | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | initialize returned a non-empty MCP session header |
| session.delete | Session handling | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | DELETE returned HTTP 202 |
| session.deleted-session | Session handling | failed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | gateway Dataplane: dataplane response backend marker identifies controlplane fallback; exchange=Exchange { mode: Dataplane, request: RequestCapture { mode: Dataplane, method: "POST", url: "http://127.0.0.1:8080/servers/3f33286667d34b65a31c3bafd30e4c21/mcp", headers: {"accept": "application/json, text/event-stream", "authorization": "&lt;redacted&gt;", "content-type": "application/json", "mcp-protocol-version": "2025-11-25", "mcp-session-id": "&lt;redacted&gt;"}, body: Some("{\\"id\\":30,\\"jsonrpc\\":\\"2.0\\",\\"method\\":\\"ping\\"}") }, status: 404, headers: {"cache-control": "no-store, private", "connection": "keep-alive", "content-length": "91", "content-security-policy": "default-src 'self'; script-src-elem 'self' 'nonce-p6fob_gYMFTFuqoo85bC-Q' https://cdnjs.cloudflare.com https://cdn.jsdelivr.net https://unpkg.com; script-src-attr 'unsafe-inline'; script-src 'self' 'unsafe-eval'; style-src 'self' 'unsafe-inline' https://cdnjs.cloudflare.com https://cdn.jsdelivr.net; img-src 'self' data: https:; font-src 'self' data: https://cdnjs.cloudflare.com; connect-src 'self' ws: wss: https:; frame-ancestors 'none';", "content-type": "application/json", "date": "Sun, 12 Jul 2026 16:46:22 GMT", "expires": "0", "pragma": "no-cache", "referrer-policy": "strict-origin-when-cross-origin", "server": "nginx", "vary": "Authorization", "x-accel-buffering": "no", "x-cf-integration-backend": "controlplane-fallback", "x-content-type-options": "nosniff", "x-contextforge-mcp-affinity-core": "python", "x-contextforge-mcp-live-stream-core": "python", "x-contextforge-mcp-resume-core": "python", "x-contextforge-mcp-runtime": "python", "x-contextforge-mcp-session-auth-reuse": "python", "x-contextforge-mcp-session-core": "python", "x-download-options": "noopen", "x-frame-options": "DENY", "x-xss-protection": "0"}, body: "&lt;response body rejected before reading&gt;", message: None, session_id: None } |
| session.expired-session | Session handling | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | fixture does not expose a deterministic short session TTL |
| session.invalid-session | Session handling | failed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | gateway Dataplane: dataplane response backend marker identifies controlplane fallback; exchange=Exchange { mode: Dataplane, request: RequestCapture { mode: Dataplane, method: "POST", url: "http://127.0.0.1:8080/servers/3f33286667d34b65a31c3bafd30e4c21/mcp", headers: {"accept": "application/json, text/event-stream", "authorization": "&lt;redacted&gt;", "content-type": "application/json", "mcp-protocol-version": "2025-11-25", "mcp-session-id": "&lt;redacted&gt;"}, body: Some("{\\"id\\":21,\\"jsonrpc\\":\\"2.0\\",\\"method\\":\\"ping\\"}") }, status: 404, headers: {"cache-control": "no-store, private", "connection": "keep-alive", "content-length": "91", "content-security-policy": "default-src 'self'; script-src-elem 'self' 'nonce-QoWH6KmsXkFlIE32xns_8w' https://cdnjs.cloudflare.com https://cdn.jsdelivr.net https://unpkg.com; script-src-attr 'unsafe-inline'; script-src 'self' 'unsafe-eval'; style-src 'self' 'unsafe-inline' https://cdnjs.cloudflare.com https://cdn.jsdelivr.net; img-src 'self' data: https:; font-src 'self' data: https://cdnjs.cloudflare.com; connect-src 'self' ws: wss: https:; frame-ancestors 'none';", "content-type": "application/json", "date": "Sun, 12 Jul 2026 16:46:21 GMT", "expires": "0", "pragma": "no-cache", "referrer-policy": "strict-origin-when-cross-origin", "server": "nginx", "vary": "Authorization", "x-accel-buffering": "no", "x-cf-integration-backend": "controlplane-fallback", "x-content-type-options": "nosniff", "x-contextforge-mcp-affinity-core": "python", "x-contextforge-mcp-live-stream-core": "python", "x-contextforge-mcp-resume-core": "python", "x-contextforge-mcp-runtime": "python", "x-contextforge-mcp-session-auth-reuse": "python", "x-contextforge-mcp-session-core": "python", "x-download-options": "noopen", "x-frame-options": "DENY", "x-xss-protection": "0"}, body: "&lt;response body rejected before reading&gt;", message: None, session_id: None } |
| session.reuse | Session handling | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | post-initialize ping reused the assigned session |
| transport.get-behaviour | HTTP transport | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | GET returned an HTTP 200 SSE stream |
| transport.invalid-protocol-version | HTTP transport | failed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | gateway Dataplane: dataplane response backend marker identifies controlplane fallback; exchange=Exchange { mode: Dataplane, request: RequestCapture { mode: Dataplane, method: "POST", url: "http://127.0.0.1:8080/servers/3f33286667d34b65a31c3bafd30e4c21/mcp", headers: {"accept": "application/json, text/event-stream", "authorization": "&lt;redacted&gt;", "content-type": "application/json", "mcp-protocol-version": "unsupported-version", "mcp-session-id": "&lt;redacted&gt;"}, body: Some("{\\"id\\":20,\\"jsonrpc\\":\\"2.0\\",\\"method\\":\\"ping\\"}") }, status: 400, headers: {"connection": "close", "content-length": "153", "content-type": "application/json", "date": "Sun, 12 Jul 2026 16:46:21 GMT", "server": "nginx", "x-accel-buffering": "no", "x-cf-integration-backend": "controlplane-fallback"}, body: "&lt;response body rejected before reading&gt;", message: None, session_id: None } |
| transport.malformed-json | HTTP transport | failed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | expected HTTP 400 or JSON-RPC Parse Error -32700, got HTTP 415 |
| transport.malformed-jsonrpc | HTTP transport | failed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | malformed JSON-RPC returned HTTP 415 without HTTP 400 or a valid -32600 Invalid Request envelope |
| virtualization.a2a-to-mcp | Virtualization | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | live fixture advertises no A2A-generated MCP tool |
| virtualization.grpc-to-mcp | Virtualization | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | live fixture advertises no gRPC-generated MCP tool |
| virtualization.rest-to-mcp | Virtualization | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | live fixture advertises no REST-generated MCP tool |

## Failure evidence

### security.invalid-origin

| Field | Value |
|---|---|
| Stack mode | dataplane |
| Protocol version | 2025-11-25 |
| Request availability | captured |
| Request method | POST |
| Request URL | http://127.0.0.1:8080/servers/3f33286667d34b65a31c3bafd30e4c21/mcp |
| Request headers | {"accept":"application/json, text/event-stream","authorization":"&lt;redacted&gt;","content-type":"application/json","origin":"https://attacker.invalid"} |
| Request body | {"id":91,"jsonrpc":"2.0","method":"initialize","params":{"capabilities":{},"clientInfo":{"name":"cf-integration","version":"1.0"},"protocolVersion":"2025-11-25"}} |
| Response availability | captured |
| Response status | 200 |
| Response headers | {"access-control-allow-origin":"*","access-control-expose-headers":"*","cache-control":"no-cache","connection":"keep-alive","content-type":"text/event-stream","date":"Sun, 12 Jul 2026 16:46:21 GMT","mcp-session-id":"&lt;redacted&gt;","server":"nginx","transfer-encoding":"chunked","vary":"origin, access-control-request-method, access-control-request-headers","x-accel-buffering":"no","x-cf-integration-backend":"dataplane"} |
| Response body | data: \\nid: 0\\nretry: 3000\\n\\ndata: {"jsonrpc":"2.0","id":91,"result":{"protocolVersion":"2025-11-25","capabilities":{"completions":{},"prompts":{},"resources":{},"tools":{}},"serverInfo":{"name":"rust-conformance-server","version":"0.1.0"},"instructions":"Rust MCP conformance test server"}}\\n\\n |

### security.authorization-wrong-server

| Field | Value |
|---|---|
| Stack mode | dataplane |
| Protocol version | 2025-11-25 |
| Request availability | captured |
| Request method | POST |
| Request URL | http://127.0.0.1:8080/servers/3f33286667d34b65a31c3bafd30e4c21/mcp |
| Request headers | {"accept":"application/json, text/event-stream","authorization":"&lt;redacted&gt;","content-type":"application/json"} |
| Request body | {"id":92,"jsonrpc":"2.0","method":"initialize","params":{"capabilities":{},"clientInfo":{"name":"cf-integration","version":"1.0"},"protocolVersion":"2025-11-25"}} |
| Response availability | captured |
| Response status | 200 |
| Response headers | {"access-control-allow-origin":"*","access-control-expose-headers":"*","cache-control":"no-cache","connection":"keep-alive","content-type":"text/event-stream","date":"Sun, 12 Jul 2026 16:46:21 GMT","mcp-session-id":"&lt;redacted&gt;","server":"nginx","transfer-encoding":"chunked","vary":"origin, access-control-request-method, access-control-request-headers","x-accel-buffering":"no","x-cf-integration-backend":"dataplane"} |
| Response body | data: \\nid: 0\\nretry: 3000\\n\\ndata: {"jsonrpc":"2.0","id":92,"result":{"protocolVersion":"2025-11-25","capabilities":{"completions":{},"prompts":{},"resources":{},"tools":{}},"serverInfo":{"name":"rust-conformance-server","version":"0.1.0"},"instructions":"Rust MCP conformance test server"}}\\n\\n |

### transport.invalid-protocol-version

| Field | Value |
|---|---|
| Stack mode | dataplane |
| Protocol version | 2025-11-25 |
| Request availability | captured |
| Request method | POST |
| Request URL | http://127.0.0.1:8080/servers/3f33286667d34b65a31c3bafd30e4c21/mcp |
| Request headers | {"accept":"application/json, text/event-stream","authorization":"&lt;redacted&gt;","content-type":"application/json","mcp-protocol-version":"unsupported-version","mcp-session-id":"&lt;redacted&gt;"} |
| Request body | {"id":20,"jsonrpc":"2.0","method":"ping"} |
| Response availability | captured |
| Response status | 400 |
| Response headers | {"connection":"close","content-length":"153","content-type":"application/json","date":"Sun, 12 Jul 2026 16:46:21 GMT","server":"nginx","x-accel-buffering":"no","x-cf-integration-backend":"controlplane-fallback"} |
| Response body | &lt;response body rejected before reading&gt; |

### session.invalid-session

| Field | Value |
|---|---|
| Stack mode | dataplane |
| Protocol version | 2025-11-25 |
| Request availability | captured |
| Request method | POST |
| Request URL | http://127.0.0.1:8080/servers/3f33286667d34b65a31c3bafd30e4c21/mcp |
| Request headers | {"accept":"application/json, text/event-stream","authorization":"&lt;redacted&gt;","content-type":"application/json","mcp-protocol-version":"2025-11-25","mcp-session-id":"&lt;redacted&gt;"} |
| Request body | {"id":21,"jsonrpc":"2.0","method":"ping"} |
| Response availability | captured |
| Response status | 404 |
| Response headers | {"cache-control":"no-store, private","connection":"keep-alive","content-length":"91","content-security-policy":"default-src 'self'; script-src-elem 'self' 'nonce-QoWH6KmsXkFlIE32xns_8w' https://cdnjs.cloudflare.com https://cdn.jsdelivr.net https://unpkg.com; script-src-attr 'unsafe-inline'; script-src 'self' 'unsafe-eval'; style-src 'self' 'unsafe-inline' https://cdnjs.cloudflare.com https://cdn.jsdelivr.net; img-src 'self' data: https:; font-src 'self' data: https://cdnjs.cloudflare.com; connect-src 'self' ws: wss: https:; frame-ancestors 'none';","content-type":"application/json","date":"Sun, 12 Jul 2026 16:46:21 GMT","expires":"0","pragma":"no-cache","referrer-policy":"strict-origin-when-cross-origin","server":"nginx","vary":"Authorization","x-accel-buffering":"no","x-cf-integration-backend":"controlplane-fallback","x-content-type-options":"nosniff","x-contextforge-mcp-affinity-core":"python","x-contextforge-mcp-live-stream-core":"python","x-contextforge-mcp-resume-core":"python","x-contextforge-mcp-runtime":"python","x-contextforge-mcp-session-auth-reuse":"python","x-contextforge-mcp-session-core":"python","x-download-options":"noopen","x-frame-options":"DENY","x-xss-protection":"0"} |
| Response body | &lt;response body rejected before reading&gt; |

### transport.malformed-json

| Field | Value |
|---|---|
| Stack mode | dataplane |
| Protocol version | 2025-11-25 |
| Request availability | captured |
| Request method | POST |
| Request URL | http://127.0.0.1:8080/servers/3f33286667d34b65a31c3bafd30e4c21/mcp |
| Request headers | {"accept":"application/json, text/event-stream","authorization":"&lt;redacted&gt;","content-type":"application/json","mcp-protocol-version":"2025-11-25","mcp-session-id":"&lt;redacted&gt;"} |
| Request body | {not-json |
| Response availability | captured |
| Response status | 415 |
| Response headers | {"access-control-allow-origin":"*","access-control-expose-headers":"*","connection":"keep-alive","content-length":"72","date":"Sun, 12 Jul 2026 16:46:21 GMT","server":"nginx","vary":"origin, access-control-request-method, access-control-request-headers","x-accel-buffering":"no","x-cf-integration-backend":"dataplane"} |
| Response body | fail to deserialize request body key must be a string at line 1 column 2 |

### transport.malformed-jsonrpc

| Field | Value |
|---|---|
| Stack mode | dataplane |
| Protocol version | 2025-11-25 |
| Request availability | captured |
| Request method | POST |
| Request URL | http://127.0.0.1:8080/servers/3f33286667d34b65a31c3bafd30e4c21/mcp |
| Request headers | {"accept":"application/json, text/event-stream","authorization":"&lt;redacted&gt;","content-type":"application/json","mcp-protocol-version":"2025-11-25","mcp-session-id":"&lt;redacted&gt;"} |
| Request body | {"id":22,"method":"ping"} |
| Response availability | captured |
| Response status | 415 |
| Response headers | {"access-control-allow-origin":"*","access-control-expose-headers":"*","connection":"keep-alive","content-length":"95","date":"Sun, 12 Jul 2026 16:46:21 GMT","server":"nginx","vary":"origin, access-control-request-method, access-control-request-headers","x-accel-buffering":"no","x-cf-integration-backend":"dataplane"} |
| Response body | fail to deserialize request body data did not match any variant of untagged enum JsonRpcMessage |

### session.deleted-session

| Field | Value |
|---|---|
| Stack mode | dataplane |
| Protocol version | 2025-11-25 |
| Request availability | captured |
| Request method | POST |
| Request URL | http://127.0.0.1:8080/servers/3f33286667d34b65a31c3bafd30e4c21/mcp |
| Request headers | {"accept":"application/json, text/event-stream","authorization":"&lt;redacted&gt;","content-type":"application/json","mcp-protocol-version":"2025-11-25","mcp-session-id":"&lt;redacted&gt;"} |
| Request body | {"id":30,"jsonrpc":"2.0","method":"ping"} |
| Response availability | captured |
| Response status | 404 |
| Response headers | {"cache-control":"no-store, private","connection":"keep-alive","content-length":"91","content-security-policy":"default-src 'self'; script-src-elem 'self' 'nonce-p6fob_gYMFTFuqoo85bC-Q' https://cdnjs.cloudflare.com https://cdn.jsdelivr.net https://unpkg.com; script-src-attr 'unsafe-inline'; script-src 'self' 'unsafe-eval'; style-src 'self' 'unsafe-inline' https://cdnjs.cloudflare.com https://cdn.jsdelivr.net; img-src 'self' data: https:; font-src 'self' data: https://cdnjs.cloudflare.com; connect-src 'self' ws: wss: https:; frame-ancestors 'none';","content-type":"application/json","date":"Sun, 12 Jul 2026 16:46:22 GMT","expires":"0","pragma":"no-cache","referrer-policy":"strict-origin-when-cross-origin","server":"nginx","vary":"Authorization","x-accel-buffering":"no","x-cf-integration-backend":"controlplane-fallback","x-content-type-options":"nosniff","x-contextforge-mcp-affinity-core":"python","x-contextforge-mcp-live-stream-core":"python","x-contextforge-mcp-resume-core":"python","x-contextforge-mcp-runtime":"python","x-contextforge-mcp-session-auth-reuse":"python","x-contextforge-mcp-session-core":"python","x-download-options":"noopen","x-frame-options":"DENY","x-xss-protection":"0"} |
| Response body | &lt;response body rejected before reading&gt; |
