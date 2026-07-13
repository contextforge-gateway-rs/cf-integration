//! Command-line argument model.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};

/// Default stable MCP revision exercised by compliance commands.
pub const DEFAULT_MCP_SPEC_VERSION: &str = "2025-11-25";

const RUN_TIME_ERROR: &str =
    "must be one or more positive integer+unit groups using ms, s, m, h, or d";

fn parse_positive_usize(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| String::from("must be an integer greater than zero"))?;
    if parsed == 0 {
        Err(String::from("must be an integer greater than zero"))
    } else {
        Ok(parsed)
    }
}

fn parse_positive_f64(value: &str) -> Result<f64, String> {
    let parsed = value
        .parse::<f64>()
        .map_err(|_| String::from("must be a finite number greater than zero"))?;
    if parsed.is_finite() && parsed > 0.0 {
        Ok(parsed)
    } else {
        Err(String::from("must be a finite number greater than zero"))
    }
}

fn parse_run_time(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut position = 0;

    if bytes.is_empty() {
        return Err(String::from(RUN_TIME_ERROR));
    }

    while position < bytes.len() {
        let number_start = position;
        while position < bytes.len() && bytes[position].is_ascii_digit() {
            position += 1;
        }
        if number_start == position {
            return Err(String::from(RUN_TIME_ERROR));
        }

        let amount = value[number_start..position]
            .parse::<u64>()
            .map_err(|_| String::from(RUN_TIME_ERROR))?;
        if amount == 0 {
            return Err(String::from(RUN_TIME_ERROR));
        }

        if bytes[position..].starts_with(b"ms") {
            position += 2;
        } else if matches!(bytes.get(position), Some(b's' | b'm' | b'h' | b'd')) {
            position += 1;
        } else {
            return Err(String::from(RUN_TIME_ERROR));
        }
    }

    Ok(value.to_owned())
}

/// Orchestrates control-plane and dataplane integration workflows.
#[derive(Debug, Clone, PartialEq, Parser)]
#[command(name = "cf-integration", arg_required_else_help = true)]
pub struct Cli {
    /// Workflow to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Top-level integration workflow.
#[derive(Debug, Clone, PartialEq, Subcommand)]
pub enum Command {
    /// Synchronize source checkouts.
    Sync,
    /// Manage Compose stacks.
    Stack(StackArgs),
    /// Print a gateway-compatible JWT.
    Token(TokenArgs),
    /// Run integration test workflows.
    Test(TestArgs),
    /// Run and report MCP compliance checks.
    Compliance(ComplianceArgs),
    /// Debug a live endpoint with the official MCP Inspector.
    #[command(
        long_about = "Debug a live endpoint with the official MCP Inspector. This is not a compliance gate."
    )]
    Inspect(InspectArgs),
}

/// Stack command selection.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct StackArgs {
    /// Stack operation to run.
    #[command(subcommand)]
    pub command: StackCommand,
}

/// Operation on one or more Compose stacks.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum StackCommand {
    /// Start one stack mode.
    Up(ModeArgs),
    /// Stop one or both stack modes.
    Down(ModeSelectionArgs),
    /// Stop one or both stack modes and remove volumes.
    Reset(ModeSelectionArgs),
    /// Show services for one stack mode.
    Status(ModeArgs),
    /// Follow logs for one stack mode.
    Logs(StackLogsArgs),
    /// Render the merged configuration for one stack mode.
    Config(ModeArgs),
}

/// A command targeting one stack mode.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct ModeArgs {
    /// Stack mode; defaults to CF_MCP_STACK_MODE, then dataplane.
    #[arg(long, value_enum)]
    pub mode: Option<CliStackMode>,
}

/// A command targeting one or both stack modes.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct ModeSelectionArgs {
    /// Stack mode; cleanup defaults to all.
    #[arg(long, value_enum)]
    pub mode: Option<ComplianceMode>,
}

/// Options for following stack logs.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct StackLogsArgs {
    /// Stack mode; defaults to CF_MCP_STACK_MODE, then dataplane.
    #[arg(long, value_enum)]
    pub mode: Option<CliStackMode>,

    /// Services whose logs to follow; all services when omitted.
    #[arg(value_name = "SERVICE")]
    pub services: Vec<OsString>,
}

