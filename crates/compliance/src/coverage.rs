//! Pinned MCP specification requirement extraction and coverage reporting.
//!
//! The inventory is intentionally sourced from an explicit checkout of the
//! official repository. A runner can prepare that checkout with:
//!
//! ```text
//! git clone --filter=blob:none https://github.com/modelcontextprotocol/modelcontextprotocol.git <checkout>
//! git -C <checkout> checkout --detach 38c84e9f93ad191d9eb26d92b945d17bd0efcaf3
//! git -C <checkout> rev-parse --verify HEAD
//! ```
//!
//! The final command must equal [`PINNED_SOURCE_COMMIT`] before the checkout is
//! passed to [`extract_catalog_from_checkout`]. This module never fetches live
//! documentation and never treats its generated local IDs as upstream IDs.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::conformance::{
    ConformanceFixtureMetadata, ConformanceResults, ScenarioOutcome, is_trusted_official_fixture,
};
use crate::gateway_compliance::{GatewayCaseStatus, GatewayComplianceReport};

/// MCP specification release inventoried by this module.
pub const SPEC_VERSION: &str = "2025-11-25";
/// Official source repository for the pinned specification.
pub const PINNED_SOURCE_REPOSITORY: &str =
    "https://github.com/modelcontextprotocol/modelcontextprotocol.git";
/// Immutable commit behind the official `2025-11-25` release tag.
pub const PINNED_SOURCE_COMMIT: &str = "38c84e9f93ad191d9eb26d92b945d17bd0efcaf3";

/// One normative specification page included in the requirement inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecPage {
    /// Stable URL-style path used in local requirement IDs.
    pub path: &'static str,
    /// Path relative to `docs/specification/2025-11-25` in the pinned checkout.
    pub source_path: &'static str,
    /// Human-readable page title.
    pub title: &'static str,
}

/// Complete normative page catalog for the gateway compliance inventory.
pub const NORMATIVE_PAGES: &[SpecPage] = &[
    SpecPage {
        path: "basic",
        source_path: "basic/index.mdx",
        title: "Overview",
    },
    SpecPage {
        path: "basic/lifecycle",
        source_path: "basic/lifecycle.mdx",
        title: "Lifecycle",
    },
    SpecPage {
        path: "basic/transports",
        source_path: "basic/transports.mdx",
        title: "Transports",
    },
    SpecPage {
        path: "basic/authorization",
        source_path: "basic/authorization.mdx",
        title: "Authorization",
    },
    SpecPage {
        path: "basic/security_best_practices",
        source_path: "basic/security_best_practices.mdx",
        title: "Security Best Practices",
    },
    SpecPage {
        path: "basic/utilities/cancellation",
        source_path: "basic/utilities/cancellation.mdx",
        title: "Cancellation",
    },
    SpecPage {
        path: "basic/utilities/ping",
        source_path: "basic/utilities/ping.mdx",
        title: "Ping",
    },
    SpecPage {
        path: "basic/utilities/progress",
        source_path: "basic/utilities/progress.mdx",
        title: "Progress",
    },
    SpecPage {
        path: "basic/utilities/tasks",
        source_path: "basic/utilities/tasks.mdx",
        title: "Tasks",
    },
    SpecPage {
        path: "client/roots",
        source_path: "client/roots.mdx",
        title: "Roots",
    },
    SpecPage {
        path: "client/sampling",
        source_path: "client/sampling.mdx",
        title: "Sampling",
    },
    SpecPage {
        path: "client/elicitation",
        source_path: "client/elicitation.mdx",
        title: "Elicitation",
    },
    SpecPage {
        path: "server/prompts",
        source_path: "server/prompts.mdx",
        title: "Prompts",
    },
    SpecPage {
        path: "server/resources",
        source_path: "server/resources.mdx",
        title: "Resources",
    },
    SpecPage {
        path: "server/tools",
        source_path: "server/tools.mdx",
        title: "Tools",
    },
    SpecPage {
        path: "server/utilities/completion",
        source_path: "server/utilities/completion.mdx",
        title: "Completion",
    },
    SpecPage {
        path: "server/utilities/logging",
        source_path: "server/utilities/logging.mdx",
        title: "Logging",
    },
    SpecPage {
        path: "server/utilities/pagination",
        source_path: "server/utilities/pagination.mdx",
        title: "Pagination",
    },
    SpecPage {
        path: "schema",
        source_path: "schema.mdx",
        title: "Schema Reference",
    },
];

/// Normative BCP 14 keyword attached to one extracted requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RequirementKeyword {
    /// Absolute positive requirement.
    Must,
    /// Absolute prohibition.
    MustNot,
    /// Recommended behavior.
    Should,
    /// Discouraged behavior.
    ShouldNot,
    /// Optional behavior.
    May,
}

