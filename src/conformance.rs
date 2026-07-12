//! Official MCP conformance result and baseline support.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::process::CommandSpec;

/// Pinned official MCP conformance package used as the protocol oracle.
pub const OFFICIAL_CONFORMANCE_PACKAGE: &str = "@modelcontextprotocol/conformance@0.1.16";
/// Default MCP specification version exercised by the harness.
pub const DEFAULT_MCP_SPEC_VERSION: &str = "2025-11-25";
/// Default official server scenario suite.
pub const DEFAULT_CONFORMANCE_SUITE: &str = "all";

// Exact server catalogs emitted by @modelcontextprotocol/conformance@0.1.16.
// Keep these coupled to OFFICIAL_CONFORMANCE_PACKAGE and verify the pin with
// the ignored package-backed test before updating either.
const SERVER_SCENARIOS_2025_06_18: [&str; 26] = [
    "server-initialize",
    "logging-set-level",
    "ping",
    "completion-complete",
    "tools-list",
    "tools-call-simple-text",
    "tools-call-image",
    "tools-call-audio",
    "tools-call-embedded-resource",
    "tools-call-mixed-content",
    "tools-call-with-logging",
    "tools-call-error",
    "tools-call-with-progress",
    "tools-call-sampling",
    "tools-call-elicitation",
    "resources-list",
    "resources-read-text",
    "resources-read-binary",
    "resources-templates-read",
    "resources-subscribe",
    "resources-unsubscribe",
    "prompts-list",
    "prompts-get-simple",
    "prompts-get-with-args",
    "prompts-get-embedded-resource",
    "prompts-get-with-image",
];
const SERVER_ACTIVE_ADDITIONS_2025_11_25: [&str; 4] = [
    "elicitation-sep1034-defaults",
    "server-sse-multiple-streams",
    "elicitation-sep1330-enums",
    "dns-rebinding-protection",
];
const SERVER_PENDING_2025_11_25: [&str; 2] = ["json-schema-2020-12", "server-sse-polling"];

const MAX_CHECKS_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Builds the exact official server-conformance invocation.
#[must_use = "a command specification does nothing until a process runner executes it"]
pub fn official_server_command(
    endpoint: &str,
    suite: &str,
    spec_version: &str,
    expected_failures: &Path,
    output_dir: &Path,
) -> CommandSpec {
    CommandSpec::new("npx")
        .clear_environment()
        .arg("-y")
        .arg(OFFICIAL_CONFORMANCE_PACKAGE)
        .arg("server")
        .arg("--url")
        .arg(endpoint)
        .arg("--suite")
        .arg(suite)
        .arg("--spec-version")
        .arg(spec_version)
        .arg("--expected-failures")
        .arg(expected_failures.as_os_str().to_owned())
        .arg("--output-dir")
        .arg(output_dir.as_os_str().to_owned())
        .arg("--verbose")
}

/// Stack whose independent expected-failure baseline is being validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BaselineTarget {
    /// Python control-plane gateway.
    Controlplane,
    /// Rust dataplane routed through the control plane.
    Dataplane,
}

impl BaselineTarget {
    /// Stable report label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Controlplane => "control-plane",
            Self::Dataplane => "dataplane",
        }
    }
}

impl fmt::Display for BaselineTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// Implementation ownership for an expected conformance gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BaselineClassification {
    /// Gap exists only in the Python control plane.
    Controlplane,
    /// Gap exists only in the Rust dataplane path.
    Dataplane,
    /// Gap is shared by both paths.
    Shared,
}

/// One fully documented expected failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineEntry {
    /// Exact official conformance scenario name.
    pub scenario: String,
    /// MCP specification URL, including the relevant page or section.
    pub spec_reference: String,
    /// Concrete implementation behavior that differs from the specification.
    pub implementation_gap: String,
    /// Tracking issue URL for removing this expected failure.
    pub linked_issue: String,
    /// Component that owns the gap.
    pub classification: BaselineClassification,
}

/// Rich repository baseline. It is projected before passing it to the official CLI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Baseline {
    /// Server scenarios with documented expected failures.
    #[serde(default)]
    pub server: Vec<BaselineEntry>,
}

/// Parses and validates a rich expected-failure baseline.
pub fn parse_baseline(source: &str, target: BaselineTarget) -> Result<Baseline> {
    let baseline: Baseline =
        serde_yaml::from_str(source).context("failed to parse rich conformance baseline YAML")?;
    validate_baseline(&baseline, target)?;
    Ok(baseline)
}

/// Loads and validates a rich expected-failure baseline.
pub fn load_baseline(path: &Path, target: BaselineTarget) -> Result<Baseline> {
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read conformance baseline {path:?}"))?;
    parse_baseline(&source, target)
        .with_context(|| format!("invalid conformance baseline {path:?}"))
}

