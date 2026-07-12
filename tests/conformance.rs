use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use cf_integration::conformance::{
    Baseline, BaselineClassification, BaselineEntry, BaselineTarget, CheckStatus,
    ComparisonClassification, ComparisonReport, ConformanceCheck, ConformanceResults,
    ConformanceScenarioResult, DEFAULT_CONFORMANCE_SUITE, DEFAULT_MCP_SPEC_VERSION,
    OFFICIAL_CONFORMANCE_PACKAGE, ScenarioComparison, ScenarioOutcome, SpecReference,
    audit_baseline, classify_outcomes, compare_result_sets, expected_server_scenarios,
    load_baseline, load_server_results, official_server_command, parse_baseline,
    project_official_baseline, render_comparison_markdown, validate_baseline,
    validate_no_fixture_failures, validate_server_scenario_set, write_comparison_report,
    write_official_baseline_projection,
};

const SPEC_REFERENCE: &str =
    "https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle#initialization";
const ISSUE: &str = "https://github.com/example/project/issues/123";

#[test]
fn pinned_server_scenario_catalog_has_exact_suite_differences() {
    let active = expected_server_scenarios("active", "2025-11-25")
        .expect("active scenario catalog should be pinned");
    let all = expected_server_scenarios("all", "2025-11-25")
        .expect("all scenario catalog should be pinned");
    let previous = expected_server_scenarios("all", "2025-06-18")
        .expect("previous stable scenario catalog should be pinned");

    assert_eq!(active.len(), 30);
    assert_eq!(all.len(), 32);
    assert_eq!(previous.len(), 26);
    assert_eq!(
        all.difference(&active).copied().collect::<BTreeSet<_>>(),
        BTreeSet::from(["json-schema-2020-12", "server-sse-polling"])
    );
    assert!(
        expected_server_scenarios("all", "2099-01-01")
            .expect_err("unverified package/spec catalogs must be rejected")
            .to_string()
            .contains("no verified")
    );
}

#[test]
fn partial_server_scenario_catalog_is_rejected() {
    let partial = results([result("server-initialize", [CheckStatus::Failure])]);

    let error = validate_server_scenario_set(&partial, "active", "2025-11-25")
        .expect_err("one parsed scenario cannot prove the suite completed");

    assert!(error.to_string().contains("missing="));
    assert!(!error.to_string().contains("server-initialize\""));
}

#[test]
fn complete_scenario_names_with_empty_checks_are_rejected() {
    let scenarios = expected_server_scenarios("active", "2025-11-25")
        .expect("active catalog should be pinned")
        .into_iter()
        .map(|scenario| {
            (
                scenario.to_owned(),
                result(scenario, std::iter::empty::<CheckStatus>()),
            )
        })
        .collect();
    let empty = ConformanceResults { scenarios };

    let error = validate_server_scenario_set(&empty, "active", "2025-11-25")
        .expect_err("scenario directories without checks cannot prove completion");

    assert!(error.to_string().contains("empty_checks="));
}

#[test]
#[ignore = "requires the pinned official npm package"]
fn pinned_server_scenario_catalog_matches_official_package() {
    for (suite, spec_version) in [
        ("active", "2025-11-25"),
        ("all", "2025-11-25"),
        ("all", "2025-06-18"),
    ] {
        let output = tempfile::tempdir().expect("temporary official output directory");
        let process = Command::new("npx")
            .args([
                "-y",
                OFFICIAL_CONFORMANCE_PACKAGE,
                "server",
                "--url",
                "http://127.0.0.1:1/mcp",
                "--suite",
                suite,
                "--spec-version",
                spec_version,
                "--output-dir",
            ])
            .arg(output.path())
            .output()
            .expect("pinned official package should execute");
        assert_eq!(process.status.code(), Some(1));
        let results = load_server_results(output.path())
            .expect("official package should emit one result per attempted scenario");
        validate_server_scenario_set(&results, suite, spec_version)
            .expect("embedded catalog must match the pinned official package");
    }
}

