use std::ffi::OsString;
use std::path::PathBuf;

use cf_integration::app::{
    Action, ComplianceAction, ResolvedComplianceCommon, ResolvedLoadArgs, StackAction, TestAction,
    resolve_action,
};
use cf_integration::cli::{Cli, ComplianceMode, LiveGroup, LoadEngine, TokenKind};
use cf_integration_compliance::ConformanceSuite;
use cf_integration_platform::StackMode;
use cf_integration_platform::config::Environment;
use clap::Parser;

fn action(arguments: &[&str], environment: &[(&str, &str)]) -> Action {
    let cli = Cli::try_parse_from(arguments.iter().copied()).expect("CLI should parse");
    let environment = environment
        .iter()
        .map(|(key, value)| (OsString::from(key), OsString::from(value)))
        .collect::<Environment>();
    resolve_action(cli, &environment).expect("action should resolve")
}

fn common(mode: ComplianceMode) -> ResolvedComplianceCommon {
    ResolvedComplianceCommon {
        mode,
        start: false,
        server_id: None,
        spec_version: "2025-11-25".to_owned(),
        results_dir: None,
    }
}

#[test]
fn resolves_sync_and_token_actions() {
    assert_eq!(action(&["cf-integration", "sync"], &[]), Action::Sync);
    assert_eq!(
        action(
            &[
                "cf-integration",
                "token",
                "--kind",
                "scoped",
                "--server-id",
                "server-1",
            ],
            &[],
        ),
        Action::Token {
            kind: TokenKind::Scoped,
            server_id: Some("server-1".to_owned()),
        }
    );
}

#[test]
fn admin_token_rejects_a_silently_ignored_server_id() {
    let cli = Cli::try_parse_from([
        "cf-integration",
        "token",
        "--kind",
        "admin",
        "--server-id",
        "server-1",
    ])
    .expect("CLI syntax should parse before semantic validation");

    let error = resolve_action(cli, &Environment::new())
        .expect_err("admin token server restriction must not be silently discarded");

    assert!(error.to_string().contains("only valid with --kind scoped"));
}

#[test]
fn stack_running_actions_use_cli_then_environment_then_dataplane() {
    assert_eq!(
        action(&["cf-integration", "stack", "up"], &[]),
        Action::Stack(StackAction::Up(StackMode::Dataplane))
    );
    assert_eq!(
        action(
            &["cf-integration", "stack", "up"],
            &[("CF_MCP_STACK_MODE", "controlplane")],
        ),
        Action::Stack(StackAction::Up(StackMode::Controlplane))
    );
    assert_eq!(
        action(
            &["cf-integration", "stack", "status", "--mode", "dataplane",],
            &[("CF_MCP_STACK_MODE", "invalid")],
        ),
        Action::Stack(StackAction::Status(StackMode::Dataplane))
    );
    assert_eq!(
        action(
            &[
                "cf-integration",
                "stack",
                "logs",
                "--mode",
                "controlplane",
                "gateway",
            ],
            &[],
        ),
        Action::Stack(StackAction::Logs {
            mode: StackMode::Controlplane,
            services: vec![OsString::from("gateway")],
        })
    );
    assert_eq!(
        action(&["cf-integration", "stack", "config"], &[]),
        Action::Stack(StackAction::Config(StackMode::Dataplane))
    );
}

#[test]
fn cleanup_defaults_to_both_modes() {
    assert_eq!(
        action(&["cf-integration", "stack", "down"], &[]),
        Action::Stack(StackAction::Down(ComplianceMode::All))
    );
    assert_eq!(
        action(
            &["cf-integration", "stack", "reset", "--mode", "controlplane",],
            &[],
        ),
        Action::Stack(StackAction::Reset(ComplianceMode::Controlplane))
    );
}

#[test]
fn test_actions_have_fully_resolved_modes_and_options() {
    assert_eq!(
        action(&["cf-integration", "test", "probe"], &[]),
        Action::Test(TestAction::Probe(StackMode::Dataplane))
    );
    assert_eq!(
        action(
            &[
                "cf-integration",
                "test",
                "load",
                "--mode",
                "controlplane",
                "--engine",
                "goose",
                "--smoke",
                "--users",
                "2",
                "--spawn-rate",
                "0.5",
                "--run-time",
                "10s",
            ],
            &[],
        ),
        Action::Test(TestAction::Load(ResolvedLoadArgs {
            mode: StackMode::Controlplane,
            engine: LoadEngine::Goose,
            smoke: true,
            users: Some(2),
            spawn_rate: Some(0.5),
            run_time: Some("10s".to_owned()),
        }))
    );
    assert_eq!(
        action(
            &["cf-integration", "test", "live", "--group", "protocol",],
            &[("CF_MCP_STACK_MODE", "controlplane")],
        ),
        Action::Test(TestAction::Live {
            mode: StackMode::Controlplane,
            group: LiveGroup::Protocol,
        })
    );
    assert_eq!(
        action(
            &[
                "cf-integration",
                "test",
                "suite",
                "--mode",
                "all",
                "--start",
                "--load",
                "locust",
                "--load",
                "goose",
                "--exclude-plugins",
            ],
            &[],
        ),
        Action::Test(TestAction::Suite {
            mode: ComplianceMode::All,
            start: true,
            load: vec![LoadEngine::Locust, LoadEngine::Goose],
            exclude_plugins: true,
        })
    );
}