impl RequirementKeyword {
    /// Canonical uppercase BCP 14 spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Must => "MUST",
            Self::MustNot => "MUST NOT",
            Self::Should => "SHOULD",
            Self::ShouldNot => "SHOULD NOT",
            Self::May => "MAY",
        }
    }
}

impl fmt::Display for RequirementKeyword {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One locally identified normative statement extracted from pinned MDX.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    /// Stable repository-local ID. This is not an upstream MCP ID.
    pub local_id: String,
    /// URL-style page path from [`NORMATIVE_PAGES`].
    pub page_path: String,
    /// Catalog title of the source page.
    pub page_title: String,
    /// Nearest Markdown heading in the source page.
    pub heading: String,
    /// One-based occurrence under the normalized heading on this page.
    pub ordinal: usize,
    /// BCP 14 keyword found in the statement.
    pub keyword: RequirementKeyword,
    /// Whitespace-normalized source statement containing the keyword.
    pub summary: String,
}

/// Whether a test framework claims to exercise a requirement.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfficialCoverageClaim {
    /// Whether the official conformance suite covers this requirement.
    #[serde(default)]
    pub covered: bool,
    /// Exact official scenario backing the coverage claim.
    #[serde(default)]
    pub scenario: Option<String>,
}

/// Rust gateway-test evidence for one requirement.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RustGatewayCoverageClaim {
    /// Whether a Rust gateway test covers this requirement.
    #[serde(default)]
    pub covered: bool,
    /// Exact Rust test name backing the coverage claim.
    #[serde(default, rename = "test")]
    pub test_name: Option<String>,
}

/// Applicability of a normative statement to the integration gateway.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GatewayApplicability {
    /// Applicability has not been classified yet.
    #[default]
    Unassessed,
    /// The gateway is unconditionally responsible for the behavior.
    Applicable,
    /// The requirement applies only when a named capability is advertised.
    Conditional,
    /// The requirement cannot apply to this gateway.
    NotApplicable,
}

impl GatewayApplicability {
    fn report_label(self) -> &'static str {
        match self {
            Self::Unassessed => "Unassessed",
            Self::Applicable => "Applicable",
            Self::Conditional => "Conditional",
            Self::NotApplicable => "N/A",
        }
    }
}

/// Result of exercising one requirement against one stack mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageResult {
    /// The requirement has not been exercised against this mode.
    #[default]
    NotRun,
    /// The stack mode satisfied the requirement.
    Pass,
    /// The stack mode violated the requirement.
    Fail,
    /// The stack did not advertise the optional capability, or the behavior
    /// otherwise cannot apply. A justification is mandatory.
    NotApplicable,
}

impl CoverageResult {
    fn report_label(self) -> &'static str {
        match self {
            Self::NotRun => "Not run",
            Self::Pass => "Pass",
            Self::Fail => "Fail",
            Self::NotApplicable => "N/A",
        }
    }
}

/// Repository-owned annotations for one extracted local requirement.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequirementCoverageOverride {
    /// Exact generated local requirement ID.
    pub id: String,
    /// Official conformance evidence.
    #[serde(default)]
    pub official_conformance: OfficialCoverageClaim,
    /// Rust gateway-test evidence.
    #[serde(default)]
    pub rust_gateway: RustGatewayCoverageClaim,
    /// Gateway applicability classification.
    #[serde(default)]
    pub gateway_applicability: GatewayApplicability,
    /// Capability advertisement that makes a conditional requirement apply.
    #[serde(default)]
    pub capability_condition: Option<String>,
    /// Required explanation for every N/A classification or result.
    #[serde(default)]
    pub not_applicable_justification: Option<String>,
    /// Latest control-plane result.
    #[serde(default)]
    pub controlplane_result: CoverageResult,
    /// Latest dataplane result.
    #[serde(default)]
    pub dataplane_result: CoverageResult,
    /// Additional evidence or limitations.
    #[serde(default)]
    pub notes: Option<String>,
    /// Tracking issue for a gap or incomplete coverage.
    #[serde(default)]
    pub issue: Option<String>,
}

/// Versioned overlay applied to the generated requirement inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageOverlay {
    /// Specification release whose generated IDs this file annotates.
    pub spec_version: String,
    /// Sparse evidence annotations. Missing entries remain explicitly untested.
    #[serde(default)]
    pub requirements: Vec<RequirementCoverageOverride>,
}

/// Per-mode result evidence used to reduce test outcomes into coverage rows.
#[derive(Debug, Default)]
pub struct ModeCoverageEvidence {
    available: bool,
    official: BTreeMap<String, CoverageResult>,
    gateway: BTreeMap<String, CoverageResult>,
}

