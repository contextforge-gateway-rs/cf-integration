use std::fs;
use std::path::Path;
use std::process::Command;

use cf_integration::coverage::{
    self, NORMATIVE_PAGES, PINNED_SOURCE_COMMIT, PINNED_SOURCE_REPOSITORY, SPEC_VERSION,
    extract_catalog_from_checkout, extract_page_requirements, parse_coverage_overlay,
    render_coverage_report, validate_coverage_overlay, write_coverage_report,
};

const EXPECTED_PAGE_PATHS: &[&str] = &[
    "basic",
    "basic/lifecycle",
    "basic/transports",
    "basic/authorization",
    "basic/security_best_practices",
    "basic/utilities/cancellation",
    "basic/utilities/ping",
    "basic/utilities/progress",
    "basic/utilities/tasks",
    "client/roots",
    "client/sampling",
    "client/elicitation",
    "server/prompts",
    "server/resources",
    "server/tools",
    "server/utilities/completion",
    "server/utilities/logging",
    "server/utilities/pagination",
    "schema",
];

#[test]
fn page_catalog_is_complete_stable_and_pinned_to_the_official_release_commit() {
    assert_eq!(SPEC_VERSION, "2025-11-25");
    assert_eq!(
        PINNED_SOURCE_REPOSITORY,
        "https://github.com/modelcontextprotocol/modelcontextprotocol.git"
    );
    assert_eq!(
        PINNED_SOURCE_COMMIT,
        "38c84e9f93ad191d9eb26d92b945d17bd0efcaf3"
    );
    assert_eq!(
        NORMATIVE_PAGES
            .iter()
            .map(|page| page.path)
            .collect::<Vec<_>>(),
        EXPECTED_PAGE_PATHS
    );
    assert_eq!(
        NORMATIVE_PAGES
            .iter()
            .map(|page| page.source_path)
            .collect::<Vec<_>>(),
        [
            "basic/index.mdx",
            "basic/lifecycle.mdx",
            "basic/transports.mdx",
            "basic/authorization.mdx",
            "basic/security_best_practices.mdx",
            "basic/utilities/cancellation.mdx",
            "basic/utilities/ping.mdx",
            "basic/utilities/progress.mdx",
            "basic/utilities/tasks.mdx",
            "client/roots.mdx",
            "client/sampling.mdx",
            "client/elicitation.mdx",
            "server/prompts.mdx",
            "server/resources.mdx",
            "server/tools.mdx",
            "server/utilities/completion.mdx",
            "server/utilities/logging.mdx",
            "server/utilities/pagination.mdx",
            "schema.mdx",
        ]
    );
}

#[test]
fn extraction_tracks_headings_keywords_summaries_and_stable_local_ordinals() {
    let source = r#"---
title: Lifecycle
---

# Lifecycle

The key words “MUST”, “MUST NOT”, “REQUIRED”, “SHOULD”, “SHOULD NOT” and “MAY”
are to be interpreted as described in BCP 14.

## Initialization & Negotiation

The initialization request MUST be the first interaction.

- Clients SHOULD advertise only capabilities they support.
- Servers MUST NOT use capabilities the client omitted and MAY reject invalid input.

```json
{"example": "a client MUST ignore this code example"}
```

Inline code such as `MUST NOT` is illustrative, but the receiver SHOULD continue.
"#;

    let requirements = extract_page_requirements("basic/lifecycle", source)
        .expect("known page should be extracted");

    assert_eq!(requirements.len(), 5);
    assert_eq!(
        requirements
            .iter()
            .map(|requirement| requirement.local_id.as_str())
            .collect::<Vec<_>>(),
        [
            "2025-11-25:basic/lifecycle#initialization-negotiation:1",
            "2025-11-25:basic/lifecycle#initialization-negotiation:2",
            "2025-11-25:basic/lifecycle#initialization-negotiation:3",
            "2025-11-25:basic/lifecycle#initialization-negotiation:4",
            "2025-11-25:basic/lifecycle#initialization-negotiation:5",
        ]
    );
    assert_eq!(requirements[0].heading, "Initialization & Negotiation");
    assert_eq!(requirements[0].keyword.as_str(), "MUST");
    assert_eq!(requirements[1].keyword.as_str(), "SHOULD");
    assert_eq!(requirements[2].keyword.as_str(), "MUST NOT");
    assert_eq!(requirements[3].keyword.as_str(), "MAY");
    assert_eq!(requirements[4].keyword.as_str(), "SHOULD");
    assert_eq!(
        requirements[0].summary,
        "The initialization request MUST be the first interaction."
    );
    assert!(
        requirements
            .iter()
            .all(|requirement| !requirement.summary.contains("code example"))
    );
}