/// Validates metadata, exact scenario names, uniqueness, and component ownership.
pub fn validate_baseline(baseline: &Baseline, target: BaselineTarget) -> Result<()> {
    let mut scenarios = BTreeSet::new();

    for entry in &baseline.server {
        if contains_wildcard(&entry.scenario) {
            bail!(
                "baseline scenario {:?} contains a wildcard; expected failures must name one exact scenario",
                entry.scenario
            );
        }
        validate_scenario_name(&entry.scenario)
            .with_context(|| format!("invalid baseline scenario {:?}", entry.scenario))?;
        if !scenarios.insert(entry.scenario.as_str()) {
            bail!("duplicate baseline scenario {:?}", entry.scenario);
        }
        if !is_specification_reference(&entry.spec_reference) {
            bail!(
                "baseline scenario {:?} has invalid spec_reference {:?}; expected an MCP specification HTTPS URL",
                entry.scenario,
                entry.spec_reference
            );
        }
        if entry.implementation_gap.trim().is_empty() {
            bail!(
                "baseline scenario {:?} has an empty implementation_gap",
                entry.scenario
            );
        }
        if !is_issue_reference(&entry.linked_issue) {
            bail!(
                "baseline scenario {:?} has invalid linked_issue {:?}; expected a specific HTTPS issue URL",
                entry.scenario,
                entry.linked_issue
            );
        }
        match (target, entry.classification) {
            (BaselineTarget::Controlplane, BaselineClassification::Dataplane) => bail!(
                "dataplane-only scenario {:?} cannot appear in the control-plane baseline",
                entry.scenario
            ),
            (BaselineTarget::Dataplane, BaselineClassification::Controlplane) => bail!(
                "control-plane-only scenario {:?} cannot appear in the dataplane baseline",
                entry.scenario
            ),
            _ => {}
        }
    }

    Ok(())
}

#[derive(Serialize)]
struct OfficialBaseline<'a> {
    server: Vec<&'a str>,
}

/// Renders the rich baseline as the official CLI's `server: [scenario...]` schema.
pub fn project_official_baseline(baseline: &Baseline, target: BaselineTarget) -> Result<String> {
    validate_baseline(baseline, target)?;
    let mut server: Vec<_> = baseline
        .server
        .iter()
        .map(|entry| entry.scenario.as_str())
        .collect();
    server.sort_unstable();

    serde_yaml::to_string(&OfficialBaseline { server })
        .context("failed to serialize official expected-failure projection")
}

/// Writes an official-compatible expected-failure projection.
pub fn write_official_baseline_projection(
    baseline: &Baseline,
    target: BaselineTarget,
    path: &Path,
) -> Result<()> {
    let projection = project_official_baseline(baseline, target)?;
    create_parent_directory(path)?;
    fs::write(path, projection)
        .with_context(|| format!("failed to write expected-failure projection {path:?}"))
}

/// Typed official check status with forward-compatible preservation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CheckStatus {
    /// Required check passed.
    Success,
    /// Required check failed.
    Failure,
    /// Warning; the official baseline logic treats this as a scenario failure.
    Warning,
    /// Scenario or check was explicitly skipped.
    Skipped,
    /// Informational event which does not establish conformance.
    Info,
    /// Status introduced by a newer official package.
    Other(String),
}

impl CheckStatus {
    fn from_wire(value: String) -> Self {
        match value.as_str() {
            "SUCCESS" => Self::Success,
            "FAILURE" => Self::Failure,
            "WARNING" => Self::Warning,
            "SKIPPED" => Self::Skipped,
            "INFO" => Self::Info,
            _ => Self::Other(value),
        }
    }

    fn as_wire(&self) -> &str {
        match self {
            Self::Success => "SUCCESS",
            Self::Failure => "FAILURE",
            Self::Warning => "WARNING",
            Self::Skipped => "SKIPPED",
            Self::Info => "INFO",
            Self::Other(value) => value,
        }
    }
}

impl Serialize for CheckStatus {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_wire())
    }
}

impl<'de> Deserialize<'de> for CheckStatus {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self::from_wire)
    }
}

/// An official MCP specification reference. Unknown fields are retained verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpecReference {
    /// Reference identifier emitted by the official framework.
    pub id: String,
    /// Optional source URL emitted by the official framework.
    #[serde(default)]
    pub url: Option<String>,
    /// Forward-compatible fields from the official framework.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