fn baseline_entry(scenario: &str, classification: BaselineClassification) -> BaselineEntry {
    BaselineEntry {
        scenario: scenario.to_owned(),
        spec_reference: SPEC_REFERENCE.to_owned(),
        implementation_gap: "The gateway does not yet preserve this protocol behavior.".to_owned(),
        linked_issue: ISSUE.to_owned(),
        classification,
    }
}

fn check(id: &str, status: CheckStatus) -> ConformanceCheck {
    ConformanceCheck {
        id: id.to_owned(),
        name: Some(id.to_owned()),
        description: Some(format!("check {id}")),
        status,
        timestamp: Some("2026-07-10T12:34:56.000Z".to_owned()),
        spec_references: vec![SpecReference {
            id: "MCP-Lifecycle".to_owned(),
            url: Some(SPEC_REFERENCE.to_owned()),
            extensions: BTreeMap::new(),
        }],
        error_message: None,
        details: None,
        metadata: None,
        logs: None,
        extensions: BTreeMap::new(),
    }
}

fn failing_check(id: &str, message: &str) -> ConformanceCheck {
    let mut check = check(id, CheckStatus::Failure);
    check.error_message = Some(message.to_owned());
    check
}

fn result(
    scenario: &str,
    statuses: impl IntoIterator<Item = CheckStatus>,
) -> ConformanceScenarioResult {
    ConformanceScenarioResult {
        scenario: scenario.to_owned(),
        checks: statuses
            .into_iter()
            .enumerate()
            .map(|(index, status)| check(&format!("check-{index}"), status))
            .collect(),
        source: PathBuf::from(format!("server-{scenario}-timestamp/checks.json")),
    }
}

fn results(entries: impl IntoIterator<Item = ConformanceScenarioResult>) -> ConformanceResults {
    ConformanceResults {
        scenarios: entries
            .into_iter()
            .map(|entry| (entry.scenario.clone(), entry))
            .collect(),
    }
}

#[test]
fn official_command_is_pinned_complete_and_ordered() {
    let spec = official_server_command(
        "http://127.0.0.1:49152/mcp",
        DEFAULT_CONFORMANCE_SUITE,
        DEFAULT_MCP_SPEC_VERSION,
        Path::new("projected.yml"),
        Path::new("results"),
    );

    assert_eq!(
        OFFICIAL_CONFORMANCE_PACKAGE,
        "@modelcontextprotocol/conformance@0.1.16"
    );
    assert_eq!(DEFAULT_MCP_SPEC_VERSION, "2025-11-25");
    assert_eq!(DEFAULT_CONFORMANCE_SUITE, "all");
    assert_eq!(spec.program(), "npx");
    assert!(
        !spec.inherits_environment(),
        "downloaded conformance code must not inherit arbitrary parent secrets"
    );
    assert_eq!(
        spec.arguments(),
        &[
            OsString::from("-y"),
            OsString::from(OFFICIAL_CONFORMANCE_PACKAGE),
            OsString::from("server"),
            OsString::from("--url"),
            OsString::from("http://127.0.0.1:49152/mcp"),
            OsString::from("--suite"),
            OsString::from("all"),
            OsString::from("--spec-version"),
            OsString::from("2025-11-25"),
            OsString::from("--expected-failures"),
            OsString::from("projected.yml"),
            OsString::from("--output-dir"),
            OsString::from("results"),
            OsString::from("--verbose"),
        ]
    );
}

#[test]
fn rich_baseline_parses_and_projects_only_official_scenario_names() {
    let source = format!(
        "server:\n  - scenario: server-initialize\n    spec_reference: {SPEC_REFERENCE}\n    implementation_gap: initialization is incomplete\n    linked_issue: {ISSUE}\n    classification: controlplane\n"
    );

    let baseline = parse_baseline(&source, BaselineTarget::Controlplane)
        .expect("complete rich baseline should parse");
    let projection = project_official_baseline(&baseline, BaselineTarget::Controlplane)
        .expect("valid baseline should project");

    assert_eq!(baseline.server.len(), 1);
    assert_eq!(projection, "server:\n- server-initialize\n");
    assert!(!projection.contains("implementation_gap"));
    assert!(!projection.contains(ISSUE));
}

