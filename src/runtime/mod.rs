//! Operating-system-backed execution of resolved CLI actions.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, anyhow};
use cf_integration_compliance::conformance::{
    ComparisonFixtureTrust, ComparisonReport, ConformanceFixtureMetadata, ConformanceResults,
    ConformanceRunMetadata, ConformanceServerEra, ConformanceTarget,
    compare_result_sets_with_fixture_trust, expected_server_scenarios, is_trusted_official_fixture,
    load_server_results, official_server_command, validate_server_scenario_set,
    write_comparison_report,
};
use cf_integration_compliance::conformance_fixture::{
    ConformanceFixtureClient, OFFICIAL_CONFORMANCE_BACKEND_URL, OFFICIAL_CONFORMANCE_REPOSITORY,
    OFFICIAL_CONFORMANCE_REVISION, OFFICIAL_CONFORMANCE_SERVER_ID, OFFICIAL_CONFORMANCE_SERVICE,
};
use cf_integration_load::{
    GooseLoadConfig, LoadEngine, LoadSettings, LocustCommand, audit_locust_reports,
};
use cf_integration_mcp::GatewayTopology;
use cf_integration_mcp::auth_proxy::AuthProxy;
use cf_integration_mcp::gateway::GatewayClient;
use cf_integration_mcp::http_transport::ReqwestProbeTransport;
use cf_integration_mcp::mcp::{ACCEPT as MCP_ACCEPT, PROTOCOL_VERSION};
use cf_integration_mcp::probe::{ProbeConfig, run_probe};
use cf_integration_platform::checkout::{CheckoutManager, CheckoutRequest};
use cf_integration_platform::compose::{ComposeProject, validate_integration_contract};
use cf_integration_platform::config::AppConfig;
use cf_integration_platform::process::{CommandSpec, ProcessRunner};
use cf_integration_platform::stack::{
    BuildInputs, BuildMode, CleanupKind, FreshnessSnapshot, ServiceSnapshot, StackCommandPlan,
    StackFreshness, resolve_build,
};
use cf_integration_platform::{PlatformError, StackMode};

use crate::OutputStyle;
use crate::app::{
    Action, ConformanceAction, DebugAction, ResolvedLoadArgs, StackAction, selected_topologies,
    topology_selection,
};
use crate::cli::{LiveGroup, TokenKind as CliTokenKind, TopologySelection};
use crate::error::AppFailure;
use crate::token::{TokenKind, make_token};

type AppResult<T> = std::result::Result<T, AppFailure>;

const STACK_READY_TIMEOUT: Duration = Duration::from_secs(90);
const STACK_READY_POLL_INTERVAL: Duration = Duration::from_millis(250);
const STACK_READY_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

mod compliance;
mod inspect;
mod live;
mod reports;
mod sources;
mod stack;
mod workloads;

use inspect::*;
use reports::*;

/// Runtime services backed by one loaded configuration and process runner.
pub struct RuntimeExecutor<R> {
    config: AppConfig,
    runner: R,
}

impl<R> RuntimeExecutor<R> {
    /// Creates an executor without starting any process.
    #[must_use]
    pub fn new(config: AppConfig, runner: R) -> Self {
        Self { config, runner }
    }

    /// Returns the loaded application configuration.
    #[must_use]
    pub fn config(&self) -> &AppConfig {
        &self.config
    }
}

impl<R: ProcessRunner> RuntimeExecutor<R> {
    /// Executes one fully resolved operation.
    pub async fn execute(&mut self, action: Action) -> AppResult<()> {
        match action {
            Action::Stack(action) => self.execute_stack(action).await,
            Action::Probe(topology) => self.run_probe(topology).await,
            Action::Load(args) => self.run_load(args).await,
            Action::Live { topology, group } => self.run_live(topology, group).await,
            Action::Conformance(action) => self.execute_conformance(action).await,
            Action::Debug(DebugAction::Token { kind, server_id }) => {
                self.print_token(kind, server_id)
            }
            Action::Debug(DebugAction::Inspect {
                topology,
                method,
                server_id,
            }) => self.inspect(topology, &method, server_id.as_deref()).await,
        }
    }
}

impl<R: ProcessRunner> RuntimeExecutor<R> {
    fn print_token(&self, kind: CliTokenKind, server_id: Option<String>) -> AppResult<()> {
        let secret = required_text(&self.config.jwt_secret_key().value, "JWT_SECRET_KEY")?;
        let subject = required_text(&self.config.jwt_subject().value, "MCP_JWT_SUBJECT")?;
        let token_kind = match kind {
            CliTokenKind::Scoped => TokenKind::Scoped {
                server_id: Some(server_id.unwrap_or_else(|| self.default_server_id().to_owned())),
            },
            CliTokenKind::Admin => TokenKind::Admin,
        };
        let token = make_token(secret, subject, token_kind).map_err(AppFailure::from)?;
        println!("{token}");
        Ok(())
    }

