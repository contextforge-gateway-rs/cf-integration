# MCP Conformance Comparison

- Official oracle: `@modelcontextprotocol/conformance@0.1.16`
- Specification: `2025-11-25`
- Suite: `all`

## Summary

| Classification | Scenarios |
|---|---:|
| both compliant | 6 |
| control-plane compliant | 0 |
| dataplane compliant | 0 |
| control-plane only failure | 2 |
| dataplane only failure | 1 |
| shared failure | 1 |
| expected failure | 0 |
| fixture failure | 21 |
| not applicable | 0 |
| ambiguous | 1 |

## Scenarios

| Scenario | Control plane | Dataplane | Classification | Expected by | Specification references |
|---|---|---|---|---|---|
| completion-complete | fixture failure | fixture failure | fixture failure | — | [MCP-Completion](https://modelcontextprotocol.io/specification/2025-06-18/server/utilities/completion) |
| dns-rebinding-protection | failure | failure | shared failure | — | [MCP-DNS-Rebinding-Protection](https://modelcontextprotocol.io/specification/2025-11-25/basic/security_best_practices#local-mcp-server-compromise)<br>[MCP-Transport-Security](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#security-warning) |
| elicitation-sep1034-defaults | fixture failure | fixture failure | fixture failure | — | [SEP-1034](https://github.com/modelcontextprotocol/modelcontextprotocol/issues/1034) |
| elicitation-sep1330-enums | fixture failure | fixture failure | fixture failure | — | [SEP-1330](https://github.com/modelcontextprotocol/modelcontextprotocol/issues/1330) |
| json-schema-2020-12 | fixture failure | fixture failure | fixture failure | — | [SEP-1613](https://github.com/modelcontextprotocol/specification/pull/655) |
| logging-set-level | compliant | failure | dataplane only failure | — | [MCP-Logging](https://modelcontextprotocol.io/specification/2025-06-18/server/utilities/logging) |
| ping | compliant | compliant | both compliant | — | [MCP-Ping](https://modelcontextprotocol.io/specification/2025-06-18/basic/utilities/ping) |
| prompts-get-embedded-resource | fixture failure | fixture failure | fixture failure | — | [MCP-Prompts-Embedded-Resources](https://modelcontextprotocol.io/specification/2025-06-18/server/prompts#embedded-resources) |
| prompts-get-simple | fixture failure | fixture failure | fixture failure | — | [MCP-Prompts-Get](https://modelcontextprotocol.io/specification/2025-06-18/server/prompts#getting-prompts) |
| prompts-get-with-args | fixture failure | fixture failure | fixture failure | — | [MCP-Prompts-Get](https://modelcontextprotocol.io/specification/2025-06-18/server/prompts#getting-prompts) |
| prompts-get-with-image | fixture failure | fixture failure | fixture failure | — | [MCP-Prompts-Image](https://modelcontextprotocol.io/specification/2025-06-18/server/prompts#image-content) |
| prompts-list | compliant | compliant | both compliant | — | [MCP-Prompts-List](https://modelcontextprotocol.io/specification/2025-06-18/server/prompts#listing-prompts) |
| resources-list | compliant | compliant | both compliant | — | [MCP-Resources-List](https://modelcontextprotocol.io/specification/2025-06-18/server/resources#listing-resources) |
| resources-read-binary | fixture failure | fixture failure | fixture failure | — | [MCP-Resources-Read](https://modelcontextprotocol.io/specification/2025-06-18/server/resources#reading-resources) |
| resources-read-text | fixture failure | fixture failure | fixture failure | — | [MCP-Resources-Read](https://modelcontextprotocol.io/specification/2025-06-18/server/resources#reading-resources) |
| resources-subscribe | failure | compliant | control-plane only failure | — | [MCP-Resources-Subscribe](https://modelcontextprotocol.io/specification/2025-06-18/server/resources#resource-subscriptions) |
| resources-templates-read | fixture failure | fixture failure | fixture failure | — | [MCP-Resources-Templates](https://modelcontextprotocol.io/specification/2025-06-18/server/resources#resource-templates) |
| resources-unsubscribe | failure | compliant | control-plane only failure | — | [MCP-Resources-Subscribe](https://modelcontextprotocol.io/specification/2025-06-18/schema#unsubscriberequest)<br>[MCP-Resources-Subscribe](https://modelcontextprotocol.io/specification/2025-06-18/server/resources#resource-subscriptions) |
| server-initialize | compliant | compliant | both compliant | — | [MCP-Initialize](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle#initialization) |
| server-sse-multiple-streams | compliant | compliant | both compliant | — | [SEP-1699](https://github.com/modelcontextprotocol/modelcontextprotocol/issues/1699) |
| server-sse-polling | ambiguous | failure | ambiguous | — | [SEP-1699](https://github.com/modelcontextprotocol/modelcontextprotocol/issues/1699) |
| tools-call-audio | fixture failure | fixture failure | fixture failure | — | [MCP-Tools-Call](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#calling-tools) |
| tools-call-elicitation | fixture failure | fixture failure | fixture failure | — | [MCP-Elicitation](https://modelcontextprotocol.io/specification/2025-06-18/server/utilities/elicitation) |
| tools-call-embedded-resource | fixture failure | fixture failure | fixture failure | — | [MCP-Tools-Call](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#calling-tools) |
| tools-call-error | fixture failure | fixture failure | fixture failure | — | [MCP-Error-Handling](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle) |
| tools-call-image | fixture failure | fixture failure | fixture failure | — | [MCP-Tools-Call](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#calling-tools) |
| tools-call-mixed-content | fixture failure | fixture failure | fixture failure | — | [MCP-Tools-Call](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#calling-tools) |
| tools-call-sampling | fixture failure | fixture failure | fixture failure | — | [MCP-Sampling](https://modelcontextprotocol.io/specification/2025-06-18/server/utilities/sampling) |
| tools-call-simple-text | fixture failure | fixture failure | fixture failure | — | [MCP-Tools-Call](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#calling-tools) |
| tools-call-with-logging | fixture failure | failure | fixture failure | — | [MCP-Logging](https://modelcontextprotocol.io/specification/2025-06-18/server/utilities/logging) |
| tools-call-with-progress | fixture failure | fixture failure | fixture failure | — | [MCP-Progress](https://modelcontextprotocol.io/specification/2025-06-18/server/utilities/progress) |
| tools-list | compliant | compliant | both compliant | — | [MCP-Tools-List](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#listing-tools) |
