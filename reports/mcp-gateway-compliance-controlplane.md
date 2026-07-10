# MCP Gateway Compliance: controlplane

- Specification: `2025-11-25`

| Status | Cases |
|---|---:|
| passed | 21 |
| failed | 3 |
| not applicable | 9 |
| fixture failure | 0 |

| Case | Category | Status | Specification | Detail |
|---|---|---|---|---|
| federation.duplicate-upstream-name | Federation | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | current live fixture does not register two upstream tools with the same name |
| federation.exposed-name-uniqueness | Federation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | federated catalog exposes unique tool names |
| federation.prompts-aggregation | Federation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/prompts) | advertised prompts capability returned a prompts array |
| federation.resources-aggregation | Federation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/resources) | advertised resources capability returned a resources array |
| preservation.cancellation-progress | Gateway preservation | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | current live fixture exposes no cancellable progress-emitting operation |
| preservation.tool-result | Gateway preservation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | tool result shape preserved for "fast-time-get-system-time" |
| preservation.tool-schema-stability | Gateway preservation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | repeated tools/list preserved exact tool definitions |
| preservation.tools-list | Gateway preservation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | validated 13 tool definitions |
| protocol.capability-negotiation | Protocol negotiation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | initialize advertised the tools capability required by the live fixture |
| protocol.initialize | Protocol negotiation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | initialize returned a valid JSON-RPC response |
| protocol.initialize-result | Protocol negotiation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | initialize returned an object result |
| protocol.initialized-notification | Protocol negotiation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | initialized notification returned HTTP 202 with no body |
| protocol.ping | Utilities | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/ping) | ping returned an empty object result |
| protocol.server-info | Protocol negotiation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | initialize advertised serverInfo |
| protocol.version-negotiation | Protocol negotiation | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle) | server selected the requested protocol version |
| security.authentication-required | Security | failed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization#error-handling) | expected HTTP 401, got 403 |
| security.authorization-wrong-server | Security | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization#token-handling) | raw control-plane /mcp is not scoped by a virtual server path |
| security.invalid-origin | Security | failed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#security-warning) | expected HTTP 403, got 200 |
| security.tenant-isolation | Security | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#security-warning) | current live fixture provisions one tenant and cannot establish cross-tenant isolation |
| security.virtual-server-isolation | Security | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#security-warning) | control-plane mode uses the raw /mcp route |
| session.creation | Session handling | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | initialize returned a non-empty MCP session header |
| session.delete | Session handling | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | DELETE returned HTTP 200 |
| session.deleted-session | Session handling | failed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | deleted session returned HTTP 200 |
| session.expired-session | Session handling | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | fixture does not expose a deterministic short session TTL |
| session.invalid-session | Session handling | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | invalid session returned HTTP 404 |
| session.reuse | Session handling | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | post-initialize ping reused the assigned session |
| transport.get-behaviour | HTTP transport | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | GET returned an HTTP 200 SSE stream |
| transport.invalid-protocol-version | HTTP transport | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | invalid MCP-Protocol-Version returned HTTP 400 |
| transport.malformed-json | HTTP transport | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | malformed JSON returned HTTP error 500 |
| transport.malformed-jsonrpc | HTTP transport | passed | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports) | malformed JSON-RPC returned an HTTP error or a valid Invalid Request error |
| virtualization.a2a-to-mcp | Virtualization | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | live fixture advertises no A2A-generated MCP tool |
| virtualization.grpc-to-mcp | Virtualization | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | live fixture advertises no gRPC-generated MCP tool |
| virtualization.rest-to-mcp | Virtualization | not applicable | [MCP 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools) | live fixture advertises no REST-generated MCP tool |