#[test]
fn extraction_rejects_unknown_pages_and_empty_heading_slugs_are_stable() {
    let error = extract_page_requirements("draft/future", "# Future\nA peer MUST comply.\n")
        .expect_err("uncatalogued pages must not enter the inventory");
    assert!(error.to_string().contains("unknown normative page"));

    let requirements = extract_page_requirements(
        "basic",
        "# `@#$`\nA peer MUST comply.\n\n# `@#$`\nA peer MAY stop.\n",
    )
    .expect("empty heading slugs should use a deterministic fallback");
    assert_eq!(
        requirements
            .iter()
            .map(|requirement| requirement.local_id.as_str())
            .collect::<Vec<_>>(),
        ["2025-11-25:basic#section:1", "2025-11-25:basic#section:2"]
    );
}

#[test]
fn extraction_reads_normative_prose_inside_generated_schema_html() {
    let source = r#"# Schema Reference

## Tool

<div class="typedoc"><span>interface</span><div><p>Consumers MUST validate the schema.</p><p>They SHOULD NOT cache invalid values.</p></div></div>
"#;

    let requirements =
        extract_page_requirements("schema", source).expect("schema HTML should be extracted");

    assert_eq!(requirements.len(), 2);
    assert_eq!(
        requirements[0].summary,
        "Consumers MUST validate the schema."
    );
    assert_eq!(
        requirements[1].summary,
        "They SHOULD NOT cache invalid values."
    );
    assert_eq!(requirements[0].keyword.as_str(), "MUST");
    assert_eq!(requirements[1].keyword.as_str(), "SHOULD NOT");
}

#[test]
fn extraction_does_not_split_normative_summaries_at_common_abbreviations() {
    let requirements = extract_page_requirements(
        "client/sampling",
        "# Sampling\n\nServers SHOULD avoid defaults (e.g. clients SHOULD NOT retry them).\n",
    )
    .expect("common abbreviations should stay inside their source sentence");

    assert_eq!(requirements.len(), 2);
    assert_eq!(requirements[0].summary, requirements[1].summary);
    assert_eq!(
        requirements[0].summary,
        "Servers SHOULD avoid defaults (e.g. clients SHOULD NOT retry them)."
    );
}

#[test]
fn extraction_preserves_balanced_trailing_inline_code() {
    let requirements = extract_page_requirements(
        "basic/transports",
        "# Transports\n\nServers MUST return `MCP-Session-Id`\n",
    )
    .expect("trailing inline code should be preserved");

    assert_eq!(requirements.len(), 1);
    assert_eq!(
        requirements[0].summary,
        "Servers MUST return `MCP-Session-Id`"
    );
    assert_eq!(requirements[0].summary.matches('`').count(), 2);

    let overlay = parse_coverage_overlay(
        &format!("spec_version: {SPEC_VERSION}\nrequirements: []\n"),
        &requirements,
    )
    .expect("empty evidence overlay should parse");
    let report = render_coverage_report(&requirements, &overlay)
        .expect("inline code should render in the report");
    assert!(report.contains("Servers MUST return `MCP-Session-Id`"));
}

#[test]
fn checked_out_catalog_requires_every_pinned_page_and_uses_no_network() {
    let directory = tempfile::tempdir().expect("temporary directory should be created");
    let checkout = directory.path();
    let spec_root = checkout.join("docs/specification").join(SPEC_VERSION);

    for page in NORMATIVE_PAGES {
        let path = spec_root.join(page.source_path);
        fs::create_dir_all(path.parent().expect("catalog paths have parents"))
            .expect("fixture directory should be created");
        fs::write(
            path,
            format!(
                "# {}\n\nImplementations MUST satisfy {}.\n",
                page.title, page.path
            ),
        )
        .expect("fixture page should be written");
    }

    let requirements = extract_catalog_from_checkout(checkout)
        .expect("complete checked-out pinned tree should be parsed");
    assert_eq!(requirements.len(), NORMATIVE_PAGES.len());

    fs::remove_file(spec_root.join(NORMATIVE_PAGES[7].source_path))
        .expect("fixture page should be removed");
    let error = extract_catalog_from_checkout(checkout)
        .expect_err("a missing catalog page must fail completeness");
    assert!(error.to_string().contains(NORMATIVE_PAGES[7].source_path));
}