/// One check from an official `checks.json` file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConformanceCheck {
    /// Stable official check identifier.
    pub id: String,
    /// Human-readable check name.
    #[serde(default)]
    pub name: Option<String>,
    /// Human-readable check description.
    #[serde(default)]
    pub description: Option<String>,
    /// Typed official status.
    pub status: CheckStatus,
    /// Framework timestamp, preserved without interpretation.
    #[serde(default)]
    pub timestamp: Option<String>,
    /// Specification references, preserved without normalization.
    #[serde(rename = "specReferences", default)]
    pub spec_references: Vec<SpecReference>,
    /// Failure text, if supplied by the official framework.
    #[serde(rename = "errorMessage", default)]
    pub error_message: Option<String>,
    /// Scenario-specific details.
    #[serde(default)]
    pub details: Option<Value>,
    /// Scenario-specific metadata.
    #[serde(default)]
    pub metadata: Option<Value>,
    /// Scenario logs in their original JSON shape.
    #[serde(default)]
    pub logs: Option<Value>,
    /// Forward-compatible fields from the official framework.
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

/// Result and provenance for one official server scenario.
#[derive(Debug, Clone, PartialEq)]
pub struct ConformanceScenarioResult {
    /// Validated scenario name extracted from the official result directory.
    pub scenario: String,
    /// Typed checks parsed from `checks.json`.
    pub checks: Vec<ConformanceCheck>,
    /// Relative path beneath the caller-provided result root.
    pub source: PathBuf,
}

impl ConformanceScenarioResult {
    /// Reduces official check statuses into a scenario-level comparison outcome.
    #[must_use]
    pub fn outcome(&self) -> ScenarioOutcome {
        let mut has_success = false;
        let mut has_skipped = false;
        let mut has_unknown = false;
        let mut failures = Vec::new();

        for check in &self.checks {
            match check.status {
                CheckStatus::Failure | CheckStatus::Warning => failures.push(check),
                CheckStatus::Success => has_success = true,
                CheckStatus::Skipped => has_skipped = true,
                CheckStatus::Other(_) => has_unknown = true,
                CheckStatus::Info => {}
            }
        }

        if !failures.is_empty()
            && failures.iter().all(|check| {
                check
                    .error_message
                    .as_deref()
                    .is_some_and(is_missing_official_fixture)
            })
        {
            ScenarioOutcome::FixtureFailure
        } else if !failures.is_empty() {
            ScenarioOutcome::NonCompliant
        } else if has_unknown {
            ScenarioOutcome::Ambiguous
        } else if has_success {
            ScenarioOutcome::Compliant
        } else if has_skipped {
            ScenarioOutcome::NotApplicable
        } else {
            ScenarioOutcome::Ambiguous
        }
    }

    /// Reduces the outcome using trusted pinned-fixture provenance.
    ///
    /// A fixture-shaped not-found result from a trusted fixture is attributed
    /// to the gateway path as a compliance failure. Unknown or caller-managed
    /// fixtures retain the historical fixture-failure outcome.
    #[must_use]
    pub fn outcome_with_trusted_fixture(&self, trusted_fixture: bool) -> ScenarioOutcome {
        match (self.outcome(), trusted_fixture) {
            (ScenarioOutcome::FixtureFailure, true) => ScenarioOutcome::NonCompliant,
            (outcome, _) => outcome,
        }
    }

    fn has_official_failure(&self) -> bool {
        self.checks
            .iter()
            .any(|check| matches!(check.status, CheckStatus::Failure | CheckStatus::Warning))
    }
}

fn is_missing_official_fixture(message: &str) -> bool {
    const EXACT_MARKERS: [&str; 8] = [
        "Tool not found: test_",
        "Tool 'json_schema_2020_12_tool' not found",
        "Prompt not found: test_",
        "Resource not found: test://",
        "Routing problem... wrong tool name",
        "Routing problem... wrong prompt name",
        "Routing problem... wrong resource name",
        "Routing problem... wrong completion reference",
    ];

    EXACT_MARKERS.iter().any(|marker| message.contains(marker))
}

/// Deterministically indexed official server results.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConformanceResults {
    /// Scenario name to parsed result.
    pub scenarios: BTreeMap<String, ConformanceScenarioResult>,
}

/// Rejects fresh runs when an unknown or caller-managed fixture prevented checks.
///
/// # Errors
///
/// Returns an error listing every scenario with a fixture-failure outcome.
/// Pinned fixtures with recorded provenance are handled by the runtime as
/// gateway failures instead of calling this compatibility validator.
pub fn validate_no_fixture_failures(results: &ConformanceResults) -> Result<()> {
    let fixture_failures = results
        .scenarios
        .iter()
        .filter_map(|(scenario, result)| {
            (result.outcome() == ScenarioOutcome::FixtureFailure).then_some(scenario.as_str())
        })
        .collect::<Vec<_>>();

    if !fixture_failures.is_empty() {
        bail!(
            "official fixture setup failed for conformance scenarios: {}",
            fixture_failures.join(", ")
        );
    }
    Ok(())
}