#[test]
fn empty_baseline_projects_to_an_official_empty_server_list() {
    let baseline = parse_baseline("server: []\n", BaselineTarget::Dataplane)
        .expect("empty baseline should be valid");

    assert_eq!(
        project_official_baseline(&baseline, BaselineTarget::Dataplane)
            .expect("empty baseline should project"),
        "server: []\n"
    );
}

#[test]
fn repository_baselines_are_independent_valid_and_initially_empty() {
    let controlplane = load_baseline(
        Path::new("conformance/baseline-controlplane.yml"),
        BaselineTarget::Controlplane,
    )
    .expect("control-plane baseline should be valid");
    let dataplane = load_baseline(
        Path::new("conformance/baseline-dataplane.yml"),
        BaselineTarget::Dataplane,
    )
    .expect("dataplane baseline should be valid");

    assert!(controlplane.server.is_empty());
    assert!(dataplane.server.is_empty());
}

#[test]
fn baseline_projection_is_sorted_and_can_be_written() {
    let baseline = Baseline {
        server: vec![
            baseline_entry("tools-list", BaselineClassification::Shared),
            baseline_entry("server-initialize", BaselineClassification::Dataplane),
        ],
    };
    let directory = tempfile::tempdir().expect("temporary directory should be created");
    let path = directory.path().join("nested/projected.yml");

    write_official_baseline_projection(&baseline, BaselineTarget::Dataplane, &path)
        .expect("projection should be written");

    assert_eq!(
        fs::read_to_string(path).expect("projection should be readable"),
        "server:\n- server-initialize\n- tools-list\n"
    );
}

#[test]
fn baseline_rejects_duplicates_wildcards_empty_metadata_and_wrong_side() {
    let mut duplicate = baseline_entry("server-initialize", BaselineClassification::Controlplane);
    let duplicate_baseline = Baseline {
        server: vec![duplicate.clone(), duplicate.clone()],
    };
    assert!(
        validate_baseline(&duplicate_baseline, BaselineTarget::Controlplane)
            .expect_err("duplicates must fail")
            .to_string()
            .contains("duplicate")
    );

    duplicate.scenario = "tools-*".to_owned();
    assert!(
        validate_baseline(
            &Baseline {
                server: vec![duplicate.clone()]
            },
            BaselineTarget::Controlplane
        )
        .expect_err("wildcards must fail")
        .to_string()
        .contains("wildcard")
    );

    duplicate.scenario = "tools-list".to_owned();
    duplicate.implementation_gap = " \n".to_owned();
    assert!(
        validate_baseline(
            &Baseline {
                server: vec![duplicate.clone()]
            },
            BaselineTarget::Controlplane
        )
        .expect_err("empty metadata must fail")
        .to_string()
        .contains("implementation_gap")
    );

    let wrong_side = Baseline {
        server: vec![baseline_entry(
            "tools-list",
            BaselineClassification::Dataplane,
        )],
    };
    assert!(
        validate_baseline(&wrong_side, BaselineTarget::Controlplane)
            .expect_err("a dataplane gap cannot be hidden in the control-plane baseline")
            .to_string()
            .contains("dataplane")
    );
}

#[test]
fn baseline_rejects_missing_fields_unknown_fields_and_non_issue_urls() {
    let missing = "server:\n  - scenario: tools-list\n";
    assert!(parse_baseline(missing, BaselineTarget::Controlplane).is_err());

    let unknown = format!(
        "server:\n  - scenario: tools-list\n    spec_reference: {SPEC_REFERENCE}\n    implementation_gap: gap\n    linked_issue: {ISSUE}\n    classification: shared\n    ignore_everything: true\n"
    );
    assert!(parse_baseline(&unknown, BaselineTarget::Controlplane).is_err());

    let mut invalid = baseline_entry("tools-list", BaselineClassification::Shared);
    invalid.linked_issue = "https://github.com/example/project/issues".to_owned();
    assert!(
        validate_baseline(
            &Baseline {
                server: vec![invalid]
            },
            BaselineTarget::Controlplane
        )
        .expect_err("an issue collection is not a linked issue")
        .to_string()
        .contains("linked_issue")
    );
}