#[test]
fn multi_mode_suite_requires_stack_ownership() {
    let cli = Cli::try_parse_from(["cf-integration", "test", "suite", "--mode", "all"])
        .expect("CLI syntax should parse before semantic validation");
    let error = resolve_action(cli, &Environment::new())
        .expect_err("two port-sharing modes cannot target one existing stack");
    assert!(error.to_string().contains("--mode all requires --start"));
}

#[test]
fn compliance_actions_preserve_reproducibility_inputs() {
    assert_eq!(
        action(
            &[
                "cf-integration",
                "compliance",
                "conformance",
                "--mode",
                "controlplane",
                "--start",
                "--server-id",
                "server-1",
                "--spec-version",
                "2025-11-25",
                "--suite",
                "active",
                "--baseline",
                "baseline.yml",
                "--results-dir",
                "results",
            ],
            &[],
        ),
        Action::Compliance(ComplianceAction::Conformance {
            common: ResolvedComplianceCommon {
                mode: ComplianceMode::Controlplane,
                start: true,
                server_id: Some("server-1".to_owned()),
                spec_version: "2025-11-25".to_owned(),
                results_dir: Some(PathBuf::from("results")),
            },
            suite: ConformanceSuite::Active,
            baseline: Some(PathBuf::from("baseline.yml")),
        })
    );
    assert_eq!(
        action(&["cf-integration", "compliance", "gateway"], &[]),
        Action::Compliance(ComplianceAction::Gateway {
            common: common(ComplianceMode::Dataplane),
        })
    );
    assert_eq!(
        action(&["cf-integration", "compliance", "all"], &[]),
        Action::Compliance(ComplianceAction::All {
            common: common(ComplianceMode::All),
            suite: ConformanceSuite::All,
        })
    );
    assert_eq!(
        action(
            &[
                "cf-integration",
                "compliance",
                "report",
                "--results-dir",
                "results",
                "--output-dir",
                "reports",
            ],
            &[],
        ),
        Action::Compliance(ComplianceAction::Report {
            results_dir: Some(PathBuf::from("results")),
            output_dir: Some(PathBuf::from("reports")),
        })
    );
}

#[test]
fn inspector_resolves_one_mode_and_stays_separate_from_compliance() {
    assert_eq!(
        action(
            &[
                "cf-integration",
                "inspect",
                "--method",
                "tools/list",
                "--server-id",
                "server-1",
            ],
            &[("CF_MCP_STACK_MODE", "controlplane")],
        ),
        Action::Inspect {
            mode: StackMode::Controlplane,
            method: "tools/list".to_owned(),
            server_id: Some("server-1".to_owned()),
        }
    );
    assert_eq!(
        action(
            &["cf-integration", "compliance", "all", "--start"],
            &[("CF_MCP_STACK_MODE", "controlplane")],
        ),
        Action::Compliance(ComplianceAction::All {
            common: ResolvedComplianceCommon {
                mode: ComplianceMode::All,
                start: true,
                server_id: None,
                spec_version: "2025-11-25".to_owned(),
                results_dir: None,
            },
            suite: ConformanceSuite::All,
        })
    );
    assert_eq!(
        action(
            &["cf-integration", "compliance", "all", "--mode", "dataplane",],
            &[("CF_MCP_STACK_MODE", "controlplane")],
        ),
        Action::Compliance(ComplianceAction::All {
            common: common(ComplianceMode::Dataplane),
            suite: ConformanceSuite::All,
        })
    );
}

#[test]
fn invalid_stack_mode_environment_fails_only_when_it_is_needed() {
    let environment = Environment::from([(
        OsString::from("CF_MCP_STACK_MODE"),
        OsString::from("integration"),
    )]);
    let cli = Cli::try_parse_from(["cf-integration", "test", "probe"]).expect("CLI should parse");
    let error = resolve_action(cli, &environment).expect_err("invalid mode must fail");
    assert!(error.to_string().contains("CF_MCP_STACK_MODE"));
    assert!(error.to_string().contains("controlplane or dataplane"));

    let cli = Cli::try_parse_from(["cf-integration", "test", "probe", "--mode", "dataplane"])
        .expect("CLI override should parse");
    assert!(resolve_action(cli, &environment).is_ok());
}