/// Returns the exact pinned server scenario set for one supported suite/spec pair.
///
/// # Errors
///
/// Returns an error for a suite or specification revision without a catalog
/// verified against [`OFFICIAL_CONFORMANCE_PACKAGE`].
pub fn expected_server_scenarios(
    suite: &str,
    spec_version: &str,
) -> Result<BTreeSet<&'static str>> {
    if !matches!(suite, "active" | "all") {
        bail!("unsupported official server suite {suite:?}; expected active or all");
    }
    let mut scenarios = SERVER_SCENARIOS_2025_06_18
        .into_iter()
        .collect::<BTreeSet<_>>();
    match spec_version {
        "2025-06-18" => {}
        "2025-11-25" => {
            scenarios.extend(SERVER_ACTIVE_ADDITIONS_2025_11_25);
            if suite == "all" {
                scenarios.extend(SERVER_PENDING_2025_11_25);
            }
        }
        _ => bail!(
            "no verified {OFFICIAL_CONFORMANCE_PACKAGE} server scenario catalog for specification {spec_version:?}"
        ),
    }
    Ok(scenarios)
}

/// Requires parsed results to exactly cover the pinned suite/spec scenario set.
///
/// # Errors
///
/// Returns an error listing missing or unexpected scenarios when a child run
/// stopped early or the pinned package catalog changed.
pub fn validate_server_scenario_set(
    results: &ConformanceResults,
    suite: &str,
    spec_version: &str,
) -> Result<()> {
    let expected = expected_server_scenarios(suite, spec_version)?;
    let actual = results
        .scenarios
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let missing = expected.difference(&actual).copied().collect::<Vec<_>>();
    let unexpected = actual.difference(&expected).copied().collect::<Vec<_>>();
    let empty_checks = results
        .scenarios
        .iter()
        .filter_map(|(scenario, result)| result.checks.is_empty().then_some(scenario.as_str()))
        .collect::<Vec<_>>();
    if !missing.is_empty() || !unexpected.is_empty() || !empty_checks.is_empty() {
        bail!(
            "official conformance scenario set is incomplete for suite {suite:?} and specification {spec_version:?}; missing={missing:?}; unexpected={unexpected:?}; empty_checks={empty_checks:?}"
        );
    }
    Ok(())
}

/// Recursively parses official `server-*/checks.json` result files.
///
/// Symlinks are never followed, scenario directory names are strictly validated,
/// and stored provenance is relative to `root`.
pub fn load_server_results(root: &Path) -> Result<ConformanceResults> {
    let root = fs::canonicalize(root)
        .with_context(|| format!("failed to resolve conformance results root {root:?}"))?;
    if !root.is_dir() {
        bail!("conformance results root {root:?} is not a directory");
    }

    let mut check_files = Vec::new();
    collect_check_files(&root, &mut check_files)?;
    check_files.sort();

    let mut scenarios = BTreeMap::new();
    for path in check_files {
        let parent = path
            .parent()
            .context("checks.json result has no parent directory")?;
        let directory_name = parent
            .file_name()
            .and_then(OsStr::to_str)
            .context("official result directory name is not valid UTF-8")?;
        if !directory_name.starts_with("server-") {
            continue;
        }
        let scenario = scenario_from_result_directory(directory_name)?;
        let metadata = fs::metadata(&path)
            .with_context(|| format!("failed to inspect official result file {path:?}"))?;
        if metadata.len() > MAX_CHECKS_FILE_BYTES {
            bail!(
                "official result file {path:?} exceeds the {} byte safety limit",
                MAX_CHECKS_FILE_BYTES
            );
        }
        let source_bytes = fs::read(&path)
            .with_context(|| format!("failed to read official result file {path:?}"))?;
        let checks: Vec<ConformanceCheck> = serde_json::from_slice(&source_bytes)
            .with_context(|| format!("failed to parse official result file {path:?}"))?;
        let source = path
            .strip_prefix(&root)
            .context("official result escaped the result root")?
            .to_owned();
        let result = ConformanceScenarioResult {
            scenario: scenario.clone(),
            checks,
            source,
        };
        if scenarios.insert(scenario.clone(), result).is_some() {
            bail!("duplicate official result for scenario {scenario:?}");
        }
    }

    Ok(ConformanceResults { scenarios })
}