impl ModeCoverageEvidence {
    /// Reduces official and gateway artifacts into named coverage evidence.
    #[must_use]
    pub fn from_artifacts(
        official: Option<&ConformanceResults>,
        gateway: Option<&GatewayComplianceReport>,
        official_fixture: Option<&ConformanceFixtureMetadata>,
    ) -> Self {
        let trusted_official_fixture = is_trusted_official_fixture(official_fixture);
        let official_results = official
            .into_iter()
            .flat_map(|results| &results.scenarios)
            .map(|(scenario, result)| {
                let result = match result.outcome_with_trusted_fixture(trusted_official_fixture) {
                    ScenarioOutcome::Compliant => CoverageResult::Pass,
                    ScenarioOutcome::NonCompliant => CoverageResult::Fail,
                    ScenarioOutcome::NotApplicable => CoverageResult::NotApplicable,
                    ScenarioOutcome::FixtureFailure
                    | ScenarioOutcome::Ambiguous
                    | ScenarioOutcome::Missing => CoverageResult::NotRun,
                };
                (scenario.clone(), result)
            })
            .collect();
        let gateway_results = gateway
            .into_iter()
            .flat_map(|report| &report.cases)
            .map(|case| {
                let result = match case.status {
                    GatewayCaseStatus::Passed => CoverageResult::Pass,
                    GatewayCaseStatus::Failed => CoverageResult::Fail,
                    GatewayCaseStatus::NotApplicable => CoverageResult::NotApplicable,
                    GatewayCaseStatus::FixtureFailure => CoverageResult::NotRun,
                };
                (case.name.clone(), result)
            })
            .collect();
        Self {
            available: official.is_some() || gateway.is_some(),
            official: official_results,
            gateway: gateway_results,
        }
    }

    /// Builds explicit evidence, primarily for deterministic reduction tests.
    #[must_use]
    pub fn from_results(
        available: bool,
        official: BTreeMap<String, CoverageResult>,
        gateway: BTreeMap<String, CoverageResult>,
    ) -> Self {
        Self {
            available,
            official,
            gateway,
        }
    }

    /// Returns reduced official evidence for one scenario.
    #[must_use]
    pub fn official_result(&self, scenario: &str) -> Option<CoverageResult> {
        self.official.get(scenario).copied()
    }
}

/// Applies available mode evidence to every sparse coverage override.
pub fn enrich_overlay_results(
    overlay: &mut CoverageOverlay,
    controlplane: &ModeCoverageEvidence,
    dataplane: &ModeCoverageEvidence,
) {
    for requirement in &mut overlay.requirements {
        if controlplane.available {
            requirement.controlplane_result = derive_coverage_result(requirement, controlplane);
        }
        if dataplane.available {
            requirement.dataplane_result = derive_coverage_result(requirement, dataplane);
        }
    }
}

fn derive_coverage_result(
    requirement: &RequirementCoverageOverride,
    evidence: &ModeCoverageEvidence,
) -> CoverageResult {
    let mut results = Vec::with_capacity(3);
    if requirement.gateway_applicability == GatewayApplicability::NotApplicable {
        results.push(CoverageResult::NotApplicable);
    }
    if requirement.official_conformance.covered
        && let Some(scenario) = requirement.official_conformance.scenario.as_deref()
        && let Some(result) = evidence.official.get(scenario)
    {
        results.push(*result);
    }
    if requirement.rust_gateway.covered
        && let Some(test_name) = requirement.rust_gateway.test_name.as_deref()
        && let Some(result) = evidence.gateway.get(test_name).or_else(|| {
            test_name
                .strip_prefix("gateway-compliance/")
                .and_then(|case_name| evidence.gateway.get(case_name))
        })
    {
        results.push(*result);
    }
    if results.contains(&CoverageResult::Fail) {
        CoverageResult::Fail
    } else if results.contains(&CoverageResult::Pass) {
        CoverageResult::Pass
    } else if results.contains(&CoverageResult::NotApplicable) {
        CoverageResult::NotApplicable
    } else {
        CoverageResult::NotRun
    }
}

/// Extracts all BCP 14 requirement occurrences from one cataloged MDX page.
///
/// # Errors
///
/// Returns an error when `page_path` is not in [`NORMATIVE_PAGES`].
pub fn extract_page_requirements(page_path: &str, source: &str) -> Result<Vec<Requirement>> {
    let page = page_for_path(page_path)
        .with_context(|| format!("unknown normative page {page_path:?}"))?;
    Ok(extract_known_page_requirements(page, source))
}

