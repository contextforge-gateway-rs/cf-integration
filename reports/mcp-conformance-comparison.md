# MCP Conformance Comparison

- Official oracle: `@modelcontextprotocol/conformance@0.2.0-alpha.9`
- Specification: `2026-07-28`
- Suite: `all`
- Fixture source: `https://github.com/modelcontextprotocol/conformance` at `794dcab99ed1ef2b89607be9999574140ea5c96e`

## Target outcomes

| Target | Compliant scenarios | Failed scenarios | Failed checks | Fixture failures | Not applicable | Ambiguous | Missing |
|---|---:|---:|---:|---:|---:|---:|---:|
| Fixture direct | 38 | 2 | 10 | 0 | 0 | 0 | 0 |
| Control plane | 4 | 36 | 74 | 0 | 0 | 0 | 0 |
| Dataplane | 3 | 37 | 76 | 0 | 0 | 0 | 0 |

## Comparison summary

| Classification | Scenarios |
|---|---:|
| all compliant | 3 |
| fixture-only failure | 0 |
| control-plane only failure | 0 |
| dataplane only failure | 1 |
| fixture + control-plane failure | 0 |
| fixture + dataplane failure | 0 |
| both gateways only failure | 34 |
| shared failure | 2 |
| fixture failure | 0 |
| not applicable | 0 |
| ambiguous | 0 |

## Scenarios