fn collect_check_files(directory: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(directory)
        .with_context(|| format!("failed to read conformance result directory {directory:?}"))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| {
            format!("failed to enumerate conformance result directory {directory:?}")
        })?;
    entries.sort_by_key(std::fs::DirEntry::file_name);

    for entry in entries {
        let file_type = entry.file_type().with_context(|| {
            format!(
                "failed to inspect conformance result entry {:?}",
                entry.path()
            )
        })?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_check_files(&entry.path(), output)?;
        } else if file_type.is_file() && entry.file_name() == OsStr::new("checks.json") {
            output.push(entry.path());
        }
    }
    Ok(())
}

fn scenario_from_result_directory(directory: &str) -> Result<String> {
    let rest = directory
        .strip_prefix("server-")
        .context("official result directory must begin with server-")?;
    if rest.len() <= 25 {
        bail!("official result directory {directory:?} has no scenario or timestamp");
    }
    let split = rest.len() - 25;
    if rest.as_bytes().get(split) != Some(&b'-') {
        bail!("official result directory {directory:?} has an invalid timestamp separator");
    }
    let scenario = &rest[..split];
    let timestamp = &rest[split + 1..];
    if !is_normalized_iso_timestamp(timestamp) {
        bail!("official result directory {directory:?} has an invalid normalized timestamp");
    }
    validate_scenario_name(scenario)
        .with_context(|| format!("invalid scenario in official result directory {directory:?}"))?;
    Ok(scenario.to_owned())
}

fn is_normalized_iso_timestamp(value: &str) -> bool {
    if value.len() != 24 || !value.is_ascii() {
        return false;
    }
    let bytes = value.as_bytes();
    for (index, expected) in [
        (4, b'-'),
        (7, b'-'),
        (10, b'T'),
        (13, b'-'),
        (16, b'-'),
        (19, b'-'),
        (23, b'Z'),
    ] {
        if bytes[index] != expected {
            return false;
        }
    }
    bytes.iter().enumerate().all(|(index, byte)| {
        matches!(index, 4 | 7 | 10 | 13 | 16 | 19 | 23) || byte.is_ascii_digit()
    })
}

fn validate_scenario_name(scenario: &str) -> Result<()> {
    if scenario.is_empty() {
        bail!("scenario name is empty");
    }
    if scenario.contains("..") {
        bail!("scenario name contains a parent-directory segment");
    }
    if scenario.starts_with(['-', '.', '/', '_'])
        || scenario.ends_with(['-', '.', '/', '_'])
        || scenario.contains("//")
    {
        bail!("scenario name has an invalid boundary or path segment");
    }
    if !scenario
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
    {
        bail!("scenario name contains unsupported characters");
    }
    Ok(())
}

fn contains_wildcard(scenario: &str) -> bool {
    scenario.contains(['*', '?', '[', ']'])
}

fn is_https_url(value: &str) -> bool {
    if value.chars().any(char::is_whitespace) {
        return false;
    }
    let Some(authority_and_path) = value.strip_prefix("https://") else {
        return false;
    };
    let Some((authority, path)) = authority_and_path.split_once('/') else {
        return false;
    };
    !authority.is_empty()
        && authority.contains('.')
        && !authority.starts_with('.')
        && !authority.ends_with('.')
        && !path.is_empty()
        && !value.contains(['*', '[', ']'])
}

fn is_specification_reference(value: &str) -> bool {
    is_https_url(value)
        && value.starts_with("https://modelcontextprotocol.io/specification/")
        && value
            .trim_start_matches("https://modelcontextprotocol.io/specification/")
            .contains('/')
}

fn is_issue_reference(value: &str) -> bool {
    if !is_https_url(value) {
        return false;
    }
    let path = value
        .strip_prefix("https://")
        .and_then(|rest| rest.split_once('/').map(|(_, path)| path))
        .unwrap_or_default()
        .trim_end_matches('/');
    let final_segment = path.rsplit('/').next().unwrap_or_default();
    !final_segment.is_empty()
        && !matches!(
            final_segment,
            "issues" | "issue" | "browse" | "tickets" | "ticket"
        )
}

/// Difference between one rich baseline and parsed official results.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BaselineAudit {
    /// Failures or warnings that are documented in the baseline.
    pub expected_failures: Vec<String>,
    /// Failures or warnings absent from the baseline.
    pub unexpected_failures: Vec<String>,
    /// Baseline entries whose observed scenario now has no failure or warning.
    pub stale_entries: Vec<String>,
    /// Baseline entries not present in the parsed result set.
    pub unobserved_entries: Vec<String>,
}

impl BaselineAudit {
    /// True when the baseline has neither regressions nor maintenance errors.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.unexpected_failures.is_empty()
            && self.stale_entries.is_empty()
            && self.unobserved_entries.is_empty()
    }
}

