use std::ffi::OsString;

use cf_integration::cli::{
    Cli, CliConformanceLane, CliConformanceServerEra, CliConformanceVersion, CliLoadEngine,
    CliTopology, Command, ConformanceArgs, ConformanceCommand, DebugArgs, DebugCommand, LiveGroup,
    LoadArgs, StackArgs, StackCommand, TokenKind, TopologySelection,
};
use clap::{CommandFactory, Parser, error::ErrorKind};

const REMOVED_COMMANDS: &[&str] = &["sync", "token", "test", "compliance", "inspect"];

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

#[test]
fn command_tree_contains_only_distinct_public_workflows() {
    assert_eq!(
        subcommands(&[]),
        ["stack", "probe", "load", "live", "conformance", "debug"]
    );
    assert_eq!(
        subcommands(&["stack"]),
        ["up", "down", "status", "logs", "config"]
    );
    assert_eq!(subcommands(&["conformance"]), ["run", "report"]);
    assert_eq!(subcommands(&["debug"]), ["inspect", "token"]);
}

#[test]
fn obsolete_root_commands_and_combined_workflows_are_rejected() {
    for command in REMOVED_COMMANDS {
        rejected(&["cf-integration", command]);
    }
    rejected(&["cf-integration", "stack", "reset"]);
    rejected(&["cf-integration", "conformance", "all"]);
    rejected(&["cf-integration", "conformance", "gateway"]);
}

#[test]
fn stack_up_and_down_make_destructive_behavior_explicit() {
    let Command::Stack(StackArgs {
        command: StackCommand::Up(up),
    }) = parse(&[
        "cf-integration",
        "stack",
        "up",
        "--topology",
        "dataplane",
        "--fresh",
    ])
    .command
    else {
        panic!("expected stack up")
    };
    assert_eq!(up.topology, Some(CliTopology::Dataplane));
    assert!(up.fresh);

    let Command::Stack(StackArgs {
        command: StackCommand::Down(down),
    }) = parse(&[
        "cf-integration",
        "stack",
        "down",
        "--topology",
        "all",
        "--volumes",
    ])
    .command
    else {
        panic!("expected stack down")
    };
    assert_eq!(down.topology, Some(TopologySelection::All));
    assert!(down.volumes);
}

#[test]
fn stack_logs_preserve_service_arguments() {
    let Command::Stack(StackArgs {
        command: StackCommand::Logs(args),
    }) = parse(&[
        "cf-integration",
        "stack",
        "logs",
        "--topology",
        "controlplane",
        "gateway",
        "worker",
    ])
    .command
    else {
        panic!("expected stack logs")
    };
    assert_eq!(args.topology, Some(CliTopology::Controlplane));
    assert_eq!(
        args.services,
        [OsString::from("gateway"), OsString::from("worker")]
    );
}

#[test]
fn load_keeps_locust_and_goose_with_validated_settings() {
    for (engine, expected) in [
        ("locust", CliLoadEngine::Locust),
        ("goose", CliLoadEngine::Goose),
    ] {
        let Command::Load(LoadArgs {
            engine: actual,
            users,
            spawn_rate,
            run_time,
            ..
        }) = parse(&[
            "cf-integration",
            "load",
            "--engine",
            engine,
            "--users",
            "2",
            "--spawn-rate",
            "0.5",
            "--run-time",
            "1m30s",
        ])
        .command
        else {
            panic!("expected load")
        };
        assert_eq!(actual, expected);
        assert_eq!(users, Some(2));
        assert_eq!(spawn_rate, Some(0.5));
        assert_eq!(run_time.as_deref(), Some("1m30s"));
    }
    rejected(&["cf-integration", "load", "--users", "0"]);
    rejected(&["cf-integration", "load", "--run-time", "zero"]);
}

#[test]
fn live_defaults_to_all_and_accepts_the_main_harness_groups() {
    let Command::Live(defaults) = parse(&["cf-integration", "live"]).command else {
        panic!("expected live workflow")
    };
    assert_eq!(defaults.topology, None);
    assert_eq!(defaults.group, LiveGroup::All);

    for (name, expected) in [
        ("mcp", LiveGroup::Mcp),
        ("rbac", LiveGroup::Rbac),
        ("protocol", LiveGroup::Protocol),
        ("all", LiveGroup::All),
    ] {
        let Command::Live(args) = parse(&[
            "cf-integration",
            "live",
            "--topology",
            "dataplane",
            "--group",
            name,
        ])
        .command
        else {
            panic!("expected live workflow")
        };
        assert_eq!(args.topology, Some(CliTopology::Dataplane));
        assert_eq!(args.group, expected);
    }
}

#[test]
fn conformance_defaults_to_all_lanes_and_july_revision_at_resolution_time() {
    let Command::Conformance(ConformanceArgs {
        command: ConformanceCommand::Run(args),
    }) = parse(&["cf-integration", "conformance", "run"]).command
    else {
        panic!("expected conformance run")
    };
    assert!(args.lane.is_empty());
    assert_eq!(args.spec_version, CliConformanceVersion::July2026);
    assert_eq!(args.server_era, CliConformanceServerEra::Dual);
    assert!(args.results_dir.is_none());
}

#[test]
fn conformance_accepts_repeatable_exact_lanes_and_supported_revisions() {
    let Command::Conformance(ConformanceArgs {
        command: ConformanceCommand::Run(args),
    }) = parse(&[
        "cf-integration",
        "conformance",
        "run",
        "--lane",
        "fixture-direct",
        "--lane",
        "dataplane",
        "--client-version",
        "2025-11-25",
        "--server-era",
        "legacy",
    ])
    .command
    else {
        panic!("expected conformance run")
    };
    assert_eq!(
        args.lane,
        [
            CliConformanceLane::FixtureDirect,
            CliConformanceLane::Dataplane
        ]
    );
    assert_eq!(args.spec_version, CliConformanceVersion::November2025);
    assert_eq!(args.server_era, CliConformanceServerEra::Legacy);
    let Command::Conformance(ConformanceArgs {
        command: ConformanceCommand::Run(compatibility_args),
    }) = parse(&[
        "cf-integration",
        "conformance",
        "run",
        "--spec-version",
        "2025-11-25",
    ])
    .command
    else {
        panic!("expected conformance run")
    };
    assert_eq!(
        compatibility_args.spec_version,
        CliConformanceVersion::November2025
    );
    assert_eq!(compatibility_args.server_era, CliConformanceServerEra::Dual);
    rejected(&["cf-integration", "conformance", "run", "--suite", "active"]);
    rejected(&[
        "cf-integration",
        "conformance",
        "run",
        "--baseline",
        "known.yml",
    ]);
}

#[test]
fn debug_token_requires_an_explicit_privilege_kind() {
    let error = Cli::try_parse_from(["cf-integration", "debug", "token"])
        .expect_err("token kind should be explicit");
    assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);

    let Command::Debug(DebugArgs {
        command: DebugCommand::Token(args),
    }) = parse(&[
        "cf-integration",
        "debug",
        "token",
        "--kind",
        "scoped",
        "--server-id",
        "server-1",
    ])
    .command
    else {
        panic!("expected debug token")
    };
    assert_eq!(args.kind, TokenKind::Scoped);
    assert_eq!(args.server_id.as_deref(), Some("server-1"));
}

#[test]
fn help_and_version_style_flags_reject_unexpected_positionals() {
    rejected(&["cf-integration", "probe", "unexpected"]);
    let error = Cli::try_parse_from(["cf-integration"])
        .expect_err("root without a workflow should show help");
    assert_eq!(
        error.kind(),
        ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    );
}
