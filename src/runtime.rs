//! Operating-system-backed execution of resolved CLI actions.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, anyhow};
use async_trait::async_trait;
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
use serde::{Deserialize, Serialize};

use crate::app::{
    Action, ActionExecutor, ComplianceAction, ResolvedComplianceCommon, ResolvedLoadArgs,
    StackAction, TestAction,
};
use crate::cli::{
    ComplianceMode, ConformanceSuite, LiveGroup, LoadArgs, LoadEngine, TokenKind as CliTokenKind,
};
use crate::conformance::{
    Baseline, BaselineAudit, BaselineTarget, ComparisonReport, ConformanceResults, ScenarioOutcome,
    audit_baseline, compare_result_sets_with_fixture_trust, expected_server_scenarios,
    load_baseline, load_server_results, official_server_command, validate_no_fixture_failures,
    validate_server_scenario_set, write_comparison_report, write_official_baseline_projection,
};
use crate::conformance_fixture::{
    ConformanceFixtureClient, OFFICIAL_CONFORMANCE_BACKEND_URL, OFFICIAL_CONFORMANCE_REPOSITORY,
    OFFICIAL_CONFORMANCE_REVISION, OFFICIAL_CONFORMANCE_SERVER_ID, OFFICIAL_CONFORMANCE_SERVICE,
};
use crate::coverage::{
    CoverageOverlay, CoverageResult, GatewayApplicability, PINNED_SOURCE_COMMIT,
    PINNED_SOURCE_REPOSITORY, RequirementCoverageOverride, extract_catalog_from_checkout,
    parse_coverage_overlay, write_coverage_report,
};
use crate::error::AppFailure;
use crate::gateway_compliance::{
    GatewayCaseStatus, GatewayComplianceConfig, GatewayComplianceReport, run_gateway_compliance,
    write_gateway_reports,
};
use crate::load::{GooseLoadConfig, LoadSettings, LocustCommand, audit_locust_reports};
use crate::token::{TokenKind, make_token};

type AppResult<T> = std::result::Result<T, AppFailure>;

const fn uses_automatic_conformance_fixture(
    has_conformance_suite: bool,
    server_id: Option<&str>,
) -> bool {
    has_conformance_suite && server_id.is_none()
}

fn selected_compliance_server_id<'a>(
    auto_fixture: bool,
    base_server_id: &'a str,
    fixture_server_id: Option<&'a str>,
) -> &'a str {
    if auto_fixture {
        fixture_server_id.unwrap_or(base_server_id)
    } else {
        base_server_id
    }
}

const INSPECTOR_PACKAGE: &str = "@modelcontextprotocol/inspector@0.22.0";
const FAST_TEST_SERVER_ID: &str = "b8e3f1a2c4d5e6f7a1b2c3d4e5f6a7b8";
const CONFORMANCE_COMPLETION_MARKER: &[u8] = b"complete\n";
const PUBLISHER_SNAPSHOT_LUA: &str = r#"
for _, key in ipairs(redis.call('KEYS', '*UserConfig*')) do
    local value = redis.call('GET', key)
    if value then
        local decoded, config = pcall(cmsgpack.unpack, value)
        if decoded
            and type(config) == 'table'
            and type(config.virtual_hosts) == 'table'
            and config.virtual_hosts[ARGV[1]] ~= nil then
            return 1
        end
    end
end
return 0
"#;
const STACK_READY_TIMEOUT: Duration = Duration::from_secs(90);
const STACK_READY_POLL_INTERVAL: Duration = Duration::from_millis(250);
const STACK_READY_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const NPM_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "TMPDIR",
    "TMP",
    "TEMP",
    "XDG_CACHE_HOME",
    "NPM_CONFIG_CACHE",
    "npm_config_cache",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
];

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

#[async_trait(?Send)]
impl<R> ActionExecutor for RuntimeExecutor<R>
where
    R: ProcessRunner,
{
    async fn execute(&mut self, action: Action) -> AppResult<()> {
        match action {
            Action::Sync => self.sync_sources(),
            Action::Stack(action) => self.execute_stack(action).await,
            Action::Token { kind, server_id } => self.print_token(kind, server_id),
            Action::Test(action) => self.execute_test(action).await,
            Action::Compliance(action) => self.execute_compliance(action).await,
            Action::Inspect {
                mode,
                method,
                server_id,
            } => self.inspect(mode, &method, server_id.as_deref()).await,
        }
    }
}

impl<R: ProcessRunner> RuntimeExecutor<R> {
    fn sync_sources(&self) -> AppResult<()> {
        self.ensure_controlplane()?;
        self.ensure_dataplane()?;
        Ok(())
    }

    fn ensure_mode_sources(&self, mode: StackMode) -> AppResult<()> {
        self.ensure_controlplane()?;
        if mode == StackMode::Dataplane {
            self.ensure_dataplane()?;
        }
        Ok(())
    }

    fn ensure_controlplane(&self) -> AppResult<()> {
        let request = CheckoutRequest::controlplane(
            self.config.controlplane_dir(),
            self.config.controlplane_repo().value.clone(),
            self.config.controlplane_ref().value.clone(),
        );
        self.ensure_checkout(&request)
    }

    fn ensure_dataplane(&self) -> AppResult<()> {
        let request = CheckoutRequest::dataplane(
            self.config.dataplane_dir(),
            self.config.dataplane_repo().value.clone(),
            self.config.dataplane_ref().value.clone(),
        );
        self.ensure_checkout(&request)
    }

    fn ensure_checkout(&self, request: &CheckoutRequest) -> AppResult<()> {
        let manager = CheckoutManager::new(&self.runner);
        let mut warnings = Vec::new();
        let result = manager.ensure(self.config.integration_dir(), request, &mut warnings);
        for warning in warnings {
            eprintln!("{warning}");
        }
        Ok(result.map(|_| ())?)
    }

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

    async fn execute_stack(&self, action: StackAction) -> AppResult<()> {
        match action {
            StackAction::Up(mode) => self.stack_up(mode).await,
            StackAction::Down(mode) => self.cleanup(mode, CleanupKind::Down),
            StackAction::Reset(mode) => self.cleanup(mode, CleanupKind::Reset),
            StackAction::Status(mode) => {
                self.ensure_mode_sources(mode)?;
                let command = StackCommandPlan::status(self.compose_project(mode));
                Ok(self.runner.run(&self.compose_environment(
                    command.command().clone(),
                    mode,
                    true,
                )?)?)
            }
            StackAction::Logs { mode, services } => {
                self.ensure_mode_sources(mode)?;
                let command = StackCommandPlan::logs(self.compose_project(mode), services);
                Ok(self.runner.run(&self.compose_environment(
                    command.command().clone(),
                    mode,
                    true,
                )?)?)
            }
            StackAction::Config(mode) => {
                self.ensure_mode_sources(mode)?;
                if mode == StackMode::Dataplane {
                    self.validate_compose_contract()?;
                }
                let command = StackCommandPlan::config(self.compose_project(mode), mode);
                Ok(self.runner.run(&self.compose_environment(
                    command.command().clone(),
                    mode,
                    true,
                )?)?)
            }
        }
    }

    async fn stack_up(&self, mode: StackMode) -> AppResult<()> {
        self.ensure_mode_sources(mode)?;
        if mode == StackMode::Dataplane {
            self.validate_compose_contract()?;
        }

        let build = self.resolve_build(mode)?;
        self.pull_images(mode)?;
        if mode == StackMode::Dataplane
            && !self.environment_flag("CF_FORCE_FRESH_STACK", false)
            && !build
            && self.integration_freshness()? == StackFreshness::Current
        {
            println!("Integration stack already current; skipping Docker Compose up.");
            return self.wait_for_public_endpoint(mode).await;
        }

        if self.environment_flag("CF_FRESH_STACK", true) {
            self.cleanup(ComplianceMode::All, CleanupKind::Reset)?;
        }
        self.ensure_other_stack_stopped(mode)?;
        if mode == StackMode::Controlplane {
            fs::create_dir_all(self.config.controlplane_dir().join("reports"))
                .context("failed to create control-plane report directory")
                .map_err(AppFailure::from)?;
        }

        let start_locust = self.environment_flag("CONTROLPLANE_START_LOCUST_UI", false);
        let locust_workers = self
            .environment_text("CONTROLPLANE_LOCUST_WORKERS")
            .filter(|value| !value.is_empty())
            .unwrap_or("1")
            .parse::<usize>()
            .map_err(|_| {
                AppFailure::from(anyhow!("CONTROLPLANE_LOCUST_WORKERS must be an integer"))
            })?;
        let command = StackCommandPlan::up(
            self.compose_project(mode),
            mode,
            build,
            start_locust,
            locust_workers,
        );
        self.runner
            .run(&self.compose_environment(command.command().clone(), mode, true)?)?;
        self.wait_for_public_endpoint(mode).await?;
        println!(
            "{} stack started.",
            match mode {
                StackMode::Controlplane => "Control-plane",
                StackMode::Dataplane => "Dataplane integration",
            }
        );
        Ok(())
    }

    async fn wait_for_public_endpoint(&self, mode: StackMode) -> AppResult<()> {
        let endpoint = GatewayClient::builder(
            gateway_topology(mode),
            self.base_url()?,
            self.default_server_id(),
            "readiness-probe",
        )
        .build()
        .context("failed to construct the public MCP readiness endpoint")
        .map_err(AppFailure::from)?
        .endpoint()
        .clone();
        eprintln!(
            "Waiting up to {}s for the public {} MCP endpoint.",
            STACK_READY_TIMEOUT.as_secs(),
            stack_mode_label(mode)
        );
        wait_for_http_endpoint(&endpoint, mode, STACK_READY_TIMEOUT).await
    }

    fn compose_project(&self, mode: StackMode) -> ComposeProject {
        match mode {
            StackMode::Dataplane => ComposeProject::dataplane(
                self.config.root(),
                self.config.controlplane_dir(),
                self.config.integration_project().value.clone(),
                !self.config.dataplane_ref().value.is_empty(),
            ),
            StackMode::Controlplane => ComposeProject::controlplane(
                self.config.root(),
                self.config.controlplane_dir(),
                self.config.controlplane_project().value.clone(),
                self.environment_flag("CONTROLPLANE_ENABLE_SSO", false),
            ),
        }
    }

    fn compose_environment(
        &self,
        command: CommandSpec,
        mode: StackMode,
        checkout_labels: bool,
    ) -> AppResult<CommandSpec> {
        let command_environment = command.environment().clone();
        let mut command = if command.working_directory().is_some() {
            command
        } else {
            command.cwd(self.config.root())
        };
        for (key, value) in self.config.environment().iter() {
            if !command_environment.contains_key(key) {
                command = command.env(key.clone(), value.value.clone());
            }
        }
        command = command
            .env("CF_INTEGRATION_ROOT", self.config.root().as_os_str())
            .env(
                "CF_INTEGRATION_DIR",
                self.config.integration_dir().as_os_str(),
            )
            .env(
                "CF_CONTROLPLANE_DIR",
                self.config.controlplane_dir().as_os_str(),
            )
            .env("CF_DATAPLANE_DIR", self.config.dataplane_dir().as_os_str())
            .env(
                "CF_CONTROLPLANE_IMAGE",
                self.config.controlplane_image().resolved().to_owned(),
            )
            .env(
                "IMAGE_LOCAL",
                self.config.controlplane_image().resolved().to_owned(),
            )
            .env(
                "CF_DATAPLANE_IMAGE",
                self.config.dataplane_image().resolved().to_owned(),
            )
            .env("CF_DATAPLANE_PLATFORM", self.dataplane_platform()?)
            .env("JWT_SECRET_KEY", self.config.jwt_secret_key().value.clone())
            .env("MCP_CLI_BASE_URL", self.config.base_url().value.clone())
            .env(
                "PLATFORM_ADMIN_EMAIL",
                self.config.platform_admin_email().value.clone(),
            )
            .env(
                "KEY_FILE_PASSWORD",
                self.config.key_file_password().value.clone(),
            );

        for (key, default) in [
            ("PASSWORD_CHANGE_ENFORCEMENT_ENABLED", "false"),
            ("ADMIN_REQUIRE_PASSWORD_CHANGE_ON_BOOTSTRAP", "false"),
            ("REQUIRE_PASSWORD_CHANGE_FOR_DEFAULT_PASSWORD", "false"),
            ("GATEWAY_REPLICAS", "1"),
            ("GATEWAY_CPU_RESERVATION", "1"),
            ("GATEWAY_MEM_LIMIT", "2G"),
            ("GATEWAY_MEM_RESERVATION", "512M"),
        ] {
            if self.config.environment().get(OsStr::new(key)).is_none() {
                command = command.env(key, default);
            }
        }
        let needs_docker_cpus = ["GATEWAY_CPU_LIMIT", "GUNICORN_WORKERS"]
            .into_iter()
            .any(|key| self.config.environment().get(OsStr::new(key)).is_none());
        let docker_cpus = needs_docker_cpus.then(|| {
            self.capture_optional(&CommandSpec::new("docker").args([
                "info",
                "--format",
                "{{.NCPU}}",
            ]))
            .filter(|value| value.parse::<usize>().is_ok_and(|value| value > 0))
            .unwrap_or_else(|| "4".to_owned())
        });
        for key in ["GATEWAY_CPU_LIMIT", "GUNICORN_WORKERS"] {
            if self.config.environment().get(OsStr::new(key)).is_none() {
                command = command.env(key, docker_cpus.as_deref().unwrap_or("4"));
            }
        }
        for (key, argument) in [("HOST_UID", "-u"), ("HOST_GID", "-g")] {
            if self.config.environment().get(OsStr::new(key)).is_none() {
                let value = self
                    .capture_optional(&CommandSpec::new("id").arg(argument))
                    .filter(|value| value.parse::<u32>().is_ok())
                    .unwrap_or_else(|| "1000".to_owned());
                command = command.env(key, value);
            }
        }
        if self
            .config
            .environment()
            .get(OsStr::new("LOCUST_EXPECT_WORKERS"))
            .is_none()
        {
            command = command.env(
                "LOCUST_EXPECT_WORKERS",
                self.environment_text("CONTROLPLANE_LOCUST_WORKERS")
                    .filter(|value| !value.is_empty())
                    .unwrap_or("1"),
            );
        }
        if mode == StackMode::Controlplane {
            command = command.env(
                "COMPOSE_PROJECT_NAME",
                self.config.controlplane_project().value.clone(),
            );
        }
        if checkout_labels {
            command = self.add_checkout_labels(command, mode)?;
        }
        Ok(command)
    }