#[test]
fn fixture_results_parse_recursively_with_typed_statuses_and_raw_references() {
    let parsed = load_server_results(Path::new("tests/fixtures/conformance/results"))
        .expect("fixture results should parse");

    assert_eq!(
        parsed
            .scenarios
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["server-initialize", "tools-list"]
    );
    let initialize = &parsed.scenarios["server-initialize"];
    assert_eq!(
        initialize.source,
        PathBuf::from("nested/server-server-initialize-2026-07-10T12-34-56-000Z/checks.json")
    );
    assert_eq!(initialize.checks[0].status, CheckStatus::Success);
    assert_eq!(initialize.checks[1].status, CheckStatus::Info);
    let reference = &initialize.checks[0].spec_references[0];
    assert_eq!(reference.id, "MCP-Lifecycle/raw[id]");
    assert_eq!(reference.url.as_deref(), Some(SPEC_REFERENCE));
    assert_eq!(
        reference.extensions["futureField"],
        serde_json::json!({"kept": true})
    );
    assert_eq!(
        initialize.checks[0].extensions["futureCheckField"],
        "preserved"
    );
    assert_eq!(
        parsed.scenarios["tools-list"].checks[0].status,
        CheckStatus::Warning
    );
}

#[test]
fn unknown_status_is_preserved_and_yields_an_ambiguous_outcome() {
    let json = r#"[{"id":"future","status":"FUTURE_STATUS"}]"#;
    let checks: Vec<ConformanceCheck> =
        serde_json::from_str(json).expect("future statuses should remain parseable");

    assert_eq!(
        checks[0].status,
        CheckStatus::Other("FUTURE_STATUS".to_owned())
    );
    assert_eq!(
        result("future", [checks[0].status.clone()]).outcome(),
        ScenarioOutcome::Ambiguous
    );
}

#[cfg(unix)]
#[test]
fn result_loader_does_not_follow_symlinked_directories() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().expect("temporary directory should be created");
    let outside = tempfile::tempdir().expect("outside directory should be created");
    let result_directory = outside
        .path()
        .join("server-malicious-2026-07-10T12-34-56-000Z");
    fs::create_dir(&result_directory).expect("outside result directory should be created");
    fs::write(
        result_directory.join("checks.json"),
        r#"[{"id":"malicious","status":"SUCCESS"}]"#,
    )
    .expect("outside checks should be written");
    symlink(&result_directory, directory.path().join("linked"))
        .expect("test symlink should be created");

    let parsed =
        load_server_results(directory.path()).expect("symlinked content should be ignored safely");

    assert!(parsed.scenarios.is_empty());
}

#[test]
fn result_loader_rejects_untrusted_directory_names_and_duplicate_scenarios() {
    let directory = tempfile::tempdir().expect("temporary directory should be created");
    let bad = directory.path().join("server-..-2026-07-10T12-34-56-000Z");
    fs::create_dir(&bad).expect("invalid result directory should be created");
    fs::write(bad.join("checks.json"), "[]").expect("invalid checks should be written");
    assert!(
        load_server_results(directory.path())
            .expect_err("untrusted scenario names must fail")
            .to_string()
            .contains("scenario")
    );

    let duplicates = tempfile::tempdir().expect("temporary directory should be created");
    for timestamp in ["2026-07-10T12-34-56-000Z", "2026-07-10T12-35-56-000Z"] {
        let path = duplicates
            .path()
            .join(format!("server-tools-list-{timestamp}"));
        fs::create_dir(&path).expect("duplicate result directory should be created");
        fs::write(path.join("checks.json"), "[]").expect("duplicate checks should be written");
    }
    assert!(
        load_server_results(duplicates.path())
            .expect_err("duplicate scenario results must fail")
            .to_string()
            .contains("duplicate")
    );
}