/// Reads every cataloged page from an explicit pinned repository checkout.
///
/// This function performs no Git or network operation. The caller is
/// responsible for verifying the checkout commit against
/// [`PINNED_SOURCE_COMMIT`] before calling it.
///
/// # Errors
///
/// Returns an error when any cataloged page is missing, unreadable, or yields
/// no normative requirements, or if generated IDs collide.
pub fn extract_catalog_from_checkout(checkout: &Path) -> Result<Vec<Requirement>> {
    let spec_root = checkout
        .join("docs")
        .join("specification")
        .join(SPEC_VERSION);
    let mut requirements = Vec::new();
    let mut ids = BTreeSet::new();

    for page in NORMATIVE_PAGES {
        let path = spec_root.join(page.source_path);
        let source = fs::read_to_string(&path)
            .with_context(|| format!("failed to read pinned specification page {path:?}"))?;
        let page_requirements = extract_known_page_requirements(page, &source);
        if page_requirements.is_empty() {
            bail!(
                "pinned specification page {:?} contained no BCP 14 requirements",
                page.source_path
            );
        }
        for requirement in page_requirements {
            if !ids.insert(requirement.local_id.clone()) {
                bail!(
                    "generated duplicate local requirement ID {:?}",
                    requirement.local_id
                );
            }
            requirements.push(requirement);
        }
    }

    Ok(requirements)
}

/// Parses and validates a sparse YAML coverage overlay.
///
/// # Errors
///
/// Returns an error for malformed YAML or invalid evidence annotations.
pub fn parse_coverage_overlay(
    source: &str,
    requirements: &[Requirement],
) -> Result<CoverageOverlay> {
    let overlay: CoverageOverlay =
        serde_yaml::from_str(source).context("failed to parse MCP coverage overlay YAML")?;
    validate_coverage_overlay(&overlay, requirements)?;
    Ok(overlay)
}

/// Validates coverage annotations against an extracted pinned inventory.
///
/// # Errors
///
/// Rejects version mismatches, duplicate or unknown IDs, evidence claims with
/// no exact scenario/test, and N/A states without a justification.
pub fn validate_coverage_overlay(
    overlay: &CoverageOverlay,
    requirements: &[Requirement],
) -> Result<()> {
    if overlay.spec_version != SPEC_VERSION {
        bail!(
            "coverage overlay spec_version {:?} does not match pinned version {:?}",
            overlay.spec_version,
            SPEC_VERSION
        );
    }

    let known_ids: BTreeSet<_> = requirements
        .iter()
        .map(|requirement| requirement.local_id.as_str())
        .collect();
    if known_ids.len() != requirements.len() {
        bail!("requirement inventory contains duplicate local IDs");
    }

    let mut annotated_ids = BTreeSet::new();
    for annotation in &overlay.requirements {
        if !known_ids.contains(annotation.id.as_str()) {
            bail!(
                "coverage override references unknown local requirement ID {:?}",
                annotation.id
            );
        }
        if !annotated_ids.insert(annotation.id.as_str()) {
            bail!("duplicate coverage override for {:?}", annotation.id);
        }

        validate_claim(
            annotation.official_conformance.covered,
            annotation.official_conformance.scenario.as_deref(),
            "official scenario",
            &annotation.id,
        )?;
        validate_claim(
            annotation.rust_gateway.covered,
            annotation.rust_gateway.test_name.as_deref(),
            "Rust gateway test",
            &annotation.id,
        )?;

        if annotation.gateway_applicability == GatewayApplicability::Conditional
            && is_blank(annotation.capability_condition.as_deref())
        {
            bail!(
                "coverage override {:?} is conditional but has no capability condition",
                annotation.id
            );
        }

        let has_not_applicable_state = annotation.gateway_applicability
            == GatewayApplicability::NotApplicable
            || annotation.controlplane_result == CoverageResult::NotApplicable
            || annotation.dataplane_result == CoverageResult::NotApplicable;
        if has_not_applicable_state && is_blank(annotation.not_applicable_justification.as_deref())
        {
            bail!(
                "coverage override {:?} uses N/A without an N/A justification",
                annotation.id
            );
        }

        if annotation.gateway_applicability == GatewayApplicability::NotApplicable
            && (matches!(
                annotation.controlplane_result,
                CoverageResult::Pass | CoverageResult::Fail
            ) || matches!(
                annotation.dataplane_result,
                CoverageResult::Pass | CoverageResult::Fail
            ))
        {
            bail!(
                "coverage override {:?} cannot record pass/fail for a gateway-inapplicable requirement",
                annotation.id
            );
        }
    }

    Ok(())
}

