use std::ffi::OsString;
use std::path::PathBuf;

use cf_integration::cli::{
    Cli, CliConformanceSuite, CliConformanceVersion, CliLoadEngine, CliStackMode, Command,
    ComplianceAllArgs, ComplianceArgs, ComplianceCommand, ComplianceMode, ComplianceReportArgs,
    InspectArgs, LiveGroup, LoadArgs, ModeArgs, ModeSelectionArgs, StackArgs, StackCommand,
    StackLogsArgs, TestArgs, TestCommand, TokenArgs, TokenKind,
};
use clap::{CommandFactory, Parser, error::ErrorKind};

const REMOVED_COMMANDS: &[&str] = &[
    "checkout",
    "up",
    "down",
    "reset",
    "ps",
    "logs",
    "config",
    "probe",
    "locust",
    "smoke",
    "live-mcp",
    "test-all",
    "controlplane-up",
    "controlplane-test-all",
];

fn parse(args: &[&str]) -> Cli {
    Cli::try_parse_from(args.iter().copied()).expect("command should parse")
}

fn rejected(args: &[&str]) {
    assert!(
        Cli::try_parse_from(args.iter().copied()).is_err(),
        "command unexpectedly parsed: {args:?}"
    );
}

fn command_at(path: &[&str]) -> clap::Command {
    let mut command = Cli::command();
    for name in path {
        command = command
            .find_subcommand(name)
            .cloned()
            .expect("help path should name a public command");
    }
    command
}

fn subcommands(path: &[&str]) -> Vec<String> {
    command_at(path)
        .get_subcommands()
        .filter(|command| command.get_name() != "help")
        .map(|command| command.get_name().to_owned())
        .collect()
}

fn values(path: &[&str], id: &str) -> Vec<String> {
    command_at(path)
        .get_arguments()
        .find(|argument| argument.get_id().as_str() == id)
        .expect("argument should exist")
        .get_possible_values()
        .into_iter()
        .map(|value| value.get_name().to_owned())
        .collect()
}

fn load(options: &[&str]) -> LoadArgs {
    let mut argv = vec!["cf-integration", "test", "load"];
    argv.extend_from_slice(options);
    let Cli {
        command: Command::Test(TestArgs {
            command: TestCommand::Load(args),
        }),
    } = parse(&argv)
    else {
        panic!("expected test load")
    };
    args
}

#[test]
fn root_and_nested_command_names_are_coherent() {
    assert_eq!(
        subcommands(&[]),
        ["sync", "stack", "token", "test", "compliance", "inspect"]
    );
    assert_eq!(
        subcommands(&["stack"]),
        ["up", "down", "reset", "status", "logs", "config"]
    );
    assert_eq!(subcommands(&["test"]), ["probe", "load", "live", "suite"]);
    assert_eq!(
        subcommands(&["compliance"]),
        ["conformance", "gateway", "all", "report"]
    );
}

#[test]
fn sync_and_token_parse_with_safe_defaults() {
    assert_eq!(parse(&["cf-integration", "sync"]).command, Command::Sync);
    assert_eq!(
        parse(&["cf-integration", "token"]),
        Cli {
            command: Command::Token(TokenArgs {
                kind: TokenKind::Scoped,
                server_id: None,
            }),
        }
    );
    assert_eq!(
        parse(&[
            "cf-integration",
            "token",
            "--kind",
            "scoped",
            "--server-id",
            "server-1",
        ]),
        Cli {
            command: Command::Token(TokenArgs {
                kind: TokenKind::Scoped,
                server_id: Some("server-1".to_owned()),
            }),
        }
    );
}

#[test]
fn stack_operations_use_controlplane_and_dataplane_names() {
    for (name, command) in [
        (
            "up",
            StackCommand::Up(ModeArgs {
                mode: Some(CliStackMode::Dataplane),
            }),
        ),
        (
            "status",
            StackCommand::Status(ModeArgs {
                mode: Some(CliStackMode::Dataplane),
            }),
        ),
        (
            "config",
            StackCommand::Config(ModeArgs {
                mode: Some(CliStackMode::Dataplane),
            }),
        ),
    ] {
        assert_eq!(
            parse(&["cf-integration", "stack", name, "--mode", "dataplane"]),
            Cli {
                command: Command::Stack(StackArgs { command }),
            }
        );
    }
    for name in ["down", "reset"] {
        let parsed = parse(&["cf-integration", "stack", name, "--mode", "all"]);
        let expected = ModeSelectionArgs {
            mode: Some(ComplianceMode::All),
        };
        let command = match name {
            "down" => StackCommand::Down(expected),
            "reset" => StackCommand::Reset(expected),
            _ => unreachable!(),
        };
        assert_eq!(parsed.command, Command::Stack(StackArgs { command }));
    }
}