#[test]
fn baseline_audit_finds_expected_unexpected_stale_and_unobserved_entries() {
    let parsed = results([
        result("expected-failure", [CheckStatus::Failure]),
        result("unexpected-warning", [CheckStatus::Warning]),
        result("fixed", [CheckStatus::Success]),
    ]);
    let baseline = Baseline {
        server: vec![
            baseline_entry("expected-failure", BaselineClassification::Controlplane),
            baseline_entry("fixed", BaselineClassification::Controlplane),
            baseline_entry("not-run", BaselineClassification::Controlplane),
        ],
    };

    let audit = audit_baseline(&parsed, &baseline);

    assert_eq!(audit.expected_failures, ["expected-failure"]);
    assert_eq!(audit.unexpected_failures, ["unexpected-warning"]);
    assert_eq!(audit.stale_entries, ["fixed"]);
    assert_eq!(audit.unobserved_entries, ["not-run"]);
    assert!(!audit.is_clean());
}

#[test]
fn scenario_outcomes_follow_official_failure_and_skip_semantics() {
    assert_eq!(
        result("success", [CheckStatus::Success, CheckStatus::Info]).outcome(),
        ScenarioOutcome::Compliant
    );
    assert_eq!(
        result("warning", [CheckStatus::Success, CheckStatus::Warning]).outcome(),
        ScenarioOutcome::NonCompliant
    );
    assert_eq!(
        result("failure", [CheckStatus::Failure]).outcome(),
        ScenarioOutcome::NonCompliant
    );
    assert_eq!(
        result("skipped", [CheckStatus::Skipped, CheckStatus::Info]).outcome(),
        ScenarioOutcome::NotApplicable
    );
    assert_eq!(
        result("info", [CheckStatus::Info]).outcome(),
        ScenarioOutcome::Ambiguous
    );
    assert_eq!(result("empty", []).outcome(), ScenarioOutcome::Ambiguous);
}

#[test]
fn explicit_missing_named_fixtures_are_not_implementation_failures() {
    let messages = [
        "Failed: MCP error -32601: Tool not found: test_simple_text",
        "Tool 'json_schema_2020_12_tool' not found. Available tools: echo",
        "Failed: MCP error -32002: Prompt not found: test_simple_prompt",
        "Failed: MCP error -32002: Resource not found: test://static-text",
        "Failed: MCP error -32603: Routing problem... wrong tool name",
        "Failed: MCP error -32603: Routing problem... wrong prompt name",
        "Failed: MCP error -32603: Routing problem... wrong resource name",
        "Failed: MCP error -32603: Routing problem... wrong completion reference",
    ];

    for (index, message) in messages.into_iter().enumerate() {
        let scenario = ConformanceScenarioResult {
            scenario: format!("fixture-{index}"),
            checks: vec![failing_check("fixture", message)],
            source: PathBuf::from("fixture/checks.json"),
        };
        assert_eq!(scenario.outcome(), ScenarioOutcome::FixtureFailure);
    }
}

#[test]
fn fresh_runs_reject_missing_official_fixtures() {
    let parsed = results([ConformanceScenarioResult {
        scenario: "tools-call-simple-text".to_owned(),
        checks: vec![failing_check(
            "fixture",
            "Failed: MCP error -32601: Tool not found: test_simple_text",
        )],
        source: PathBuf::from("tools-call-simple-text/checks.json"),
    }]);

    let error = validate_no_fixture_failures(&parsed)
        .expect_err("missing official fixtures must reject a fresh run")
        .to_string();

    assert!(error.contains("tools-call-simple-text"));
    assert!(error.contains("official fixture setup"));
}

#[test]
fn fresh_run_fixture_failures_are_listed_deterministically() {
    let fixture_result = |scenario: &str, message: &str| ConformanceScenarioResult {
        scenario: scenario.to_owned(),
        checks: vec![failing_check("fixture", message)],
        source: PathBuf::from(format!("{scenario}/checks.json")),
    };
    let parsed = results([
        fixture_result(
            "tools-call-simple-text",
            "Failed: MCP error -32601: Tool not found: test_simple_text",
        ),
        fixture_result(
            "prompts-get-simple",
            "Failed: MCP error -32002: Prompt not found: test_simple_prompt",
        ),
    ]);

    let error = validate_no_fixture_failures(&parsed)
        .expect_err("all missing official fixtures must be reported")
        .to_string();

    assert_eq!(
        error,
        "official fixture setup failed for conformance scenarios: prompts-get-simple, tools-call-simple-text"
    );
}