    fn default_server_id(&self) -> &str {
        self.environment_text("MCP_SERVER_ID")
            .filter(|value| !value.is_empty())
            .or_else(|| {
                self.environment_text("MCP_VIRTUAL_SERVER_ID")
                    .filter(|value| !value.is_empty())
            })
            .or_else(|| self.config.fast_time_server_id().value.to_str())
            .unwrap_or("9779b6698cbd4b4995ee04a4fab38737")
    }

    fn base_url(&self) -> AppResult<&str> {
        required_text(&self.config.base_url().value, "MCP_CLI_BASE_URL")
    }

    fn bearer_token(&self, mode: StackMode, server_id: &str) -> AppResult<String> {
        if let Some(token) = self
            .environment_text("MCPGATEWAY_BEARER_TOKEN")
            .filter(|token| !token.is_empty())
        {
            return Ok(token.to_owned());
        }
        self.generated_bearer_token(mode, server_id)
    }

    fn generated_bearer_token(&self, mode: StackMode, server_id: &str) -> AppResult<String> {
        let secret = required_text(&self.config.jwt_secret_key().value, "JWT_SECRET_KEY")?;
        let subject = required_text(&self.config.jwt_subject().value, "MCP_JWT_SUBJECT")?;
        let kind = match mode {
            StackMode::Dataplane => TokenKind::Scoped {
                server_id: Some(server_id.to_owned()),
            },
            StackMode::Controlplane => TokenKind::Admin,
        };
        make_token(secret, subject, kind).map_err(AppFailure::from)
    }

    fn admin_token(&self) -> AppResult<String> {
        let secret = required_text(&self.config.jwt_secret_key().value, "JWT_SECRET_KEY")?;
        let subject = required_text(&self.config.jwt_subject().value, "MCP_JWT_SUBJECT")?;
        make_token(secret, subject, TokenKind::Admin).map_err(AppFailure::from)
    }
}

fn required_text<'a>(value: &'a OsStr, name: &str) -> AppResult<&'a str> {
    value
        .to_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppFailure::from(anyhow!("{name} must be nonempty UTF-8")))
}

fn finish_with_cleanup(primary: Option<AppFailure>, cleanup: AppResult<()>) -> AppResult<()> {
    match (primary, cleanup) {
        (None, Ok(())) => Ok(()),
        (Some(primary), Ok(())) => Err(primary),
        (None, Err(cleanup)) => Err(cleanup),
        (Some(primary), Err(cleanup)) => Err(AppFailure::from(anyhow!(
            "{primary}; additionally cleanup failed: {cleanup}"
        ))),
    }
}

async fn wait_for_http_endpoint(
    endpoint: &url::Url,
    mode: StackMode,
    timeout: Duration,
) -> AppResult<()> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .context("failed to build the public MCP readiness client")
        .map_err(AppFailure::from)?;
    let deadline = tokio::time::Instant::now() + timeout;
    let mut last_failure = "no HTTP response".to_owned();

    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err(AppFailure::from(anyhow!(
                "{} public MCP endpoint {} was not ready within {:.3}s; last result: {last_failure}",
                stack_mode_label(mode),
                endpoint,
                timeout.as_secs_f64()
            )));
        }
        let request_timeout = deadline
            .saturating_duration_since(now)
            .min(STACK_READY_REQUEST_TIMEOUT);
        let request = client
            .get(endpoint.clone())
            .header(reqwest::header::ACCEPT, MCP_ACCEPT);
        match tokio::time::timeout(request_timeout, request.send()).await {
            Ok(Ok(response)) if is_expected_readiness_status(response.status()) => return Ok(()),
            Ok(Ok(response)) => {
                last_failure = format!("HTTP {}", response.status().as_u16());
            }
            Ok(Err(error)) => {
                last_failure = format!("request error: {error}");
            }
            Err(_) => {
                last_failure = format!(
                    "request timed out after {:.3}s",
                    request_timeout.as_secs_f64()
                );
            }
        }
        let now = tokio::time::Instant::now();
        tokio::time::sleep(
            deadline
                .saturating_duration_since(now)
                .min(STACK_READY_POLL_INTERVAL),
        )
        .await;
    }
}

const fn is_expected_readiness_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 401 | 403 | 405)
}

const fn stack_mode_label(mode: StackMode) -> &'static str {
    match mode {
        StackMode::Controlplane => "controlplane",
        StackMode::Dataplane => "dataplane",
    }
}

const fn gateway_topology(mode: StackMode) -> GatewayTopology {
    match mode {
        StackMode::Controlplane => GatewayTopology::Direct,
        StackMode::Dataplane => GatewayTopology::Dataplane,
    }
}