#[test]
fn omitted_modes_are_resolved_from_environment_by_dispatch() {
    let Command::Stack(StackArgs {
        command: StackCommand::Up(args),
    }) = parse(&["cf-integration", "stack", "up"]).command
    else {
        panic!("expected stack up")
    };
    assert_eq!(args.mode, None);

    let Command::Compliance(ComplianceArgs {
        command: ComplianceCommand::Conformance(args),
    }) = parse(&["cf-integration", "compliance", "conformance"]).command
    else {
        panic!("expected conformance")
    };
    assert_eq!(args.common.mode, None);
}

#[test]
fn stack_logs_preserve_service_arguments() {
    assert_eq!(
        parse(&[
            "cf-integration",
            "stack",
            "logs",
            "--mode",
            "controlplane",
            "gateway",
            "worker",
        ]),
        Cli {
            command: Command::Stack(StackArgs {
                command: StackCommand::Logs(StackLogsArgs {
                    mode: Some(CliStackMode::Controlplane),
                    services: vec![OsString::from("gateway"), OsString::from("worker")],
                }),
            }),
        }
    );
}

#[cfg(unix)]
#[test]
fn stack_logs_preserve_non_utf8_service_arguments() {
    use std::os::unix::ffi::OsStringExt;

    let service = OsString::from_vec(vec![b'a', b'p', b'i', 0x80]);
    let parsed = Cli::try_parse_from([
        OsString::from("cf-integration"),
        OsString::from("stack"),
        OsString::from("logs"),
        service.clone(),
    ])
    .expect("service should parse");
    let Command::Stack(StackArgs {
        command: StackCommand::Logs(args),
    }) = parsed.command
    else {
        panic!("expected stack logs")
    };
    assert_eq!(args.services, [service]);
}

#[test]
fn probe_live_and_suite_share_stack_modes() {
    assert_eq!(
        parse(&["cf-integration", "test", "probe", "--mode", "controlplane",]).command,
        Command::Test(TestArgs {
            command: TestCommand::Probe(ModeArgs {
                mode: Some(CliStackMode::Controlplane),
            }),
        })
    );

    let Command::Test(TestArgs {
        command: TestCommand::Live(args),
    }) = parse(&[
        "cf-integration",
        "test",
        "live",
        "--mode",
        "dataplane",
        "--group",
        "rbac",
    ])
    .command
    else {
        panic!("expected live test")
    };
    assert_eq!(args.mode, Some(CliStackMode::Dataplane));
    assert_eq!(args.group, LiveGroup::Rbac);

    let Command::Test(TestArgs {
        command: TestCommand::Suite(args),
    }) = parse(&[
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
    ])
    .command
    else {
        panic!("expected test suite")
    };
    assert_eq!(args.mode, Some(ComplianceMode::All));
    assert!(args.start);
    assert_eq!(args.load, [CliLoadEngine::Locust, CliLoadEngine::Goose]);
    assert!(args.exclude_plugins);
}

#[test]
fn load_defaults_and_overrides_are_typed_and_validated() {
    assert_eq!(
        load(&[]),
        LoadArgs {
            mode: None,
            engine: CliLoadEngine::Locust,
            smoke: false,
            users: None,
            spawn_rate: None,
            run_time: None,
        }
    );
    assert_eq!(
        load(&[
            "--mode",
            "dataplane",
            "--engine",
            "goose",
            "--smoke",
            "--users",
            "25",
            "--spawn-rate",
            "2.5",
            "--run-time",
            "1m30s",
        ]),
        LoadArgs {
            mode: Some(CliStackMode::Dataplane),
            engine: CliLoadEngine::Goose,
            smoke: true,
            users: Some(25),
            spawn_rate: Some(2.5),
            run_time: Some("1m30s".to_owned()),
        }
    );

    for args in [
        &["--users", "0"][..],
        &["--spawn-rate", "NaN"][..],
        &["--spawn-rate", "0"][..],
        &["--run-time", "0s"][..],
        &["--run-time", "1.5s"][..],
        &["--engine", "k6"][..],
    ] {
        let mut argv = vec!["cf-integration", "test", "load"];
        argv.extend_from_slice(args);
        rejected(&argv);
    }
}