#[test]
fn fresh_runs_accept_successful_scenarios() {
    let parsed = results([result("tools-call-simple-text", [CheckStatus::Success])]);

    validate_no_fixture_failures(&parsed).expect("successful scenarios must be accepted");
}

#[test]
fn fresh_runs_accept_non_fixture_gateway_failures() {
    let parsed = results([ConformanceScenarioResult {
        scenario: "logging-set-level".to_owned(),
        checks: vec![failing_check(
            "gateway",
            "Failed: MCP error -32601: logging/setLevel",
        )],
        source: PathBuf::from("logging-set-level/checks.json"),
    }]);

    validate_no_fixture_failures(&parsed)
        .expect("gateway protocol failures remain conformance results");
}

#[test]
fn unrelated_and_mixed_failures_remain_noncompliant() {
    for message in [
        "Failed: MCP error -32601: logging/setLevel",
        "Expected HTTP 4xx for invalid Host/Origin headers, got 200",
        "Server did not send tool response after reconnect",
        "Failed: MCP error -32603: Internal error",
        "Tool not found: user_defined_tool",
    ] {
        let scenario = ConformanceScenarioResult {
            scenario: "implementation-failure".to_owned(),
            checks: vec![failing_check("implementation", message)],
            source: PathBuf::from("implementation/checks.json"),
        };
        assert_eq!(scenario.outcome(), ScenarioOutcome::NonCompliant);
    }

    let mixed = ConformanceScenarioResult {
        scenario: "mixed".to_owned(),
        checks: vec![
            failing_check(
                "fixture",
                "Failed: MCP error -32601: Tool not found: test_simple_text",
            ),
            failing_check("implementation", "Failed: MCP error -32603: Internal error"),
        ],
        source: PathBuf::from("mixed/checks.json"),
    };
    assert_eq!(mixed.outcome(), ScenarioOutcome::NonCompliant);
}

#[test]
fn paired_outcomes_cover_every_report_classification() {
    use ComparisonClassification as Class;
    use ScenarioOutcome as Outcome;

    let cases = [
        (
            Outcome::Compliant,
            Outcome::Compliant,
            false,
            Class::BothCompliant,
        ),
        (
            Outcome::Compliant,
            Outcome::NotApplicable,
            false,
            Class::ControlplaneCompliant,
        ),
        (
            Outcome::NotApplicable,
            Outcome::Compliant,
            false,
            Class::DataplaneCompliant,
        ),
        (
            Outcome::NonCompliant,
            Outcome::Compliant,
            false,
            Class::ControlplaneOnlyFailure,
        ),
        (
            Outcome::Compliant,
            Outcome::NonCompliant,
            false,
            Class::DataplaneOnlyFailure,
        ),
        (
            Outcome::NonCompliant,
            Outcome::NonCompliant,
            false,
            Class::SharedFailure,
        ),
        (
            Outcome::NonCompliant,
            Outcome::Compliant,
            true,
            Class::ExpectedFailure,
        ),
        (
            Outcome::FixtureFailure,
            Outcome::Compliant,
            false,
            Class::FixtureFailure,
        ),
        (
            Outcome::NotApplicable,
            Outcome::NotApplicable,
            false,
            Class::NotApplicable,
        ),
        (
            Outcome::Missing,
            Outcome::Compliant,
            false,
            Class::Ambiguous,
        ),
        (
            Outcome::Ambiguous,
            Outcome::Compliant,
            false,
            Class::Ambiguous,
        ),
    ];

    for (controlplane, dataplane, expected, classification) in cases {
        assert_eq!(
            classify_outcomes(controlplane, dataplane, expected),
            classification
        );
    }
}

