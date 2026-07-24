# MCP Bruno Collection

Bruno collection for manually testing MCP gateway and server flows.

The collection includes requests for:

- Getting/configuring an MCP gateway user token
- Initializing an MCP session
- Sending the MCP initialized notification
- Listing MCP tools
- Calling MCP tools against one or two local server ports
- Disconnecting an MCP session

## Requirements

- [Bruno](https://www.usebruno.com/)
- A local MCP gateway/server running on the port used by your selected environment

## Open in Bruno

1. Clone this repository.
2. Open Bruno.
3. Choose **Open Workspace** from the workspace dropdown.
4. Select this repository folder.
5. Open the `MCP Collection` collection.
6. Pick one of the environments from the collection's `environments/`.

Bruno reads `workspace.yml` first, then opens `collections/mcp-collection` as the `MCP Collection` collection.

## Environments

The collection uses these variables:

- `port`: default target port for the single-server flow
- `port1`: target port for the A flow
- `port2`: target port for the B flow
- `path`: MCP endpoint path
- `token`: bearer token or token value used by the Authorization header
- `connection_action`: optional `Connection` header value used by some environments

Available environments:

- `localenv`: two-port local gateway flow, using `8001` and `8002`
- `mcp-gateway-rs-local`: gateway-rs local defaults on `8001`
- `tower-mcp-stateless`: tower-mcp July stateless HTTP example defaults on `3000`
- `contextforge`: ContextForge gateway defaults on `8080`
- `contextforge-standalone`: standalone ContextForge defaults on `4444`
- `standalone_counter copy`: standalone counter defaults on `5555`

Update the environment values in Bruno if your gateway uses a different port, server id, endpoint path, or token.

## Typical Request Order

For `mcp-gateway-rs` style local testing:

1. Run `MCP-Gateway-rs get token`.
2. Run `MCP-Gateway-rs configure user`.
3. Run `MCP Initalize`.
4. Run `MCP Notify`.
5. Run `MCP List Tools`.
6. Run one of the `MCP Call Tools` requests.
7. Run `MCP DIsconnect` when finished.

For the two-port A/B flow:

1. Run `MCP Initalize A`.
2. Run `MCP Notify A`.
3. Run `MCP List Tools A`.
4. Run an `MCP Call Tools A` request.
5. Run `MCP Initalize B`.
6. Run `MCP Notify B`.
7. Run `MCP List Tools B`.
8. Run an `MCP Call Tools B` request.

For the July 2026 stateless Streamable HTTP flow, see [mcp-stateless-streamable-http.md](mcp-stateless-streamable-http.md).

The initialize requests save response headers into Bruno variables:

- `mcp-session-id`
- `x-correlation-id`

Later requests use these values for the same Bruno session.

## Notes

- The request names intentionally match the imported collection files.
- Token values are intentionally blank in the included environments. Set fresh local values in Bruno before running authenticated requests.
- If a request fails with `401`, refresh `token`.
- If a request fails with `404`, check `path`.
- If Bruno cannot connect, check the selected environment port and confirm the MCP service is running.