/// Compares parsed scenario results to a rich baseline using official warning semantics.
#[must_use]
pub fn audit_baseline(results: &ConformanceResults, baseline: &Baseline) -> BaselineAudit {
    let expected: BTreeSet<_> = baseline
        .server
        .iter()
        .map(|entry| entry.scenario.as_str())
        .collect();
    let mut audit = BaselineAudit::default();

    for (scenario, result) in &results.scenarios {
        match (
            result.has_official_failure(),
            expected.contains(scenario.as_str()),
        ) {
            (true, true) => audit.expected_failures.push(scenario.clone()),
            (true, false) => audit.unexpected_failures.push(scenario.clone()),
            (false, true) => audit.stale_entries.push(scenario.clone()),
            (false, false) => {}
        }
    }
    for scenario in expected {
        if !results.scenarios.contains_key(scenario) {
            audit.unobserved_entries.push(scenario.to_owned());
        }
    }
    audit
}

/// Scenario-level outcome used for control-plane/dataplane comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScenarioOutcome {
    /// All applicable official checks passed.
    Compliant,
    /// At least one official failure or warning was observed.
    NonCompliant,
    /// Fixture setup failed; implementation compliance is not established.
    FixtureFailure,
    /// Official checks explicitly skipped the scenario.
    NotApplicable,
    /// Results exist but do not establish a clear outcome.
    Ambiguous,
    /// No result was produced for this side.
    Missing,
}

impl ScenarioOutcome {
    /// Stable report label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Compliant => "compliant",
            Self::NonCompliant => "failure",
            Self::FixtureFailure => "fixture failure",
            Self::NotApplicable => "not applicable",
            Self::Ambiguous => "ambiguous",
            Self::Missing => "missing",
        }
    }
}

/// Required comparison report classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ComparisonClassification {
    /// Both paths are compliant.
    BothCompliant,
    /// Only the control-plane scenario applies and it is compliant.
    ControlplaneCompliant,
    /// Only the dataplane scenario applies and it is compliant.
    DataplaneCompliant,
    /// The control plane fails while the dataplane passes or is not applicable.
    ControlplaneOnlyFailure,
    /// The dataplane fails while the control plane passes or is not applicable.
    DataplaneOnlyFailure,
    /// Both paths fail.
    SharedFailure,
    /// Every observed implementation failure is documented in its independent baseline.
    ExpectedFailure,
    /// Fixture setup prevented a compliance result.
    FixtureFailure,
    /// Neither path applies.
    NotApplicable,
    /// Missing, unknown, or internally inconsistent evidence.
    Ambiguous,
}

impl ComparisonClassification {
    const ALL: [Self; 10] = [
        Self::BothCompliant,
        Self::ControlplaneCompliant,
        Self::DataplaneCompliant,
        Self::ControlplaneOnlyFailure,
        Self::DataplaneOnlyFailure,
        Self::SharedFailure,
        Self::ExpectedFailure,
        Self::FixtureFailure,
        Self::NotApplicable,
        Self::Ambiguous,
    ];

    /// Stable report label matching the compliance-report vocabulary.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::BothCompliant => "both compliant",
            Self::ControlplaneCompliant => "control-plane compliant",
            Self::DataplaneCompliant => "dataplane compliant",
            Self::ControlplaneOnlyFailure => "control-plane only failure",
            Self::DataplaneOnlyFailure => "dataplane only failure",
            Self::SharedFailure => "shared failure",
            Self::ExpectedFailure => "expected failure",
            Self::FixtureFailure => "fixture failure",
            Self::NotApplicable => "not applicable",
            Self::Ambiguous => "ambiguous",
        }
    }
}

/// Classifies a pair of scenario outcomes.
#[must_use]
pub fn classify_outcomes(
    controlplane: ScenarioOutcome,
    dataplane: ScenarioOutcome,
    expected_failure: bool,
) -> ComparisonClassification {
    use ScenarioOutcome::{
        Ambiguous, Compliant, FixtureFailure, Missing, NonCompliant, NotApplicable,
    };

    if matches!(controlplane, FixtureFailure) || matches!(dataplane, FixtureFailure) {
        return ComparisonClassification::FixtureFailure;
    }
    if expected_failure
        && (matches!(controlplane, NonCompliant) || matches!(dataplane, NonCompliant))
    {
        return ComparisonClassification::ExpectedFailure;
    }
    match (controlplane, dataplane) {
        (Compliant, Compliant) => ComparisonClassification::BothCompliant,
        (Compliant, NotApplicable) => ComparisonClassification::ControlplaneCompliant,
        (NotApplicable, Compliant) => ComparisonClassification::DataplaneCompliant,
        (NonCompliant, Compliant | NotApplicable) => {
            ComparisonClassification::ControlplaneOnlyFailure
        }
        (Compliant | NotApplicable, NonCompliant) => ComparisonClassification::DataplaneOnlyFailure,
        (NonCompliant, NonCompliant) => ComparisonClassification::SharedFailure,
        (NotApplicable, NotApplicable) => ComparisonClassification::NotApplicable,
        (Ambiguous | Missing, _) | (_, Ambiguous | Missing) => ComparisonClassification::Ambiguous,
        _ => ComparisonClassification::Ambiguous,
    }
}