#[test]
fn result_comparison_uses_independent_baselines_and_keeps_missing_results_ambiguous() {
    let controlplane = results([
        result("both-pass", [CheckStatus::Success]),
        result("expected-cp", [CheckStatus::Failure]),
        result("cp-only-failure", [CheckStatus::Failure]),
        result("missing-dataplane", [CheckStatus::Success]),
    ]);
    let dataplane = results([
        result("both-pass", [CheckStatus::Success]),
        result("expected-cp", [CheckStatus::Success]),
        result("cp-only-failure", [CheckStatus::Success]),
    ]);
    let controlplane_baseline = Baseline {
        server: vec![baseline_entry(
            "expected-cp",
            BaselineClassification::Controlplane,
        )],
    };
    let dataplane_baseline = Baseline { server: Vec::new() };

    let comparisons = compare_result_sets(
        &controlplane,
        &dataplane,
        &controlplane_baseline,
        &dataplane_baseline,
    );
    let by_name: BTreeMap<_, _> = comparisons
        .into_iter()
        .map(|comparison| (comparison.scenario.clone(), comparison))
        .collect();

    assert_eq!(
        by_name["both-pass"].classification,
        ComparisonClassification::BothCompliant
    );
    assert_eq!(
        by_name["expected-cp"].classification,
        ComparisonClassification::ExpectedFailure
    );
    assert_eq!(
        by_name["cp-only-failure"].classification,
        ComparisonClassification::ControlplaneOnlyFailure
    );
    assert_eq!(
        by_name["missing-dataplane"].classification,
        ComparisonClassification::Ambiguous
    );
}

#[test]
fn comparison_report_is_deterministic_sorted_complete_and_markdown_safe() {
    let report = ComparisonReport {
        spec_version: DEFAULT_MCP_SPEC_VERSION.to_owned(),
        suite: DEFAULT_CONFORMANCE_SUITE.to_owned(),
        scenarios: vec![
            ScenarioComparison {
                scenario: "zeta|scenario".to_owned(),
                controlplane: ScenarioOutcome::NonCompliant,
                dataplane: ScenarioOutcome::NonCompliant,
                classification: ComparisonClassification::SharedFailure,
                expected_by: BTreeSet::new(),
                spec_references: vec![SpecReference {
                    id: "raw|ref".to_owned(),
                    url: Some("javascript:alert(1)".to_owned()),
                    extensions: BTreeMap::new(),
                }],
            },
            ScenarioComparison {
                scenario: "alpha".to_owned(),
                controlplane: ScenarioOutcome::Compliant,
                dataplane: ScenarioOutcome::Compliant,
                classification: ComparisonClassification::BothCompliant,
                expected_by: BTreeSet::new(),
                spec_references: Vec::new(),
            },
        ],
    };

    let first = render_comparison_markdown(&report);
    let second = render_comparison_markdown(&report);

    assert_eq!(first, second);
    assert!(first.starts_with("# MCP Conformance Comparison\n"));
    assert!(first.contains(OFFICIAL_CONFORMANCE_PACKAGE));
    assert!(first.contains("| both compliant | 1 |"));
    assert!(first.contains("| shared failure | 1 |"));
    assert!(
        first.find("| alpha |").expect("alpha row should exist")
            < first
                .find("| zeta\\|scenario |")
                .expect("escaped zeta row should exist")
    );
    assert!(!first.contains("](javascript:"));
    assert!(first.contains("raw\\|ref"));
}

#[test]
fn comparison_report_writer_creates_parent_directories() {
    let report = ComparisonReport {
        spec_version: DEFAULT_MCP_SPEC_VERSION.to_owned(),
        suite: DEFAULT_CONFORMANCE_SUITE.to_owned(),
        scenarios: Vec::new(),
    };
    let directory = tempfile::tempdir().expect("temporary directory should be created");
    let path = directory.path().join("reports/comparison.md");

    write_comparison_report(&path, &report).expect("report should be written");

    assert_eq!(
        fs::read_to_string(path).expect("report should be readable"),
        render_comparison_markdown(&report)
    );
}
