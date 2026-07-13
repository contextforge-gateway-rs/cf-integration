//! CLI-to-runtime action resolution.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use anyhow::{Result, bail};
use async_trait::async_trait;
use cf_integration_compliance::ConformanceSuite;
use cf_integration_load::{LoadEngine, LoadRequest};
use cf_integration_platform::StackMode;
use cf_integration_platform::config::Environment;

use crate::cli::{
    Cli, CliStackMode, Command, ComplianceCommand, ComplianceCommonArgs, ComplianceMode, LiveGroup,
    StackCommand, TestCommand, TokenKind,
};
use crate::error::AppFailure;

const STACK_MODE_ENV: &str = "CF_MCP_STACK_MODE";

/// Fully resolved application operation.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Sync,
    Stack(StackAction),
    Token {
        kind: TokenKind,
        server_id: Option<String>,
    },
    Test(TestAction),
    Compliance(ComplianceAction),
    Inspect {
        mode: StackMode,
        method: String,
        server_id: Option<String>,
    },
}

/// Fully resolved stack operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StackAction {
    Up(StackMode),
    Down(ComplianceMode),
    Reset(ComplianceMode),
    Status(StackMode),
    Logs {
        mode: StackMode,
        services: Vec<OsString>,
    },
    Config(StackMode),
}

/// Fully resolved load-test options.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedLoadArgs {
    pub mode: StackMode,
    pub request: LoadRequest,
}

/// Fully resolved test operation.
#[derive(Debug, Clone, PartialEq)]
pub enum TestAction {
    Probe(StackMode),
    Load(ResolvedLoadArgs),
    Live {
        mode: StackMode,
        group: LiveGroup,
    },
    Suite {
        mode: ComplianceMode,
        start: bool,
        load: Vec<LoadEngine>,
        exclude_plugins: bool,
    },
}

/// Shared, resolved live-compliance options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedComplianceCommon {
    pub mode: ComplianceMode,
    pub start: bool,
    pub server_id: Option<String>,
    pub spec_version: String,
    pub results_dir: Option<PathBuf>,
}

/// Fully resolved compliance operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComplianceAction {
    Conformance {
        common: ResolvedComplianceCommon,
        suite: ConformanceSuite,
        baseline: Option<PathBuf>,
    },
    Gateway {
        common: ResolvedComplianceCommon,
    },
    All {
        common: ResolvedComplianceCommon,
        suite: ConformanceSuite,
    },
    Report {
        results_dir: Option<PathBuf>,
        output_dir: Option<PathBuf>,
    },
}

/// Runtime boundary used after parsing and environment resolution.
#[async_trait(?Send)]
pub trait ActionExecutor {
    /// Executes one fully resolved operation.
    async fn execute(&mut self, action: Action) -> Result<(), AppFailure>;
}

/// Resolves a parsed CLI without starting child processes or mutating global state.
///
/// # Errors
///
/// Returns an error when a command needs `CF_MCP_STACK_MODE` and its value is
/// neither `controlplane` nor `dataplane`.
pub fn resolve_action(cli: Cli, environment: &Environment) -> Result<Action> {
    match cli.command {
        Command::Sync => Ok(Action::Sync),
        Command::Token(args) => {
            if args.kind == TokenKind::Admin && args.server_id.is_some() {
                bail!("--server-id is only valid with --kind scoped");
            }
            Ok(Action::Token {
                kind: args.kind,
                server_id: args.server_id,
            })
        }
        Command::Stack(args) => resolve_stack(args.command, environment).map(Action::Stack),
        Command::Test(args) => resolve_test(args.command, environment).map(Action::Test),
        Command::Compliance(args) => {
            resolve_compliance(args.command, environment).map(Action::Compliance)
        }
        Command::Inspect(args) => Ok(Action::Inspect {
            mode: resolve_single_mode(args.mode, environment)?,
            method: args.method,
            server_id: args.server_id,
        }),
    }
}

/// Resolves and executes a CLI operation.
///
/// # Errors
///
/// Returns mode-resolution or executor failures unchanged.
pub async fn dispatch<E: ActionExecutor + ?Sized>(
    cli: Cli,
    environment: &Environment,
    executor: &mut E,
) -> Result<(), AppFailure> {
    let action = resolve_action(cli, environment).map_err(AppFailure::from)?;
    executor.execute(action).await
}

fn resolve_stack(command: StackCommand, environment: &Environment) -> Result<StackAction> {
    match command {
        StackCommand::Up(args) => Ok(StackAction::Up(resolve_single_mode(
            args.mode,
            environment,
        )?)),
        StackCommand::Down(args) => Ok(StackAction::Down(args.mode.unwrap_or(ComplianceMode::All))),
        StackCommand::Reset(args) => {
            Ok(StackAction::Reset(args.mode.unwrap_or(ComplianceMode::All)))
        }
        StackCommand::Status(args) => Ok(StackAction::Status(resolve_single_mode(
            args.mode,
            environment,
        )?)),
        StackCommand::Logs(args) => Ok(StackAction::Logs {
            mode: resolve_single_mode(args.mode, environment)?,
            services: args.services,
        }),
        StackCommand::Config(args) => Ok(StackAction::Config(resolve_single_mode(
            args.mode,
            environment,
        )?)),
    }
}