/// One scenario row in the deterministic comparison report.
#[derive(Debug, Clone, PartialEq)]
pub struct ScenarioComparison {
    /// Official scenario name.
    pub scenario: String,
    /// Control-plane result.
    pub controlplane: ScenarioOutcome,
    /// Dataplane result.
    pub dataplane: ScenarioOutcome,
    /// Reduced report classification.
    pub classification: ComparisonClassification,
    /// Baselines that expected an observed failure.
    pub expected_by: BTreeSet<BaselineTarget>,
    /// Raw official references from both result sets.
    pub spec_references: Vec<SpecReference>,
}

/// Compares parsed official results using independent expected-failure baselines.
#[must_use]
pub fn compare_result_sets(
    controlplane: &ConformanceResults,
    dataplane: &ConformanceResults,
    controlplane_baseline: &Baseline,
    dataplane_baseline: &Baseline,
) -> Vec<ScenarioComparison> {
    compare_result_sets_with_fixture_trust(
        controlplane,
        dataplane,
        controlplane_baseline,
        dataplane_baseline,
        false,
        false,
    )
}

/// Compares results while independently attributing trusted fixture failures.
///
/// A trusted side converts fixture-shaped not-found results into ordinary
/// implementation failures. An untrusted side preserves historical behavior.
#[must_use]
pub fn compare_result_sets_with_fixture_trust(
    controlplane: &ConformanceResults,
    dataplane: &ConformanceResults,
    controlplane_baseline: &Baseline,
    dataplane_baseline: &Baseline,
    controlplane_trusted_fixture: bool,
    dataplane_trusted_fixture: bool,
) -> Vec<ScenarioComparison> {
    let controlplane_expected: BTreeSet<_> = controlplane_baseline
        .server
        .iter()
        .map(|entry| entry.scenario.as_str())
        .collect();
    let dataplane_expected: BTreeSet<_> = dataplane_baseline
        .server
        .iter()
        .map(|entry| entry.scenario.as_str())
        .collect();
    let mut scenarios: BTreeSet<_> = controlplane.scenarios.keys().cloned().collect();
    scenarios.extend(dataplane.scenarios.keys().cloned());
    scenarios.extend(
        controlplane_expected
            .iter()
            .map(|scenario| (*scenario).to_owned()),
    );
    scenarios.extend(
        dataplane_expected
            .iter()
            .map(|scenario| (*scenario).to_owned()),
    );

    scenarios
        .into_iter()
        .map(|scenario| {
            let controlplane_result = controlplane.scenarios.get(&scenario);
            let dataplane_result = dataplane.scenarios.get(&scenario);
            let controlplane_outcome = controlplane_result
                .map(|result| result.outcome_with_trusted_fixture(controlplane_trusted_fixture))
                .unwrap_or(ScenarioOutcome::Missing);
            let dataplane_outcome = dataplane_result
                .map(|result| result.outcome_with_trusted_fixture(dataplane_trusted_fixture))
                .unwrap_or(ScenarioOutcome::Missing);

            let mut expected_by = BTreeSet::new();
            if controlplane_outcome == ScenarioOutcome::NonCompliant
                && controlplane_expected.contains(scenario.as_str())
            {
                expected_by.insert(BaselineTarget::Controlplane);
            }
            if dataplane_outcome == ScenarioOutcome::NonCompliant
                && dataplane_expected.contains(scenario.as_str())
            {
                expected_by.insert(BaselineTarget::Dataplane);
            }
            let failed_sides = usize::from(controlplane_outcome == ScenarioOutcome::NonCompliant)
                + usize::from(dataplane_outcome == ScenarioOutcome::NonCompliant);
            let evidence_is_complete = !matches!(
                controlplane_outcome,
                ScenarioOutcome::Missing
                    | ScenarioOutcome::Ambiguous
                    | ScenarioOutcome::FixtureFailure
            ) && !matches!(
                dataplane_outcome,
                ScenarioOutcome::Missing
                    | ScenarioOutcome::Ambiguous
                    | ScenarioOutcome::FixtureFailure
            );
            let expected =
                evidence_is_complete && failed_sides > 0 && expected_by.len() == failed_sides;

            let mut spec_references = Vec::new();
            if let Some(result) = controlplane_result {
                append_references(&mut spec_references, result);
            }
            if let Some(result) = dataplane_result {
                append_references(&mut spec_references, result);
            }
            sort_and_deduplicate_references(&mut spec_references);

            ScenarioComparison {
                scenario,
                controlplane: controlplane_outcome,
                dataplane: dataplane_outcome,
                classification: classify_outcomes(
                    controlplane_outcome,
                    dataplane_outcome,
                    expected,
                ),
                expected_by,
                spec_references,
            }
        })
        .collect()
}