/// Renders a deterministic page-by-page coverage matrix.
///
/// # Errors
///
/// Returns an error when the overlay is invalid for `requirements`.
pub fn render_coverage_report(
    requirements: &[Requirement],
    overlay: &CoverageOverlay,
) -> Result<String> {
    validate_coverage_overlay(overlay, requirements)?;
    let annotations: BTreeMap<_, _> = overlay
        .requirements
        .iter()
        .map(|annotation| (annotation.id.as_str(), annotation))
        .collect();

    let mut report = String::new();
    report.push_str("# MCP 2025-11-25 Specification Coverage\n\n");
    report.push_str("Pinned source: [`modelcontextprotocol/modelcontextprotocol` commit `");
    report.push_str(PINNED_SOURCE_COMMIT);
    report.push_str("`](https://github.com/modelcontextprotocol/modelcontextprotocol/commit/");
    report.push_str(PINNED_SOURCE_COMMIT);
    report.push_str(") (`2025-11-25`).\n\n");
    report.push_str(
        "Requirement IDs are stable identifiers generated by this repository; they are not upstream MCP requirement IDs. Missing evidence deliberately renders as **No upstream scenario**, **Not exercised**, and **Not run** rather than implying coverage. Requirements without a reviewed override receive a conservative role/capability applicability classification; that classification never implies a pass. Optional capability requirements must be marked N/A, with a justification, unless that capability was advertised by the tested stack.\n\n",
    );
    report.push_str("| Specification version | Local requirement ID | Page | Heading | Keyword | Requirement summary | Official conformance coverage | Official scenario | Rust gateway coverage | Rust gateway test | Gateway applicability | Capability condition | N/A justification | Control-plane result | Dataplane result | Notes | Issue |\n");
    report.push_str("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");

    for requirement in requirements {
        let annotation = annotations.get(requirement.local_id.as_str()).copied();
        let default_classification = default_gateway_classification(requirement);
        let official_coverage = annotation
            .filter(|entry| entry.official_conformance.covered)
            .map_or("No upstream scenario", |_| "Covered");
        let official_scenario = annotation
            .and_then(|entry| entry.official_conformance.scenario.as_deref())
            .unwrap_or("No upstream scenario");
        let rust_coverage = annotation
            .filter(|entry| entry.rust_gateway.covered)
            .map_or("Not exercised", |_| "Covered");
        let rust_test = annotation
            .and_then(|entry| entry.rust_gateway.test_name.as_deref())
            .unwrap_or("Not exercised");
        let applicability = annotation.map_or(
            default_classification.applicability.report_label(),
            |entry| entry.gateway_applicability.report_label(),
        );
        let capability_condition = annotation
            .and_then(|entry| entry.capability_condition.as_deref())
            .or_else(|| {
                annotation
                    .is_none()
                    .then_some(default_classification.capability_condition)
                    .flatten()
            })
            .unwrap_or("—");
        let not_applicable_justification = annotation
            .and_then(|entry| entry.not_applicable_justification.as_deref())
            .or_else(|| {
                annotation
                    .is_none()
                    .then_some(default_classification.not_applicable_justification)
                    .flatten()
            })
            .unwrap_or("—");
        let controlplane_result =
            annotation.map_or("Not run", |entry| entry.controlplane_result.report_label());
        let dataplane_result =
            annotation.map_or("Not run", |entry| entry.dataplane_result.report_label());
        let notes = annotation
            .and_then(|entry| entry.notes.as_deref())
            .unwrap_or("—");
        let issue = annotation
            .and_then(|entry| entry.issue.as_deref())
            .unwrap_or("—");

        let cells = [
            SPEC_VERSION,
            requirement.local_id.as_str(),
            requirement.page_path.as_str(),
            requirement.heading.as_str(),
            requirement.keyword.as_str(),
            requirement.summary.as_str(),
            official_coverage,
            official_scenario,
            rust_coverage,
            rust_test,
            applicability,
            capability_condition,
            not_applicable_justification,
            controlplane_result,
            dataplane_result,
            notes,
            issue,
        ];
        report.push('|');
        for cell in cells {
            report.push(' ');
            report.push_str(&escape_markdown_cell(cell));
            report.push_str(" |");
        }
        report.push('\n');
    }

    Ok(report)
}

/// Writes [`render_coverage_report`] output, creating its parent directory.
///
/// # Errors
///
/// Returns an error for invalid coverage data or filesystem failures.
pub fn write_coverage_report(
    path: &Path,
    requirements: &[Requirement],
    overlay: &CoverageOverlay,
) -> Result<()> {
    let report = render_coverage_report(requirements, overlay)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create coverage report directory {parent:?}"))?;
    }
    fs::write(path, report).with_context(|| format!("failed to write coverage report {path:?}"))
}

fn page_for_path(page_path: &str) -> Option<&'static SpecPage> {
    NORMATIVE_PAGES.iter().find(|page| page.path == page_path)
}