#[test]
fn compliance_commands_parse_reproducible_options() {
    let Command::Compliance(ComplianceArgs {
        command: ComplianceCommand::Conformance(args),
    }) = parse(&[
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
        "all",
        "--baseline",
        "baseline.yml",
        "--results-dir",
        "results",
    ])
    .command
    else {
        panic!("expected conformance")
    };
    assert_eq!(args.common.mode, Some(ComplianceMode::Controlplane));
    assert!(args.common.start);
    assert_eq!(args.common.server_id.as_deref(), Some("server-1"));
    assert_eq!(args.spec_version, CliConformanceVersion::November2025);
    assert_eq!(args.suite, CliConformanceSuite::All);
    assert_eq!(args.baseline, Some(PathBuf::from("baseline.yml")));
    assert_eq!(args.common.results_dir, Some(PathBuf::from("results")));

    let Command::Compliance(ComplianceArgs {
        command:
            ComplianceCommand::All(ComplianceAllArgs {
                common,
                spec_version,
                suite,
            }),
    }) = parse(&[
        "cf-integration",
        "compliance",
        "all",
        "--mode",
        "all",
        "--suite",
        "active",
    ])
    .command
    else {
        panic!("expected all compliance")
    };
    assert_eq!(common.mode, Some(ComplianceMode::All));
    assert_eq!(spec_version, CliConformanceVersion::July2026);
    assert_eq!(suite, CliConformanceSuite::Active);

    for unsupported in ["draft", "2099-01-01"] {
        rejected(&[
            "cf-integration",
            "compliance",
            "conformance",
            "--spec-version",
            unsupported,
        ]);
    }

    let Command::Compliance(ComplianceArgs {
        command:
            ComplianceCommand::Report(ComplianceReportArgs {
                results_dir,
                output_dir,
            }),
    }) = parse(&[
        "cf-integration",
        "compliance",
        "report",
        "--results-dir",
        "in",
        "--output-dir",
        "out",
    ])
    .command
    else {
        panic!("expected report")
    };
    assert_eq!(results_dir, Some(PathBuf::from("in")));
    assert_eq!(output_dir, Some(PathBuf::from("out")));
}

#[test]
fn inspector_is_explicitly_a_debug_command() {
    assert_eq!(
        parse(&[
            "cf-integration",
            "inspect",
            "--mode",
            "dataplane",
            "--method",
            "tools/list",
            "--server-id",
            "server-1",
        ]),
        Cli {
            command: Command::Inspect(InspectArgs {
                mode: Some(CliStackMode::Dataplane),
                method: "tools/list".to_owned(),
                server_id: Some("server-1".to_owned()),
            }),
        }
    );
    assert!(
        command_at(&["inspect"])
            .get_long_about()
            .map(ToString::to_string)
            .is_some_and(|text| {
                let text = text.to_ascii_lowercase();
                text.contains("debug") && text.contains("not a compliance gate")
            })
    );
}

#[test]
fn help_lists_exact_enum_values_and_default_spec() {
    assert_eq!(
        values(&["stack", "up"], "mode"),
        ["controlplane", "dataplane"]
    );
    assert_eq!(
        values(&["stack", "down"], "mode"),
        ["controlplane", "dataplane", "all"]
    );
    assert_eq!(
        values(&["compliance", "conformance"], "suite"),
        ["active", "all"]
    );
    let conformance = command_at(&["compliance", "conformance"]);
    let spec = conformance
        .get_arguments()
        .find(|arg| arg.get_id().as_str() == "spec_version")
        .and_then(|arg| arg.get_default_values().first())
        .expect("spec version default");
    assert_eq!(spec, "2026-07-28");
    assert_eq!(
        values(&["compliance", "conformance"], "spec_version"),
        ["2025-06-18", "2025-11-25", "2026-07-28"]
    );
    let gateway = command_at(&["compliance", "gateway"]);
    let gateway_spec = gateway
        .get_arguments()
        .find(|arg| arg.get_id().as_str() == "spec_version")
        .and_then(|arg| arg.get_default_values().first())
        .expect("gateway spec version default");
    assert_eq!(gateway_spec, "2025-11-25");

    for path in [
        &["compliance", "conformance"][..],
        &["compliance", "gateway"][..],
        &["compliance", "all"][..],
        &["inspect"][..],
    ] {
        let server_id_help = command_at(path)
            .get_arguments()
            .find(|argument| argument.get_id().as_str() == "server_id")
            .and_then(|argument| argument.get_help())
            .expect("server-id should have help")
            .to_string();
        assert!(server_id_help.contains("configured/default fixture"));
        assert!(!server_id_help.contains("discovered"));
    }
}

#[test]
fn missing_groups_show_help() {
    for args in [
        &["cf-integration"][..],
        &["cf-integration", "stack"][..],
        &["cf-integration", "test"][..],
        &["cf-integration", "compliance"][..],
    ] {
        let error =
            Cli::try_parse_from(args.iter().copied()).expect_err("must reject missing leaf");
        assert_eq!(
            error.kind(),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }
}

#[test]
fn old_ambiguous_names_and_removed_controlplane_tree_are_rejected() {
    for command in REMOVED_COMMANDS {
        rejected(&["cf-integration", command]);
    }
    rejected(&["cf-integration", "stack", "ps"]);
    rejected(&["cf-integration", "stack", "up", "--mode", "integration"]);
    rejected(&["cf-integration", "test", "control-plane", "core"]);
    rejected(&["cf-integration", "stack", "up", "--mode", "all"]);
}