/// A live stack topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliStackMode {
    /// Python control plane only.
    Controlplane,
    /// Python control plane routed through the Rust dataplane.
    Dataplane,
}

impl From<CliStackMode> for cf_integration_platform::StackMode {
    fn from(mode: CliStackMode) -> Self {
        match mode {
            CliStackMode::Controlplane => Self::Controlplane,
            CliStackMode::Dataplane => Self::Dataplane,
        }
    }
}

/// One or both stack topologies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ComplianceMode {
    /// Python control plane only.
    Controlplane,
    /// Python control plane routed through the Rust dataplane.
    Dataplane,
    /// Run controlplane and dataplane sequentially.
    All,
}

/// Token generation options.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct TokenArgs {
    /// Token privilege level.
    #[arg(long, value_enum, default_value = "scoped")]
    pub kind: TokenKind,

    /// Virtual server restriction for a scoped token.
    #[arg(long)]
    pub server_id: Option<String>,
}

/// Token privilege level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TokenKind {
    /// Minimum scopes needed by public MCP tests.
    Scoped,
    /// Platform-admin token for fixture setup.
    Admin,
}

/// Test command selection.
#[derive(Debug, Clone, PartialEq, Args)]
pub struct TestArgs {
    /// Test workflow to run.
    #[command(subcommand)]
    pub command: TestCommand,
}

/// Available integration test workflows.
#[derive(Debug, Clone, PartialEq, Subcommand)]
pub enum TestCommand {
    /// Probe authentication, initialization, discovery, and one safe tool call.
    Probe(ModeArgs),
    /// Run a load test.
    Load(LoadArgs),
    /// Run upstream live gateway tests.
    Live(LiveArgs),
    /// Run the integration test suite.
    Suite(SuiteArgs),
}

/// Load-test options.
#[derive(Debug, Clone, PartialEq, Args)]
pub struct LoadArgs {
    /// Stack mode; defaults to CF_MCP_STACK_MODE, then dataplane.
    #[arg(long, value_enum)]
    pub mode: Option<CliStackMode>,

    /// Load-test engine.
    #[arg(long, value_enum, default_value = "locust")]
    pub engine: CliLoadEngine,

    /// Use smoke-test settings.
    #[arg(long)]
    pub smoke: bool,

    /// Concurrent users; must be greater than zero.
    #[arg(long, value_parser = parse_positive_usize)]
    pub users: Option<usize>,

    /// Users spawned per second; must be finite and greater than zero.
    #[arg(long, value_parser = parse_positive_f64)]
    pub spawn_rate: Option<f64>,

    /// Duration using positive ms, s, m, h, or d groups, such as 1h30m.
    #[arg(long, value_parser = parse_run_time)]
    pub run_time: Option<String>,
}

/// Load-test implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliLoadEngine {
    /// Python Locust adapter.
    Locust,
    /// Native Rust Goose runner.
    Goose,
}

impl From<CliLoadEngine> for cf_integration_load::LoadEngine {
    fn from(engine: CliLoadEngine) -> Self {
        match engine {
            CliLoadEngine::Locust => Self::Locust,
            CliLoadEngine::Goose => Self::Goose,
        }
    }
}

/// Live-test options.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct LiveArgs {
    /// Stack mode; defaults to CF_MCP_STACK_MODE, then dataplane.
    #[arg(long, value_enum)]
    pub mode: Option<CliStackMode>,

    /// Upstream live-test group.
    #[arg(long, value_enum, default_value = "all")]
    pub group: LiveGroup,
}

/// Upstream live-test group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LiveGroup {
    /// MCP route tests.
    Mcp,
    /// Authorization tests.
    Rbac,
    /// Protocol-specific tests.
    Protocol,
    /// Every live group.
    All,
}