    fn add_checkout_labels(
        &self,
        mut command: CommandSpec,
        mode: StackMode,
    ) -> AppResult<CommandSpec> {
        let controlplane_revision =
            self.git_required(self.config.controlplane_dir(), ["rev-parse", "HEAD"])?;
        let controlplane_ref = self
            .git_optional(
                self.config.controlplane_dir(),
                ["symbolic-ref", "--quiet", "--short", "HEAD"],
            )
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                self.config
                    .controlplane_ref()
                    .value
                    .to_string_lossy()
                    .into_owned()
            });
        command = command
            .env("CF_CONTROLPLANE_CHECKOUT_REVISION", controlplane_revision)
            .env("CF_CONTROLPLANE_CHECKOUT_REF", controlplane_ref);
        if mode == StackMode::Dataplane && !self.config.dataplane_ref().value.is_empty() {
            let revision = self.git_required(self.config.dataplane_dir(), ["rev-parse", "HEAD"])?;
            let reference = self
                .git_optional(
                    self.config.dataplane_dir(),
                    ["symbolic-ref", "--quiet", "--short", "HEAD"],
                )
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| {
                    self.config
                        .dataplane_ref()
                        .value
                        .to_string_lossy()
                        .into_owned()
                });
            command = command
                .env("CF_DATAPLANE_CHECKOUT_REVISION", revision)
                .env("CF_DATAPLANE_CHECKOUT_REF", reference);
        }
        Ok(command)
    }

    fn validate_compose_contract(&self) -> AppResult<()> {
        let command = self
            .compose_project(StackMode::Dataplane)
            .command(["config", "--format", "json"]);
        let command = self.compose_environment(command, StackMode::Dataplane, true)?;
        let rendered = self.runner.capture_stdout(&command)?;
        let rendered: serde_json::Value = serde_json::from_slice(&rendered)
            .context("failed to parse rendered integration Compose JSON")
            .map_err(AppFailure::from)?;
        let expected = required_text(
            &self.config.fast_time_expected_image().value,
            "CF_FAST_TIME_EXPECTED_IMAGE",
        )?;
        let violations = validate_integration_contract(&rendered, expected);
        if violations.is_empty() {
            return Ok(());
        }
        let details = violations
            .into_iter()
            .map(|violation| format!("  - {violation}"))
            .collect::<Vec<_>>()
            .join("\n");
        Err(AppFailure::from(anyhow!(
            "Compose contract failed:\n{details}"
        )))
    }

    fn resolve_build(&self, mode: StackMode) -> AppResult<bool> {
        let setting = required_text(&self.config.compose_build().value, "CF_COMPOSE_BUILD")?;
        let mode_setting =
            BuildMode::from_str(setting).map_err(|error| AppFailure::from(anyhow!(error)))?;
        let controlplane_checkout_revision =
            Some(self.git_required(self.config.controlplane_dir(), ["rev-parse", "HEAD"])?);
        let (controlplane_image_present, controlplane_image_revision) =
            self.image_state(self.config.controlplane_image().resolved());
        let dataplane_source = (!self.config.dataplane_ref().value.is_empty()).then(|| {
            self.config
                .dataplane_ref()
                .value
                .to_string_lossy()
                .into_owned()
        });
        let dataplane_checkout_revision = if dataplane_source.is_some() {
            Some(self.git_required(self.config.dataplane_dir(), ["rev-parse", "HEAD"])?)
        } else {
            None
        };
        let (dataplane_image_present, dataplane_image_revision) =
            self.image_state(self.config.dataplane_image().resolved());
        let decision = resolve_build(
            mode_setting,
            &BuildInputs {
                controlplane_image_explicit: self.config.controlplane_image().is_explicitly_set(),
                controlplane_image_present,
                controlplane_checkout_revision,
                controlplane_image_revision,
                include_dataplane: mode == StackMode::Dataplane,
                dataplane_source_ref: dataplane_source,
                dataplane_image_present,
                dataplane_checkout_revision,
                dataplane_image_revision,
            },
        );
        for reason in decision.reasons {
            println!("CF_COMPOSE_BUILD: {reason}");
        }
        Ok(decision.build)
    }

    fn image_state(&self, image: &OsStr) -> (bool, Option<String>) {
        let command = CommandSpec::new("docker").args([
            OsString::from("image"),
            OsString::from("inspect"),
            image.to_owned(),
            OsString::from("--format"),
            OsString::from("{{ index .Config.Labels \"org.opencontainers.image.revision\" }}"),
        ]);
        match self.capture_optional(&command) {
            Some(revision) => (true, (!revision.is_empty()).then_some(revision)),
            None => (false, None),
        }
    }

    fn pull_images(&self, mode: StackMode) -> AppResult<()> {
        if self.config.controlplane_image().is_explicitly_set() {
            self.pull_if_changed(
                "cf-controlplane",
                self.config.controlplane_image().resolved(),
                None,
            )?;
        }
        if mode == StackMode::Dataplane && self.config.dataplane_ref().value.is_empty() {
            let platform = self.dataplane_platform()?;
            self.pull_if_changed(
                "cf-dataplane",
                self.config.dataplane_image().resolved(),
                Some(platform.as_os_str()),
            )?;
        }
        Ok(())
    }

    fn pull_if_changed(
        &self,
        label: &str,
        image: &OsStr,
        platform: Option<&OsStr>,
    ) -> AppResult<()> {
        let inspect = CommandSpec::new("docker").args([
            OsString::from("buildx"),
            OsString::from("imagetools"),
            OsString::from("inspect"),
            image.to_owned(),
            OsString::from("--format"),
            OsString::from("{{.Manifest.Digest}}"),
        ]);
        let remote_digest = self
            .capture_optional(&inspect)
            .filter(|value| !value.is_empty());
        let local_exists = self
            .capture_optional(&CommandSpec::new("docker").args([
                OsString::from("image"),
                OsString::from("inspect"),
                image.to_owned(),
                OsString::from("--format"),
                OsString::from("{{.Id}}"),
            ]))
            .is_some();
        if let Some(digest) = remote_digest {
            let repo_digests = self.capture_optional(&CommandSpec::new("docker").args([
                OsString::from("image"),
                OsString::from("inspect"),
                image.to_owned(),
                OsString::from("--format"),
                OsString::from("{{range .RepoDigests}}{{println .}}{{end}}"),
            ]));
            if repo_digests.as_deref().is_some_and(|values| {
                values
                    .lines()
                    .any(|value| value.ends_with(&format!("@{digest}")))
            }) {
                println!("{label} image digest unchanged: {digest}");
                return Ok(());
            }
        } else if local_exists {
            println!("{label} remote digest unavailable; using local image.");
            return Ok(());
        }

        let mut arguments = vec![OsString::from("pull")];
        if let Some(platform) = platform {
            arguments.push(OsString::from("--platform"));
            arguments.push(platform.to_owned());
        }
        arguments.push(image.to_owned());
        Ok(self
            .runner
            .run(&CommandSpec::new("docker").args(arguments))?)
    }

    fn integration_freshness(&self) -> AppResult<StackFreshness> {
        let project = required_text(
            &self.config.integration_project().value,
            "CF_INTEGRATION_PROJECT",
        )?;
        let mut services = std::collections::BTreeMap::new();
        for service in [
            "gateway",
            "cf-dataplane",
            "nginx",
            "postgres",
            "pgbouncer",
            "redis",
            "fast_time_server",
            "migration",
            "register_fast_time",
        ] {
            services.insert(service.to_owned(), self.service_snapshot(project, service));
        }
        for service in ["fast_test_server", "register_fast_test"] {
            if self.container_id(project, service, true).is_some() {
                services.insert(service.to_owned(), self.service_snapshot(project, service));
            }
        }
        let snapshot = FreshnessSnapshot {
            services,
            controlplane_checkout_revision: self
                .git_optional(self.config.controlplane_dir(), ["rev-parse", "HEAD"]),
            dataplane_checkout_revision: self
                .git_optional(self.config.dataplane_dir(), ["rev-parse", "HEAD"]),
            controlplane_image_explicit: self.config.controlplane_image().is_explicitly_set(),
            dataplane_source_enabled: !self.config.dataplane_ref().value.is_empty(),
            expected_controlplane_image: required_text(
                self.config.controlplane_image().resolved(),
                "CF_CONTROLPLANE_IMAGE",
            )?
            .to_owned(),
            expected_dataplane_image: required_text(
                self.config.dataplane_image().resolved(),
                "CF_DATAPLANE_IMAGE",
            )?
            .to_owned(),
            expected_fast_time_image: required_text(
                &self.config.fast_time_expected_image().value,
                "CF_FAST_TIME_EXPECTED_IMAGE",
            )?
            .to_owned(),
        };
        Ok(snapshot.evaluate())
    }

    fn service_snapshot(&self, project: &str, service: &str) -> ServiceSnapshot {
        let running_id = self.container_id(project, service, false);
        let all_id = self.container_id(project, service, true);
        let configured_image = running_id
            .as_deref()
            .and_then(|id| self.docker_inspect(id, "{{.Config.Image}}"));
        let running_image = running_id
            .as_deref()
            .and_then(|id| self.docker_inspect(id, "{{.Image}}"));
        let expected_image_id = configured_image.as_deref().and_then(|image| {
            self.capture_optional(
                &CommandSpec::new("docker")
                    .args(["image", "inspect", image, "--format", "{{.Id}}"]),
            )
        });
        let completed_successfully = all_id.as_deref().is_some_and(|id| {
            self.docker_inspect(id, "{{.State.Status}}").as_deref() == Some("exited")
                && self.docker_inspect(id, "{{.State.ExitCode}}").as_deref() == Some("0")
        });
        let image_revision = running_id.as_deref().and_then(|id| {
            self.docker_inspect(
                id,
                "{{ index .Config.Labels \"org.opencontainers.image.revision\" }}",
            )
            .filter(|value| !value.is_empty())
        });
        ServiceSnapshot {
            running: running_id.is_some(),
            completed_successfully,
            configured_image,
            running_image_matches_configured: running_image.is_some()
                && running_image == expected_image_id,
            image_revision,
        }
    }

    fn container_id(&self, project: &str, service: &str, all: bool) -> Option<String> {
        let mut arguments = vec![OsString::from("ps")];
        arguments.push(OsString::from(if all { "-aq" } else { "-q" }));
        arguments.extend([
            OsString::from("--filter"),
            OsString::from(format!("label=com.docker.compose.project={project}")),
            OsString::from("--filter"),
            OsString::from(format!("label=com.docker.compose.service={service}")),
        ]);
        self.capture_optional(&CommandSpec::new("docker").args(arguments))
            .and_then(|value| value.lines().next().map(str::to_owned))
            .filter(|value| !value.is_empty())
    }

    fn docker_inspect(&self, id: &str, format: &str) -> Option<String> {
        self.capture_optional(&CommandSpec::new("docker").args(["inspect", id, "--format", format]))
    }

    fn ensure_other_stack_stopped(&self, mode: StackMode) -> AppResult<()> {
        let (other, label) = match mode {
            StackMode::Dataplane => (
                required_text(
                    &self.config.controlplane_project().value,
                    "CF_CONTROLPLANE_PROJECT",
                )?,
                "control-plane",
            ),
            StackMode::Controlplane => (
                required_text(
                    &self.config.integration_project().value,
                    "CF_INTEGRATION_PROJECT",
                )?,
                "dataplane integration",
            ),
        };
        if self.project_has_running_containers(other) {
            return Err(AppFailure::from(anyhow!(
                "the {label} stack is running on the same host ports; run `cf-integration stack down --mode all` first"
            )));
        }
        Ok(())
    }

    fn project_has_running_containers(&self, project: &str) -> bool {
        self.capture_optional(&CommandSpec::new("docker").args([
            "ps",
            "-q",
            "--filter",
            &format!("label=com.docker.compose.project={project}"),
        ]))
        .is_some_and(|value| !value.is_empty())
    }

    fn cleanup(&self, selection: ComplianceMode, kind: CleanupKind) -> AppResult<()> {
        let mut last_failure = None;
        for mode in selected_modes(selection) {
            if self
                .config
                .controlplane_dir()
                .join("docker-compose.yml")
                .is_file()
            {
                let project =
                    self.compose_project(mode)
                        .with_profiles(["testing", "inspector", "sso"]);
                let command = StackCommandPlan::cleanup(project, kind);
                match self.compose_environment(command.command().clone(), mode, false) {
                    Ok(command) => {
                        if let Err(error) = self.runner.run(&command) {
                            last_failure = Some(error.into());
                        }
                    }
                    Err(error) => last_failure = Some(error),
                }
            }
            let project = match mode {
                StackMode::Controlplane => &self.config.controlplane_project().value,
                StackMode::Dataplane => &self.config.integration_project().value,
            };
            match required_text(project, "Compose project name")
                .and_then(|project| self.remove_project_by_label(project, kind))
            {
                Ok(()) => {}
                Err(error) => last_failure = Some(error),
            }
        }
        match last_failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn remove_project_by_label(&self, project: &str, kind: CleanupKind) -> AppResult<()> {
        let filter = format!("label=com.docker.compose.project={project}");
        for id in self.docker_list(["ps", "-aq", "--filter", filter.as_str()])? {
            self.runner
                .run(&CommandSpec::new("docker").args(["rm", "-f", id.as_str()]))?;
        }
        for id in self.docker_list(["network", "ls", "-q", "--filter", filter.as_str()])? {
            let _ =
                self.runner
                    .run(&CommandSpec::new("docker").args(["network", "rm", id.as_str()]));
        }
        if kind == CleanupKind::Reset {
            for id in self.docker_list(["volume", "ls", "-q", "--filter", filter.as_str()])? {
                self.runner
                    .run(&CommandSpec::new("docker").args(["volume", "rm", id.as_str()]))?;
            }
        }
        Ok(())
    }

    fn docker_list<const N: usize>(&self, arguments: [&str; N]) -> AppResult<Vec<String>> {
        let output = self
            .runner
            .capture_stdout(&CommandSpec::new("docker").args(arguments))?;
        let output = String::from_utf8(output)
            .context("Docker returned non-UTF-8 resource identifiers")
            .map_err(AppFailure::from)?;
        Ok(output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
    }

    fn dataplane_platform(&self) -> AppResult<OsString> {
        if self.config.dataplane_platform().value != "auto" {
            return Ok(self.config.dataplane_platform().value.clone());
        }
        if self.config.dataplane_ref().value.is_empty() {
            return Ok(OsString::from("linux/amd64"));
        }
        Ok(self
            .capture_optional(&CommandSpec::new("docker").args([
                "version",
                "--format",
                "{{.Server.Os}}/{{.Server.Arch}}",
            ]))
            .filter(|value| !value.is_empty())
            .map_or_else(|| OsString::from("linux/amd64"), OsString::from))
    }

    fn git_required<const N: usize>(
        &self,
        directory: &Path,
        arguments: [&str; N],
    ) -> AppResult<String> {
        let mut command = CommandSpec::new("git").arg("-C").arg(directory.as_os_str());
        command = command.args(arguments);
        let output = self.runner.capture_stdout(&command)?;
        String::from_utf8(output)
            .context("Git returned non-UTF-8 revision data")
            .map(|value| value.trim().to_owned())
            .map_err(AppFailure::from)
    }

    fn git_optional<const N: usize>(
        &self,
        directory: &Path,
        arguments: [&str; N],
    ) -> Option<String> {
        let mut command = CommandSpec::new("git").arg("-C").arg(directory.as_os_str());
        command = command.args(arguments);
        self.capture_optional(&command)
    }

    fn capture_optional(&self, command: &CommandSpec) -> Option<String> {
        self.runner
            .capture_stdout(command)
            .ok()
            .and_then(|output| String::from_utf8(output).ok())
            .map(|value| value.trim().to_owned())
    }

    fn environment_text(&self, key: &str) -> Option<&str> {
        self.config
            .environment()
            .get(OsStr::new(key))
            .and_then(|value| value.value.to_str())
    }

    fn environment_flag(&self, key: &str, default: bool) -> bool {
        self.environment_text(key)
            .map_or(default, |value| matches!(value, "true" | "1"))
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

    fn caller_managed_server_id<'a>(&'a self, explicit: Option<&'a str>) -> Option<&'a str> {
        explicit
            .filter(|value| !value.is_empty())
            .or_else(|| {
                self.environment_text("MCP_SERVER_ID")
                    .filter(|value| !value.is_empty())
            })
            .or_else(|| {
                self.environment_text("MCP_VIRTUAL_SERVER_ID")
                    .filter(|value| !value.is_empty())
            })
    }

    fn require_loopback_fixture_base_url(&self) -> AppResult<()> {
        let base_url = self.base_url()?;
        let url = url::Url::parse(base_url)
            .context("MCP_CLI_BASE_URL is not a valid URL")
            .map_err(AppFailure::from)?;
        let is_loopback = match url.host() {
            Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(address)) => address.is_loopback(),
            Some(url::Host::Ipv6(address)) => address.is_loopback(),
            None => false,
        };
        if !is_loopback {
            return Err(AppFailure::from(anyhow!(
                "automatic conformance fixture provisioning requires a loopback MCP_CLI_BASE_URL; pass --server-id for a remote or shared deployment"
            )));
        }
        Ok(())
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

    fn conformance_compose_project(&self, mode: StackMode) -> ComposeProject {
        self.compose_project(mode)
            .with_conformance_fixture(self.config.root())
    }

    async fn start_conformance_service(&self, mode: StackMode) -> AppResult<()> {
        let project = self.conformance_compose_project(mode);
        let build = project.command(["build", OFFICIAL_CONFORMANCE_SERVICE]);
        let build = self.compose_environment(build, mode, true)?;
        self.runner.run_async(&build).await?;

        let up = project.command([
            "up",
            "-d",
            "--wait",
            "gateway",
            OFFICIAL_CONFORMANCE_SERVICE,
        ]);
        let up = self.compose_environment(up, mode, true)?;
        Ok(self.runner.run_async(&up).await?)
    }

    async fn stop_conformance_service(&self, mode: StackMode) -> AppResult<()> {
        let remove = self.conformance_compose_project(mode).command([
            "rm",
            "--stop",
            "--force",
            OFFICIAL_CONFORMANCE_SERVICE,
        ]);
        let remove = match self.compose_environment(remove, mode, true) {
            Ok(command) => self
                .runner
                .run_async(&command)
                .await
                .map_err(AppFailure::from),
            Err(error) => Err(error),
        };

        let restore = self
            .compose_project(mode)
            .command(["up", "-d", "--wait", "gateway"]);
        let restore = match self.compose_environment(restore, mode, true) {
            Ok(command) => self
                .runner
                .run_async(&command)
                .await
                .map_err(AppFailure::from),
            Err(error) => Err(error),
        };

        combine_cleanup_results(remove, restore)
    }

    async fn execute_test(&self, action: TestAction) -> AppResult<()> {
        match action {
            TestAction::Probe(mode) => self.run_probe(mode).await,
            TestAction::Load(args) => self.run_load(args).await,
            TestAction::Live { mode, group } => self.run_live(mode, group, false).await,
            TestAction::Suite {
                mode,
                start,
                load,
                exclude_plugins,
            } => self.run_suite(mode, start, &load, exclude_plugins).await,
        }
    }

    async fn run_probe(&self, mode: StackMode) -> AppResult<()> {
        let server_id = self.default_server_id().to_owned();
        self.prepare_test_target(mode, &server_id).await?;
        let token = self.bearer_token(mode, &server_id)?;
        let protocol_version = self
            .environment_text("MCP_SPEC_VERSION")
            .unwrap_or(PROTOCOL_VERSION)
            .to_owned();
        let config = ProbeConfig {
            mode: gateway_topology(mode),
            base_url: self.base_url()?.to_owned(),
            server_id,
            bearer_token: token,
            config_timeout: Duration::from_secs(
                self.environment_u64("CF_PROBE_CONFIG_TIMEOUT", 120)?,
            ),
            retry_interval: Duration::from_secs(5),
            request_timeout: Duration::from_secs(
                self.environment_u64("CF_PROBE_REQUEST_TIMEOUT", 30)?,
            ),
            protocol_version,
        };
        let transport = ReqwestProbeTransport::new().map_err(AppFailure::from)?;
        let stdout = std::io::stdout();
        let mut output = stdout.lock();
        run_probe(&transport, &config, &mut output)
            .await
            .map_err(AppFailure::from)
    }

    async fn run_load(&self, args: ResolvedLoadArgs) -> AppResult<()> {
        let server_id = self.default_server_id().to_owned();
        self.prepare_test_target(args.mode, &server_id).await?;
        let token = self.bearer_token(args.mode, &server_id)?;
        let cli_args = LoadArgs {
            mode: None,
            engine: args.engine,
            smoke: args.smoke,
            users: args.users,
            spawn_rate: args.spawn_rate,
            run_time: args.run_time,
        };
        let settings = LoadSettings::resolve(&self.config, &cli_args).map_err(AppFailure::from)?;
        match args.engine {
            LoadEngine::Locust => {
                let command = LocustCommand::new(
                    &self.config,
                    args.mode,
                    &settings,
                    &token,
                    (args.mode == StackMode::Dataplane).then_some(server_id.as_str()),
                )
                .map_err(AppFailure::from)?;
                let process_result = self
                    .runner
                    .run(&self.compose_environment(command.command().clone(), args.mode, true)?)
                    .map_err(AppFailure::from);
                finalize_locust_run(process_result, command.report_dir(), &token)
            }
            LoadEngine::Goose => {
                self.run_goose(args.mode, &settings, &token, &server_id)
                    .await
            }
        }
    }

    async fn run_live(
        &self,
        mode: StackMode,
        group: LiveGroup,
        exclude_plugins: bool,
    ) -> AppResult<()> {
        self.ensure_controlplane()?;
        self.ensure_other_stack_for_tests(mode)?;
        match group {
            LiveGroup::Mcp => {
                self.ensure_fast_test_fixture(mode).await?;
                self.run_controlplane_make(mode, "test-mcp-protocol-e2e")
            }
            LiveGroup::Rbac => self.run_controlplane_make(mode, "test-mcp-rbac"),
            LiveGroup::Protocol => {
                self.run_controlplane_make(mode, "test-protocol-compliance-gateway")
            }
            LiveGroup::All => {
                self.ensure_fast_test_fixture(mode).await?;
                self.run_live_all(mode, exclude_plugins)
            }
        }
    }

    async fn ensure_fast_test_fixture(&self, mode: StackMode) -> AppResult<()> {
        let project = self.compose_project(mode);
        for plan in [
            StackCommandPlan::fast_test_up(project.clone()),
            StackCommandPlan::fast_test_register(project),
        ] {
            self.runner
                .run(&self.compose_environment(plan.command().clone(), mode, true)?)?;
        }
        if mode == StackMode::Dataplane {
            self.wait_for_publisher_snapshot(FAST_TEST_SERVER_ID)
                .await?;
        }
        Ok(())
    }

    fn run_controlplane_make(&self, mode: StackMode, target: &str) -> AppResult<()> {
        let command = CommandSpec::new("make")
            .arg("-C")
            .arg(self.config.controlplane_dir().as_os_str())
            .arg(target);
        Ok(self
            .runner
            .run(&self.compose_environment(command, mode, false)?)?)
    }

    fn run_live_all(&self, mode: StackMode, exclude_plugins: bool) -> AppResult<()> {
        let mut pass_one = CommandSpec::new("uv")
            .args([
                "run",
                "--extra",
                "plugins",
                "pytest",
                "-p",
                "no:playwright",
                "tests/live_gateway/",
                "--ignore=tests/live_gateway/sso",
                "--ignore=tests/live_gateway/mcp/test_mcp_rbac_transport.py",
            ])
            .cwd(self.config.controlplane_dir());
        if exclude_plugins {
            pass_one = pass_one.arg("--ignore=tests/live_gateway/plugins");
        }
        pass_one = pass_one.args(["-v", "--tb=short"]);
        let pass_two = CommandSpec::new("uv")
            .args([
                "run",
                "--extra",
                "plugins",
                "pytest",
                "-p",
                "playwright",
                "tests/live_gateway/sso",
                "tests/live_gateway/mcp/test_mcp_rbac_transport.py",
                "-v",
                "--tb=short",
            ])
            .cwd(self.config.controlplane_dir());
        let mut failure = self
            .runner
            .run(&self.compose_environment(pass_one, mode, false)?)
            .err()
            .map(AppFailure::from);
        if let Err(error) = self
            .runner
            .run(&self.compose_environment(pass_two, mode, false)?)
        {
            failure = Some(error.into());
        }
        failure.map_or(Ok(()), Err)
    }

    fn ensure_other_stack_for_tests(&self, mode: StackMode) -> AppResult<()> {
        self.ensure_other_stack_stopped(mode)
    }

    async fn prepare_test_target(&self, mode: StackMode, server_id: &str) -> AppResult<()> {
        self.ensure_other_stack_for_tests(mode)?;
        if mode == StackMode::Dataplane {
            self.wait_for_publisher_snapshot(server_id).await?;
        }
        Ok(())
    }

    async fn complete_compliance_setup(
        &self,
        mode: StackMode,
        server_id: &str,
        setup: AppResult<()>,
    ) -> AppResult<()> {
        setup?;
        if mode == StackMode::Dataplane {
            self.wait_for_publisher_snapshot(server_id).await?;
        }
        Ok(())
    }

    async fn run_suite(
        &self,
        selection: ComplianceMode,
        start: bool,
        loads: &[LoadEngine],
        exclude_plugins: bool,
    ) -> AppResult<()> {
        if selection == ComplianceMode::All && !start {
            return Err(AppFailure::from(anyhow!(
                "--mode all requires --start because the two stacks share host ports"
            )));
        }
        let mut last_failure = None;
        for mode in selected_modes(selection) {
            if start {
                let mut setup = self.stack_up(mode).await;
                if setup.is_ok() && mode == StackMode::Dataplane {
                    setup = self
                        .wait_for_publisher_snapshot(self.default_server_id())
                        .await;
                }
                if let Err(error) = setup {
                    last_failure = Some(error);
                    if selection == ComplianceMode::All
                        && let Err(error) = self.cleanup(mode_selection(mode), CleanupKind::Down)
                    {
                        last_failure = Some(error);
                    }
                    continue;
                }
            }
            for result in [
                self.run_probe(mode).await,
                self.run_load(ResolvedLoadArgs {
                    mode,
                    engine: LoadEngine::Locust,
                    smoke: true,
                    users: None,
                    spawn_rate: None,
                    run_time: None,
                })
                .await,
                self.run_live(mode, LiveGroup::Mcp, exclude_plugins).await,
                self.run_live(mode, LiveGroup::Rbac, exclude_plugins).await,
                self.run_live(mode, LiveGroup::Protocol, exclude_plugins)
                    .await,
                self.run_live(mode, LiveGroup::All, exclude_plugins).await,
            ] {
                if let Err(error) = result {
                    last_failure = Some(error);
                }
            }
            for engine in loads {
                if let Err(error) = self
                    .run_load(ResolvedLoadArgs {
                        mode,
                        engine: *engine,
                        smoke: false,
                        users: None,
                        spawn_rate: None,
                        run_time: None,
                    })
                    .await
                {
                    last_failure = Some(error);
                }
            }
            self.record_suite_cleanup(selection, mode, &mut last_failure);
        }
        last_failure.map_or(Ok(()), Err)
    }

    fn record_suite_cleanup(
        &self,
        selection: ComplianceMode,
        mode: StackMode,
        last_failure: &mut Option<AppFailure>,
    ) {
        if selection == ComplianceMode::All
            && let Err(error) = self.cleanup(mode_selection(mode), CleanupKind::Down)
        {
            *last_failure = Some(error);
        }
    }

    async fn wait_for_publisher_snapshot(&self, server_id: &str) -> AppResult<()> {
        let timeout_seconds = self.environment_u64("CF_PUBLISHER_WAIT_SECONDS", 90)?;
        let project = required_text(
            &self.config.integration_project().value,
            "CF_INTEGRATION_PROJECT",
        )?;
        let redis = self.container_id(project, "redis", false).ok_or_else(|| {
            AppFailure::from(anyhow!(
                "cannot wait for publisher snapshot: the dataplane Redis container is not running"
            ))
        })?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_seconds);
        eprintln!(
            "Waiting up to {timeout_seconds}s for a publisher snapshot containing server {server_id}."
        );
        loop {
            let command = CommandSpec::new("docker").args([
                "exec",
                redis.as_str(),
                "redis-cli",
                "EVAL",
                PUBLISHER_SNAPSHOT_LUA,
                "0",
                server_id,
            ]);
            if self.capture_optional(&command).as_deref() == Some("1") {
                return Ok(());
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                return Err(AppFailure::from(anyhow!(
                    "publisher snapshot did not contain server {server_id} within {timeout_seconds}s; inspect the dataplane publisher and Redis logs"
                )));
            }
            tokio::time::sleep(
                deadline
                    .saturating_duration_since(now)
                    .min(Duration::from_secs(2)),
            )
            .await;
        }
    }

    fn environment_u64(&self, key: &str, default: u64) -> AppResult<u64> {
        self.environment_text(key).map_or(Ok(default), |value| {
            value
                .parse::<u64>()
                .map_err(|_| AppFailure::from(anyhow!("{key} must be a non-negative integer")))
        })
    }

    async fn run_goose(
        &self,
        mode: StackMode,
        settings: &LoadSettings,
        token: &str,
        server_id: &str,
    ) -> AppResult<()> {
        let run = GooseLoadConfig::new(
            &self.config,
            mode,
            settings,
            token,
            (mode == StackMode::Dataplane).then_some(server_id),
        )
        .map_err(AppFailure::from)?;
        let outcome = run
            .execute()
            .await
            .map_err(|error| AppFailure::from(anyhow!(error)))?;
        println!(
            "Goose reports: {} and {}",
            outcome.reports().html().display(),
            outcome.reports().json().display()
        );
        Ok(())
    }

    async fn execute_compliance(&self, action: ComplianceAction) -> AppResult<()> {
        match action {
            ComplianceAction::Conformance {
                common,
                suite,
                baseline,
            } => {
                if common.mode == ComplianceMode::All && baseline.is_some() {
                    return Err(AppFailure::from(anyhow!(
                        "--baseline can target only one mode; run each mode with its independent rich baseline"
                    )));
                }
                self.run_compliance_workflow(&common, Some(suite), false, baseline.as_deref())
                    .await
            }
            ComplianceAction::Gateway { common } => {
                self.run_compliance_workflow(&common, None, true, None)
                    .await
            }
            ComplianceAction::All { common, suite } => {
                self.run_compliance_workflow(&common, Some(suite), true, None)
                    .await
            }
            ComplianceAction::Report {
                results_dir,
                output_dir,
            } => self.regenerate_compliance_reports(results_dir.as_deref(), output_dir.as_deref()),
        }
    }

    async fn run_compliance_workflow(
        &self,
        common: &ResolvedComplianceCommon,
        conformance_suite: Option<ConformanceSuite>,
        run_gateway: bool,
        custom_baseline: Option<&Path>,
    ) -> AppResult<()> {
        self.run_compliance_workflow_with_interrupt(
            common,
            conformance_suite,
            run_gateway,
            custom_baseline,
            async {
                if tokio::signal::ctrl_c().await.is_err() {
                    std::future::pending::<()>().await;
                }
            },
        )
        .await
    }

    async fn run_compliance_workflow_with_interrupt<I>(
        &self,
        common: &ResolvedComplianceCommon,
        conformance_suite: Option<ConformanceSuite>,
        run_gateway: bool,
        custom_baseline: Option<&Path>,
        interrupt: I,
    ) -> AppResult<()>
    where
        I: Future<Output = ()>,
    {
        if common.mode == ComplianceMode::All && !common.start {
            return Err(AppFailure::from(anyhow!(
                "--mode all requires --start because the two stacks share host ports"
            )));
        }

        let paths = CompliancePaths::new(
            common
                .results_dir
                .as_deref()
                .unwrap_or_else(|| self.config.integration_dir()),
            self.config.root().join("reports"),
        );
        let suite_name = conformance_suite.map(conformance_suite_name);
        let caller_server_id = self.caller_managed_server_id(common.server_id.as_deref());
        let auto_fixture =
            uses_automatic_conformance_fixture(conformance_suite.is_some(), caller_server_id);
        if auto_fixture {
            self.require_loopback_fixture_base_url()?;
        }
        paths.clear_selected(common.mode, suite_name.is_some(), run_gateway)?;
        let mut last_failure = None;
        let mut interrupted = false;
        tokio::pin!(interrupt);
        let (cancellation_sender, cancellation_receiver) = tokio::sync::watch::channel(false);

        for mode in selected_modes(common.mode) {
            let base_server_id = caller_server_id
                .unwrap_or_else(|| self.default_server_id())
                .to_owned();
            let setup = if common.start {
                self.stack_up(mode).await
            } else {
                self.ensure_mode_sources(mode)
                    .and_then(|()| self.ensure_other_stack_for_tests(mode))
            };
            let mut mode_failure = if auto_fixture {
                setup.err()
            } else {
                self.complete_compliance_setup(mode, &base_server_id, setup)
                    .await
                    .err()
            };
            let mut fixture_state = None;
            let mut fixture_metadata = None;
            let mut selected_server_id = base_server_id;

            if mode_failure.is_none() && auto_fixture {
                let (start_result, start_interrupted) = finish_phase_after_interrupt(
                    self.start_conformance_service(mode),
                    interrupt.as_mut(),
                )
                .await;
                interrupted |= start_interrupted;
                match start_result {
                    Ok(()) => match self.admin_token().and_then(|token| {
                        ConformanceFixtureClient::builder(self.base_url()?, token)
                            .build()
                            .map_err(AppFailure::from)
                    }) {
                        Ok(client) => {
                            if interrupted {
                                mode_failure = Some(interrupted_conformance_failure());
                                let cleanup = self.stop_conformance_service(mode).await;
                                mode_failure = finish_with_cleanup(mode_failure, cleanup).err();
                            } else {
                                let (provision_result, provision_interrupted) =
                                    finish_phase_after_interrupt(
                                        client.provision(OFFICIAL_CONFORMANCE_BACKEND_URL),
                                        interrupt.as_mut(),
                                    )
                                    .await;
                                interrupted |= provision_interrupted;
                                match provision_result {
                                    Ok(fixture) => {
                                        selected_server_id = selected_compliance_server_id(
                                            auto_fixture,
                                            &selected_server_id,
                                            Some(&fixture.server_id),
                                        )
                                        .to_owned();
                                        fixture_metadata = Some(ConformanceFixtureMetadata {
                                            repository: OFFICIAL_CONFORMANCE_REPOSITORY.to_owned(),
                                            revision: OFFICIAL_CONFORMANCE_REVISION.to_owned(),
                                            server_id: fixture.server_id.clone(),
                                        });
                                        fixture_state = Some((client, fixture));
                                        if interrupted {
                                            mode_failure = Some(interrupted_conformance_failure());
                                        } else if mode == StackMode::Dataplane {
                                            tokio::select! {
                                                result = self.wait_for_publisher_snapshot(
                                                    &selected_server_id,
                                                ) => {
                                                    if let Err(error) = result {
                                                        mode_failure = Some(error);
                                                    }
                                                }
                                                () = interrupt.as_mut() => {
                                                    interrupted = true;
                                                    mode_failure = Some(
                                                        interrupted_conformance_failure()
                                                    );
                                                }
                                            }
                                        }
                                    }
                                    Err(error) => {
                                        mode_failure = Some(if interrupted {
                                            interrupted_conformance_failure()
                                        } else {
                                            AppFailure::from(error)
                                        });
                                        let cleanup = self.stop_conformance_service(mode).await;
                                        mode_failure =
                                            finish_with_cleanup(mode_failure, cleanup).err();
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            mode_failure = Some(error);
                            let cleanup = self.stop_conformance_service(mode).await;
                            mode_failure = finish_with_cleanup(mode_failure, cleanup).err();
                        }
                    },
                    Err(error) => {
                        mode_failure = Some(if interrupted {
                            interrupted_conformance_failure()
                        } else {
                            error
                        });
                        let cleanup = self.stop_conformance_service(mode).await;
                        mode_failure = finish_with_cleanup(mode_failure, cleanup).err();
                    }
                }
            }

            if mode_failure.is_none() {
                let mut tests_cancellation = cancellation_receiver.clone();
                let tests = async {
                    let token = match self.bearer_token(mode, &selected_server_id) {
                        Ok(token) => token,
                        Err(error) => return Some(error),
                    };
                    if *tests_cancellation.borrow() {
                        return Some(interrupted_conformance_failure());
                    }
                    let mut failure = None;
                    if let Some(suite_name) = suite_name
                        && let Err(error) = self
                            .run_official_conformance_mode(
                                &OfficialConformanceRun {
                                    mode,
                                    server_id: &selected_server_id,
                                    token: &token,
                                    spec_version: &common.spec_version,
                                    suite: suite_name,
                                    custom_baseline,
                                    fixture: fixture_metadata.as_ref(),
                                    cancellation: cancellation_receiver.clone(),
                                },
                                &paths,
                            )
                            .await
                    {
                        failure = Some(error);
                    }
                    tokio::task::yield_now().await;
                    if *tests_cancellation.borrow() {
                        return Some(interrupted_conformance_failure());
                    }
                    if run_gateway {
                        let gateway_result = tokio::select! {
                            result = self.run_gateway_compliance_mode(
                                mode,
                                &selected_server_id,
                                &token,
                                &common.spec_version,
                                &paths,
                            ) => result,
                            () = wait_for_runtime_cancellation(&mut tests_cancellation) => {
                                return Some(interrupted_conformance_failure());
                            }
                        };
                        if let Err(error) = gateway_result
                            && failure.is_none()
                        {
                            failure = Some(error);
                        }
                    }
                    failure
                };
                if auto_fixture {
                    tokio::pin!(tests);
                    tokio::select! {
                        failure = &mut tests => mode_failure = failure,
                        () = interrupt.as_mut() => {
                            interrupted = true;
                            cancellation_sender.send_replace(true);
                            let _ = tests.await;
                            mode_failure = Some(interrupted_conformance_failure());
                        }
                    }
                } else {
                    mode_failure = tests.await;
                }
            }

            if let Some((client, fixture)) = fixture_state {
                let api_cleanup = client
                    .cleanup(Some(&fixture))
                    .await
                    .map_err(AppFailure::from);
                let service_cleanup = self.stop_conformance_service(mode).await;
                let cleanup = combine_cleanup_results(api_cleanup, service_cleanup);
                mode_failure = finish_with_cleanup(mode_failure, cleanup).err();
            }

            if let Some(error) = mode_failure {
                last_failure = Some(error);
            }

            if common.mode == ComplianceMode::All
                && let Err(error) = self.cleanup(mode_selection(mode), CleanupKind::Down)
            {
                last_failure = finish_with_cleanup(last_failure, Err(error)).err();
            }
            if interrupted {
                break;
            }
        }

        if !interrupted
            && common.mode == ComplianceMode::All
            && let Some(suite_name) = suite_name
        {
            match self
                .write_comparison_from_artifacts(&paths, Some((&common.spec_version, suite_name)))
            {
                Ok(path) => println!("Conformance comparison: {}", path.display()),
                Err(error) => last_failure = Some(error),
            }
        }
        if !interrupted && run_gateway && suite_name.is_some() {
            match self.write_spec_coverage_report(&paths.report_output, &paths) {
                Ok(path) => println!("Specification coverage: {}", path.display()),
                Err(error) => last_failure = Some(error),
            }
        }

        last_failure.map_or(Ok(()), Err)
    }

    async fn run_official_conformance_mode(
        &self,
        run: &OfficialConformanceRun<'_>,
        paths: &CompliancePaths,
    ) -> AppResult<()> {
        let target = baseline_target(run.mode);
        let baseline_path = run
            .custom_baseline
            .map(Path::to_owned)
            .unwrap_or_else(|| default_baseline_path(self.config.root(), target));
        let baseline = load_baseline(&baseline_path, target).map_err(AppFailure::from)?;
        expected_server_scenarios(run.suite, run.spec_version).map_err(AppFailure::from)?;
        let mode_paths = paths.conformance_mode(target);
        remove_file_if_exists(&mode_paths.completion)?;
        recreate_directory(&mode_paths.official_results)?;
        fs::create_dir_all(&mode_paths.root)
            .with_context(|| {
                format!(
                    "failed to create conformance artifact directory {:?}",
                    mode_paths.root
                )
            })
            .map_err(AppFailure::from)?;
        write_official_baseline_projection(&baseline, target, &mode_paths.projection)
            .map_err(AppFailure::from)?;
        write_rich_baseline(&mode_paths.rich_baseline, &baseline)?;
        write_run_metadata(
            &mode_paths.metadata,
            &ConformanceRunMetadata {
                oracle: crate::conformance::OFFICIAL_CONFORMANCE_PACKAGE.to_owned(),
                target: target.label().to_owned(),
                spec_version: run.spec_version.to_owned(),
                suite: run.suite.to_owned(),
                fixture: run.fixture.cloned(),
            },
        )?;

        let endpoint = GatewayClient::builder(
            gateway_topology(run.mode),
            self.base_url()?,
            run.server_id,
            run.token,
        )
        .protocol_version(run.spec_version)
        .build()
        .context("failed to construct the conformance gateway endpoint")
        .map_err(AppFailure::from)?
        .endpoint()
        .clone();
        let proxy = AuthProxy::start(endpoint, run.token)
            .await
            .context("failed to start the conformance authentication proxy")
            .map_err(AppFailure::from)?;
        let command = allowlisted_npx_environment(
            official_server_command(
                proxy.url().as_str(),
                run.suite,
                run.spec_version,
                &mode_paths.projection,
                &mode_paths.official_results,
            )
            .cwd(self.config.root()),
        );
        let process_result = self
            .runner
            .run_async_cancellable(&command, run.cancellation.clone())
            .await
            .map_err(AppFailure::from);
        let shutdown_result = proxy
            .shutdown()
            .await
            .context("failed to stop the conformance authentication proxy")
            .map_err(AppFailure::from);

        let results = load_server_results(&mode_paths.official_results).map_err(AppFailure::from);
        let audit = results
            .as_ref()
            .ok()
            .map(|results| audit_baseline(results, &baseline));
        if let Some(audit) = audit.as_ref()
            && !audit.is_clean()
        {
            eprintln!("{}", format_baseline_audit(target, audit));
        }

        if !conformance_process_completed(&process_result) {
            return process_result;
        }
        shutdown_result?;
        let results = results?;
        if !is_trusted_official_fixture(run.fixture) {
            validate_no_fixture_failures(&results).map_err(AppFailure::from)?;
        }
        mark_conformance_complete(
            &process_result,
            &results,
            target,
            run.suite,
            run.spec_version,
            &mode_paths.completion,
        )?;
        let audit = audit.ok_or_else(|| {
            AppFailure::from(anyhow!(
                "failed to audit parsed official conformance results for {target}"
            ))
        })?;
        if !audit.is_clean() {
            return Err(AppFailure::from(anyhow!(format_baseline_audit(
                target, &audit
            ))));
        }
        process_result?;
        println!(
            "Official conformance artifacts ({}): {}",
            target,
            mode_paths.root.display()
        );
        Ok(())
    }

    async fn run_gateway_compliance_mode(
        &self,
        mode: StackMode,
        server_id: &str,
        token: &str,
        spec_version: &str,
        paths: &CompliancePaths,
    ) -> AppResult<()> {
        let wrong_scope_token = self.wrong_scope_token(server_id)?;
        let report = run_gateway_compliance(&GatewayComplianceConfig {
            mode: gateway_topology(mode),
            base_url: self.base_url()?,
            server_id,
            bearer_token: token,
            wrong_scope_token: wrong_scope_token.as_deref(),
            protocol_version: spec_version,
        })
        .await
        .map_err(AppFailure::from)?;
        let target = baseline_target(mode);
        let mode_paths = paths.gateway_mode(target);
        write_gateway_reports(&mode_paths.markdown, &mode_paths.json, &report)
            .map_err(AppFailure::from)?;
        println!(
            "Gateway compliance artifacts ({}): {}",
            target,
            mode_paths.root.display()
        );
        if !report.is_compliant() {
            return Err(AppFailure::from(anyhow!(
                "gateway compliance failed for {target}; see {}",
                mode_paths.markdown.display()
            )));
        }
        Ok(())
    }

    fn wrong_scope_token(&self, server_id: &str) -> AppResult<Option<String>> {
        if let Some(token) = self
            .environment_text("MCPGATEWAY_WRONG_SCOPE_BEARER_TOKEN")
            .filter(|token| !token.is_empty())
        {
            return Ok(Some(token.to_owned()));
        }
        if self
            .environment_text("MCPGATEWAY_BEARER_TOKEN")
            .is_some_and(|token| !token.is_empty())
        {
            return Ok(None);
        }
        let secret = required_text(&self.config.jwt_secret_key().value, "JWT_SECRET_KEY")?;
        let subject = required_text(&self.config.jwt_subject().value, "MCP_JWT_SUBJECT")?;
        make_token(
            secret,
            subject,
            TokenKind::Scoped {
                server_id: Some(format!("wrong-scope-{server_id}")),
            },
        )
        .map(Some)
        .map_err(AppFailure::from)
    }

    fn regenerate_compliance_reports(
        &self,
        results_dir: Option<&Path>,
        output_dir: Option<&Path>,
    ) -> AppResult<()> {
        let paths = CompliancePaths::new(
            results_dir.unwrap_or_else(|| self.config.integration_dir()),
            output_dir
                .map(Path::to_owned)
                .unwrap_or_else(|| self.config.root().join("reports")),
        );
        let coverage = self.write_spec_coverage_report(&paths.report_output, &paths)?;
        println!("Specification coverage: {}", coverage.display());
        if [BaselineTarget::Controlplane, BaselineTarget::Dataplane]
            .into_iter()
            .map(|target| paths.conformance_mode(target))
            .any(|artifact| artifact.metadata.is_file() || artifact.official_results.is_dir())
        {
            let comparison = self.write_comparison_from_artifacts(&paths, None)?;
            println!("Conformance comparison: {}", comparison.display());
        }

        for target in [BaselineTarget::Controlplane, BaselineTarget::Dataplane] {
            let Some(report) = self.load_gateway_artifact(&paths, target)? else {
                continue;
            };
            let markdown = paths
                .report_output
                .join(format!("mcp-gateway-compliance-{}.md", target_slug(target)));
            let json = paths.report_output.join(format!(
                "mcp-gateway-compliance-{}.json",
                target_slug(target)
            ));
            write_gateway_reports(&markdown, &json, &report).map_err(AppFailure::from)?;
            println!("Gateway compliance report: {}", markdown.display());
        }
        Ok(())
    }

    fn write_spec_coverage_report(
        &self,
        output_dir: &Path,
        paths: &CompliancePaths,
    ) -> AppResult<PathBuf> {
        let checkout = self.config.integration_dir().join("mcp-spec-source");
        let request = CheckoutRequest::controlplane(
            &checkout,
            PINNED_SOURCE_REPOSITORY,
            PINNED_SOURCE_COMMIT,
        );
        self.ensure_checkout(&request)?;
        let actual_commit = self.git_required(&checkout, ["rev-parse", "HEAD"])?;
        if actual_commit != PINNED_SOURCE_COMMIT {
            return Err(AppFailure::from(anyhow!(
                "MCP specification checkout resolved to {actual_commit}, expected pinned commit {PINNED_SOURCE_COMMIT}"
            )));
        }
        let requirements = extract_catalog_from_checkout(&checkout).map_err(AppFailure::from)?;
        let overlay_path = self
            .config
            .root()
            .join("conformance/coverage-overrides.yml");
        let overlay_source = fs::read_to_string(&overlay_path)
            .with_context(|| format!("failed to read MCP coverage overlay {overlay_path:?}"))
            .map_err(AppFailure::from)?;
        let mut overlay =
            parse_coverage_overlay(&overlay_source, &requirements).map_err(AppFailure::from)?;
        self.enrich_coverage_overlay(&mut overlay, paths)?;
        let output = output_dir.join("mcp-spec-coverage.md");
        write_coverage_report(&output, &requirements, &overlay).map_err(AppFailure::from)?;
        Ok(output)
    }

    fn enrich_coverage_overlay(
        &self,
        overlay: &mut CoverageOverlay,
        paths: &CompliancePaths,
    ) -> AppResult<()> {
        let controlplane_official =
            self.load_conformance_artifact(paths, BaselineTarget::Controlplane)?;
        let dataplane_official =
            self.load_conformance_artifact(paths, BaselineTarget::Dataplane)?;
        let controlplane_gateway =
            self.load_gateway_artifact(paths, BaselineTarget::Controlplane)?;
        let dataplane_gateway = self.load_gateway_artifact(paths, BaselineTarget::Dataplane)?;
        for artifact in [controlplane_official.as_ref(), dataplane_official.as_ref()]
            .into_iter()
            .flatten()
        {
            if artifact.metadata.spec_version != overlay.spec_version {
                return Err(AppFailure::from(anyhow!(
                    "official conformance artifact specification {:?} does not match coverage specification {:?}",
                    artifact.metadata.spec_version,
                    overlay.spec_version
                )));
            }
        }
        let controlplane = ModeCoverageEvidence::from_artifacts(
            controlplane_official
                .as_ref()
                .map(|artifact| &artifact.results),
            controlplane_gateway.as_ref(),
            controlplane_official
                .as_ref()
                .and_then(|artifact| artifact.metadata.fixture.as_ref()),
        );
        let dataplane = ModeCoverageEvidence::from_artifacts(
            dataplane_official
                .as_ref()
                .map(|artifact| &artifact.results),
            dataplane_gateway.as_ref(),
            dataplane_official
                .as_ref()
                .and_then(|artifact| artifact.metadata.fixture.as_ref()),
        );
        enrich_overlay_results(overlay, &controlplane, &dataplane);
        Ok(())
    }

    fn load_gateway_artifact(
        &self,
        paths: &CompliancePaths,
        target: BaselineTarget,
    ) -> AppResult<Option<GatewayComplianceReport>> {
        let artifact = paths.gateway_mode(target);
        if !artifact.json.is_file() {
            return Ok(None);
        }
        let source = fs::read(&artifact.json)
            .with_context(|| format!("failed to read gateway artifact {:?}", artifact.json))
            .map_err(AppFailure::from)?;
        let report: GatewayComplianceReport = serde_json::from_slice(&source)
            .with_context(|| format!("failed to parse gateway artifact {:?}", artifact.json))
            .map_err(AppFailure::from)?;
        if report.mode != target_slug(target) {
            return Err(AppFailure::from(anyhow!(
                "gateway artifact mode {:?} does not match {} path",
                report.mode,
                target_slug(target)
            )));
        }
        if report.specification_version != crate::coverage::SPEC_VERSION {
            return Err(AppFailure::from(anyhow!(
                "gateway artifact specification {:?} does not match coverage specification {:?}",
                report.specification_version,
                crate::coverage::SPEC_VERSION
            )));
        }
        Ok(Some(report))
    }

    fn write_comparison_from_artifacts(
        &self,
        paths: &CompliancePaths,
        expected_run: Option<(&str, &str)>,
    ) -> AppResult<PathBuf> {
        let controlplane = self.load_conformance_artifact(paths, BaselineTarget::Controlplane)?;
        let dataplane = self.load_conformance_artifact(paths, BaselineTarget::Dataplane)?;
        if controlplane.is_none() && dataplane.is_none() {
            return Err(AppFailure::from(anyhow!(
                "no official conformance artifacts found beneath {}",
                paths.conformance_root.display()
            )));
        }

        let metadata = compatible_metadata(
            controlplane.as_ref().map(|artifact| &artifact.metadata),
            dataplane.as_ref().map(|artifact| &artifact.metadata),
            expected_run,
        )?;
        let empty_results = ConformanceResults::default();
        let empty_baseline = Baseline::default();
        let scenarios = compare_result_sets_with_fixture_trust(
            controlplane
                .as_ref()
                .map_or(&empty_results, |artifact| &artifact.results),
            dataplane
                .as_ref()
                .map_or(&empty_results, |artifact| &artifact.results),
            controlplane
                .as_ref()
                .map_or(&empty_baseline, |artifact| &artifact.baseline),
            dataplane
                .as_ref()
                .map_or(&empty_baseline, |artifact| &artifact.baseline),
            is_trusted_official_fixture(
                controlplane
                    .as_ref()
                    .and_then(|artifact| artifact.metadata.fixture.as_ref()),
            ),
            is_trusted_official_fixture(
                dataplane
                    .as_ref()
                    .and_then(|artifact| artifact.metadata.fixture.as_ref()),
            ),
        );
        let output = paths.report_output.join("mcp-conformance-comparison.md");
        write_comparison_report(
            &output,
            &ComparisonReport {
                spec_version: metadata.spec_version.clone(),
                suite: metadata.suite.clone(),
                scenarios,
            },
        )
        .map_err(AppFailure::from)?;
        Ok(output)
    }

    fn load_conformance_artifact(
        &self,
        paths: &CompliancePaths,
        target: BaselineTarget,
    ) -> AppResult<Option<LoadedConformanceArtifact>> {
        let artifact = paths.conformance_mode(target);
        if !artifact.metadata.is_file()
            && !artifact.official_results.is_dir()
            && !artifact.completion.is_file()
        {
            return Ok(None);
        }
        if !artifact.metadata.is_file()
            || !artifact.official_results.is_dir()
            || !artifact.completion.is_file()
        {
            return Err(AppFailure::from(anyhow!(
                "incomplete conformance artifacts for {target} beneath {}",
                artifact.root.display()
            )));
        }
        verify_completion_marker(&artifact.completion)?;
        let metadata = read_run_metadata(&artifact.metadata)?;
        if metadata.target != target.label() {
            return Err(AppFailure::from(anyhow!(
                "conformance metadata target {:?} does not match {target}",
                metadata.target
            )));
        }
        if metadata.oracle != crate::conformance::OFFICIAL_CONFORMANCE_PACKAGE {
            return Err(AppFailure::from(anyhow!(
                "conformance artifacts used oracle {:?}, expected {:?}",
                metadata.oracle,
                crate::conformance::OFFICIAL_CONFORMANCE_PACKAGE
            )));
        }
        let results = load_server_results(&artifact.official_results).map_err(AppFailure::from)?;
        validate_server_scenario_set(&results, &metadata.suite, &metadata.spec_version)
            .map_err(AppFailure::from)?;
        let baseline_path = if artifact.rich_baseline.is_file() {
            artifact.rich_baseline
        } else {
            default_baseline_path(self.config.root(), target)
        };
        let baseline = load_baseline(&baseline_path, target).map_err(AppFailure::from)?;
        Ok(Some(LoadedConformanceArtifact {
            results,
            baseline,
            metadata,
        }))
    }

    async fn inspect(
        &self,
        mode: StackMode,
        method: &str,
        server_id: Option<&str>,
    ) -> AppResult<()> {
        self.ensure_mode_sources(mode)?;
        let server_id = server_id.unwrap_or_else(|| self.default_server_id());
        self.prepare_test_target(mode, server_id).await?;
        let token = self.bearer_token(mode, server_id)?;
        let endpoint =
            GatewayClient::new(gateway_topology(mode), self.base_url()?, server_id, &token)
                .context("failed to construct the Inspector gateway endpoint")
                .map_err(AppFailure::from)?
                .endpoint()
                .clone();
        let proxy = AuthProxy::start(endpoint, &token)
            .await
            .context("failed to start the Inspector authentication proxy")
            .map_err(AppFailure::from)?;
        let command = allowlisted_npx_environment(
            inspector_command(proxy.url().as_str(), method).cwd(self.config.root()),
        );
        let process_result = self.runner.run_async(&command).await;
        let shutdown_result = proxy
            .shutdown()
            .await
            .context("failed to stop the Inspector authentication proxy")
            .map_err(AppFailure::from);
        process_result?;
        shutdown_result
    }
}

#[derive(Debug, Clone)]
struct CompliancePaths {
    conformance_root: PathBuf,
    gateway_root: PathBuf,
    report_output: PathBuf,
}

impl CompliancePaths {
    fn new(artifact_root: &Path, report_output: PathBuf) -> Self {
        Self {
            conformance_root: artifact_root.join("conformance"),
            gateway_root: artifact_root.join("gateway-compliance"),
            report_output,
        }
    }

    fn conformance_mode(&self, target: BaselineTarget) -> ConformanceModePaths {
        let root = self.conformance_root.join(target_slug(target));
        ConformanceModePaths {
            official_results: root.join("official"),
            projection: root.join("expected-failures.yml"),
            rich_baseline: root.join("baseline.yml"),
            metadata: root.join("metadata.json"),
            completion: root.join("complete"),
            root,
        }
    }

    fn gateway_mode(&self, target: BaselineTarget) -> GatewayModePaths {
        let root = self.gateway_root.join(target_slug(target));
        GatewayModePaths {
            markdown: root.join("report.md"),
            json: root.join("report.json"),
            root,
        }
    }

    fn clear_selected(
        &self,
        selection: ComplianceMode,
        conformance: bool,
        gateway: bool,
    ) -> AppResult<()> {
        for mode in selected_modes(selection) {
            let target = baseline_target(mode);
            if conformance {
                remove_artifact_directory(&self.conformance_mode(target).root)?;
            }
            if gateway {
                remove_artifact_directory(&self.gateway_mode(target).root)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct ConformanceModePaths {
    root: PathBuf,
    official_results: PathBuf,
    projection: PathBuf,
    rich_baseline: PathBuf,
    metadata: PathBuf,
    completion: PathBuf,
}

#[derive(Debug, Clone)]
struct GatewayModePaths {
    root: PathBuf,
    markdown: PathBuf,
    json: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConformanceFixtureMetadata {
    repository: String,
    revision: String,
    server_id: String,
}

fn is_trusted_official_fixture(fixture: Option<&ConformanceFixtureMetadata>) -> bool {
    fixture.is_some_and(|fixture| {
        fixture.repository == OFFICIAL_CONFORMANCE_REPOSITORY
            && fixture.revision == OFFICIAL_CONFORMANCE_REVISION
            && fixture.server_id == OFFICIAL_CONFORMANCE_SERVER_ID
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConformanceRunMetadata {
    oracle: String,
    target: String,
    spec_version: String,
    suite: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fixture: Option<ConformanceFixtureMetadata>,
}

struct LoadedConformanceArtifact {
    results: ConformanceResults,
    baseline: Baseline,
    metadata: ConformanceRunMetadata,
}

#[derive(Debug, Default)]
struct ModeCoverageEvidence {
    available: bool,
    official: BTreeMap<String, CoverageResult>,
    gateway: BTreeMap<String, CoverageResult>,
}

impl ModeCoverageEvidence {
    fn from_artifacts(
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
}

fn enrich_overlay_results(
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

struct OfficialConformanceRun<'a> {
    mode: StackMode,
    server_id: &'a str,
    token: &'a str,
    spec_version: &'a str,
    suite: &'a str,
    custom_baseline: Option<&'a Path>,
    fixture: Option<&'a ConformanceFixtureMetadata>,
    cancellation: tokio::sync::watch::Receiver<bool>,
}

fn inspector_command(endpoint: &str, method: &str) -> CommandSpec {
    CommandSpec::new("npx").clear_environment().args([
        "-y",
        INSPECTOR_PACKAGE,
        "--cli",
        endpoint,
        "--transport",
        "http",
        "--method",
        method,
    ])
}

fn allowlisted_npx_environment(mut command: CommandSpec) -> CommandSpec {
    command = command.clear_environment();
    for key in NPM_ENV_ALLOWLIST {
        if let Some(value) = std::env::var_os(key) {
            command = command.env(key, value);
        }
    }
    command
}

fn recreate_directory(path: &Path) -> AppResult<()> {
    if path.exists() {
        fs::remove_dir_all(path)
            .with_context(|| format!("failed to clear result directory {path:?}"))
            .map_err(AppFailure::from)?;
    }
    fs::create_dir_all(path)
        .with_context(|| format!("failed to create result directory {path:?}"))
        .map_err(AppFailure::from)
}

fn remove_file_if_exists(path: &Path) -> AppResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppFailure::from(
            anyhow!(error).context(format!("failed to clear completion marker {path:?}")),
        )),
    }
}

fn write_completion_marker(path: &Path) -> AppResult<()> {
    fs::write(path, CONFORMANCE_COMPLETION_MARKER)
        .with_context(|| format!("failed to write conformance completion marker {path:?}"))
        .map_err(AppFailure::from)
}

fn conformance_process_completed(process_result: &AppResult<()>) -> bool {
    match process_result {
        Ok(()) => true,
        Err(AppFailure::Platform(PlatformError::ChildExit { status, .. })) => {
            status.code().is_some()
        }
        Err(AppFailure::Platform(PlatformError::Native(_))) | Err(AppFailure::Native(_)) => false,
    }
}

fn mark_conformance_complete(
    process_result: &AppResult<()>,
    results: &ConformanceResults,
    target: BaselineTarget,
    suite: &str,
    spec_version: &str,
    path: &Path,
) -> AppResult<bool> {
    if !conformance_process_completed(process_result) {
        return Ok(false);
    }
    validate_server_scenario_set(results, suite, spec_version)
        .with_context(|| format!("official conformance did not complete for {target}"))
        .map_err(AppFailure::from)?;
    write_completion_marker(path)?;
    Ok(true)
}

fn verify_completion_marker(path: &Path) -> AppResult<()> {
    let marker = fs::read(path)
        .with_context(|| format!("failed to read conformance completion marker {path:?}"))
        .map_err(AppFailure::from)?;
    if marker != CONFORMANCE_COMPLETION_MARKER {
        return Err(AppFailure::from(anyhow!(
            "invalid conformance completion marker {path:?}"
        )));
    }
    Ok(())
}

fn write_rich_baseline(path: &Path, baseline: &Baseline) -> AppResult<()> {
    let serialized = serde_yaml::to_string(baseline)
        .context("failed to serialize rich conformance baseline")
        .map_err(AppFailure::from)?;
    fs::write(path, serialized)
        .with_context(|| format!("failed to write rich conformance baseline {path:?}"))
        .map_err(AppFailure::from)
}

fn write_run_metadata(path: &Path, metadata: &ConformanceRunMetadata) -> AppResult<()> {
    let serialized = serde_json::to_vec_pretty(metadata)
        .context("failed to serialize conformance run metadata")
        .map_err(AppFailure::from)?;
    fs::write(path, serialized)
        .with_context(|| format!("failed to write conformance run metadata {path:?}"))
        .map_err(AppFailure::from)
}

fn read_run_metadata(path: &Path) -> AppResult<ConformanceRunMetadata> {
    let source = fs::read(path)
        .with_context(|| format!("failed to read conformance run metadata {path:?}"))
        .map_err(AppFailure::from)?;
    serde_json::from_slice(&source)
        .with_context(|| format!("failed to parse conformance run metadata {path:?}"))
        .map_err(AppFailure::from)
}

fn compatible_metadata<'a>(
    controlplane: Option<&'a ConformanceRunMetadata>,
    dataplane: Option<&'a ConformanceRunMetadata>,
    expected_run: Option<(&str, &str)>,
) -> AppResult<&'a ConformanceRunMetadata> {
    let metadata = controlplane.or(dataplane).ok_or_else(|| {
        AppFailure::from(anyhow!(
            "no conformance metadata is available for reporting"
        ))
    })?;
    if let (Some(controlplane), Some(dataplane)) = (controlplane, dataplane)
        && controlplane.fixture != dataplane.fixture
    {
        return Err(AppFailure::from(anyhow!(
            "control-plane and dataplane conformance fixture provenance mismatch"
        )));
    }
    if let (Some(controlplane), Some(dataplane)) = (controlplane, dataplane)
        && (controlplane.spec_version != dataplane.spec_version
            || controlplane.suite != dataplane.suite
            || controlplane.oracle != dataplane.oracle)
    {
        return Err(AppFailure::from(anyhow!(
            "control-plane and dataplane conformance artifacts were produced by incompatible runs"
        )));
    }
    if let Some((spec_version, suite)) = expected_run
        && (metadata.spec_version != spec_version || metadata.suite != suite)
    {
        return Err(AppFailure::from(anyhow!(
            "conformance artifacts do not match requested spec version {spec_version:?} and suite {suite:?}"
        )));
    }
    Ok(metadata)
}

fn format_baseline_audit(target: BaselineTarget, audit: &BaselineAudit) -> String {
    let mut details = Vec::new();
    for (label, scenarios) in [
        ("unexpected failures", &audit.unexpected_failures),
        ("stale baseline entries", &audit.stale_entries),
        ("unobserved baseline entries", &audit.unobserved_entries),
    ] {
        if !scenarios.is_empty() {
            details.push(format!("{label}: {}", scenarios.join(", ")));
        }
    }
    format!(
        "conformance baseline audit failed for {target}: {}",
        details.join("; ")
    )
}

fn default_baseline_path(root: &Path, target: BaselineTarget) -> PathBuf {
    root.join("conformance").join(match target {
        BaselineTarget::Controlplane => "baseline-controlplane.yml",
        BaselineTarget::Dataplane => "baseline-dataplane.yml",
    })
}

const fn baseline_target(mode: StackMode) -> BaselineTarget {
    match mode {
        StackMode::Controlplane => BaselineTarget::Controlplane,
        StackMode::Dataplane => BaselineTarget::Dataplane,
    }
}

const fn target_slug(target: BaselineTarget) -> &'static str {
    match target {
        BaselineTarget::Controlplane => "controlplane",
        BaselineTarget::Dataplane => "dataplane",
    }
}

const fn mode_selection(mode: StackMode) -> ComplianceMode {
    match mode {
        StackMode::Controlplane => ComplianceMode::Controlplane,
        StackMode::Dataplane => ComplianceMode::Dataplane,
    }
}

const fn conformance_suite_name(suite: ConformanceSuite) -> &'static str {
    match suite {
        ConformanceSuite::Active => "active",
        ConformanceSuite::All => "all",
    }
}

fn selected_modes(selection: ComplianceMode) -> Vec<StackMode> {
    match selection {
        ComplianceMode::Controlplane => vec![StackMode::Controlplane],
        ComplianceMode::Dataplane => vec![StackMode::Dataplane],
        ComplianceMode::All => vec![StackMode::Controlplane, StackMode::Dataplane],
    }
}

fn required_text<'a>(value: &'a OsStr, name: &str) -> AppResult<&'a str> {
    value
        .to_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppFailure::from(anyhow!("{name} must be nonempty UTF-8")))
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

fn remove_artifact_directory(path: &Path) -> AppResult<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppFailure::from(
            anyhow!(error).context(format!("failed to clear compliance artifacts {path:?}")),
        )),
    }
}

fn finalize_locust_run(
    process_result: AppResult<()>,
    report_dir: &Path,
    bearer_token: &str,
) -> AppResult<()> {
    audit_locust_reports(report_dir, bearer_token).map_err(AppFailure::from)?;
    process_result
}

fn combine_cleanup_results(first: AppResult<()>, second: AppResult<()>) -> AppResult<()> {
    match (first, second) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(first), Err(second)) => Err(AppFailure::from(anyhow!(
            "{first}; additionally conformance service cleanup failed: {second}"
        ))),
    }
}