fn resolve_test(command: TestCommand, environment: &Environment) -> Result<TestAction> {
    match command {
        TestCommand::Probe(args) => Ok(TestAction::Probe(resolve_single_mode(
            args.mode,
            environment,
        )?)),
        TestCommand::Load(args) => Ok(TestAction::Load(ResolvedLoadArgs {
            mode: resolve_single_mode(args.mode, environment)?,
            request: LoadRequest {
                engine: args.engine.into(),
                smoke: args.smoke,
                users: args.users,
                spawn_rate: args.spawn_rate,
                run_time: args.run_time,
            },
        })),
        TestCommand::Live(args) => Ok(TestAction::Live {
            mode: resolve_single_mode(args.mode, environment)?,
            group: args.group,
        }),
        TestCommand::Suite(args) => {
            let mode = resolve_multi_mode(args.mode, environment, ComplianceMode::Dataplane)?;
            if mode == ComplianceMode::All && !args.start {
                bail!("--mode all requires --start because the two stacks share host ports");
            }
            Ok(TestAction::Suite {
                mode,
                start: args.start,
                load: args.load.into_iter().map(Into::into).collect(),
                exclude_plugins: args.exclude_plugins,
            })
        }
    }
}

fn resolve_compliance(
    command: ComplianceCommand,
    environment: &Environment,
) -> Result<ComplianceAction> {
    match command {
        ComplianceCommand::Conformance(args) => {
            let mut common = args.common;
            if common.mode.is_none() {
                common.mode = Some(ComplianceMode::All);
            }
            Ok(ComplianceAction::Conformance {
                common: resolve_compliance_common(
                    common,
                    args.spec_version,
                    environment,
                    ComplianceMode::All,
                )?,
                suite: args.suite.into(),
                baseline: args.baseline,
            })
        }
        ComplianceCommand::Gateway(args) => Ok(ComplianceAction::Gateway {
            common: resolve_compliance_common(
                args.common,
                args.spec_version,
                environment,
                ComplianceMode::Dataplane,
            )?,
        }),
        ComplianceCommand::All(args) => {
            let mut common = args.common;
            if common.mode.is_none() {
                common.mode = Some(ComplianceMode::All);
            }
            Ok(ComplianceAction::All {
                common: resolve_compliance_common(
                    common,
                    args.spec_version,
                    environment,
                    ComplianceMode::All,
                )?,
                suite: args.suite.into(),
            })
        }
        ComplianceCommand::Report(args) => Ok(ComplianceAction::Report {
            results_dir: args.results_dir,
            output_dir: args.output_dir,
        }),
    }
}

fn resolve_compliance_common(
    args: ComplianceCommonArgs,
    spec_version: crate::cli::CliConformanceVersion,
    environment: &Environment,
    default: ComplianceMode,
) -> Result<ResolvedComplianceCommon> {
    Ok(ResolvedComplianceCommon {
        mode: resolve_multi_mode(args.mode, environment, default)?,
        start: args.start,
        server_id: args.server_id,
        spec_version: spec_version.spec_version().to_owned(),
        results_dir: args.results_dir,
    })
}

fn resolve_single_mode(
    explicit: Option<CliStackMode>,
    environment: &Environment,
) -> Result<StackMode> {
    if let Some(mode) = explicit {
        return Ok(mode.into());
    }
    Ok(environment_mode(environment)?.unwrap_or(StackMode::Dataplane))
}

fn resolve_multi_mode(
    explicit: Option<ComplianceMode>,
    environment: &Environment,
    default: ComplianceMode,
) -> Result<ComplianceMode> {
    if let Some(mode) = explicit {
        return Ok(mode);
    }
    Ok(
        environment_mode(environment)?.map_or(default, |mode| match mode {
            StackMode::Controlplane => ComplianceMode::Controlplane,
            StackMode::Dataplane => ComplianceMode::Dataplane,
        }),
    )
}

fn environment_mode(environment: &Environment) -> Result<Option<StackMode>> {
    let Some(value) = environment.get(OsStr::new(STACK_MODE_ENV)) else {
        return Ok(None);
    };
    match value.to_str() {
        Some("controlplane") => Ok(Some(StackMode::Controlplane)),
        Some("dataplane") => Ok(Some(StackMode::Dataplane)),
        _ => bail!(
            "invalid {STACK_MODE_ENV}; expected controlplane or dataplane (got {:?})",
            value
        ),
    }
}