#[derive(Debug, Clone, Copy)]
struct DefaultGatewayClassification {
    applicability: GatewayApplicability,
    capability_condition: Option<&'static str>,
    not_applicable_justification: Option<&'static str>,
}

fn default_gateway_classification(requirement: &Requirement) -> DefaultGatewayClassification {
    let applicable = DefaultGatewayClassification {
        applicability: GatewayApplicability::Applicable,
        capability_condition: None,
        not_applicable_justification: None,
    };
    let conditional = |condition| DefaultGatewayClassification {
        applicability: GatewayApplicability::Conditional,
        capability_condition: Some(condition),
        not_applicable_justification: None,
    };
    let heading = requirement.heading.trim_matches('`');

    match requirement.page_path.as_str() {
        "client/roots" => conditional(
            "The connected MCP client advertises the roots capability, or the gateway performs the downstream MCP client role named by the requirement.",
        ),
        "client/sampling" => conditional(
            "The connected MCP client advertises the sampling capability, or the gateway performs the downstream MCP client role named by the requirement.",
        ),
        "client/elicitation" => conditional(
            "The connected MCP client advertises the elicitation capability, or the gateway performs the downstream MCP client role named by the requirement.",
        ),
        "basic/utilities/ping" => applicable,
        "basic" if matches!(heading, "Auth" | "icons") => conditional(
            "The gateway uses the optional authorization or icon feature named by the requirement.",
        ),
        "basic" => applicable,
        "basic/lifecycle" => applicable,
        "basic/transports"
            if matches!(
                heading,
                "Custom Transports"
                    | "Listening for Messages from the Server"
                    | "Multiple Connections"
                    | "Resumability and Redelivery"
                    | "Session Management"
            ) =>
        {
            conditional(
                "The gateway enables the optional Streamable HTTP behavior named by the requirement.",
            )
        }
        "basic/transports" => applicable,
        "basic/authorization" => conditional(
            "HTTP authorization is enabled and the gateway performs the protected-resource or OAuth role named by the requirement.",
        ),
        "basic/security_best_practices" => conditional(
            "The deployment has the attack preconditions named by the requirement, such as proxy authorization, third-party OAuth, or session use.",
        ),
        "basic/utilities/cancellation" => conditional(
            "The gateway handles a cancellable in-flight request in the protocol role named by the requirement.",
        ),
        "basic/utilities/progress" => conditional(
            "The gateway handles an operation that advertises or emits progress notifications.",
        ),
        "basic/utilities/tasks" => conditional(
            "The gateway advertises the experimental tasks capability for the request type named by the requirement.",
        ),
        "server/prompts" => conditional("The gateway advertises the prompts capability."),
        "server/resources" => conditional("The gateway advertises the resources capability."),
        "server/tools" => conditional("The gateway advertises the tools capability."),
        "server/utilities/completion" => {
            conditional("The gateway advertises the completion capability.")
        }
        "server/utilities/logging" => conditional("The gateway advertises the logging capability."),
        "server/utilities/pagination" => conditional(
            "The gateway returns a paginated list result for an advertised server capability.",
        ),
        "schema" => conditional(
            "The gateway emits or accepts the named schema type as part of an advertised MCP capability.",
        ),
        _ => conditional(
            "The gateway performs the protocol role or advertises the capability named by the requirement.",
        ),
    }
}

fn extract_known_page_requirements(page: &SpecPage, source: &str) -> Vec<Requirement> {
    let mut requirements = Vec::new();
    let mut heading = page.title.to_owned();
    let mut heading_slug = slugify(&heading);
    let mut ordinals = BTreeMap::<String, usize>::new();
    let mut block = String::new();
    let mut in_frontmatter = false;
    let mut frontmatter_checked = false;
    let mut fence: Option<char> = None;

    for raw_line in source.lines() {
        let line = raw_line.trim_end_matches('\r');
        let trimmed = line.trim();

        if !frontmatter_checked {
            if trimmed.is_empty() {
                continue;
            }
            frontmatter_checked = true;
            if trimmed == "---" {
                in_frontmatter = true;
                continue;
            }
        } else if in_frontmatter {
            if trimmed == "---" {
                in_frontmatter = false;
            }
            continue;
        }

        if let Some(marker) = fence {
            if is_fence(trimmed, marker) {
                fence = None;
            }
            continue;
        }
        if let Some(marker) = opening_fence(trimmed) {
            flush_requirement_block(
                page,
                &heading,
                &heading_slug,
                &mut block,
                &mut ordinals,
                &mut requirements,
            );
            fence = Some(marker);
            continue;
        }

        if let Some(new_heading) = markdown_heading(trimmed) {
            flush_requirement_block(
                page,
                &heading,
                &heading_slug,
                &mut block,
                &mut ordinals,
                &mut requirements,
            );
            heading = clean_visible_text(new_heading);
            heading_slug = slugify(&heading);
            continue;
        }

        if trimmed.is_empty() {
            flush_requirement_block(
                page,
                &heading,
                &heading_slug,
                &mut block,
                &mut ordinals,
                &mut requirements,
            );
            continue;
        }

        if is_mdx_control_line(trimmed) {
            continue;
        }

        let starts_new_item = markdown_list_item(trimmed).is_some() || trimmed.starts_with('|');
        if starts_new_item && !block.is_empty() {
            flush_requirement_block(
                page,
                &heading,
                &heading_slug,
                &mut block,
                &mut ordinals,
                &mut requirements,
            );
        }

        let visible = markdown_list_item(trimmed).unwrap_or(trimmed);
        let visible = visible.strip_prefix('>').map_or(visible, str::trim_start);
        if !visible.is_empty() {
            if !block.is_empty() {
                block.push(' ');
            }
            block.push_str(visible);
        }
    }

    flush_requirement_block(
        page,
        &heading,
        &heading_slug,
        &mut block,
        &mut ordinals,
        &mut requirements,
    );
    requirements
}