async fn finish_phase_after_interrupt<F, I, T>(
    operation: F,
    interrupt: std::pin::Pin<&mut I>,
) -> (T, bool)
where
    F: Future<Output = T>,
    I: Future<Output = ()>,
{
    tokio::pin!(operation);
    tokio::select! {
        output = &mut operation => (output, false),
        () = interrupt => (operation.await, true),
    }
}

async fn wait_for_runtime_cancellation(cancellation: &mut tokio::sync::watch::Receiver<bool>) {
    while !*cancellation.borrow_and_update() {
        if cancellation.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

fn interrupted_conformance_failure() -> AppFailure {
    AppFailure::from(anyhow!("conformance workflow interrupted by Ctrl-C"))
}

fn finish_with_cleanup(primary: Option<AppFailure>, cleanup: AppResult<()>) -> AppResult<()> {
    match (primary, cleanup) {
        (None, Ok(())) => Ok(()),
        (Some(primary), Ok(())) => Err(primary),
        (None, Err(cleanup)) => Err(cleanup),
        (Some(primary), Err(cleanup)) => Err(AppFailure::from(anyhow!(
            "{primary}; additionally conformance cleanup failed: {cleanup}"
        ))),
    }
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::ffi::OsString;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    type PlatformResult<T> = std::result::Result<T, PlatformError>;
    use axum::Router;
    use axum::extract::State;
    use axum::http::{Method, StatusCode, Uri};
    use axum::routing::{any, get};
    use cf_integration_platform::config::Environment;
    use cf_integration_platform::process::CapturedOutput;

    struct CompletedFixtureFailureRunner;

    impl ProcessRunner for CompletedFixtureFailureRunner {
        fn run(&self, spec: &CommandSpec) -> PlatformResult<()> {
            let output = spec
                .arguments()
                .windows(2)
                .find(|values| values[0] == "--output-dir")
                .map(|values| PathBuf::from(&values[1]))
                .expect("official runner output directory should be present");
            let scenarios =
                expected_server_scenarios("active", "2025-11-25").expect("pinned active catalog");
            for scenario in scenarios {
                let directory = output.join(format!("server-{scenario}-2026-07-10T03-17-47-000Z"));
                fs::create_dir_all(&directory)
                    .expect("official scenario directory should be created");
                let fixture_failure = scenario == "resources-templates-read";
                let checks = serde_json::to_vec(&serde_json::json!([{
                    "id": format!("{scenario}-check"),
                    "status": if fixture_failure { "FAILURE" } else { "SUCCESS" },
                    "errorMessage": fixture_failure
                        .then_some("Failed: MCP error -32002: Resource not found: test://static-text"),
                }]))
                .expect("official checks should serialize");
                fs::write(directory.join("checks.json"), checks)
                    .expect("official checks should be written");
            }
            Ok(())
        }

        fn capture_stdout(&self, _spec: &CommandSpec) -> PlatformResult<Vec<u8>> {
            Err(PlatformError::from(anyhow!("unexpected capture command")))
        }

        fn capture_output(&self, _spec: &CommandSpec) -> PlatformResult<CapturedOutput> {
            Err(PlatformError::from(anyhow!("unexpected capture command")))
        }

        fn run_to_log(&self, _spec: &CommandSpec, _log_path: &Path) -> PlatformResult<()> {
            Err(PlatformError::from(anyhow!("unexpected log command")))
        }
    }

    #[derive(Default)]
    struct DefaultsRunner {
        captures: RefCell<Vec<CommandSpec>>,
        runs: RefCell<Vec<CommandSpec>>,
    }

    #[derive(Default)]
    struct FailingNpxRunner {
        command: RefCell<Option<CommandSpec>>,
    }

    struct PublisherRunner {
        snapshot_present: bool,
        commands: RefCell<Vec<CommandSpec>>,
    }

    struct CleanupFailingRunner;

    struct TargetGuardRunner {
        other_running: bool,
        commands: RefCell<Vec<CommandSpec>>,
    }

    struct ExactPublisherRunner {
        virtual_hosts: BTreeSet<&'static str>,
        commands: RefCell<Vec<CommandSpec>>,
    }

    struct FixtureLifecycleRunner {
        events: Arc<Mutex<Vec<String>>>,
        fail_service_start: bool,
        fail_service_stop: bool,
        fail_gateway_restore: bool,
    }

    struct FixtureApiState {
        events: Arc<Mutex<Vec<String>>>,
        fail_cleanup: bool,
        expected_scoped_server_id: Option<String>,
        block_gateway: bool,
        finish_provision_after_interrupt: bool,
    }

    struct CancellingFixtureRunner {
        inner: FixtureLifecycleRunner,
    }

    impl ProcessRunner for CancellingFixtureRunner {
        fn run(&self, spec: &CommandSpec) -> PlatformResult<()> {
            self.inner.run(spec)
        }

        fn run_async<'a>(
            &'a self,
            spec: &'a CommandSpec,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PlatformResult<()>> + 'a>> {
            self.inner.run_async(spec)
        }

        fn run_async_cancellable<'a>(
            &'a self,
            spec: &'a CommandSpec,
            mut cancellation: tokio::sync::watch::Receiver<bool>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PlatformResult<()>> + 'a>> {
            if spec.program() != "npx" {
                return self.inner.run_async_cancellable(spec, cancellation);
            }
            Box::pin(async move {
                self.inner
                    .events
                    .lock()
                    .expect("event lock")
                    .push("child started".into());
                while !*cancellation.borrow_and_update() {
                    cancellation
                        .changed()
                        .await
                        .expect("runtime cancellation sender should remain alive");
                }
                self.inner
                    .events
                    .lock()
                    .expect("event lock")
                    .push("child terminated".into());
                Err(PlatformError::from(anyhow!("child cancelled and reaped")))
            })
        }

        fn capture_stdout(&self, spec: &CommandSpec) -> PlatformResult<Vec<u8>> {
            self.inner.capture_stdout(spec)
        }

        fn capture_output(&self, spec: &CommandSpec) -> PlatformResult<CapturedOutput> {
            self.inner.capture_output(spec)
        }

        fn run_to_log(&self, spec: &CommandSpec, log_path: &Path) -> PlatformResult<()> {
            self.inner.run_to_log(spec, log_path)
        }
    }

    impl ProcessRunner for FixtureLifecycleRunner {
        fn run(&self, spec: &CommandSpec) -> PlatformResult<()> {
            let arguments = spec
                .arguments()
                .iter()
                .map(|value| value.to_string_lossy())
                .collect::<Vec<_>>();
            if spec.program() == "npx" {
                self.events.lock().expect("event lock").push("npx".into());
                return Err(PlatformError::from(anyhow!("intentional npx failure")));
            }
            if arguments
                .iter()
                .any(|value| value == OFFICIAL_CONFORMANCE_SERVICE)
            {
                return Err(PlatformError::from(anyhow!(
                    "conformance Compose commands must use async runner"
                )));
            }
            if spec.program() == "docker" && arguments.iter().any(|value| value == "down") {
                let project = arguments
                    .windows(2)
                    .find(|values| values[0] == "-p")
                    .map_or("unknown", |values| values[1].as_ref());
                self.events
                    .lock()
                    .expect("event lock")
                    .push(format!("stack down {project}"));
            }
            if spec.program() == "docker"
                && arguments.iter().any(|value| value == "up")
                && !arguments
                    .iter()
                    .any(|value| value == OFFICIAL_CONFORMANCE_SERVICE)
            {
                let project = arguments
                    .windows(2)
                    .find(|values| values[0] == "-p")
                    .map_or("unknown", |values| values[1].as_ref());
                self.events
                    .lock()
                    .expect("event lock")
                    .push(format!("stack up {project}"));
            }
            Ok(())
        }

        fn run_async<'a>(
            &'a self,
            spec: &'a CommandSpec,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = PlatformResult<()>> + 'a>> {
            if spec.program() != "npx" {
                let arguments = spec
                    .arguments()
                    .iter()
                    .map(|value| value.to_string_lossy())
                    .collect::<Vec<_>>();
                if spec
                    .arguments()
                    .iter()
                    .any(|value| value == OFFICIAL_CONFORMANCE_SERVICE)
                {
                    if arguments.iter().any(|value| value == "build") {
                        assert!(
                            !arguments.iter().any(|value| value == "gateway"),
                            "fixture build must not rebuild gateway"
                        );
                        let mut events = self.events.lock().expect("event lock");
                        let fail = events.iter().any(|event| event == "fail fixture build");
                        events.push("compose fixture build".into());
                        drop(events);
                        if fail {
                            return Box::pin(async {
                                Err(PlatformError::from(anyhow!("fixture build failed")))
                            });
                        }
                        return Box::pin(async { Ok(()) });
                    }
                    if arguments.iter().any(|value| value == "up") {
                        assert!(
                            !arguments.iter().any(|value| value == "--build"),
                            "fixture up must use the separately built image"
                        );
                        let gateway = arguments
                            .iter()
                            .position(|value| value == "gateway")
                            .expect("conformance start must recreate gateway");
                        let server = arguments
                            .iter()
                            .position(|value| value == OFFICIAL_CONFORMANCE_SERVICE)
                            .expect("conformance server target");
                        assert!(gateway < server, "gateway must precede conformance server");
                    }
                    let event = if arguments.iter().any(|value| value == "up") {
                        "compose fixture up"
                    } else {
                        "compose fixture rm"
                    };
                    self.events.lock().expect("event lock").push(event.into());
                    if self.fail_service_start && event.ends_with("up") {
                        return Box::pin(async {
                            Err(PlatformError::from(anyhow!("fixture start failed")))
                        });
                    }
                    if self.fail_service_stop && event.ends_with("rm") {
                        return Box::pin(async {
                            Err(PlatformError::from(anyhow!("fixture stop failed")))
                        });
                    }
                    return Box::pin(async { Ok(()) });
                }
                if arguments.iter().any(|value| value == "up")
                    && arguments.iter().any(|value| value == "gateway")
                {
                    assert!(arguments.iter().any(|value| value == "--wait"));
                    assert!(
                        !arguments
                            .iter()
                            .any(|value| value.ends_with("docker-compose.cf-conformance.yaml"))
                    );
                    self.events
                        .lock()
                        .expect("event lock")
                        .push("compose gateway restore".into());
                    if self.fail_gateway_restore {
                        return Box::pin(async {
                            Err(PlatformError::from(anyhow!("gateway restore failed")))
                        });
                    }
                    return Box::pin(async { Ok(()) });
                }
                return Box::pin(async move { self.run(spec) });
            }
            Box::pin(async move {
                self.events.lock().expect("event lock").push("npx".into());
                let endpoint = spec
                    .arguments()
                    .windows(2)
                    .find(|values| values[0] == "--url")
                    .map(|values| values[1].to_string_lossy().into_owned())
                    .expect("official runner URL should be present");
                reqwest::Client::builder()
                    .timeout(Duration::from_secs(2))
                    .build()
                    .expect("bounded proxy test client should build")
                    .get(endpoint)
                    .send()
                    .await
                    .expect("official proxy request should complete");
                self.events
                    .lock()
                    .expect("event lock")
                    .push("official proxy complete".into());
                Err(PlatformError::from(anyhow!("intentional npx failure")))
            })
        }

        fn capture_stdout(&self, spec: &CommandSpec) -> PlatformResult<Vec<u8>> {
            match spec.program().to_str() {
                Some("docker") if spec.arguments().iter().any(|value| value == "config") => {
                    Ok(serde_json::to_vec(&serde_json::json!({
                        "services": {
                            "fast_time_server": {
                                "image": "ghcr.io/ibm/cfex-mcp-fast-time-server:latest"
                            },
                            "register_fast_time": {
                                "command": [
                                    "wait for http://fast_time_server:9080/health",
                                    "register http://fast_time_server:9080/mcp"
                                ]
                            }
                        }
                    }))
                    .expect("valid Compose JSON"))
                }
                Some("docker")
                    if spec.arguments().first().is_some_and(|value| value == "ps")
                        && spec.arguments().iter().any(|value| {
                            value
                                .to_string_lossy()
                                .contains("com.docker.compose.service=redis")
                        }) =>
                {
                    Ok(b"redis-container\n".to_vec())
                }
                Some("docker")
                    if spec
                        .arguments()
                        .first()
                        .is_some_and(|value| value == "exec") =>
                {
                    let server_id = spec
                        .arguments()
                        .last()
                        .and_then(|value| value.to_str())
                        .expect("publisher server ID should be UTF-8");
                    self.events
                        .lock()
                        .expect("event lock")
                        .push(format!("publisher {server_id}"));
                    if server_id == "9779b6698cbd4b4995ee04a4fab38737" {
                        return Err(PlatformError::from(anyhow!(
                            "automatic fixture workflow probed Fast Time publisher snapshot"
                        )));
                    }
                    Ok(b"1\n".to_vec())
                }
                Some("docker") => Ok(Vec::new()),
                Some("git") => Ok(b"revision\n".to_vec()),
                Some("id") if spec.arguments().first().is_some_and(|value| value == "-u") => {
                    Ok(b"501\n".to_vec())
                }
                Some("id") => Ok(b"20\n".to_vec()),
                _ => Ok(Vec::new()),
            }
        }

        fn capture_output(&self, _spec: &CommandSpec) -> PlatformResult<CapturedOutput> {
            Ok(CapturedOutput::new(Vec::new(), Vec::new()))
        }

        fn run_to_log(&self, _spec: &CommandSpec, _log_path: &Path) -> PlatformResult<()> {
            Ok(())
        }
    }

    impl ProcessRunner for DefaultsRunner {
        fn run(&self, spec: &CommandSpec) -> PlatformResult<()> {
            self.runs.borrow_mut().push(spec.clone());
            Ok(())
        }

        fn capture_stdout(&self, spec: &CommandSpec) -> PlatformResult<Vec<u8>> {
            self.captures.borrow_mut().push(spec.clone());
            match (
                spec.program().to_string_lossy().as_ref(),
                spec.arguments()
                    .first()
                    .map(|value| value.to_string_lossy()),
            ) {
                ("docker", Some(argument)) if argument == "info" => Ok(b"6\n".to_vec()),
                ("id", Some(argument)) if argument == "-u" => Ok(b"501\n".to_vec()),
                ("id", Some(argument)) if argument == "-g" => Ok(b"20\n".to_vec()),
                _ => Err(PlatformError::from(anyhow!("unexpected capture command"))),
            }
        }

        fn capture_output(&self, _spec: &CommandSpec) -> PlatformResult<CapturedOutput> {
            Ok(CapturedOutput::new(Vec::new(), Vec::new()))
        }

        fn run_to_log(&self, _spec: &CommandSpec, _log_path: &Path) -> PlatformResult<()> {
            Ok(())
        }
    }

    impl ProcessRunner for FailingNpxRunner {
        fn run(&self, spec: &CommandSpec) -> PlatformResult<()> {
            self.command.replace(Some(spec.clone()));
            Err(PlatformError::from(anyhow!("deliberate npx failure")))
        }

        fn capture_stdout(&self, _spec: &CommandSpec) -> PlatformResult<Vec<u8>> {
            Err(PlatformError::from(anyhow!("unexpected capture command")))
        }

        fn capture_output(&self, _spec: &CommandSpec) -> PlatformResult<CapturedOutput> {
            Err(PlatformError::from(anyhow!("unexpected capture command")))
        }

        fn run_to_log(&self, _spec: &CommandSpec, _log_path: &Path) -> PlatformResult<()> {
            Err(PlatformError::from(anyhow!("unexpected log command")))
        }
    }

    impl ProcessRunner for PublisherRunner {
        fn run(&self, spec: &CommandSpec) -> PlatformResult<()> {
            self.commands.borrow_mut().push(spec.clone());
            Ok(())
        }

        fn capture_stdout(&self, spec: &CommandSpec) -> PlatformResult<Vec<u8>> {
            self.commands.borrow_mut().push(spec.clone());
            let first = spec
                .arguments()
                .first()
                .and_then(|argument| argument.to_str());
            match (spec.program().to_str(), first) {
                (Some("docker"), Some("ps")) => Ok(b"redis-container\n".to_vec()),
                (Some("docker"), Some("exec")) if self.snapshot_present => Ok(b"1\n".to_vec()),
                (Some("docker"), Some("exec")) => Ok(b"0\n".to_vec()),
                (Some("docker"), Some("info")) => Ok(b"6\n".to_vec()),
                (Some("id"), Some("-u")) => Ok(b"501\n".to_vec()),
                (Some("id"), Some("-g")) => Ok(b"20\n".to_vec()),
                (Some("git"), _) if spec.arguments().iter().any(|arg| arg == "rev-parse") => {
                    Ok(b"revision\n".to_vec())
                }
                (Some("git"), _) if spec.arguments().iter().any(|arg| arg == "symbolic-ref") => {
                    Ok(b"main\n".to_vec())
                }
                _ => Err(PlatformError::from(anyhow!("unexpected publisher command"))),
            }
        }

        fn capture_output(&self, _spec: &CommandSpec) -> PlatformResult<CapturedOutput> {
            Ok(CapturedOutput::new(Vec::new(), Vec::new()))
        }

        fn run_to_log(&self, _spec: &CommandSpec, _log_path: &Path) -> PlatformResult<()> {
            Ok(())
        }
    }

    impl ProcessRunner for CleanupFailingRunner {
        fn run(&self, spec: &CommandSpec) -> PlatformResult<()> {
            if spec.arguments().iter().any(|argument| argument == "down") {
                return Err(PlatformError::from(anyhow!("deliberate cleanup failure")));
            }
            Ok(())
        }

        fn capture_stdout(&self, spec: &CommandSpec) -> PlatformResult<Vec<u8>> {
            match (
                spec.program().to_str(),
                spec.arguments().first().and_then(|arg| arg.to_str()),
            ) {
                (Some("docker"), Some("info")) => Ok(b"6\n".to_vec()),
                (Some("docker"), Some("ps" | "network" | "volume")) => Ok(Vec::new()),
                (Some("id"), Some("-u")) => Ok(b"501\n".to_vec()),
                (Some("id"), Some("-g")) => Ok(b"20\n".to_vec()),
                _ => Err(PlatformError::from(anyhow!("unexpected cleanup capture"))),
            }
        }

        fn capture_output(&self, _spec: &CommandSpec) -> PlatformResult<CapturedOutput> {
            Ok(CapturedOutput::new(Vec::new(), Vec::new()))
        }

        fn run_to_log(&self, _spec: &CommandSpec, _log_path: &Path) -> PlatformResult<()> {
            Ok(())
        }
    }

    impl ProcessRunner for TargetGuardRunner {
        fn run(&self, _spec: &CommandSpec) -> PlatformResult<()> {
            Ok(())
        }

        fn capture_stdout(&self, spec: &CommandSpec) -> PlatformResult<Vec<u8>> {
            self.commands.borrow_mut().push(spec.clone());
            let first = spec
                .arguments()
                .first()
                .and_then(|argument| argument.to_str());
            match (spec.program().to_str(), first) {
                (Some("docker"), Some("ps"))
                    if spec.arguments().iter().any(|argument| {
                        argument
                            .to_string_lossy()
                            .contains("com.docker.compose.service=redis")
                    }) =>
                {
                    Ok(b"redis-container\n".to_vec())
                }
                (Some("docker"), Some("ps")) if self.other_running => {
                    Ok(b"other-stack-container\n".to_vec())
                }
                (Some("docker"), Some("ps")) => Ok(Vec::new()),
                (Some("docker"), Some("exec")) => Ok(b"1\n".to_vec()),
                _ => Err(PlatformError::from(anyhow!(
                    "unexpected target guard capture"
                ))),
            }
        }

        fn capture_output(&self, _spec: &CommandSpec) -> PlatformResult<CapturedOutput> {
            Ok(CapturedOutput::new(Vec::new(), Vec::new()))
        }

        fn run_to_log(&self, _spec: &CommandSpec, _log_path: &Path) -> PlatformResult<()> {
            Ok(())
        }
    }

    impl ProcessRunner for ExactPublisherRunner {
        fn run(&self, _spec: &CommandSpec) -> PlatformResult<()> {
            Ok(())
        }

        fn capture_stdout(&self, spec: &CommandSpec) -> PlatformResult<Vec<u8>> {
            self.commands.borrow_mut().push(spec.clone());
            match spec
                .arguments()
                .first()
                .and_then(|argument| argument.to_str())
            {
                Some("ps") => Ok(b"redis-container\n".to_vec()),
                Some("exec") => {
                    assert_eq!(
                        spec.arguments().get(4),
                        Some(&OsString::from(PUBLISHER_SNAPSHOT_LUA))
                    );
                    let target = spec
                        .arguments()
                        .last()
                        .and_then(|argument| argument.to_str())
                        .expect("publisher target should be UTF-8");
                    Ok(if self.virtual_hosts.contains(target) {
                        b"1\n".to_vec()
                    } else {
                        b"0\n".to_vec()
                    })
                }
                _ => Err(PlatformError::from(anyhow!(
                    "unexpected exact publisher command"
                ))),
            }
        }

        fn capture_output(&self, _spec: &CommandSpec) -> PlatformResult<CapturedOutput> {
            Ok(CapturedOutput::new(Vec::new(), Vec::new()))
        }

        fn run_to_log(&self, _spec: &CommandSpec, _log_path: &Path) -> PlatformResult<()> {
            Ok(())
        }
    }

    fn test_config(overrides: impl IntoIterator<Item = (&'static str, &'static str)>) -> AppConfig {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut environment = Environment::new();
        environment.insert(
            OsString::from("CF_INTEGRATION_ROOT"),
            root.as_os_str().to_owned(),
        );
        environment.extend(
            overrides
                .into_iter()
                .map(|(key, value)| (OsString::from(key), OsString::from(value))),
        );
        AppConfig::load(&environment, &root.join("cf-integration"), root)
            .expect("test configuration should load")
            .config
    }

    fn fixture_test_config(
        base_url: &str,
        integration_dir: &Path,
        controlplane_dir: &Path,
    ) -> AppConfig {
        fixture_test_config_with_server(base_url, integration_dir, controlplane_dir, None)
    }

    fn fixture_test_config_with_server(
        base_url: &str,
        integration_dir: &Path,
        controlplane_dir: &Path,
        caller_server: Option<(&str, &str)>,
    ) -> AppConfig {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut environment = Environment::new();
        for (key, value) in [
            ("CF_INTEGRATION_ROOT", root.as_os_str()),
            ("CF_INTEGRATION_DIR", integration_dir.as_os_str()),
            ("CF_CONTROLPLANE_DIR", controlplane_dir.as_os_str()),
            ("MCP_CLI_BASE_URL", OsStr::new(base_url)),
            ("MCPGATEWAY_BEARER_TOKEN", OsStr::new("")),
            ("CF_FRESH_STACK", OsStr::new("false")),
            ("CF_COMPOSE_BUILD", OsStr::new("false")),
        ] {
            environment.insert(OsString::from(key), value.to_owned());
        }
        if let Some((key, value)) = caller_server {
            environment.insert(OsString::from(key), OsString::from(value));
        }
        AppConfig::load(&environment, &root.join("cf-integration"), root)
            .expect("fixture test configuration should load")
            .config
    }

    async fn fixture_api(
        State(state): State<Arc<FixtureApiState>>,
        method: Method,
        uri: Uri,
        headers: axum::http::HeaderMap,
    ) -> (StatusCode, axum::Json<serde_json::Value>) {
        let event = format!("{} {}", method, uri.path());
        let fail = {
            let mut events = state.events.lock().expect("event lock");
            let fail = state.fail_cleanup
                && method == Method::DELETE
                && uri.path().starts_with("/servers/")
                && events.iter().any(|previous| previous == "POST /servers");
            events.push(event);
            fail
        };
        if uri.path().ends_with("/mcp") {
            let authorization = headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok());
            if authorization.is_none() {
                return (StatusCode::UNAUTHORIZED, axum::Json(serde_json::json!({})));
            }
            let has_expected_scope = authorization
                .and_then(|value| value.strip_prefix("Bearer "))
                .and_then(|token| {
                    let mut validation =
                        jsonwebtoken::Validation::new(jsonwebtoken::Algorithm::HS256);
                    validation.set_audience(&["mcpgateway-api"]);
                    jsonwebtoken::decode::<serde_json::Value>(
                        token,
                        &jsonwebtoken::DecodingKey::from_secret(
                            b"my-test-key-but-now-longer-than-32-bytes",
                        ),
                        &validation,
                    )
                    .ok()
                })
                .and_then(|data| {
                    data.claims["scopes"]["server_id"]
                        .as_str()
                        .map(str::to_owned)
                })
                == state.expected_scoped_server_id;
            if has_expected_scope && state.expected_scoped_server_id.is_some() {
                state
                    .events
                    .lock()
                    .expect("event lock")
                    .push(format!("scoped {}", uri.path()));
            }
            if state.block_gateway && method == Method::POST {
                state
                    .events
                    .lock()
                    .expect("event lock")
                    .push("gateway started".into());
                std::future::pending::<()>().await;
            }
        }
        if state.finish_provision_after_interrupt && method == Method::GET && uri.path() == "/tools"
        {
            state
                .events
                .lock()
                .expect("event lock")
                .push("provision finishing".into());
            loop {
                if state
                    .events
                    .lock()
                    .expect("event lock")
                    .iter()
                    .any(|event| event == "interrupt completed")
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        }
        if fail {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({})),
            );
        }
        let resource_exists = |created: &str, deleted_prefix: &str| {
            let events = state.events.lock().expect("event lock");
            let created = events.iter().rposition(|event| event == created);
            let deleted = events
                .iter()
                .rposition(|event| event.starts_with(deleted_prefix));
            created.is_some_and(|created| deleted.is_none_or(|deleted| created > deleted))
        };
        let (status, body) = match (method, uri.path()) {
            (Method::GET, "/servers") => (
                StatusCode::OK,
                if resource_exists("POST /servers", "DELETE /servers/") {
                    serde_json::json!([{
                        "id": crate::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID,
                        "name": "Official MCP Conformance Server",
                        "description": "Virtual server for the pinned official MCP conformance fixture."
                    }])
                } else {
                    serde_json::json!([])
                },
            ),
            (Method::GET, "/gateways") => (
                StatusCode::OK,
                if resource_exists("POST /gateways", "DELETE /gateways/") {
                    serde_json::json!([{
                        "id": "fixture-gateway",
                        "name": crate::conformance_fixture::OFFICIAL_CONFORMANCE_GATEWAY_NAME,
                        "url": crate::conformance_fixture::OFFICIAL_CONFORMANCE_BACKEND_URL,
                        "transport": "STREAMABLEHTTP",
                        "description": "Official MCP conformance fixture"
                    }])
                } else {
                    serde_json::json!([])
                },
            ),
            (Method::POST, "/gateways") => (
                StatusCode::OK,
                serde_json::json!({
                    "id": "fixture-gateway",
                    "name": crate::conformance_fixture::OFFICIAL_CONFORMANCE_GATEWAY_NAME,
                    "url": crate::conformance_fixture::OFFICIAL_CONFORMANCE_BACKEND_URL,
                    "transport": "STREAMABLEHTTP",
                    "description": "Official MCP conformance fixture",
                }),
            ),
            (Method::GET, "/tools") => (
                StatusCode::OK,
                serde_json::json!([{
                    "id": "tool-id", "name": "test_simple_text", "gateway_id": "fixture-gateway"
                }]),
            ),
            (Method::GET, "/resources") => (
                StatusCode::OK,
                serde_json::json!([{
                    "id": "resource-id", "uri": "test://static-text", "gateway_id": "fixture-gateway"
                }]),
            ),
            (Method::GET, "/prompts") => (
                StatusCode::OK,
                serde_json::json!([{
                    "id": "prompt-id", "name": "test_simple_prompt", "gateway_id": "fixture-gateway"
                }]),
            ),
            (_, path) if path.ends_with("/mcp") => (StatusCode::OK, serde_json::json!({})),
            (Method::DELETE, path)
                if path.starts_with("/servers/") || path.starts_with("/gateways/") =>
            {
                (StatusCode::OK, serde_json::json!({}))
            }
            (Method::POST, "/servers") => (StatusCode::OK, serde_json::json!({})),
            (Method::POST, path) if path.ends_with("/tools/refresh") => {
                (StatusCode::OK, serde_json::json!({}))
            }
            _ => (StatusCode::NOT_FOUND, serde_json::json!({})),
        };
        (status, axum::Json(body))
    }

    #[test]
    fn official_fixture_is_automatic_only_for_default_conformance_workflows() {
        assert!(uses_automatic_conformance_fixture(true, None));
        assert!(!uses_automatic_conformance_fixture(false, None));
        assert!(!uses_automatic_conformance_fixture(
            true,
            Some("caller-server")
        ));
        assert_eq!(
            selected_compliance_server_id(true, "fast-time", Some("official")),
            "official"
        );
        assert_eq!(
            selected_compliance_server_id(false, "caller-server", Some("official")),
            "caller-server"
        );

        let runtime = RuntimeExecutor::new(
            test_config([
                ("MCP_SERVER_ID", "primary-env-server"),
                ("MCP_VIRTUAL_SERVER_ID", "legacy-env-server"),
            ]),
            DefaultsRunner::default(),
        );
        assert_eq!(
            runtime.caller_managed_server_id(Some("explicit-server")),
            Some("explicit-server")
        );
        assert_eq!(
            runtime.caller_managed_server_id(None),
            Some("primary-env-server")
        );
        assert!(!uses_automatic_conformance_fixture(
            true,
            runtime.caller_managed_server_id(None)
        ));
    }

    #[tokio::test]
    async fn environment_server_ids_bypass_fixture_and_target_caller_server() {
        for (key, server_id) in [
            ("MCP_SERVER_ID", "primary-env-server"),
            ("MCP_VIRTUAL_SERVER_ID", "legacy-env-server"),
        ] {
            let events = Arc::new(Mutex::new(Vec::new()));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("fixture API listener should bind");
            let base_url = format!("http://{}", listener.local_addr().expect("API address"));
            let app = Router::new()
                .fallback(any(fixture_api))
                .with_state(Arc::new(FixtureApiState {
                    events: Arc::clone(&events),
                    fail_cleanup: false,
                    expected_scoped_server_id: None,
                    block_gateway: false,
                    finish_provision_after_interrupt: false,
                }));
            let server = tokio::spawn(async move {
                axum::serve(listener, app)
                    .await
                    .expect("fixture API should run");
            });
            let state = tempfile::tempdir().expect("temporary integration state");
            let controlplane = state.path().join("mcp-context-forge");
            fs::create_dir_all(controlplane.join(".git")).expect("fake checkout should exist");
            let reports = tempfile::tempdir().expect("temporary compliance artifacts");
            let runtime = RuntimeExecutor::new(
                fixture_test_config_with_server(
                    &base_url,
                    state.path(),
                    &controlplane,
                    Some((key, server_id)),
                ),
                FixtureLifecycleRunner {
                    events: Arc::clone(&events),
                    fail_service_start: false,
                    fail_service_stop: false,
                    fail_gateway_restore: false,
                },
            );
            let common = ResolvedComplianceCommon {
                mode: ComplianceMode::Dataplane,
                start: false,
                server_id: None,
                spec_version: "2025-11-25".to_owned(),
                results_dir: Some(reports.path().to_owned()),
            };

            runtime
                .run_compliance_workflow(&common, Some(ConformanceSuite::Active), false, None)
                .await
                .expect_err("intentional official runner failure should remain");

            let metadata_path =
                CompliancePaths::new(reports.path(), reports.path().join("reports"))
                    .conformance_mode(BaselineTarget::Dataplane)
                    .metadata;
            let metadata: serde_json::Value = serde_json::from_slice(
                &fs::read(metadata_path).expect("caller-managed metadata should be written"),
            )
            .expect("caller-managed metadata should parse");
            assert!(metadata.get("fixture").is_none());

            let events = events.lock().expect("event lock");
            assert!(
                events
                    .iter()
                    .any(|event| { event.ends_with(&format!("/servers/{server_id}/mcp")) })
            );
            assert!(
                events
                    .iter()
                    .any(|event| event == &format!("publisher {server_id}"))
            );
            assert!(events.iter().all(|event| {
                !event.starts_with("compose fixture")
                    && event != "POST /gateways"
                    && event != "POST /servers"
            }));
            server.abort();
        }
    }

    #[tokio::test]
    async fn remote_base_url_rejects_automatic_fixture_before_service_or_api_calls() {
        let state = tempfile::tempdir().expect("temporary integration state");
        let controlplane = state.path().join("mcp-context-forge");
        fs::create_dir_all(controlplane.join(".git")).expect("fake checkout should exist");
        let reports = tempfile::tempdir().expect("temporary compliance artifacts");
        let events = Arc::new(Mutex::new(Vec::new()));
        let runtime = RuntimeExecutor::new(
            fixture_test_config("https://shared.example.test", state.path(), &controlplane),
            FixtureLifecycleRunner {
                events: Arc::clone(&events),
                fail_service_start: false,
                fail_service_stop: false,
                fail_gateway_restore: false,
            },
        );
        let common = ResolvedComplianceCommon {
            mode: ComplianceMode::All,
            start: true,
            server_id: None,
            spec_version: "2025-11-25".to_owned(),
            results_dir: Some(reports.path().to_owned()),
        };

        let error = runtime
            .run_compliance_workflow(&common, Some(ConformanceSuite::Active), false, None)
            .await
            .expect_err("remote deployment must require caller ownership");

        assert!(error.to_string().contains("loopback MCP_CLI_BASE_URL"));
        assert!(error.to_string().contains("--server-id"));
        assert!(events.lock().expect("event lock").is_empty());
    }

    #[test]
    fn automatic_fixture_accepts_localhost_and_ip_loopback_urls() {
        for base_url in [
            "http://localhost:4444",
            "http://127.0.0.1:4444",
            "https://[::1]:4444",
        ] {
            let runtime = RuntimeExecutor::new(
                test_config([("MCP_CLI_BASE_URL", base_url)]),
                DefaultsRunner::default(),
            );
            runtime
                .require_loopback_fixture_base_url()
                .unwrap_or_else(|error| panic!("{base_url} should be loopback: {error}"));
        }
    }

    #[tokio::test]
    async fn default_conformance_provisions_and_cleans_fixture_after_runner_failure() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture API listener should bind");
        let base_url = format!(
            "http://{}",
            listener.local_addr().expect("fixture API address")
        );
        let app = Router::new()
            .fallback(any(fixture_api))
            .with_state(Arc::new(FixtureApiState {
                events: Arc::clone(&events),
                fail_cleanup: false,
                expected_scoped_server_id: None,
                block_gateway: false,
                finish_provision_after_interrupt: false,
            }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("fixture API should run");
        });
        let state = tempfile::tempdir().expect("temporary integration state");
        let controlplane = state.path().join("mcp-context-forge");
        fs::create_dir_all(controlplane.join(".git")).expect("fake checkout should exist");
        let reports = tempfile::tempdir().expect("temporary compliance artifacts");
        let runtime = RuntimeExecutor::new(
            fixture_test_config(&base_url, state.path(), &controlplane),
            FixtureLifecycleRunner {
                events: Arc::clone(&events),
                fail_service_start: false,
                fail_service_stop: false,
                fail_gateway_restore: false,
            },
        );
        let common = ResolvedComplianceCommon {
            mode: ComplianceMode::Controlplane,
            start: false,
            server_id: None,
            spec_version: "2025-11-25".to_owned(),
            results_dir: Some(reports.path().to_owned()),
        };

        let error = runtime
            .run_compliance_workflow(&common, Some(ConformanceSuite::Active), false, None)
            .await
            .expect_err("intentional runner failure should remain primary");

        assert!(error.to_string().contains("intentional npx failure"));
        let metadata = read_run_metadata(
            &CompliancePaths::new(reports.path(), reports.path().join("reports"))
                .conformance_mode(BaselineTarget::Controlplane)
                .metadata,
        )
        .expect("automatic fixture metadata should be readable");
        assert_eq!(
            metadata.fixture,
            Some(ConformanceFixtureMetadata {
                repository: crate::conformance_fixture::OFFICIAL_CONFORMANCE_REPOSITORY.to_owned(),
                revision: crate::conformance_fixture::OFFICIAL_CONFORMANCE_REVISION.to_owned(),
                server_id: crate::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID.to_owned(),
            })
        );
        let events = events.lock().expect("event lock");
        let up = events
            .iter()
            .position(|event| event == "compose fixture up")
            .expect("fixture service should start");
        let create = events
            .iter()
            .position(|event| event == "POST /gateways")
            .expect("fixture gateway should be provisioned");
        let runner = events
            .iter()
            .position(|event| event == "npx")
            .expect("official runner should execute");
        let cleanup_delete = events
            .iter()
            .enumerate()
            .skip(runner + 1)
            .find(|(_, event)| event.starts_with("DELETE /servers/"))
            .map(|(index, _)| index)
            .expect("fixture server should be deleted after the run");
        let rm = events
            .iter()
            .position(|event| event == "compose fixture rm")
            .expect("fixture service should be removed");
        let restore = events
            .iter()
            .position(|event| event == "compose gateway restore")
            .expect("base gateway configuration should be restored");
        assert!(
            up < create
                && create < runner
                && runner < cleanup_delete
                && cleanup_delete < rm
                && rm < restore
        );
        assert!(events.iter().any(|event| {
            event
                == &format!(
                    "DELETE /servers/{}",
                    crate::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID
                )
        }));
        server.abort();
    }

    #[tokio::test]
    async fn fixture_fake_api_returns_not_found_for_unknown_routes() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture API listener should bind");
        let endpoint = format!(
            "http://{}/unknown",
            listener.local_addr().expect("fixture API address")
        );
        let app = Router::new()
            .fallback(any(fixture_api))
            .with_state(Arc::new(FixtureApiState {
                events,
                fail_cleanup: false,
                expected_scoped_server_id: None,
                block_gateway: false,
                finish_provision_after_interrupt: false,
            }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("fixture API should run");
        });

        let response = reqwest::get(endpoint)
            .await
            .expect("unknown fixture API request should complete");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        server.abort();
    }

    #[tokio::test]
    async fn combined_workflow_routes_official_and_gateway_checks_to_scoped_fixture() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let api_state = Arc::new(FixtureApiState {
            events: Arc::clone(&events),
            fail_cleanup: false,
            expected_scoped_server_id: Some(
                crate::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID.to_owned(),
            ),
            block_gateway: false,
            finish_provision_after_interrupt: false,
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture API listener should bind");
        let base_url = format!("http://{}", listener.local_addr().expect("API address"));
        let app = Router::new()
            .fallback(any(fixture_api))
            .with_state(Arc::clone(&api_state));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("fixture API should run");
        });
        let state = tempfile::tempdir().expect("temporary integration state");
        let controlplane = state.path().join("mcp-context-forge");
        fs::create_dir_all(controlplane.join(".git")).expect("fake checkout should exist");
        let reports = tempfile::tempdir().expect("temporary compliance artifacts");
        let runtime = RuntimeExecutor::new(
            fixture_test_config(&base_url, state.path(), &controlplane),
            FixtureLifecycleRunner {
                events: Arc::clone(&events),
                fail_service_start: false,
                fail_service_stop: false,
                fail_gateway_restore: false,
            },
        );
        let common = ResolvedComplianceCommon {
            mode: ComplianceMode::Dataplane,
            start: false,
            server_id: None,
            spec_version: "2025-11-25".to_owned(),
            results_dir: Some(reports.path().to_owned()),
        };

        runtime
            .run_compliance_workflow(&common, Some(ConformanceSuite::Active), true, None)
            .await
            .expect_err("intentional official and gateway responses should fail");

        let events = events.lock().expect("event lock");
        let npx = events
            .iter()
            .position(|event| event == "npx")
            .expect("npx event");
        let official_complete = events
            .iter()
            .position(|event| event == "official proxy complete")
            .expect("official proxy boundary");
        let expected_path = format!(
            "/servers/{}/mcp",
            crate::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID
        );
        assert!(
            events[npx + 1..official_complete]
                .iter()
                .any(|event| event.ends_with(&expected_path))
        );
        assert!(
            events[official_complete + 1..]
                .iter()
                .any(|event| event.ends_with(&expected_path))
        );
        let scoped_event = format!("scoped {expected_path}");
        assert!(
            events[npx + 1..official_complete]
                .iter()
                .any(|event| event == &scoped_event),
            "{events:?}"
        );
        assert!(
            events[official_complete + 1..]
                .iter()
                .any(|event| event == &scoped_event)
        );
        assert!(events.iter().all(|event| {
            !event.contains(&format!("/servers/{}/mcp", runtime.default_server_id()))
        }));
        assert_eq!(
            events
                .iter()
                .filter(|event| event.starts_with("publisher "))
                .cloned()
                .collect::<Vec<_>>(),
            [format!(
                "publisher {}",
                crate::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID
            )]
        );
        server.abort();
    }

    #[tokio::test]
    async fn interruption_during_dataplane_provision_skips_publisher_and_cleans_fixture() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture API listener should bind");
        let base_url = format!("http://{}", listener.local_addr().expect("API address"));
        let app = Router::new()
            .fallback(any(fixture_api))
            .with_state(Arc::new(FixtureApiState {
                events: Arc::clone(&events),
                fail_cleanup: false,
                expected_scoped_server_id: None,
                block_gateway: false,
                finish_provision_after_interrupt: true,
            }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("fixture API should run");
        });
        let state = tempfile::tempdir().expect("temporary integration state");
        let controlplane = state.path().join("mcp-context-forge");
        fs::create_dir_all(controlplane.join(".git")).expect("fake checkout should exist");
        let reports = tempfile::tempdir().expect("temporary compliance artifacts");
        let runtime = RuntimeExecutor::new(
            fixture_test_config(&base_url, state.path(), &controlplane),
            FixtureLifecycleRunner {
                events: Arc::clone(&events),
                fail_service_start: false,
                fail_service_stop: false,
                fail_gateway_restore: false,
            },
        );
        let common = ResolvedComplianceCommon {
            mode: ComplianceMode::Dataplane,
            start: false,
            server_id: None,
            spec_version: "2025-11-25".to_owned(),
            results_dir: Some(reports.path().to_owned()),
        };
        let interrupt_events = Arc::clone(&events);
        let interrupt = async move {
            loop {
                if interrupt_events
                    .lock()
                    .expect("event lock")
                    .iter()
                    .any(|event| event == "provision finishing")
                {
                    interrupt_events
                        .lock()
                        .expect("event lock")
                        .push("interrupt completed".into());
                    return;
                }
                tokio::task::yield_now().await;
            }
        };

        let error = runtime
            .run_compliance_workflow_with_interrupt(
                &common,
                Some(ConformanceSuite::Active),
                false,
                None,
                interrupt,
            )
            .await
            .expect_err("injected interruption should remain primary");

        assert!(error.to_string().contains("interrupted by Ctrl-C"));
        let events = events.lock().expect("event lock");
        assert!(events.iter().all(|event| !event.starts_with("publisher ")));
        assert!(events.iter().all(|event| event != "npx"));
        let interrupt = events
            .iter()
            .position(|event| event == "interrupt completed")
            .expect("interrupt should complete during provisioning");
        let delete = events
            .iter()
            .rposition(|event| event.starts_with("DELETE /"))
            .expect("owned fixture API cleanup should run");
        let rm = events
            .iter()
            .position(|event| event == "compose fixture rm")
            .expect("fixture service cleanup should run");
        assert!(interrupt < delete && delete < rm, "{events:?}");
        server.abort();
    }

    #[tokio::test]
    async fn interruption_after_provision_cleans_api_before_async_service_removal() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture API listener should bind");
        let base_url = format!("http://{}", listener.local_addr().expect("API address"));
        let app = Router::new()
            .fallback(any(fixture_api))
            .with_state(Arc::new(FixtureApiState {
                events: Arc::clone(&events),
                fail_cleanup: false,
                expected_scoped_server_id: None,
                block_gateway: false,
                finish_provision_after_interrupt: false,
            }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("fixture API should run");
        });
        let state = tempfile::tempdir().expect("temporary integration state");
        let controlplane = state.path().join("mcp-context-forge");
        fs::create_dir_all(controlplane.join(".git")).expect("fake checkout should exist");
        fs::write(controlplane.join("docker-compose.yml"), "services: {}\n")
            .expect("fake Compose file should exist for stack cleanup");
        let reports = tempfile::tempdir().expect("temporary compliance artifacts");
        let runtime = RuntimeExecutor::new(
            fixture_test_config(&base_url, state.path(), &controlplane),
            CancellingFixtureRunner {
                inner: FixtureLifecycleRunner {
                    events: Arc::clone(&events),
                    fail_service_start: false,
                    fail_service_stop: false,
                    fail_gateway_restore: false,
                },
            },
        );
        let common = ResolvedComplianceCommon {
            mode: ComplianceMode::All,
            start: true,
            server_id: None,
            spec_version: "2025-11-25".to_owned(),
            results_dir: Some(reports.path().to_owned()),
        };
        let interrupt_events = Arc::clone(&events);
        let interrupt = async move {
            loop {
                if interrupt_events
                    .lock()
                    .expect("event lock")
                    .iter()
                    .any(|event| event == "child started")
                {
                    return;
                }
                tokio::task::yield_now().await;
            }
        };

        let error = runtime
            .run_compliance_workflow_with_interrupt(
                &common,
                Some(ConformanceSuite::Active),
                false,
                None,
                interrupt,
            )
            .await
            .expect_err("injected interruption should remain primary");

        assert!(error.to_string().contains("interrupted by Ctrl-C"));
        let events = events.lock().expect("event lock");
        let create = events
            .iter()
            .position(|event| event == "POST /servers")
            .expect("fixture should finish provisioning");
        let child_terminated = events
            .iter()
            .position(|event| event == "child terminated")
            .expect("active child should terminate before fixture cleanup");
        let delete = events[create + 1..]
            .iter()
            .position(|event| event.starts_with("DELETE /servers/"))
            .map(|index| create + 1 + index)
            .expect("post-create server cleanup should run");
        let rm = events
            .iter()
            .position(|event| event == "compose fixture rm")
            .expect("async service cleanup should run");
        let restore = events
            .iter()
            .position(|event| event == "compose gateway restore")
            .expect("base gateway configuration should be restored");
        assert!(
            create < child_terminated && child_terminated < delete && delete < rm && rm < restore
        );
        assert!(events.iter().all(|event| event != "npx"));
        assert!(
            events
                .iter()
                .any(|event| event == "stack down cf-controlplane-only")
        );
        assert!(
            events
                .iter()
                .all(|event| event != "stack up cf-integration")
        );
        server.abort();
    }

    #[tokio::test]
    async fn interruption_during_blocked_gateway_finishes_before_fixture_cleanup() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture API listener should bind");
        let base_url = format!("http://{}", listener.local_addr().expect("API address"));
        let app = Router::new()
            .fallback(any(fixture_api))
            .with_state(Arc::new(FixtureApiState {
                events: Arc::clone(&events),
                fail_cleanup: false,
                expected_scoped_server_id: None,
                block_gateway: true,
                finish_provision_after_interrupt: false,
            }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("fixture API should run");
        });
        let state = tempfile::tempdir().expect("temporary integration state");
        let controlplane = state.path().join("mcp-context-forge");
        fs::create_dir_all(controlplane.join(".git")).expect("fake checkout should exist");
        let reports = tempfile::tempdir().expect("temporary compliance artifacts");
        let runtime = RuntimeExecutor::new(
            fixture_test_config(&base_url, state.path(), &controlplane),
            FixtureLifecycleRunner {
                events: Arc::clone(&events),
                fail_service_start: false,
                fail_service_stop: false,
                fail_gateway_restore: false,
            },
        );
        let common = ResolvedComplianceCommon {
            mode: ComplianceMode::Controlplane,
            start: false,
            server_id: None,
            spec_version: "2025-11-25".to_owned(),
            results_dir: Some(reports.path().to_owned()),
        };
        let interrupt_events = Arc::clone(&events);
        let interrupt = async move {
            loop {
                if interrupt_events
                    .lock()
                    .expect("event lock")
                    .iter()
                    .any(|event| event == "gateway started")
                {
                    interrupt_events
                        .lock()
                        .expect("event lock")
                        .push("gateway cancellation sent".into());
                    return;
                }
                tokio::task::yield_now().await;
            }
        };

        let error = tokio::time::timeout(
            Duration::from_secs(2),
            runtime.run_compliance_workflow_with_interrupt(
                &common,
                Some(ConformanceSuite::Active),
                true,
                None,
                interrupt,
            ),
        )
        .await
        .expect("blocked gateway cancellation should return promptly")
        .expect_err("injected interruption should remain primary");

        assert!(error.to_string().contains("interrupted by Ctrl-C"));
        let events = events.lock().expect("event lock");
        let gateway = events
            .iter()
            .position(|event| event == "gateway started")
            .expect("gateway request should start");
        let cancelled = events
            .iter()
            .position(|event| event == "gateway cancellation sent")
            .expect("gateway cancellation should be recorded");
        let delete = events
            .iter()
            .rposition(|event| event.starts_with("DELETE /servers/"))
            .expect("fixture API cleanup should run");
        let rm = events
            .iter()
            .position(|event| event == "compose fixture rm")
            .expect("fixture service cleanup should run");
        let restore = events
            .iter()
            .position(|event| event == "compose gateway restore")
            .expect("base gateway configuration should be restored");
        assert!(gateway < cancelled && cancelled < delete && delete < rm && rm < restore);
        server.abort();
    }

    #[tokio::test]
    async fn interruption_after_official_completion_prevents_gateway_start() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture API listener should bind");
        let base_url = format!("http://{}", listener.local_addr().expect("API address"));
        let app = Router::new()
            .fallback(any(fixture_api))
            .with_state(Arc::new(FixtureApiState {
                events: Arc::clone(&events),
                fail_cleanup: false,
                expected_scoped_server_id: None,
                block_gateway: true,
                finish_provision_after_interrupt: false,
            }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("fixture API should run");
        });
        let state = tempfile::tempdir().expect("temporary integration state");
        let controlplane = state.path().join("mcp-context-forge");
        fs::create_dir_all(controlplane.join(".git")).expect("fake checkout should exist");
        let reports = tempfile::tempdir().expect("temporary compliance artifacts");
        let runtime = RuntimeExecutor::new(
            fixture_test_config(&base_url, state.path(), &controlplane),
            FixtureLifecycleRunner {
                events: Arc::clone(&events),
                fail_service_start: false,
                fail_service_stop: false,
                fail_gateway_restore: false,
            },
        );
        let common = ResolvedComplianceCommon {
            mode: ComplianceMode::Controlplane,
            start: false,
            server_id: None,
            spec_version: "2025-11-25".to_owned(),
            results_dir: Some(reports.path().to_owned()),
        };
        let interrupt_events = Arc::clone(&events);
        let interrupt = async move {
            loop {
                if interrupt_events
                    .lock()
                    .expect("event lock")
                    .iter()
                    .any(|event| event == "official proxy complete")
                {
                    interrupt_events
                        .lock()
                        .expect("event lock")
                        .push("post-official cancellation sent".into());
                    return;
                }
                tokio::task::yield_now().await;
            }
        };

        let error = tokio::time::timeout(
            Duration::from_secs(2),
            runtime.run_compliance_workflow_with_interrupt(
                &common,
                Some(ConformanceSuite::Active),
                true,
                None,
                interrupt,
            ),
        )
        .await
        .expect("post-official cancellation should return promptly")
        .expect_err("injected interruption should remain primary");

        assert!(error.to_string().contains("interrupted by Ctrl-C"));
        let events = events.lock().expect("event lock");
        let official_complete = events
            .iter()
            .position(|event| event == "official proxy complete")
            .expect("official runner should complete first");
        assert!(
            events[official_complete + 1..]
                .iter()
                .all(|event| event != "gateway started" && !event.starts_with("POST /servers/"))
        );
        assert!(events.iter().any(|event| event == "compose fixture rm"));
        server.abort();
    }

    #[tokio::test]
    async fn all_mode_cleans_each_fixture_before_stack_down_and_next_mode() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture API listener should bind");
        let base_url = format!("http://{}", listener.local_addr().expect("API address"));
        let app = Router::new()
            .fallback(any(fixture_api))
            .with_state(Arc::new(FixtureApiState {
                events: Arc::clone(&events),
                fail_cleanup: false,
                expected_scoped_server_id: None,
                block_gateway: false,
                finish_provision_after_interrupt: false,
            }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("fixture API should run");
        });
        let state = tempfile::tempdir().expect("temporary integration state");
        let controlplane = state.path().join("mcp-context-forge");
        fs::create_dir_all(controlplane.join(".git")).expect("fake checkout should exist");
        fs::write(controlplane.join("docker-compose.yml"), "services: {}\n")
            .expect("fake Compose file should exist for stack cleanup");
        let reports = tempfile::tempdir().expect("temporary compliance artifacts");
        let runtime = RuntimeExecutor::new(
            fixture_test_config(&base_url, state.path(), &controlplane),
            FixtureLifecycleRunner {
                events: Arc::clone(&events),
                fail_service_start: false,
                fail_service_stop: false,
                fail_gateway_restore: false,
            },
        );
        let common = ResolvedComplianceCommon {
            mode: ComplianceMode::All,
            start: true,
            server_id: None,
            spec_version: "2025-11-25".to_owned(),
            results_dir: Some(reports.path().to_owned()),
        };

        let workflow_error = runtime
            .run_compliance_workflow(&common, Some(ConformanceSuite::Active), false, None)
            .await
            .expect_err("intentional conformance responses should fail");

        let events = events.lock().expect("event lock");
        let controlplane_up = events
            .iter()
            .position(|event| event == "stack up cf-controlplane-only")
            .expect("controlplane stack should start first");
        let controlplane_down = events
            .iter()
            .position(|event| event == "stack down cf-controlplane-only")
            .expect("controlplane stack should be stopped");
        let dataplane_up = events
            .iter()
            .position(|event| event == "stack up cf-integration")
            .unwrap_or_else(|| {
                panic!("dataplane stack should start second: {events:?}; {workflow_error}")
            });
        let dataplane_down = events
            .iter()
            .position(|event| event == "stack down cf-integration")
            .expect("dataplane stack should be stopped");
        let fixture_removals = events
            .iter()
            .enumerate()
            .filter(|(_, event)| event.as_str() == "compose fixture rm")
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        assert_eq!(fixture_removals.len(), 2, "{events:?}");
        let gateway_restores = events
            .iter()
            .enumerate()
            .filter(|(_, event)| event.as_str() == "compose gateway restore")
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        assert_eq!(gateway_restores.len(), 2, "{events:?}");
        assert!(fixture_removals[0] < gateway_restores[0]);
        assert!(gateway_restores[0] < controlplane_down);
        assert!(controlplane_down < dataplane_up);
        assert!(fixture_removals[1] < gateway_restores[1]);
        assert!(gateway_restores[1] < dataplane_down);
        let server_cleanup = format!(
            "DELETE /servers/{}",
            crate::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID
        );
        for (start, rm, down) in [
            (controlplane_up, fixture_removals[0], controlplane_down),
            (dataplane_up, fixture_removals[1], dataplane_down),
        ] {
            let server_creation = events[start..rm]
                .iter()
                .position(|event| event == "POST /servers")
                .map(|index| start + index)
                .expect("fixture virtual server should be created before cleanup");
            let server_delete = events[server_creation + 1..rm]
                .iter()
                .position(|event| event == &server_cleanup)
                .map(|index| server_creation + 1 + index)
                .expect("created fixture server should be deleted before service removal");
            let gateway_delete = events[server_creation + 1..rm]
                .iter()
                .position(|event| event == "DELETE /gateways/fixture-gateway")
                .map(|index| server_creation + 1 + index)
                .expect("created fixture gateway should be deleted before service removal");
            assert!(server_creation < server_delete);
            assert!(server_delete < gateway_delete);
            assert!(gateway_delete < rm && rm < down);
        }
        server.abort();
    }

    #[tokio::test]
    async fn conformance_service_start_failure_attempts_removal_and_preserves_start_error() {
        let state = tempfile::tempdir().expect("temporary integration state");
        let controlplane = state.path().join("mcp-context-forge");
        fs::create_dir_all(controlplane.join(".git")).expect("fake checkout should exist");
        let reports = tempfile::tempdir().expect("temporary compliance artifacts");
        let events = Arc::new(Mutex::new(Vec::new()));
        let runtime = RuntimeExecutor::new(
            fixture_test_config("http://127.0.0.1:9", state.path(), &controlplane),
            FixtureLifecycleRunner {
                events: Arc::clone(&events),
                fail_service_start: true,
                fail_service_stop: false,
                fail_gateway_restore: false,
            },
        );
        let common = ResolvedComplianceCommon {
            mode: ComplianceMode::Controlplane,
            start: false,
            server_id: None,
            spec_version: "2025-11-25".to_owned(),
            results_dir: Some(reports.path().to_owned()),
        };

        let error = runtime
            .run_compliance_workflow(&common, Some(ConformanceSuite::Active), false, None)
            .await
            .expect_err("service start should fail");

        assert!(error.to_string().starts_with("fixture start failed"));
        assert_eq!(
            *events.lock().expect("event lock"),
            [
                "compose fixture build",
                "compose fixture up",
                "compose fixture rm",
                "compose gateway restore"
            ]
        );
    }

    #[tokio::test]
    async fn conformance_service_build_failure_skips_up_restores_and_preserves_build_error() {
        let state = tempfile::tempdir().expect("temporary integration state");
        let controlplane = state.path().join("mcp-context-forge");
        fs::create_dir_all(controlplane.join(".git")).expect("fake checkout should exist");
        let reports = tempfile::tempdir().expect("temporary compliance artifacts");
        let events = Arc::new(Mutex::new(vec!["fail fixture build".into()]));
        let runtime = RuntimeExecutor::new(
            fixture_test_config("http://127.0.0.1:9", state.path(), &controlplane),
            FixtureLifecycleRunner {
                events: Arc::clone(&events),
                fail_service_start: false,
                fail_service_stop: false,
                fail_gateway_restore: false,
            },
        );
        let common = ResolvedComplianceCommon {
            mode: ComplianceMode::Controlplane,
            start: false,
            server_id: None,
            spec_version: "2025-11-25".to_owned(),
            results_dir: Some(reports.path().to_owned()),
        };

        let error = runtime
            .run_compliance_workflow(&common, Some(ConformanceSuite::Active), false, None)
            .await
            .expect_err("service build should fail");

        assert!(error.to_string().starts_with("fixture build failed"));
        assert_eq!(
            *events.lock().expect("event lock"),
            [
                "fail fixture build",
                "compose fixture build",
                "compose fixture rm",
                "compose gateway restore"
            ]
        );
    }

    #[tokio::test]
    async fn conformance_provision_failure_stops_service() {
        let state = tempfile::tempdir().expect("temporary integration state");
        let controlplane = state.path().join("mcp-context-forge");
        fs::create_dir_all(controlplane.join(".git")).expect("fake checkout should exist");
        let reports = tempfile::tempdir().expect("temporary compliance artifacts");
        let events = Arc::new(Mutex::new(Vec::new()));
        let runtime = RuntimeExecutor::new(
            fixture_test_config("http://127.0.0.1:9", state.path(), &controlplane),
            FixtureLifecycleRunner {
                events: Arc::clone(&events),
                fail_service_start: false,
                fail_service_stop: false,
                fail_gateway_restore: false,
            },
        );
        let common = ResolvedComplianceCommon {
            mode: ComplianceMode::Controlplane,
            start: false,
            server_id: None,
            spec_version: "2025-11-25".to_owned(),
            results_dir: Some(reports.path().to_owned()),
        };

        let error = runtime
            .run_compliance_workflow(&common, Some(ConformanceSuite::Active), false, None)
            .await
            .expect_err("unreachable fixture API should fail provisioning");

        assert!(error.to_string().contains("GET /servers"));
        assert_eq!(
            *events.lock().expect("event lock"),
            [
                "compose fixture build",
                "compose fixture up",
                "compose fixture rm",
                "compose gateway restore"
            ]
        );
    }

    #[tokio::test]
    async fn explicit_server_and_gateway_only_defaults_bypass_fixture_service_and_api() {
        let state = tempfile::tempdir().expect("temporary integration state");
        let controlplane = state.path().join("mcp-context-forge");
        fs::create_dir_all(controlplane.join(".git")).expect("fake checkout should exist");
        let reports = tempfile::tempdir().expect("temporary compliance artifacts");
        let events = Arc::new(Mutex::new(Vec::new()));
        let runtime = RuntimeExecutor::new(
            fixture_test_config("http://127.0.0.1:9", state.path(), &controlplane),
            FixtureLifecycleRunner {
                events: Arc::clone(&events),
                fail_service_start: false,
                fail_service_stop: false,
                fail_gateway_restore: false,
            },
        );
        let explicit = ResolvedComplianceCommon {
            mode: ComplianceMode::Controlplane,
            start: false,
            server_id: Some("caller-server".to_owned()),
            spec_version: "2025-11-25".to_owned(),
            results_dir: Some(reports.path().to_owned()),
        };
        runtime
            .run_compliance_workflow(&explicit, Some(ConformanceSuite::Active), false, None)
            .await
            .expect_err("intentional runner failure should occur without fixture setup");
        let explicit_metadata: serde_json::Value = serde_json::from_slice(
            &fs::read(
                CompliancePaths::new(reports.path(), reports.path().join("reports"))
                    .conformance_mode(BaselineTarget::Controlplane)
                    .metadata,
            )
            .expect("explicit server metadata should be written"),
        )
        .expect("explicit server metadata should parse");
        assert!(explicit_metadata.get("fixture").is_none());
        assert!(
            events
                .lock()
                .expect("event lock")
                .iter()
                .any(|event| event == "npx")
        );
        assert!(
            events
                .lock()
                .expect("event lock")
                .iter()
                .all(|event| !event.starts_with("compose fixture"))
        );

        events.lock().expect("event lock").clear();
        let gateway_only = ResolvedComplianceCommon {
            server_id: None,
            ..explicit
        };
        runtime
            .run_compliance_workflow(&gateway_only, None, true, None)
            .await
            .expect_err("unreachable Fast Time gateway should fail");
        assert!(events.lock().expect("event lock").is_empty());
    }

    #[test]
    fn conformance_tokens_are_absent_from_compose_args_debug_and_cleanup_errors() {
        let runtime = RuntimeExecutor::new(test_config([]), DefaultsRunner::default());
        let admin = runtime.admin_token().expect("admin token should mint");
        let scoped = runtime
            .bearer_token(StackMode::Dataplane, "official-server")
            .expect("scoped token should mint");

        let project = runtime.conformance_compose_project(StackMode::Controlplane);
        let commands = [
            project.command(["build", OFFICIAL_CONFORMANCE_SERVICE]),
            project.command([
                "up",
                "-d",
                "--wait",
                "gateway",
                OFFICIAL_CONFORMANCE_SERVICE,
            ]),
        ];
        let debug = format!("{commands:?}");
        let error = finish_with_cleanup(
            Some(AppFailure::from(anyhow!("primary"))),
            Err(AppFailure::from(anyhow!("gateway restore failed"))),
        )
        .expect_err("combined failure should remain an error")
        .to_string();
        for token in [admin, scoped] {
            assert!(commands.iter().all(|command| {
                command
                    .arguments()
                    .iter()
                    .all(|argument| !argument.to_string_lossy().contains(&token))
            }));
            assert!(!debug.contains(&token));
            assert!(!error.contains(&token));
        }
    }

    #[tokio::test]
    async fn conformance_compose_helpers_use_async_process_execution() {
        let state = tempfile::tempdir().expect("temporary integration state");
        let controlplane = state.path().join("mcp-context-forge");
        fs::create_dir_all(controlplane.join(".git")).expect("fake checkout should exist");
        let events = Arc::new(Mutex::new(Vec::new()));
        let runtime = RuntimeExecutor::new(
            fixture_test_config("http://127.0.0.1:4444", state.path(), &controlplane),
            FixtureLifecycleRunner {
                events: Arc::clone(&events),
                fail_service_start: false,
                fail_service_stop: false,
                fail_gateway_restore: false,
            },
        );

        runtime
            .start_conformance_service(StackMode::Controlplane)
            .await
            .expect("async fixture start should pass");
        runtime
            .stop_conformance_service(StackMode::Controlplane)
            .await
            .expect("async fixture removal should pass");

        assert_eq!(
            *events.lock().expect("event lock"),
            [
                "compose fixture build",
                "compose fixture up",
                "compose fixture rm",
                "compose gateway restore"
            ]
        );
    }

    #[tokio::test]
    async fn runner_and_both_cleanup_failures_keep_primary_and_diagnostics_in_order() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("fixture API listener should bind");
        let base_url = format!("http://{}", listener.local_addr().expect("API address"));
        let app = Router::new()
            .fallback(any(fixture_api))
            .with_state(Arc::new(FixtureApiState {
                events: Arc::clone(&events),
                fail_cleanup: true,
                expected_scoped_server_id: None,
                block_gateway: false,
                finish_provision_after_interrupt: false,
            }));
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("fixture API should run");
        });
        let state = tempfile::tempdir().expect("temporary integration state");
        let controlplane = state.path().join("mcp-context-forge");
        fs::create_dir_all(controlplane.join(".git")).expect("fake checkout should exist");
        let reports = tempfile::tempdir().expect("temporary compliance artifacts");
        let runtime = RuntimeExecutor::new(
            fixture_test_config(&base_url, state.path(), &controlplane),
            FixtureLifecycleRunner {
                events: Arc::clone(&events),
                fail_service_start: false,
                fail_service_stop: true,
                fail_gateway_restore: true,
            },
        );
        let common = ResolvedComplianceCommon {
            mode: ComplianceMode::Controlplane,
            start: false,
            server_id: None,
            spec_version: "2025-11-25".to_owned(),
            results_dir: Some(reports.path().to_owned()),
        };

        let error = runtime
            .run_compliance_workflow(&common, Some(ConformanceSuite::Active), false, None)
            .await
            .expect_err("runner and cleanup should fail");

        let message = error.to_string();
        let primary = message
            .find("intentional npx failure")
            .expect("primary error");
        let api = message.find("DELETE /servers/").expect("API cleanup error");
        let service = message
            .find("fixture stop failed")
            .expect("service cleanup error");
        let restore_error = message
            .find("gateway restore failed")
            .expect("gateway restore error");
        assert!(primary < api && api < service && service < restore_error);
        assert!(message.contains("additionally conformance cleanup failed"));
        let events = events.lock().expect("event lock");
        let cleanup_delete = events
            .iter()
            .rposition(|event| event.starts_with("DELETE /servers/"))
            .expect("API cleanup should run");
        let rm = events
            .iter()
            .position(|event| event == "compose fixture rm")
            .expect("service cleanup should run");
        let restore = events
            .iter()
            .position(|event| event == "compose gateway restore")
            .expect("gateway restore should run despite removal failure");
        assert!(cleanup_delete < rm && rm < restore);
        server.abort();
    }

    fn write_official_results(
        root: &Path,
        scenarios: impl IntoIterator<Item = &'static str>,
        status: &str,
    ) {
        for scenario in scenarios {
            let directory = root.join(format!("server-{scenario}-2026-07-10T03-17-47-000Z"));
            fs::create_dir_all(&directory).expect("official scenario directory should be created");
            let checks = serde_json::to_vec(&serde_json::json!([{
                "id": format!("{scenario}-check"),
                "status": status,
                "errorMessage": (status == "FAILURE").then_some("real product gap"),
            }]))
            .expect("official checks should serialize");
            fs::write(directory.join("checks.json"), checks)
                .expect("official checks should be written");
        }
    }

    fn write_fixture_failure_result(root: &Path, scenario: &str) {
        let directory = fs::read_dir(root)
            .expect("official results directory should exist")
            .filter_map(Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().contains(scenario))
            .expect("fixture-failure scenario directory should exist")
            .path();
        fs::write(
            directory.join("checks.json"),
            serde_json::to_vec(&serde_json::json!([{
                "id": "fixture-check",
                "status": "FAILURE",
                "errorMessage": "Failed: MCP error -32002: Resource not found: test://static-text",
            }]))
            .expect("fixture failure should serialize"),
        )
        .expect("fixture failure should be written");
    }

    #[test]
    fn inspector_command_is_pinned_and_contains_no_credentials() {
        let command = inspector_command("http://127.0.0.1:49152/mcp-auth/random", "tools/list");

        assert_eq!(command.program(), "npx");
        assert!(!command.inherits_environment());
        assert_eq!(
            command.arguments(),
            &[
                OsString::from("-y"),
                OsString::from("@modelcontextprotocol/inspector@0.22.0"),
                OsString::from("--cli"),
                OsString::from("http://127.0.0.1:49152/mcp-auth/random"),
                OsString::from("--transport"),
                OsString::from("http"),
                OsString::from("--method"),
                OsString::from("tools/list"),
            ]
        );
        assert!(
            command
                .arguments()
                .iter()
                .all(|argument| !argument.to_string_lossy().contains("Bearer"))
        );
    }

    #[test]
    fn compliance_artifacts_are_partitioned_by_mode() {
        let paths = CompliancePaths::new(Path::new(".integration"), PathBuf::from("reports"));

        assert_eq!(
            paths
                .conformance_mode(BaselineTarget::Controlplane)
                .official_results,
            Path::new(".integration/conformance/controlplane/official")
        );
        assert_eq!(
            paths.gateway_mode(BaselineTarget::Dataplane).json,
            Path::new(".integration/gateway-compliance/dataplane/report.json")
        );
    }

    #[test]
    fn compliance_run_clears_only_requested_mode_artifacts_before_setup() {
        let artifacts = tempfile::tempdir().expect("temporary artifact root");
        let paths = CompliancePaths::new(artifacts.path(), artifacts.path().join("reports"));
        let controlplane = paths.conformance_mode(BaselineTarget::Controlplane);
        let dataplane = paths.conformance_mode(BaselineTarget::Dataplane);
        let dataplane_gateway = paths.gateway_mode(BaselineTarget::Dataplane);
        for sentinel in [
            controlplane.root.join("stale"),
            dataplane.root.join("stale"),
            dataplane_gateway.root.join("stale"),
        ] {
            fs::create_dir_all(sentinel.parent().expect("sentinel parent"))
                .expect("artifact directory should be created");
            fs::write(sentinel, "old run").expect("stale artifact should be written");
        }

        paths
            .clear_selected(ComplianceMode::Dataplane, true, true)
            .expect("selected artifact roots should be cleared");

        assert!(controlplane.root.join("stale").is_file());
        assert!(!dataplane.root.exists());
        assert!(!dataplane_gateway.root.exists());
    }

    #[test]
    fn conformance_metadata_roundtrips_exact_fixture_provenance() {
        let metadata = ConformanceRunMetadata {
            oracle: crate::conformance::OFFICIAL_CONFORMANCE_PACKAGE.to_owned(),
            target: "control-plane".to_owned(),
            spec_version: "2025-11-25".to_owned(),
            suite: "active".to_owned(),
            fixture: Some(ConformanceFixtureMetadata {
                repository: "https://example.test/conformance".to_owned(),
                revision: "exact-revision".to_owned(),
                server_id: "actual-provisioned-server".to_owned(),
            }),
        };

        let serialized = serde_json::to_vec(&metadata).expect("metadata should serialize");
        let roundtrip: ConformanceRunMetadata =
            serde_json::from_slice(&serialized).expect("metadata should deserialize");

        assert_eq!(roundtrip, metadata);
    }

    #[test]
    fn historical_metadata_without_fixture_roundtrips_without_new_field() {
        let historical = serde_json::to_vec(&serde_json::json!({
            "oracle": crate::conformance::OFFICIAL_CONFORMANCE_PACKAGE,
            "target": "control-plane",
            "spec_version": "2025-11-25",
            "suite": "active",
        }))
        .expect("historical metadata should serialize");

        let metadata: ConformanceRunMetadata =
            serde_json::from_slice(&historical).expect("historical metadata should deserialize");
        let reserialized = serde_json::to_value(metadata).expect("metadata should reserialize");

        assert!(reserialized.get("fixture").is_none());
    }

    #[test]
    fn trusted_official_fixture_requires_exact_pinned_provenance() {
        let exact = fixture_metadata(
            OFFICIAL_CONFORMANCE_REPOSITORY,
            OFFICIAL_CONFORMANCE_REVISION,
            crate::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID,
        );

        assert!(is_trusted_official_fixture(Some(&exact)));
        assert!(!is_trusted_official_fixture(None));
        for mismatch in [
            ConformanceFixtureMetadata {
                repository: "https://example.test/untrusted".to_owned(),
                ..exact.clone()
            },
            ConformanceFixtureMetadata {
                revision: "untrusted-revision".to_owned(),
                ..exact.clone()
            },
            ConformanceFixtureMetadata {
                server_id: "untrusted-server".to_owned(),
                ..exact.clone()
            },
        ] {
            assert!(!is_trusted_official_fixture(Some(&mismatch)));
        }
    }

    #[tokio::test]
    async fn caller_managed_resource_template_not_found_is_not_complete_or_reportable() {
        let artifacts = tempfile::tempdir().expect("temporary artifact root");
        let paths = CompliancePaths::new(artifacts.path(), artifacts.path().join("reports"));
        let runtime = RuntimeExecutor::new(test_config([]), CompletedFixtureFailureRunner);

        let error = runtime
            .run_official_conformance_mode(
                &OfficialConformanceRun {
                    mode: StackMode::Controlplane,
                    server_id: "server-id",
                    token: "token",
                    spec_version: "2025-11-25",
                    suite: "active",
                    custom_baseline: None,
                    fixture: None,
                    cancellation: tokio::sync::watch::channel(false).1,
                },
                &paths,
            )
            .await
            .expect_err("fresh fixture failures must reject completion");

        assert!(error.to_string().contains("official fixture setup failed"));
        let mode_paths = paths.conformance_mode(BaselineTarget::Controlplane);
        assert!(!mode_paths.completion.exists());
        let report_error =
            match runtime.load_conformance_artifact(&paths, BaselineTarget::Controlplane) {
                Err(error) => error,
                Ok(_) => panic!("report-only mode must reject incomplete fresh artifacts"),
            };
        assert!(
            report_error
                .to_string()
                .contains("incomplete conformance artifacts")
        );
    }

    #[tokio::test]
    async fn trusted_resource_template_not_found_is_complete_and_reportable_as_gateway_failure() {
        let artifacts = tempfile::tempdir().expect("temporary artifact root");
        let paths = CompliancePaths::new(artifacts.path(), artifacts.path().join("reports"));
        let runtime = RuntimeExecutor::new(test_config([]), CompletedFixtureFailureRunner);
        let fixture = fixture_metadata(
            OFFICIAL_CONFORMANCE_REPOSITORY,
            OFFICIAL_CONFORMANCE_REVISION,
            crate::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID,
        );

        let error = runtime
            .run_official_conformance_mode(
                &OfficialConformanceRun {
                    mode: StackMode::Controlplane,
                    server_id: crate::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID,
                    token: "token",
                    spec_version: "2025-11-25",
                    suite: "active",
                    custom_baseline: None,
                    fixture: Some(&fixture),
                    cancellation: tokio::sync::watch::channel(false).1,
                },
                &paths,
            )
            .await
            .expect_err("trusted gateway failure should remain a baseline audit failure");

        assert!(error.to_string().contains("unexpected failures"));
        let mode_paths = paths.conformance_mode(BaselineTarget::Controlplane);
        assert!(mode_paths.completion.is_file());
        let loaded = runtime
            .load_conformance_artifact(&paths, BaselineTarget::Controlplane)
            .expect("completed trusted artifacts should load")
            .expect("trusted artifact should be present");
        assert_eq!(loaded.metadata.fixture, Some(fixture));
    }

    #[tokio::test]
    async fn mismatched_fixture_provenance_does_not_bypass_fresh_fixture_validation() {
        let artifacts = tempfile::tempdir().expect("temporary artifact root");
        let paths = CompliancePaths::new(artifacts.path(), artifacts.path().join("reports"));
        let runtime = RuntimeExecutor::new(test_config([]), CompletedFixtureFailureRunner);
        let fixture = fixture_metadata(
            OFFICIAL_CONFORMANCE_REPOSITORY,
            "untrusted-revision",
            crate::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID,
        );

        let error = runtime
            .run_official_conformance_mode(
                &OfficialConformanceRun {
                    mode: StackMode::Controlplane,
                    server_id: crate::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID,
                    token: "token",
                    spec_version: "2025-11-25",
                    suite: "active",
                    custom_baseline: None,
                    fixture: Some(&fixture),
                    cancellation: tokio::sync::watch::channel(false).1,
                },
                &paths,
            )
            .await
            .expect_err("untrusted provenance must retain fresh fixture validation");

        assert!(error.to_string().contains("official fixture setup failed"));
        assert!(
            !paths
                .conformance_mode(BaselineTarget::Controlplane)
                .completion
                .exists()
        );
    }

    #[test]
    fn trusted_artifact_report_attributes_fixture_not_found_to_gateway() {
        let artifacts = tempfile::tempdir().expect("temporary artifact root");
        let paths = CompliancePaths::new(artifacts.path(), artifacts.path().join("reports"));
        let scenarios =
            expected_server_scenarios("active", "2025-11-25").expect("pinned active catalog");
        let fixture_scenario = "resources-templates-read";
        let fixture = fixture_metadata(
            OFFICIAL_CONFORMANCE_REPOSITORY,
            OFFICIAL_CONFORMANCE_REVISION,
            crate::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID,
        );
        for target in [BaselineTarget::Controlplane, BaselineTarget::Dataplane] {
            let mode_paths = paths.conformance_mode(target);
            write_official_results(
                &mode_paths.official_results,
                scenarios.iter().copied(),
                "SUCCESS",
            );
            if target == BaselineTarget::Controlplane {
                write_fixture_failure_result(&mode_paths.official_results, fixture_scenario);
            }
            fs::write(&mode_paths.rich_baseline, "server: []\n")
                .expect("empty rich baseline should be written");
            write_run_metadata(
                &mode_paths.metadata,
                &ConformanceRunMetadata {
                    oracle: crate::conformance::OFFICIAL_CONFORMANCE_PACKAGE.to_owned(),
                    target: target.label().to_owned(),
                    spec_version: "2025-11-25".to_owned(),
                    suite: "active".to_owned(),
                    fixture: Some(fixture.clone()),
                },
            )
            .expect("trusted metadata should be written");
            write_completion_marker(&mode_paths.completion)
                .expect("completion marker should be written");
        }
        let runtime = RuntimeExecutor::new(test_config([]), DefaultsRunner::default());

        let report_path = runtime
            .write_comparison_from_artifacts(&paths, None)
            .expect("matching trusted provenance should be reportable");
        let report = fs::read_to_string(report_path).expect("comparison report should be readable");

        assert!(report.contains("| fixture failure | 0 |"));
        assert!(report.contains("| control-plane only failure | 1 |"));
        assert!(report.contains(&format!(
            "| {fixture_scenario} | failure | compliant | control-plane only failure |"
        )));
    }

    #[test]
    fn mismatched_fixture_provenance_remains_fixture_failure_in_report() {
        let artifacts = tempfile::tempdir().expect("temporary artifact root");
        let paths = CompliancePaths::new(artifacts.path(), artifacts.path().join("reports"));
        let scenarios =
            expected_server_scenarios("active", "2025-11-25").expect("pinned active catalog");
        let fixture_scenario = "resources-templates-read";
        let fixture = fixture_metadata(
            OFFICIAL_CONFORMANCE_REPOSITORY,
            "untrusted-revision",
            crate::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID,
        );
        for target in [BaselineTarget::Controlplane, BaselineTarget::Dataplane] {
            let mode_paths = paths.conformance_mode(target);
            write_official_results(
                &mode_paths.official_results,
                scenarios.iter().copied(),
                "SUCCESS",
            );
            if target == BaselineTarget::Controlplane {
                write_fixture_failure_result(&mode_paths.official_results, fixture_scenario);
            }
            fs::write(&mode_paths.rich_baseline, "server: []\n")
                .expect("empty rich baseline should be written");
            write_run_metadata(
                &mode_paths.metadata,
                &ConformanceRunMetadata {
                    oracle: crate::conformance::OFFICIAL_CONFORMANCE_PACKAGE.to_owned(),
                    target: target.label().to_owned(),
                    spec_version: "2025-11-25".to_owned(),
                    suite: "active".to_owned(),
                    fixture: Some(fixture.clone()),
                },
            )
            .expect("mismatched metadata should be written");
            write_completion_marker(&mode_paths.completion)
                .expect("completion marker should be written");
        }
        let runtime = RuntimeExecutor::new(test_config([]), DefaultsRunner::default());

        let report_path = runtime
            .write_comparison_from_artifacts(&paths, None)
            .expect("matching mismatched provenance should remain historically reportable");
        let report = fs::read_to_string(report_path).expect("comparison report should be readable");

        assert!(report.contains("| fixture failure | 1 |"));
        assert!(report.contains(&format!(
            "| {fixture_scenario} | fixture failure | compliant | fixture failure |"
        )));
    }

    #[test]
    fn historical_completed_fixture_failure_artifact_remains_readable() {
        let artifacts = tempfile::tempdir().expect("temporary artifact root");
        let paths = CompliancePaths::new(artifacts.path(), artifacts.path().join("reports"));
        let mode_paths = paths.conformance_mode(BaselineTarget::Controlplane);
        let scenarios =
            expected_server_scenarios("active", "2025-11-25").expect("pinned active catalog");
        write_official_results(&mode_paths.official_results, scenarios.clone(), "SUCCESS");
        let fixture_scenario = scenarios.into_iter().next().expect("active scenario");
        let fixture_directory = fs::read_dir(&mode_paths.official_results)
            .expect("official results directory")
            .filter_map(Result::ok)
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains(fixture_scenario)
            })
            .expect("fixture scenario directory")
            .path();
        fs::write(
            fixture_directory.join("checks.json"),
            serde_json::to_vec(&serde_json::json!([{
                "id": "fixture-check",
                "status": "FAILURE",
                "errorMessage": "Failed: MCP error -32601: Tool not found: test_simple_text",
            }]))
            .expect("fixture failure should serialize"),
        )
        .expect("fixture failure should be written");
        fs::create_dir_all(&mode_paths.root).expect("artifact root should exist");
        fs::write(&mode_paths.rich_baseline, "server: []\n")
            .expect("empty rich baseline should be written");
        fs::write(
            &mode_paths.metadata,
            serde_json::to_vec(&serde_json::json!({
                "oracle": crate::conformance::OFFICIAL_CONFORMANCE_PACKAGE,
                "target": "control-plane",
                "spec_version": "2025-11-25",
                "suite": "active",
            }))
            .expect("historical metadata should serialize"),
        )
        .expect("historical metadata should be written");
        write_completion_marker(&mode_paths.completion)
            .expect("historical completion marker should be written");
        let runtime = RuntimeExecutor::new(test_config([]), DefaultsRunner::default());

        let loaded = runtime
            .load_conformance_artifact(&paths, BaselineTarget::Controlplane)
            .expect("historical fixture-failure artifacts should remain readable")
            .expect("historical artifact should be present");

        assert_eq!(loaded.metadata.fixture, None);
        assert_eq!(
            loaded
                .results
                .scenarios
                .get(fixture_scenario)
                .expect("fixture scenario should be loaded")
                .outcome(),
            ScenarioOutcome::FixtureFailure
        );
    }

    #[test]
    fn normal_nonzero_conformance_exit_marks_dirty_results_complete_and_reportable() {
        let artifacts = tempfile::tempdir().expect("temporary artifact root");
        let paths = CompliancePaths::new(artifacts.path(), artifacts.path().join("reports"));
        let mode_paths = paths.conformance_mode(BaselineTarget::Controlplane);
        write_official_results(
            &mode_paths.official_results,
            expected_server_scenarios("active", "2025-11-25").expect("pinned active catalog"),
            "FAILURE",
        );
        fs::write(&mode_paths.rich_baseline, "server: []\n")
            .expect("empty rich baseline should be written");
        write_run_metadata(
            &mode_paths.metadata,
            &ConformanceRunMetadata {
                oracle: crate::conformance::OFFICIAL_CONFORMANCE_PACKAGE.to_owned(),
                target: BaselineTarget::Controlplane.label().to_owned(),
                spec_version: "2025-11-25".to_owned(),
                suite: "active".to_owned(),
                fixture: None,
            },
        )
        .expect("metadata should be written");
        let results = load_server_results(&mode_paths.official_results)
            .expect("completed official results should parse");
        let status = std::process::Command::new("sh")
            .args(["-c", "exit 7"])
            .status()
            .expect("test child should exit normally");
        let process_result: AppResult<()> = Err(PlatformError::ChildExit {
            program: "npx".into(),
            status,
        }
        .into());

        assert!(
            mark_conformance_complete(
                &process_result,
                &results,
                BaselineTarget::Controlplane,
                "active",
                "2025-11-25",
                &mode_paths.completion,
            )
            .expect("normal child exit should permit a completion marker")
        );

        let runtime = RuntimeExecutor::new(test_config([]), DefaultsRunner::default());
        let loaded = runtime
            .load_conformance_artifact(&paths, BaselineTarget::Controlplane)
            .expect("completed dirty artifacts should load for reporting")
            .expect("completed artifacts should be present");
        assert!(!audit_baseline(&loaded.results, &loaded.baseline).is_clean());
    }

    #[test]
    fn normal_nonzero_partial_scenario_set_is_never_marked_or_reportable() {
        let artifacts = tempfile::tempdir().expect("temporary artifact root");
        let paths = CompliancePaths::new(artifacts.path(), artifacts.path().join("reports"));
        let mode_paths = paths.conformance_mode(BaselineTarget::Controlplane);
        write_official_results(
            &mode_paths.official_results,
            ["server-initialize"],
            "FAILURE",
        );
        fs::write(&mode_paths.rich_baseline, "server: []\n")
            .expect("empty rich baseline should be written");
        write_run_metadata(
            &mode_paths.metadata,
            &ConformanceRunMetadata {
                oracle: crate::conformance::OFFICIAL_CONFORMANCE_PACKAGE.to_owned(),
                target: BaselineTarget::Controlplane.label().to_owned(),
                spec_version: "2025-11-25".to_owned(),
                suite: "active".to_owned(),
                fixture: None,
            },
        )
        .expect("metadata should be written");
        let results = load_server_results(&mode_paths.official_results)
            .expect("partial official results should parse");
        let status = std::process::Command::new("sh")
            .args(["-c", "exit 1"])
            .status()
            .expect("test child should exit normally");
        let process_result: AppResult<()> = Err(PlatformError::ChildExit {
            program: "npx".into(),
            status,
        }
        .into());
        let completion_error = mark_conformance_complete(
            &process_result,
            &results,
            BaselineTarget::Controlplane,
            "active",
            "2025-11-25",
            &mode_paths.completion,
        )
        .expect_err("partial normal-exit results must not be marked complete");
        assert!(completion_error.to_string().contains("missing="));
        assert!(!mode_paths.completion.exists());

        // Even a forged/stale marker cannot make the partial set reportable.
        write_completion_marker(&mode_paths.completion)
            .expect("completion marker should be written");
        let runtime = RuntimeExecutor::new(test_config([]), DefaultsRunner::default());

        let error = match runtime.load_conformance_artifact(&paths, BaselineTarget::Controlplane) {
            Err(error) => error,
            Ok(_) => panic!("partial artifacts must not be reportable"),
        };

        assert!(error.to_string().contains("scenario set is incomplete"));
    }

    #[cfg(unix)]
    #[test]
    fn signal_terminated_conformance_process_is_not_complete() {
        use std::os::unix::process::ExitStatusExt;

        let process_result: AppResult<()> = Err(PlatformError::ChildExit {
            program: "npx".into(),
            status: std::process::ExitStatus::from_raw(9),
        }
        .into());
        assert!(!conformance_process_completed(&process_result));
    }

    #[test]
    fn multi_mode_suite_records_final_cleanup_failure() {
        let checkout = tempfile::tempdir().expect("temporary control-plane checkout");
        fs::write(checkout.path().join("docker-compose.yml"), "services: {}\n")
            .expect("Compose fixture should be written");
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let environment = Environment::from([
            (
                OsString::from("CF_INTEGRATION_ROOT"),
                root.as_os_str().to_owned(),
            ),
            (
                OsString::from("CF_CONTROLPLANE_DIR"),
                checkout.path().as_os_str().to_owned(),
            ),
        ]);
        let config = AppConfig::load(&environment, &root.join("cf-integration"), root)
            .expect("cleanup test configuration should load")
            .config;
        let runtime = RuntimeExecutor::new(config, CleanupFailingRunner);
        let mut last_failure = None;

        runtime.record_suite_cleanup(
            ComplianceMode::All,
            StackMode::Controlplane,
            &mut last_failure,
        );

        let error = last_failure.expect("cleanup failure must be retained by the suite");
        assert!(error.to_string().contains("deliberate cleanup failure"));
    }

    #[test]
    fn incompatible_paired_metadata_is_rejected() {
        let controlplane = ConformanceRunMetadata {
            oracle: crate::conformance::OFFICIAL_CONFORMANCE_PACKAGE.to_owned(),
            target: "control-plane".to_owned(),
            spec_version: "2025-11-25".to_owned(),
            suite: "active".to_owned(),
            fixture: None,
        };
        let mut dataplane = controlplane.clone();
        dataplane.target = "dataplane".to_owned();
        dataplane.suite = "all".to_owned();

        let error = compatible_metadata(Some(&controlplane), Some(&dataplane), None)
            .expect_err("mixed suites must not be reported as one comparison");
        assert!(error.to_string().contains("incompatible runs"));
    }

    fn paired_metadata(fixture: Option<ConformanceFixtureMetadata>) -> ConformanceRunMetadata {
        ConformanceRunMetadata {
            oracle: crate::conformance::OFFICIAL_CONFORMANCE_PACKAGE.to_owned(),
            target: "control-plane".to_owned(),
            spec_version: "2025-11-25".to_owned(),
            suite: "active".to_owned(),
            fixture,
        }
    }

    fn fixture_metadata(
        repository: &str,
        revision: &str,
        server_id: &str,
    ) -> ConformanceFixtureMetadata {
        ConformanceFixtureMetadata {
            repository: repository.to_owned(),
            revision: revision.to_owned(),
            server_id: server_id.to_owned(),
        }
    }

    #[test]
    fn historical_paired_metadata_without_fixture_is_compatible() {
        let controlplane = paired_metadata(None);
        let mut dataplane = controlplane.clone();
        dataplane.target = "dataplane".to_owned();

        compatible_metadata(Some(&controlplane), Some(&dataplane), None)
            .expect("historical paired metadata should remain compatible");
    }

    #[test]
    fn identical_paired_fixture_provenance_is_compatible() {
        let controlplane = paired_metadata(Some(fixture_metadata(
            "repository",
            "revision",
            "server-id",
        )));
        let mut dataplane = controlplane.clone();
        dataplane.target = "dataplane".to_owned();

        compatible_metadata(Some(&controlplane), Some(&dataplane), None)
            .expect("identical fixture provenance should be compatible");
    }

    #[test]
    fn paired_metadata_rejects_fixture_presence_mismatch_without_leaking_values() {
        let controlplane = paired_metadata(Some(fixture_metadata(
            "sensitive-repository-value",
            "sensitive-revision-value",
            "sensitive-server-value",
        )));
        let mut dataplane = paired_metadata(None);
        dataplane.target = "dataplane".to_owned();

        let error = compatible_metadata(Some(&controlplane), Some(&dataplane), None)
            .expect_err("fresh and historical fixture provenance must not be mixed");
        let message = error.to_string();

        assert!(message.contains("fixture provenance mismatch"));
        for secret in [
            "sensitive-repository-value",
            "sensitive-revision-value",
            "sensitive-server-value",
        ] {
            assert!(!message.contains(secret));
        }
    }

    #[test]
    fn paired_metadata_rejects_different_fixture_fields_without_leaking_values() {
        for (repository, revision, server_id) in [
            ("other-repository", "revision", "server-id"),
            ("repository", "other-revision", "server-id"),
            ("repository", "revision", "other-server-id"),
        ] {
            let controlplane = paired_metadata(Some(fixture_metadata(
                "repository",
                "revision",
                "server-id",
            )));
            let mut dataplane =
                paired_metadata(Some(fixture_metadata(repository, revision, server_id)));
            dataplane.target = "dataplane".to_owned();

            let error = compatible_metadata(Some(&controlplane), Some(&dataplane), None)
                .expect_err("different fixture provenance must not be compared");

            let message = error.to_string();
            assert!(message.contains("fixture provenance mismatch"));
            for value in [repository, revision, server_id] {
                assert!(!message.contains(value));
            }
        }
    }

    #[test]
    fn coverage_enrichment_uses_fail_pass_na_precedence_and_preserves_missing_modes() {
        use crate::coverage::{OfficialCoverageClaim, RustGatewayCoverageClaim};

        let requirement = RequirementCoverageOverride {
            id: "requirement".to_owned(),
            official_conformance: OfficialCoverageClaim {
                covered: true,
                scenario: Some("official-case".to_owned()),
            },
            rust_gateway: RustGatewayCoverageClaim {
                covered: true,
                test_name: Some("gateway-compliance/gateway-case".to_owned()),
            },
            gateway_applicability: GatewayApplicability::Applicable,
            capability_condition: None,
            not_applicable_justification: None,
            controlplane_result: CoverageResult::NotRun,
            dataplane_result: CoverageResult::NotApplicable,
            notes: None,
            issue: None,
        };
        let mut overlay = CoverageOverlay {
            spec_version: crate::coverage::SPEC_VERSION.to_owned(),
            requirements: vec![requirement.clone(), requirement.clone(), requirement],
        };
        overlay.requirements[0].id = "failure-dominates".to_owned();
        overlay.requirements[1].id = "pass-dominates-na".to_owned();
        overlay.requirements[1].rust_gateway.test_name =
            Some("gateway-compliance/gateway-na".to_owned());
        overlay.requirements[2].id = "explicit-na".to_owned();
        overlay.requirements[2].official_conformance.covered = false;
        overlay.requirements[2].official_conformance.scenario = None;
        overlay.requirements[2].rust_gateway.test_name =
            Some("gateway-compliance/gateway-na".to_owned());
        let controlplane = ModeCoverageEvidence {
            available: true,
            official: BTreeMap::from([("official-case".to_owned(), CoverageResult::Pass)]),
            gateway: BTreeMap::from([
                ("gateway-case".to_owned(), CoverageResult::Fail),
                ("gateway-na".to_owned(), CoverageResult::NotApplicable),
            ]),
        };
        let dataplane = ModeCoverageEvidence::default();

        enrich_overlay_results(&mut overlay, &controlplane, &dataplane);

        assert_eq!(
            overlay.requirements[0].controlplane_result,
            CoverageResult::Fail
        );
        assert_eq!(
            overlay.requirements[1].controlplane_result,
            CoverageResult::Pass
        );
        assert_eq!(
            overlay.requirements[2].controlplane_result,
            CoverageResult::NotApplicable
        );
        assert!(
            overlay
                .requirements
                .iter()
                .all(|requirement| requirement.dataplane_result == CoverageResult::NotApplicable)
        );
    }

    #[test]
    fn coverage_enrichment_marks_missing_claim_evidence_not_run() {
        let requirement = RequirementCoverageOverride {
            id: "missing-evidence".to_owned(),
            official_conformance: crate::coverage::OfficialCoverageClaim {
                covered: true,
                scenario: Some("missing-scenario".to_owned()),
            },
            controlplane_result: CoverageResult::Pass,
            ..RequirementCoverageOverride::default()
        };
        let mut overlay = CoverageOverlay {
            spec_version: crate::coverage::SPEC_VERSION.to_owned(),
            requirements: vec![requirement],
        };

        enrich_overlay_results(
            &mut overlay,
            &ModeCoverageEvidence {
                available: true,
                ..ModeCoverageEvidence::default()
            },
            &ModeCoverageEvidence::default(),
        );

        assert_eq!(
            overlay.requirements[0].controlplane_result,
            CoverageResult::NotRun
        );
    }

    #[test]
    fn trusted_fixture_failure_is_failed_coverage_while_historical_is_not_run() {
        let artifacts = tempfile::tempdir().expect("temporary official results");
        write_official_results(artifacts.path(), ["resources-templates-read"], "SUCCESS");
        write_fixture_failure_result(artifacts.path(), "resources-templates-read");
        let results = load_server_results(artifacts.path()).expect("fixture failure should parse");

        let exact = fixture_metadata(
            OFFICIAL_CONFORMANCE_REPOSITORY,
            OFFICIAL_CONFORMANCE_REVISION,
            crate::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID,
        );
        let mismatch = ConformanceFixtureMetadata {
            revision: "untrusted-revision".to_owned(),
            ..exact.clone()
        };

        let trusted = ModeCoverageEvidence::from_artifacts(Some(&results), None, Some(&exact));
        let historical = ModeCoverageEvidence::from_artifacts(Some(&results), None, None);
        let mismatched =
            ModeCoverageEvidence::from_artifacts(Some(&results), None, Some(&mismatch));

        assert_eq!(
            trusted.official["resources-templates-read"],
            CoverageResult::Fail
        );
        assert_eq!(
            historical.official["resources-templates-read"],
            CoverageResult::NotRun
        );
        assert_eq!(
            mismatched.official["resources-templates-read"],
            CoverageResult::NotRun
        );
    }

    #[test]
    fn compose_defaults_scale_to_docker_and_preserve_explicit_values() {
        let config = test_config([
            ("GATEWAY_REPLICAS", "3"),
            ("GATEWAY_MEM_LIMIT", ""),
            ("CONTROLPLANE_LOCUST_WORKERS", "4"),
            ("LOCUST_USERS", "100"),
        ]);
        let runtime = RuntimeExecutor::new(config, DefaultsRunner::default());

        let command = runtime
            .compose_environment(
                CommandSpec::new("true").env("LOCUST_USERS", "1"),
                StackMode::Dataplane,
                false,
            )
            .expect("Compose environment should resolve");
        let environment = command.environment();

        assert_eq!(environment.get(OsStr::new("HOST_UID")), Some(&"501".into()));
        assert_eq!(environment.get(OsStr::new("HOST_GID")), Some(&"20".into()));
        assert_eq!(
            environment.get(OsStr::new("GATEWAY_REPLICAS")),
            Some(&"3".into())
        );
        assert_eq!(
            environment.get(OsStr::new("GATEWAY_CPU_LIMIT")),
            Some(&"6".into())
        );
        assert_eq!(
            environment.get(OsStr::new("GUNICORN_WORKERS")),
            Some(&"6".into())
        );
        assert_eq!(
            environment.get(OsStr::new("GATEWAY_MEM_LIMIT")),
            Some(&OsString::new())
        );
        assert_eq!(
            environment.get(OsStr::new("LOCUST_EXPECT_WORKERS")),
            Some(&"4".into())
        );
        assert_eq!(
            environment.get(OsStr::new("LOCUST_USERS")),
            Some(&"1".into())
        );
    }

    #[test]
    fn compose_environment_preserves_live_test_working_directories() {
        let config = test_config([]);
        let expected = config.controlplane_dir().to_path_buf();
        let runtime = RuntimeExecutor::new(config, DefaultsRunner::default());

        runtime
            .run_live_all(StackMode::Dataplane, false)
            .expect("both live-test passes should be prepared");

        let runs = runtime.runner.runs.borrow();
        assert_eq!(runs.len(), 2);
        assert!(
            runs.iter()
                .all(|command| command.working_directory() == Some(expected.as_path()))
        );
    }

    #[test]
    fn external_primary_token_does_not_mint_a_wrong_signature_scope_fixture() {
        let runtime = RuntimeExecutor::new(
            test_config([("MCPGATEWAY_BEARER_TOKEN", "externally-signed")]),
            DefaultsRunner::default(),
        );
        assert_eq!(
            runtime
                .wrong_scope_token("selected-server")
                .expect("fixture selection should succeed"),
            None
        );

        let runtime = RuntimeExecutor::new(
            test_config([
                ("MCPGATEWAY_BEARER_TOKEN", "externally-signed"),
                (
                    "MCPGATEWAY_WRONG_SCOPE_BEARER_TOKEN",
                    "externally-signed-for-another-server",
                ),
            ]),
            DefaultsRunner::default(),
        );
        assert_eq!(
            runtime
                .wrong_scope_token("selected-server")
                .expect("explicit fixture selection should succeed")
                .as_deref(),
            Some("externally-signed-for-another-server")
        );
    }

    #[test]
    fn readiness_accepts_only_expected_unauthenticated_mcp_statuses() {
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::METHOD_NOT_ALLOWED,
        ] {
            assert!(is_expected_readiness_status(status));
        }
        for status in [
            StatusCode::OK,
            StatusCode::FOUND,
            StatusCode::NOT_FOUND,
            StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            assert!(!is_expected_readiness_status(status));
        }
    }

    #[test]
    fn locust_report_audit_runs_after_a_failed_child_and_removes_every_leak() {
        let reports = tempfile::tempdir().expect("temporary report directory");
        let nested = reports.path().join("nested");
        fs::create_dir(&nested).expect("nested report directory should be created");
        let token = "locust-token-in-failed-child-reports";
        let first = reports.path().join("report.html");
        let second = nested.join("failures.csv");
        fs::write(&first, format!("first {token}")).expect("first report should be written");
        fs::write(&second, format!("second {token}")).expect("second report should be written");

        let error = finalize_locust_run(
            Err(AppFailure::from(anyhow!("Locust child failed"))),
            reports.path(),
            token,
        )
        .expect_err("report audit must run even when Locust exits unsuccessfully");

        assert!(error.to_string().contains("removed 2 Locust report"));
        assert!(!first.exists());
        assert!(!second.exists());
    }

    #[tokio::test]
    async fn public_endpoint_wait_retries_server_errors_until_the_route_is_ready() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let handler_attempts = Arc::clone(&attempts);
        let application = Router::new().route(
            "/mcp",
            get(move || {
                let handler_attempts = Arc::clone(&handler_attempts);
                async move {
                    if handler_attempts.fetch_add(1, Ordering::SeqCst) < 2 {
                        StatusCode::SERVICE_UNAVAILABLE
                    } else {
                        StatusCode::UNAUTHORIZED
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let endpoint = url::Url::parse(&format!(
            "http://{}/mcp",
            listener.local_addr().expect("listener address")
        ))
        .expect("test endpoint should parse");
        let server = tokio::spawn(async move {
            axum::serve(listener, application)
                .await
                .expect("test server should run");
        });

        wait_for_http_endpoint(&endpoint, StackMode::Controlplane, Duration::from_secs(2))
            .await
            .expect("readiness should wait through transient server errors");

        assert!(attempts.load(Ordering::SeqCst) >= 3);
        server.abort();
    }

    #[tokio::test]
    async fn public_endpoint_wait_retries_404_and_redirect_responses() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let handler_attempts = Arc::clone(&attempts);
        let application = Router::new().route(
            "/mcp",
            get(move || {
                let attempt = handler_attempts.fetch_add(1, Ordering::SeqCst);
                async move {
                    match attempt {
                        0 => StatusCode::NOT_FOUND,
                        1 => StatusCode::FOUND,
                        _ => StatusCode::FORBIDDEN,
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let endpoint = url::Url::parse(&format!(
            "http://{}/mcp",
            listener.local_addr().expect("listener address")
        ))
        .expect("test endpoint should parse");
        let server = tokio::spawn(async move {
            axum::serve(listener, application)
                .await
                .expect("test server should run");
        });

        wait_for_http_endpoint(&endpoint, StackMode::Controlplane, Duration::from_secs(2))
            .await
            .expect("readiness should wait for a real unauthenticated MCP response");

        assert!(attempts.load(Ordering::SeqCst) >= 3);
        server.abort();
    }

    #[tokio::test]
    async fn public_endpoint_wait_times_out_with_actionable_context() {
        let application =
            Router::new().route("/mcp", get(|| async { StatusCode::SERVICE_UNAVAILABLE }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let endpoint = url::Url::parse(&format!(
            "http://{}/mcp",
            listener.local_addr().expect("listener address")
        ))
        .expect("test endpoint should parse");
        let server = tokio::spawn(async move {
            axum::serve(listener, application)
                .await
                .expect("test server should run");
        });

        let error =
            wait_for_http_endpoint(&endpoint, StackMode::Dataplane, Duration::from_millis(75))
                .await
                .expect_err("persistent server errors must not be treated as ready");

        let message = error.to_string();
        assert!(message.contains("dataplane"));
        assert!(message.contains("503"));
        assert!(message.contains(endpoint.as_str()));
        server.abort();
    }

    #[tokio::test]
    async fn conformance_proxy_shuts_down_after_subprocess_failure_and_hides_token() {
        let config = test_config([]);
        let runtime = RuntimeExecutor::new(config, FailingNpxRunner::default());
        let artifacts = tempfile::tempdir().expect("temporary artifact root");
        let paths = CompliancePaths::new(artifacts.path(), artifacts.path().join("reports"));
        let token = "must-not-appear-in-child-argv";

        let error = runtime
            .run_official_conformance_mode(
                &OfficialConformanceRun {
                    mode: StackMode::Controlplane,
                    server_id: "server-id",
                    token,
                    spec_version: "2025-11-25",
                    suite: "active",
                    custom_baseline: None,
                    fixture: None,
                    cancellation: tokio::sync::watch::channel(false).1,
                },
                &paths,
            )
            .await
            .expect_err("subprocess failure must be returned");
        assert!(error.to_string().contains("deliberate npx failure"));

        let command = runtime
            .runner
            .command
            .borrow()
            .clone()
            .expect("npx command should be captured");
        assert!(
            command
                .arguments()
                .iter()
                .all(|argument| !argument.to_string_lossy().contains(token))
        );
        assert!(!command.inherits_environment());
        assert!(command.environment().keys().all(|key| {
            NPM_ENV_ALLOWLIST
                .iter()
                .any(|allowed| key == OsStr::new(allowed))
        }));
        assert!(
            !command
                .environment()
                .contains_key(OsStr::new("JWT_SECRET_KEY"))
        );
        let endpoint_index = command
            .arguments()
            .iter()
            .position(|argument| argument == "--url")
            .expect("official command contains --url")
            + 1;
        let endpoint = url::Url::parse(
            command.arguments()[endpoint_index]
                .to_str()
                .expect("proxy URL is UTF-8"),
        )
        .expect("proxy URL should parse");
        let address = (
            endpoint.host_str().expect("proxy URL host"),
            endpoint.port().expect("proxy URL port"),
        );
        assert!(
            tokio::net::TcpStream::connect(address).await.is_err(),
            "proxy listener must be closed before the subprocess failure is returned"
        );
        let mode_paths = paths.conformance_mode(BaselineTarget::Controlplane);
        assert!(!mode_paths.completion.exists());
        let report_error = match runtime
            .load_conformance_artifact(&paths, BaselineTarget::Controlplane)
        {
            Err(error) => error,
            Ok(_) => panic!("report-only mode must reject artifacts from an incomplete child run"),
        };
        assert!(
            report_error
                .to_string()
                .contains("incomplete conformance artifacts")
        );
    }

    #[tokio::test]
    async fn existing_dataplane_compliance_setup_waits_for_the_selected_server() {
        let config = test_config([("CF_PUBLISHER_WAIT_SECONDS", "0")]);
        let runtime = RuntimeExecutor::new(
            config,
            PublisherRunner {
                snapshot_present: true,
                commands: RefCell::new(Vec::new()),
            },
        );

        runtime
            .complete_compliance_setup(StackMode::Dataplane, "selected-server", Ok(()))
            .await
            .expect("existing-stack compliance should wait for its publisher snapshot");
        let commands = runtime.runner.commands.borrow();
        let probe = commands.last().expect("publisher probe should run");
        assert_eq!(probe.program(), "docker");
        assert_eq!(probe.arguments().last(), Some(&"selected-server".into()));
    }

    #[tokio::test]
    async fn publisher_wait_timeout_is_a_hard_actionable_failure() {
        let config = test_config([("CF_PUBLISHER_WAIT_SECONDS", "0")]);
        let runtime = RuntimeExecutor::new(
            config,
            PublisherRunner {
                snapshot_present: false,
                commands: RefCell::new(Vec::new()),
            },
        );

        let error = runtime
            .wait_for_publisher_snapshot("missing-server")
            .await
            .expect_err("missing server must fail instead of racing the test");
        let message = error.to_string();
        assert!(message.contains("missing-server"));
        assert!(message.contains("publisher"));
        assert!(message.contains("Redis logs"));
    }

    #[tokio::test]
    async fn publisher_wait_matches_decoded_virtual_host_keys_not_byte_substrings() {
        assert!(PUBLISHER_SNAPSHOT_LUA.contains("pcall(cmsgpack.unpack, value)"));
        assert!(PUBLISHER_SNAPSHOT_LUA.contains("config.virtual_hosts[ARGV[1]] ~= nil"));
        assert!(!PUBLISHER_SNAPSHOT_LUA.contains("string.find"));
        let runtime = RuntimeExecutor::new(
            test_config([("CF_PUBLISHER_WAIT_SECONDS", "0")]),
            ExactPublisherRunner {
                virtual_hosts: BTreeSet::from(["xabcx"]),
                commands: RefCell::new(Vec::new()),
            },
        );

        let error = runtime
            .wait_for_publisher_snapshot("abc")
            .await
            .expect_err("a substring collision must not satisfy publisher readiness");
        assert!(error.to_string().contains("abc"));
        runtime
            .wait_for_publisher_snapshot("xabcx")
            .await
            .expect("the exact decoded virtual-host key should satisfy readiness");
    }

    #[tokio::test]
    async fn individual_dataplane_tests_and_inspect_guard_ports_and_wait_for_selected_server() {
        let runtime = RuntimeExecutor::new(
            test_config([("CF_PUBLISHER_WAIT_SECONDS", "0")]),
            TargetGuardRunner {
                other_running: false,
                commands: RefCell::new(Vec::new()),
            },
        );

        runtime
            .prepare_test_target(StackMode::Dataplane, "selected-server")
            .await
            .expect("dataplane target should be isolated and published");

        let commands = runtime.runner.commands.borrow();
        assert_eq!(commands.len(), 3);
        assert!(commands[0].arguments().iter().any(|argument| {
            argument
                .to_string_lossy()
                .contains("com.docker.compose.project=cf-controlplane-only")
        }));
        assert!(commands[1].arguments().iter().any(|argument| {
            argument
                .to_string_lossy()
                .contains("com.docker.compose.service=redis")
        }));
        assert_eq!(
            commands[2].arguments().last(),
            Some(&"selected-server".into())
        );
    }

    #[tokio::test]
    async fn individual_tests_reject_the_opposite_stack_on_shared_ports() {
        let runtime = RuntimeExecutor::new(
            test_config([]),
            TargetGuardRunner {
                other_running: true,
                commands: RefCell::new(Vec::new()),
            },
        );

        let error = runtime
            .prepare_test_target(StackMode::Controlplane, "unused-server")
            .await
            .expect_err("the dataplane stack must not satisfy a control-plane test");

        assert!(
            error
                .to_string()
                .contains("dataplane integration stack is running")
        );
    }

    #[tokio::test]
    async fn dataplane_fast_test_registration_waits_for_its_publisher_snapshot() {
        let config = test_config([("CF_PUBLISHER_WAIT_SECONDS", "0")]);
        let runtime = RuntimeExecutor::new(
            config,
            PublisherRunner {
                snapshot_present: true,
                commands: RefCell::new(Vec::new()),
            },
        );

        runtime
            .ensure_fast_test_fixture(StackMode::Dataplane)
            .await
            .expect("registration should wait for the publisher snapshot");

        let commands = runtime.runner.commands.borrow();
        let registration = commands
            .iter()
            .position(|command| {
                command
                    .arguments()
                    .iter()
                    .any(|argument| argument == "register_fast_test")
            })
            .expect("registration command should run");
        let snapshot = commands
            .iter()
            .position(|command| {
                command.program() == "docker"
                    && command
                        .arguments()
                        .first()
                        .is_some_and(|argument| argument == "exec")
                    && command
                        .arguments()
                        .last()
                        .is_some_and(|argument| argument == FAST_TEST_SERVER_ID)
            })
            .expect("publisher snapshot should be queried for the registered server");
        assert!(registration < snapshot);
    }
}
