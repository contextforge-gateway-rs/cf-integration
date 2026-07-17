# MCP Conformance Comparison

- Official oracle: `@modelcontextprotocol/conformance@0.2.0-alpha.9`
- Specification: `2025-11-25`
- Suite: `all`
- Fixture source: `https://github.com/modelcontextprotocol/conformance` at `794dcab99ed1ef2b89607be9999574140ea5c96e`

## Target outcomes

| Target | Compliant scenarios | Failed scenarios | Failed checks | Fixture failures | Not applicable | Ambiguous | Missing |
|---|---:|---:|---:|---:|---:|---:|---:|
| Fixture direct | 32 | 0 | 0 | 0 | 0 | 0 | 0 |
| Control plane | 22 | 10 | 10 | 0 | 0 | 0 | 0 |
| Dataplane | 6 | 26 | 26 | 0 | 0 | 0 | 0 |

## Comparison summary

| Classification | Scenarios |
|---|---:|
| all compliant | 5 |
| fixture-only failure | 0 |
| control-plane only failure | 1 |
| dataplane only failure | 17 |
| fixture + control-plane failure | 0 |
| fixture + dataplane failure | 0 |
| both gateways only failure | 9 |
| shared failure | 0 |
| fixture failure | 0 |
| not applicable | 0 |
| ambiguous | 0 |

## Scenarios

| Scenario | Fixture direct | Control plane | Dataplane | Classification | Specification references |
|---|---|---|---|---|---|
| completion-complete | compliant | failure | failure | both gateways only failure | [MCP-Completion](https://modelcontextprotocol.io/specification/2025-06-18/server/utilities/completion) |
| dns-rebinding-protection | compliant | failure | failure | both gateways only failure | [MCP-DNS-Rebinding-Protection](https://modelcontextprotocol.io/specification/2025-11-25/basic/security_best_practices#local-mcp-server-compromise)<br>[MCP-Transport-Security](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#security-warning) |
| elicitation-sep1034-defaults | compliant | failure | failure | both gateways only failure | [SEP-1034](https://github.com/modelcontextprotocol/modelcontextprotocol/issues/1034) |
| elicitation-sep1330-enums | compliant | failure | failure | both gateways only failure | [SEP-1330](https://github.com/modelcontextprotocol/modelcontextprotocol/issues/1330) |
| json-schema-2020-12 | compliant | compliant | failure | dataplane only failure | [SEP-1613](https://github.com/modelcontextprotocol/specification/pull/655)<br>[SEP-2106](https://github.com/modelcontextprotocol/modelcontextprotocol/pull/2106) |
| logging-set-level | compliant | compliant | failure | dataplane only failure | [MCP-Logging](https://modelcontextprotocol.io/specification/2025-06-18/server/utilities/logging) |
| ping | compliant | compliant | compliant | all compliant | [MCP-Ping](https://modelcontextprotocol.io/specification/2025-06-18/basic/utilities/ping) |
| prompts-get-embedded-resource | compliant | failure | failure | both gateways only failure | [MCP-Prompts-Embedded-Resources](https://modelcontextprotocol.io/specification/2025-06-18/server/prompts#embedded-resources) |
| prompts-get-simple | compliant | compliant | failure | dataplane only failure | [MCP-Prompts-Get](https://modelcontextprotocol.io/specification/2025-06-18/server/prompts#getting-prompts) |
| prompts-get-with-args | compliant | compliant | failure | dataplane only failure | [MCP-Prompts-Get](https://modelcontextprotocol.io/specification/2025-06-18/server/prompts#getting-prompts) |
| prompts-get-with-image | compliant | compliant | failure | dataplane only failure | [MCP-Prompts-Image](https://modelcontextprotocol.io/specification/2025-06-18/server/prompts#image-content) |
| prompts-list | compliant | compliant | compliant | all compliant | [MCP-Prompts-List](https://modelcontextprotocol.io/specification/2025-06-18/server/prompts#listing-prompts) |
| resources-list | compliant | compliant | compliant | all compliant | [MCP-Resources-List](https://modelcontextprotocol.io/specification/2025-06-18/server/resources#listing-resources) |
| resources-read-binary | compliant | failure | failure | both gateways only failure | [MCP-Resources-Read](https://modelcontextprotocol.io/specification/2025-06-18/server/resources#reading-resources) |
| resources-read-text | compliant | compliant | failure | dataplane only failure | [MCP-Resources-Read](https://modelcontextprotocol.io/specification/2025-06-18/server/resources#reading-resources) |
| resources-subscribe | compliant | compliant | failure | dataplane only failure | [MCP-Resources-Subscribe](https://modelcontextprotocol.io/specification/2025-06-18/server/resources#resource-subscriptions) |
| resources-templates-read | compliant | failure | failure | both gateways only failure | [MCP-Resources-Templates](https://modelcontextprotocol.io/specification/2025-06-18/server/resources#resource-templates) |
| resources-unsubscribe | compliant | compliant | failure | dataplane only failure | [MCP-Resources-Subscribe](https://modelcontextprotocol.io/specification/2025-06-18/schema#unsubscriberequest)<br>[MCP-Resources-Subscribe](https://modelcontextprotocol.io/specification/2025-06-18/server/resources#resource-subscriptions) |
| server-initialize | compliant | compliant | compliant | all compliant | [MCP-Initialize](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle#initialization)<br>[MCP-Session-Management](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#session-management) |
| server-sse-multiple-streams | compliant | failure | compliant | control-plane only failure | [SEP-1699](https://github.com/modelcontextprotocol/modelcontextprotocol/issues/1699) |
| server-sse-polling | compliant | compliant | compliant | all compliant | [SEP-1699](https://github.com/modelcontextprotocol/modelcontextprotocol/issues/1699) |
| tools-call-audio | compliant | compliant | failure | dataplane only failure | [MCP-Tools-Call](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#calling-tools) |
| tools-call-elicitation | compliant | failure | failure | both gateways only failure | [MCP-Elicitation](https://modelcontextprotocol.io/specification/2025-06-18/server/utilities/elicitation) |
| tools-call-embedded-resource | compliant | compliant | failure | dataplane only failure | [MCP-Tools-Call](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#calling-tools) |
| tools-call-error | compliant | compliant | failure | dataplane only failure | [MCP-Error-Handling](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle) |
| tools-call-image | compliant | compliant | failure | dataplane only failure | [MCP-Tools-Call](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#calling-tools) |
| tools-call-mixed-content | compliant | compliant | failure | dataplane only failure | [MCP-Tools-Call](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#calling-tools) |
| tools-call-sampling | compliant | failure | failure | both gateways only failure | [MCP-Sampling](https://modelcontextprotocol.io/specification/2025-06-18/server/utilities/sampling) |
| tools-call-simple-text | compliant | compliant | failure | dataplane only failure | [MCP-Tools-Call](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#calling-tools) |
| tools-call-with-logging | compliant | compliant | failure | dataplane only failure | [MCP-Logging](https://modelcontextprotocol.io/specification/2025-06-18/server/utilities/logging) |
| tools-call-with-progress | compliant | compliant | failure | dataplane only failure | [MCP-Progress](https://modelcontextprotocol.io/specification/2025-06-18/server/utilities/progress) |
| tools-list | compliant | compliant | failure | dataplane only failure | [MCP-Tools-List](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#listing-tools)<br>[MCP-Tools-List](https://modelcontextprotocol.io/specification/2025-11-25/server/tools#listing-tools)<br>[SEP-986](https://modelcontextprotocol.io/specification/2025-11-25/server/tools#tool-names) |