fn flush_requirement_block(
    page: &SpecPage,
    heading: &str,
    heading_slug: &str,
    block: &mut String,
    ordinals: &mut BTreeMap<String, usize>,
    requirements: &mut Vec<Requirement>,
) {
    if block.is_empty() {
        return;
    }
    let visible_text = clean_visible_text(block);
    block.clear();

    for statement in requirement_statements(&visible_text) {
        let summary = normalize_whitespace(statement);
        if summary.is_empty() || is_bcp14_boilerplate(&summary) {
            continue;
        }

        let detection_text = remove_inline_code_and_tags(&summary);
        for keyword in find_bcp14_keywords(&detection_text) {
            let ordinal = ordinals.entry(heading_slug.to_owned()).or_default();
            *ordinal += 1;
            requirements.push(Requirement {
                local_id: format!("{SPEC_VERSION}:{}#{}:{}", page.path, heading_slug, *ordinal),
                page_path: page.path.to_owned(),
                page_title: page.title.to_owned(),
                heading: heading.to_owned(),
                ordinal: *ordinal,
                keyword,
                summary: summary.clone(),
            });
        }
    }
}

fn validate_claim(
    covered: bool,
    evidence: Option<&str>,
    evidence_name: &str,
    id: &str,
) -> Result<()> {
    if covered && is_blank(evidence) {
        bail!("coverage override {id:?} claims coverage without an exact {evidence_name}");
    }
    if !covered && !is_blank(evidence) {
        bail!("coverage override {id:?} names an {evidence_name} without setting covered: true");
    }
    if let Some(value) = evidence.filter(|value| !value.trim().is_empty())
        && value
            .chars()
            .any(|character| matches!(character, '*' | '?' | '[' | ']'))
    {
        bail!("coverage override {id:?} uses a non-exact {evidence_name} {value:?}");
    }
    Ok(())
}

fn is_blank(value: Option<&str>) -> bool {
    value.is_none_or(|value| value.trim().is_empty())
}

fn opening_fence(line: &str) -> Option<char> {
    if line.starts_with("```") {
        Some('`')
    } else if line.starts_with("~~~") {
        Some('~')
    } else {
        None
    }
}

fn is_fence(line: &str, marker: char) -> bool {
    match marker {
        '`' => line.starts_with("```"),
        '~' => line.starts_with("~~~"),
        _ => false,
    }
}

fn markdown_heading(line: &str) -> Option<&str> {
    let hash_count = line.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&hash_count) || line.as_bytes().get(hash_count) != Some(&b' ') {
        return None;
    }
    Some(line[hash_count + 1..].trim().trim_end_matches('#').trim())
}

fn markdown_list_item(line: &str) -> Option<&str> {
    for prefix in ["- ", "* ", "+ "] {
        if let Some(item) = line.strip_prefix(prefix) {
            return Some(item.trim_start());
        }
    }

    let digit_count = line.bytes().take_while(u8::is_ascii_digit).count();
    if digit_count > 0
        && line.as_bytes().get(digit_count) == Some(&b'.')
        && line.as_bytes().get(digit_count + 1) == Some(&b' ')
    {
        return Some(line[digit_count + 2..].trim_start());
    }
    None
}

fn is_mdx_control_line(line: &str) -> bool {
    line.starts_with("import ")
        || line.starts_with("export ")
        || line.starts_with("{/*")
        || line.starts_with("<!--")
        || (line.starts_with('<')
            && line.ends_with('>')
            && clean_visible_text(line).trim().is_empty())
}