| Scenario | Fixture direct | Control plane | Dataplane | Classification | Specification references |
|---|---|---|---|---|---|
| caching | compliant | failure | failure | both gateways only failure | [MCP-Caching](https://modelcontextprotocol.io/specification/draft/server/utilities/caching)<br>[SEP-2549](https://github.com/modelcontextprotocol/modelcontextprotocol/pull/2549) |
| completion-complete | compliant | failure | failure | both gateways only failure | [MCP-Completion](https://modelcontextprotocol.io/specification/2025-06-18/server/utilities/completion) |
| dns-rebinding-protection | compliant | failure | failure | both gateways only failure | [MCP-DNS-Rebinding-Protection](https://modelcontextprotocol.io/specification/2025-11-25/basic/security_best_practices#local-mcp-server-compromise)<br>[MCP-Transport-Security](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#security-warning) |
| http-custom-header-server-validation | failure | failure | failure | shared failure | [SEP-2243-Custom-Headers](https://modelcontextprotocol.io/specification/draft/basic/transports#server-behavior-for-custom-headers) |
| http-header-validation | failure | failure | failure | shared failure | [RFC-9110-5.5-Field-Values](https://www.rfc-editor.org/rfc/rfc9110#section-5.5)<br>[SEP-2243-Case-Sensitivity](https://modelcontextprotocol.io/specification/draft/basic/transports#case-sensitivity)<br>[SEP-2243-Server-Validation](https://modelcontextprotocol.io/specification/draft/basic/transports#server-validation) |
| input-required-result-basic-elicitation | compliant | failure | failure | both gateways only failure | [SEP-2322](https://modelcontextprotocol.io/specification/draft/basic/utilities/mrtr) |
| input-required-result-basic-list-roots | compliant | failure | failure | both gateways only failure | [SEP-2322](https://modelcontextprotocol.io/specification/draft/basic/utilities/mrtr) |
| input-required-result-basic-sampling | compliant | failure | failure | both gateways only failure | [SEP-2322](https://modelcontextprotocol.io/specification/draft/basic/utilities/mrtr) |
| input-required-result-capability-check | compliant | failure | failure | both gateways only failure | [SEP-2322](https://modelcontextprotocol.io/specification/draft/basic/utilities/mrtr) |
| input-required-result-ignore-extra-params | compliant | compliant | compliant | all compliant | [SEP-2322](https://modelcontextprotocol.io/specification/draft/basic/utilities/mrtr) |
| input-required-result-missing-input-response | compliant | compliant | compliant | all compliant | [SEP-2322](https://modelcontextprotocol.io/specification/draft/basic/utilities/mrtr) |
| input-required-result-multi-round | compliant | failure | failure | both gateways only failure | [SEP-2322](https://modelcontextprotocol.io/specification/draft/basic/utilities/mrtr) |
| input-required-result-multiple-input-requests | compliant | failure | failure | both gateways only failure | [SEP-2322](https://modelcontextprotocol.io/specification/draft/basic/utilities/mrtr) |
| input-required-result-non-tool-request | compliant | failure | failure | both gateways only failure | [SEP-2322](https://modelcontextprotocol.io/specification/draft/basic/utilities/mrtr) |
| input-required-result-request-state | compliant | failure | failure | both gateways only failure | [SEP-2322](https://modelcontextprotocol.io/specification/draft/basic/utilities/mrtr) |
| input-required-result-result-type | compliant | failure | failure | both gateways only failure | [SEP-2322](https://modelcontextprotocol.io/specification/draft/basic/utilities/mrtr) |
| input-required-result-tampered-state | compliant | failure | failure | both gateways only failure | [SEP-2322](https://modelcontextprotocol.io/specification/draft/basic/utilities/mrtr) |
| input-required-result-unsupported-methods | compliant | compliant | failure | dataplane only failure | [SEP-2322](https://modelcontextprotocol.io/specification/draft/basic/utilities/mrtr) |
| input-required-result-validate-input | compliant | compliant | compliant | all compliant | [SEP-2322](https://modelcontextprotocol.io/specification/draft/basic/utilities/mrtr) |
| json-schema-2020-12 | compliant | failure | failure | both gateways only failure | [SEP-1613](https://github.com/modelcontextprotocol/specification/pull/655)<br>[SEP-2106](https://github.com/modelcontextprotocol/modelcontextprotocol/pull/2106) |
| prompts-get-embedded-resource | compliant | failure | failure | both gateways only failure | [MCP-Prompts-Embedded-Resources](https://modelcontextprotocol.io/specification/2025-06-18/server/prompts#embedded-resources) |
| prompts-get-simple | compliant | failure | failure | both gateways only failure | [MCP-Prompts-Get](https://modelcontextprotocol.io/specification/2025-06-18/server/prompts#getting-prompts) |
| prompts-get-with-args | compliant | failure | failure | both gateways only failure | [MCP-Prompts-Get](https://modelcontextprotocol.io/specification/2025-06-18/server/prompts#getting-prompts) |
| prompts-get-with-image | compliant | failure | failure | both gateways only failure | [MCP-Prompts-Image](https://modelcontextprotocol.io/specification/2025-06-18/server/prompts#image-content) |
| prompts-list | compliant | failure | failure | both gateways only failure | [MCP-Prompts-List](https://modelcontextprotocol.io/specification/2025-06-18/server/prompts#listing-prompts) |
| resources-list | compliant | failure | failure | both gateways only failure | [MCP-Resources-List](https://modelcontextprotocol.io/specification/2025-06-18/server/resources#listing-resources) |
| resources-read-binary | compliant | failure | failure | both gateways only failure | [MCP-Resources-Read](https://modelcontextprotocol.io/specification/2025-06-18/server/resources#reading-resources) |
| resources-read-text | compliant | failure | failure | both gateways only failure | [MCP-Resources-Read](https://modelcontextprotocol.io/specification/2025-06-18/server/resources#reading-resources) |
| resources-templates-read | compliant | failure | failure | both gateways only failure | [MCP-Resources-Templates](https://modelcontextprotocol.io/specification/2025-06-18/server/resources#resource-templates) |
| sep-2164-resource-not-found | compliant | failure | failure | both gateways only failure | [SEP-2164](https://modelcontextprotocol.io/specification/draft/server/resources#error-handling) |
| server-sse-multiple-streams | compliant | failure | failure | both gateways only failure | [SEP-1699](https://github.com/modelcontextprotocol/modelcontextprotocol/issues/1699) |
| server-stateless | compliant | failure | failure | both gateways only failure | [SEP-2575](https://github.com/modelcontextprotocol/modelcontextprotocol/pull/2575) |
| tools-call-audio | compliant | failure | failure | both gateways only failure | [MCP-Tools-Call](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#calling-tools) |
| tools-call-embedded-resource | compliant | failure | failure | both gateways only failure | [MCP-Tools-Call](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#calling-tools) |
| tools-call-error | compliant | failure | failure | both gateways only failure | [MCP-Error-Handling](https://modelcontextprotocol.io/specification/2025-06-18/basic/lifecycle) |
| tools-call-image | compliant | failure | failure | both gateways only failure | [MCP-Tools-Call](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#calling-tools) |
| tools-call-mixed-content | compliant | failure | failure | both gateways only failure | [MCP-Tools-Call](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#calling-tools) |
| tools-call-simple-text | compliant | failure | failure | both gateways only failure | [MCP-Tools-Call](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#calling-tools) |
| tools-call-with-progress | compliant | failure | failure | both gateways only failure | [MCP-Progress](https://modelcontextprotocol.io/specification/2025-06-18/server/utilities/progress) |
| tools-list | compliant | failure | failure | both gateways only failure | [MCP-Tools-List](https://modelcontextprotocol.io/specification/2025-06-18/server/tools#listing-tools)<br>[MCP-Tools-List](https://modelcontextprotocol.io/specification/2025-11-25/server/tools#listing-tools)<br>[SEP-986](https://modelcontextprotocol.io/specification/2025-11-25/server/tools#tool-names) |