#[test]
#[ignore = "requires MCP_SPEC_CHECKOUT pointing at the pinned official repository checkout"]
fn pinned_official_checkout_extracts_every_catalog_page() {
    let checkout = std::env::var_os("MCP_SPEC_CHECKOUT")
        .expect("MCP_SPEC_CHECKOUT must point at an explicit pinned checkout");
    let head = Command::new("git")
        .args(["-C"])
        .arg(&checkout)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git should inspect the explicit checkout");
    assert!(head.status.success());
    assert_eq!(
        String::from_utf8(head.stdout)
            .expect("git commit should be UTF-8")
            .trim(),
        PINNED_SOURCE_COMMIT
    );

    let requirements = extract_catalog_from_checkout(Path::new(&checkout))
        .expect("the pinned official source should parse");
    for page in NORMATIVE_PAGES {
        assert!(
            requirements
                .iter()
                .any(|requirement| requirement.page_path == page.path),
            "missing extracted requirements for {}",
            page.path
        );
    }

    let overlay_source = fs::read_to_string("conformance/coverage-overrides.yml")
        .expect("repository coverage overlay should be readable");
    let overlay = parse_coverage_overlay(&overlay_source, &requirements)
        .expect("repository coverage overlay should be valid");
    let checked_in = fs::read_to_string("reports/mcp-spec-coverage.md")
        .expect("checked-in coverage report should be readable");
    let static_report = render_coverage_report(&requirements, &overlay)
        .expect("pinned coverage report should render");
    assert!(
        !static_report.contains("Unassessed"),
        "every pinned catalog row must have an applicability classification"
    );
    assert_eq!(
        without_runtime_results(&checked_in),
        without_runtime_results(&static_report),
        "only the two live result columns may differ from a static pinned render"
    );
}