/// Integration-suite options.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct SuiteArgs {
    /// Stack mode; use all to exercise both topologies sequentially.
    #[arg(long, value_enum)]
    pub mode: Option<ComplianceMode>,

    /// Start a fresh stack before testing.
    #[arg(long)]
    pub start: bool,

    /// Load-test engine to include; repeat for multiple engines.
    #[arg(long, value_enum, action = ArgAction::Append)]
    pub load: Vec<CliLoadEngine>,

    /// Exclude plugin-dependent tests.
    #[arg(long)]
    pub exclude_plugins: bool,
}

/// Compliance command selection.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct ComplianceArgs {
    /// Compliance workflow to run.
    #[command(subcommand)]
    pub command: ComplianceCommand,
}

/// MCP compliance workflows.
#[derive(Debug, Clone, PartialEq, Eq, Subcommand)]
pub enum ComplianceCommand {
    /// Run the official MCP Conformance Test Framework only.
    Conformance(ConformanceArgs),
    /// Run Rust gateway-specific compliance tests only.
    Gateway(GatewayComplianceArgs),
    /// Run both layers and generate comparison reports.
    All(ComplianceAllArgs),
    /// Regenerate reports from existing result artifacts.
    Report(ComplianceReportArgs),
}

/// Options shared by live compliance commands.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct ComplianceCommonArgs {
    /// Stack mode; defaults to CF_MCP_STACK_MODE, or command-specific behavior.
    #[arg(long, value_enum)]
    pub mode: Option<ComplianceMode>,

    /// Start a fresh stack before testing each selected mode.
    #[arg(long)]
    pub start: bool,

    /// Existing virtual server ID; uses the configured/default fixture when omitted.
    #[arg(long)]
    pub server_id: Option<String>,

    /// Stable MCP specification revision to test.
    #[arg(long, default_value = DEFAULT_MCP_SPEC_VERSION)]
    pub spec_version: String,

    /// Result artifact root; defaults below CF_INTEGRATION_DIR.
    #[arg(long)]
    pub results_dir: Option<PathBuf>,
}

/// Official conformance options.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct ConformanceArgs {
    #[command(flatten)]
    pub common: ComplianceCommonArgs,

    /// Official scenario suite; all includes pending 2025-11-25 scenarios.
    #[arg(long, value_enum, default_value = "all")]
    pub suite: CliConformanceSuite,

    /// Rich expected-failure baseline; mode-specific default when omitted.
    #[arg(long)]
    pub baseline: Option<PathBuf>,
}

/// Rust gateway-specific compliance options.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct GatewayComplianceArgs {
    #[command(flatten)]
    pub common: ComplianceCommonArgs,
}

/// Combined compliance options.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct ComplianceAllArgs {
    #[command(flatten)]
    pub common: ComplianceCommonArgs,

    /// Official scenario suite to run before gateway-specific tests.
    #[arg(long, value_enum, default_value = "all")]
    pub suite: CliConformanceSuite,
}

/// Official server scenario suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CliConformanceSuite {
    /// Stable scenarios, excluding upstream pending scenarios.
    Active,
    /// Every scenario tagged for the selected revision.
    All,
}

impl From<CliConformanceSuite> for cf_integration_compliance::ConformanceSuite {
    fn from(suite: CliConformanceSuite) -> Self {
        match suite {
            CliConformanceSuite::Active => Self::Active,
            CliConformanceSuite::All => Self::All,
        }
    }
}

/// Report-only options.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct ComplianceReportArgs {
    /// Existing result artifact root.
    #[arg(long)]
    pub results_dir: Option<PathBuf>,

    /// Markdown report directory; defaults to the repository reports directory.
    #[arg(long)]
    pub output_dir: Option<PathBuf>,
}

/// Official Inspector options.
#[derive(Debug, Clone, PartialEq, Eq, Args)]
pub struct InspectArgs {
    /// Stack mode; defaults to CF_MCP_STACK_MODE, then dataplane.
    #[arg(long, value_enum)]
    pub mode: Option<CliStackMode>,

    /// Inspector method such as tools/list.
    #[arg(long, default_value = "tools/list")]
    pub method: String,

    /// Existing virtual server ID; uses the configured/default fixture when omitted.
    #[arg(long)]
    pub server_id: Option<String>,
}