fn clean_visible_text(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut remainder = value;
    while let Some(tag_start) = remainder.find('<') {
        output.push_str(&remainder[..tag_start]);
        let after_start = &remainder[tag_start + 1..];
        let Some(tag_end) = after_start.find('>') else {
            output.push_str(&remainder[tag_start..]);
            remainder = "";
            break;
        };
        let tag = after_start[..tag_end]
            .trim()
            .trim_start_matches('/')
            .split_ascii_whitespace()
            .next()
            .unwrap_or_default()
            .trim_end_matches('/');
        if matches!(tag, "p" | "li" | "br" | "section" | "aside") {
            output.push('\n');
        } else {
            output.push(' ');
        }
        remainder = &after_start[tag_end + 1..];
    }
    output.push_str(remainder);
    output.trim().to_owned()
}

fn requirement_statements(value: &str) -> Vec<&str> {
    let mut statements = Vec::new();
    let mut start = 0;
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let is_line_boundary = matches!(bytes[index], b'\n' | b'\r');
        let is_sentence_boundary = matches!(bytes[index], b'.' | b'!' | b'?')
            && bytes
                .get(index + 1)
                .is_none_or(|next| next.is_ascii_whitespace())
            && !is_common_abbreviation(value, index);
        if is_line_boundary || is_sentence_boundary {
            let end = if is_sentence_boundary {
                index + 1
            } else {
                index
            };
            let statement = value[start..end].trim();
            if !statement.is_empty() {
                statements.push(statement);
            }
            start = index + 1;
        }
        index += 1;
    }
    let statement = value[start..].trim();
    if !statement.is_empty() {
        statements.push(statement);
    }
    statements
}

fn is_common_abbreviation(value: &str, punctuation_index: usize) -> bool {
    if value.as_bytes()[punctuation_index] != b'.' {
        return false;
    }
    let prefix = value[..=punctuation_index].to_ascii_lowercase();
    [
        "e.g.", "i.e.", "etc.", "vs.", "mr.", "mrs.", "ms.", "dr.", "prof.", "fig.", "no.", "u.s.",
    ]
    .iter()
    .any(|abbreviation| prefix.ends_with(abbreviation))
}

fn remove_inline_code_and_tags(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut in_code = false;
    let mut in_tag = false;
    for character in value.chars() {
        match character {
            '`' if !in_tag => {
                in_code = !in_code;
                output.push(' ');
            }
            '<' if !in_code => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_code && !in_tag => output.push(character),
            _ => {}
        }
    }
    output
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_bcp14_boilerplate(value: &str) -> bool {
    let lowercase = value.to_ascii_lowercase();
    lowercase.contains("key words")
        && (lowercase.contains("bcp 14") || lowercase.contains("rfc 2119"))
}

fn find_bcp14_keywords(value: &str) -> Vec<RequirementKeyword> {
    const PATTERNS: &[(&str, RequirementKeyword)] = &[
        ("SHOULD NOT", RequirementKeyword::ShouldNot),
        ("MUST NOT", RequirementKeyword::MustNot),
        ("SHOULD", RequirementKeyword::Should),
        ("MUST", RequirementKeyword::Must),
        ("MAY", RequirementKeyword::May),
    ];

    let bytes = value.as_bytes();
    let mut keywords = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let mut matched = None;
        for (pattern, keyword) in PATTERNS {
            let pattern_bytes = pattern.as_bytes();
            if bytes[index..].starts_with(pattern_bytes)
                && is_keyword_boundary(bytes, index.checked_sub(1))
                && is_keyword_boundary(bytes, Some(index + pattern_bytes.len()))
            {
                matched = Some((pattern_bytes.len(), *keyword));
                break;
            }
        }

        if let Some((length, keyword)) = matched {
            keywords.push(keyword);
            index += length;
        } else {
            index += 1;
        }
    }
    keywords
}

fn is_keyword_boundary(bytes: &[u8], index: Option<usize>) -> bool {
    index.is_none_or(|index| {
        bytes
            .get(index)
            .is_none_or(|byte| !byte.is_ascii_alphanumeric() && *byte != b'_')
    })
}

fn slugify(value: &str) -> String {
    let mut slug = String::with_capacity(value.len());
    let mut pending_separator = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            if pending_separator && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(character.to_ascii_lowercase());
            pending_separator = false;
        } else if !slug.is_empty() {
            pending_separator = true;
        }
    }
    if slug.is_empty() {
        "section".to_owned()
    } else {
        slug
    }
}

fn escape_markdown_cell(value: &str) -> String {
    value
        .replace('|', "\\|")
        .replace(['\r', '\n'], "<br>")
        .trim()
        .to_owned()
}