fn append_references(output: &mut Vec<SpecReference>, result: &ConformanceScenarioResult) {
    for check in &result.checks {
        output.extend(check.spec_references.iter().cloned());
    }
}

fn sort_and_deduplicate_references(references: &mut Vec<SpecReference>) {
    references.sort_by_cached_key(reference_key);
    references.dedup();
}

fn reference_key(reference: &SpecReference) -> String {
    serde_json::to_string(reference).unwrap_or_else(|_| {
        format!(
            "{}\u{0}{}",
            reference.id,
            reference.url.as_deref().unwrap_or_default()
        )
    })
}

/// Inputs for a deterministic Markdown comparison report.
#[derive(Debug, Clone, PartialEq)]
pub struct ComparisonReport {
    /// MCP specification version exercised by both result sets.
    pub spec_version: String,
    /// Official scenario suite exercised by both result sets.
    pub suite: String,
    /// Scenario comparisons in any order; rendering sorts them.
    pub scenarios: Vec<ScenarioComparison>,
}

/// Renders a deterministic, untrusted-input-safe Markdown comparison report.
#[must_use]
pub fn render_comparison_markdown(report: &ComparisonReport) -> String {
    let mut output = String::new();
    output.push_str("# MCP Conformance Comparison\n\n");
    output.push_str(&format!(
        "- Official oracle: `{}`\n- Specification: `{}`\n- Suite: `{}`\n\n",
        OFFICIAL_CONFORMANCE_PACKAGE,
        markdown_code(&report.spec_version),
        markdown_code(&report.suite)
    ));

    let mut counts = BTreeMap::new();
    for scenario in &report.scenarios {
        *counts.entry(scenario.classification).or_insert(0_usize) += 1;
    }
    output.push_str("## Summary\n\n");
    output.push_str("| Classification | Scenarios |\n|---|---:|\n");
    for classification in ComparisonClassification::ALL {
        output.push_str(&format!(
            "| {} | {} |\n",
            classification.label(),
            counts.get(&classification).copied().unwrap_or_default()
        ));
    }

    output.push_str("\n## Scenarios\n\n");
    output.push_str(
        "| Scenario | Control plane | Dataplane | Classification | Expected by | Specification references |\n",
    );
    output.push_str("|---|---|---|---|---|---|\n");
    let mut scenarios: Vec<_> = report.scenarios.iter().collect();
    scenarios.sort_by(|left, right| left.scenario.cmp(&right.scenario));
    for scenario in scenarios {
        let expected_by = if scenario.expected_by.is_empty() {
            "—".to_owned()
        } else {
            scenario
                .expected_by
                .iter()
                .map(|target| target.label())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let references = if scenario.spec_references.is_empty() {
            "—".to_owned()
        } else {
            scenario
                .spec_references
                .iter()
                .map(render_reference)
                .collect::<Vec<_>>()
                .join("<br>")
        };
        output.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            markdown_cell(&scenario.scenario),
            scenario.controlplane.label(),
            scenario.dataplane.label(),
            scenario.classification.label(),
            markdown_cell(&expected_by),
            references
        ));
    }
    output
}

/// Writes a deterministic Markdown comparison report.
pub fn write_comparison_report(path: &Path, report: &ComparisonReport) -> Result<()> {
    create_parent_directory(path)?;
    fs::write(path, render_comparison_markdown(report))
        .with_context(|| format!("failed to write conformance comparison report {path:?}"))
}

fn render_reference(reference: &SpecReference) -> String {
    let id = markdown_cell(&reference.id);
    match reference.url.as_deref() {
        Some(url) if is_safe_markdown_url(url) => format!("[{id}]({url})"),
        Some(_) => format!("{id} (unsafe URL omitted)"),
        None => id,
    }
}

fn is_safe_markdown_url(url: &str) -> bool {
    (url.starts_with("https://") || url.starts_with("http://"))
        && !url
            .bytes()
            .any(|byte| byte.is_ascii_control() || matches!(byte, b'<' | b'>' | b'(' | b')' | b'|'))
}

fn markdown_cell(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('\r', "")
        .replace('\n', "<br>")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn markdown_code(value: &str) -> String {
    value.replace('`', "\\`").replace(['\r', '\n'], " ")
}

fn create_parent_directory(path: &Path) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directory {parent:?}"))?;
    }
    Ok(())
}