fn without_runtime_results(report: &str) -> String {
    report
        .lines()
        .map(|line| {
            if !line.starts_with("| 2025-11-25 |") {
                return line.to_owned();
            }
            let mut columns: Vec<_> = line.split(" | ").collect();
            assert_eq!(columns.len(), 17, "coverage row shape changed: {line}");
            columns[13] = "<runtime-result>";
            columns[14] = "<runtime-result>";
            columns.join(" | ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn two_requirements() -> Vec<coverage::Requirement> {
    extract_page_requirements(
        "server/tools",
        "# Tools\n\n## Listing\nServers MUST return tools.\n\nClients MAY paginate.\n",
    )
    .expect("fixture requirements should parse")
}

#[test]
fn overlay_rejects_duplicate_unknown_unjustified_and_unbacked_claims() {
    let requirements = two_requirements();
    let id = &requirements[0].local_id;

    let duplicate =
        format!("spec_version: {SPEC_VERSION}\nrequirements:\n  - id: {id}\n  - id: {id}\n");
    let error =
        parse_coverage_overlay(&duplicate, &requirements).expect_err("duplicate IDs must fail");
    assert!(error.to_string().contains("duplicate coverage override"));

    let unknown = format!(
        "spec_version: {SPEC_VERSION}\nrequirements:\n  - id: {SPEC_VERSION}:server/tools#missing:99\n"
    );
    let error = parse_coverage_overlay(&unknown, &requirements).expect_err("unknown IDs must fail");
    assert!(error.to_string().contains("unknown local requirement ID"));

    let unbacked_official = format!(
        "spec_version: {SPEC_VERSION}\nrequirements:\n  - id: {id}\n    official_conformance:\n      covered: true\n"
    );
    let error = parse_coverage_overlay(&unbacked_official, &requirements)
        .expect_err("official coverage needs an exact scenario");
    assert!(error.to_string().contains("official scenario"));

    let unbacked_rust = format!(
        "spec_version: {SPEC_VERSION}\nrequirements:\n  - id: {id}\n    rust_gateway:\n      covered: true\n"
    );
    let error = parse_coverage_overlay(&unbacked_rust, &requirements)
        .expect_err("Rust coverage needs an exact test name");
    assert!(error.to_string().contains("Rust gateway test"));

    let unjustified = format!(
        "spec_version: {SPEC_VERSION}\nrequirements:\n  - id: {id}\n    gateway_applicability: not_applicable\n    controlplane_result: not_applicable\n"
    );
    let error = parse_coverage_overlay(&unjustified, &requirements)
        .expect_err("N/A needs an explicit justification");
    assert!(error.to_string().contains("N/A justification"));
}

#[test]
fn overlay_accepts_evidence_backed_coverage_and_explicit_optional_feature_results() {
    let requirements = two_requirements();
    let first = &requirements[0].local_id;
    let second = &requirements[1].local_id;
    let source = format!(
        r#"spec_version: {SPEC_VERSION}
requirements:
  - id: {first}
    official_conformance:
      covered: true
      scenario: server-tools-list
    rust_gateway:
      covered: true
      test: gateway_tools_list_preserves_schema
    gateway_applicability: applicable
    controlplane_result: pass
    dataplane_result: fail
    notes: Dataplane failure remains visible.
    issue: https://github.com/cloudflare/example/issues/42
  - id: {second}
    gateway_applicability: conditional
    capability_condition: tools pagination is advertised
    not_applicable_justification: neither test stack advertised pagination
    controlplane_result: not_applicable
    dataplane_result: not_applicable
"#
    );

    let overlay = parse_coverage_overlay(&source, &requirements)
        .expect("evidence-backed claims and justified N/A should parse");
    validate_coverage_overlay(&overlay, &requirements)
        .expect("the parsed overlay should remain valid");
    assert_eq!(overlay.requirements.len(), 2);
}

#[test]
fn report_is_deterministic_complete_and_explicit_about_missing_evidence() {
    let requirements = two_requirements();
    let first = &requirements[0].local_id;
    let source = format!(
        r#"spec_version: {SPEC_VERSION}
requirements:
  - id: {first}
    official_conformance:
      covered: true
      scenario: server-tools-list
    rust_gateway:
      covered: true
      test: gateway_tools_list_preserves_schema
    gateway_applicability: applicable
    controlplane_result: pass
    dataplane_result: fail
    notes: "keeps | table safe"
    issue: https://github.com/cloudflare/example/issues/42
"#
    );
    let overlay =
        parse_coverage_overlay(&source, &requirements).expect("fixture overlay should parse");

    let first_report =
        render_coverage_report(&requirements, &overlay).expect("valid inventory should render");
    let second_report =
        render_coverage_report(&requirements, &overlay).expect("rendering should be deterministic");

    assert_eq!(first_report, second_report);
    assert!(first_report.starts_with("# MCP 2025-11-25 Specification Coverage\n"));
    assert!(first_report.contains("not upstream MCP requirement IDs"));
    assert!(
        first_report.contains(
            "| Specification version | Local requirement ID | Page | Heading | Keyword |"
        )
    );
    assert!(first_report.contains("server-tools-list"));
    assert!(first_report.contains("gateway_tools_list_preserves_schema"));
    assert!(first_report.contains("keeps \\| table safe"));
    assert!(first_report.contains("No upstream scenario"));
    assert!(first_report.contains("Not exercised"));
    assert!(first_report.contains("Not run"));
    assert!(!first_report.contains("Unassessed"));
    assert!(first_report.contains("The gateway advertises the tools capability."));
    for requirement in &requirements {
        assert_eq!(first_report.matches(&requirement.local_id).count(), 1);
    }
}

#[test]
fn default_applicability_is_capability_aware_and_never_fabricates_results() {
    let mut requirements = extract_page_requirements(
        "client/roots",
        "# Roots\n\nClients MUST return only authorized roots.\n",
    )
    .expect("client requirement should parse");
    requirements.extend(
        extract_page_requirements(
            "basic/utilities/ping",
            "# Ping\n\nThe receiver MUST respond to ping.\n",
        )
        .expect("ping requirement should parse"),
    );
    let overlay = parse_coverage_overlay(
        &format!("spec_version: {SPEC_VERSION}\nrequirements: []\n"),
        &requirements,
    )
    .expect("empty evidence overlay should parse");

    let report = render_coverage_report(&requirements, &overlay)
        .expect("default classifications should render");

    assert!(!report.contains("Unassessed"));
    assert!(report.contains("| Conditional | The connected MCP client advertises the roots capability, or the gateway performs the downstream MCP client role named by the requirement. | — | Not run | Not run |"));
    assert!(report.contains("| Applicable | — | — | Not run | Not run |"));
    assert!(!report.contains("| Pass |"));
}

#[test]
fn report_writer_creates_the_requested_parent_and_exact_markdown() {
    let requirements = two_requirements();
    let overlay = parse_coverage_overlay(
        &format!("spec_version: {SPEC_VERSION}\nrequirements: []\n"),
        &requirements,
    )
    .expect("empty overlay should parse");
    let directory = tempfile::tempdir().expect("temporary directory should be created");
    let path = directory.path().join("reports/mcp-spec-coverage.md");

    write_coverage_report(&path, &requirements, &overlay)
        .expect("coverage report should be written");

    assert_eq!(
        fs::read_to_string(&path).expect("coverage report should be readable"),
        render_coverage_report(&requirements, &overlay).expect("report should render")
    );
    assert!(Path::new(&path).ends_with("reports/mcp-spec-coverage.md"));
}
