//! Probe, load, live-test, and suite workflow composition.

use super::*;

const FAST_TEST_SERVER_ID: &str = "b8e3f1a2c4d5e6f7a1b2c3d4e5f6a7b8";
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

impl<R: ProcessRunner> RuntimeExecutor<R> {
    pub(super) async fn execute_test(&self, action: TestAction) -> AppResult<()> {
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
        let settings =
            LoadSettings::resolve(&self.config, &args.request).map_err(AppFailure::from)?;
        match args.request.engine {
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

    pub(super) fn ensure_other_stack_for_tests(&self, mode: StackMode) -> AppResult<()> {
        self.ensure_other_stack_stopped(mode)
    }

    pub(super) async fn prepare_test_target(
        &self,
        mode: StackMode,
        server_id: &str,
    ) -> AppResult<()> {
        self.ensure_other_stack_for_tests(mode)?;
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
                    request: LoadRequest {
                        engine: LoadEngine::Locust,
                        smoke: true,
                        users: None,
                        spawn_rate: None,
                        run_time: None,
                    },
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
                        request: LoadRequest {
                            engine: *engine,
                            smoke: false,
                            users: None,
                            spawn_rate: None,
                            run_time: None,
                        },
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

    pub(super) async fn wait_for_publisher_snapshot(&self, server_id: &str) -> AppResult<()> {
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
}

fn finalize_locust_run(
    process_result: AppResult<()>,
    report_dir: &Path,
    bearer_token: &str,
) -> AppResult<()> {
    audit_locust_reports(report_dir, bearer_token).map_err(AppFailure::from)?;
    process_result
}

#[cfg(test)]
mod unit {
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
    use cf_integration_compliance::conformance::ScenarioOutcome;
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
                assert_eq!(
                    spec.arguments().get(1).and_then(|value| value.to_str()),
                    Some(cf_integration_compliance::OFFICIAL_CONFORMANCE_PACKAGE),
                    "official workflow must execute the pinned alpha.9 oracle"
                );
                let spec_version = spec
                    .arguments()
                    .windows(2)
                    .find(|values| values[0] == "--spec-version")
                    .map(|values| values[1].to_string_lossy().into_owned())
                    .expect("official runner spec version should be present");
                self.events
                    .lock()
                    .expect("event lock")
                    .extend(["npx".into(), format!("npx spec {spec_version}")]);
                let endpoint = spec
                    .arguments()
                    .windows(2)
                    .find(|values| values[0] == "--url")
                    .map(|values| values[1].to_string_lossy().into_owned())
                    .expect("official runner URL should be present");
                if endpoint == "http://127.0.0.1:43123/mcp" {
                    self.events
                        .lock()
                        .expect("event lock")
                        .push("official fixture direct".into());
                    return Err(PlatformError::from(anyhow!(
                        "intentional direct npx failure"
                    )));
                }
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
                    if spec.arguments().iter().any(|value| value == "port")
                        && spec
                            .arguments()
                            .iter()
                            .any(|value| value == OFFICIAL_CONFORMANCE_SERVICE) =>
                {
                    Ok(b"127.0.0.1:43123\n".to_vec())
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
                        "id": cf_integration_compliance::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID,
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
                        "name": cf_integration_compliance::conformance_fixture::OFFICIAL_CONFORMANCE_GATEWAY_NAME,
                        "url": cf_integration_compliance::conformance_fixture::OFFICIAL_CONFORMANCE_BACKEND_URL,
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
                    "name": cf_integration_compliance::conformance_fixture::OFFICIAL_CONFORMANCE_GATEWAY_NAME,
                    "url": cf_integration_compliance::conformance_fixture::OFFICIAL_CONFORMANCE_BACKEND_URL,
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

    #[test]
    fn direct_fixture_endpoint_accepts_only_a_published_loopback_address() {
        assert_eq!(
            parse_conformance_fixture_endpoint(b"127.0.0.1:43123\n")
                .expect("loopback fixture endpoint should parse")
                .as_str(),
            "http://127.0.0.1:43123/mcp"
        );
        assert_eq!(
            parse_conformance_fixture_endpoint(b"[::1]:43124\n")
                .expect("IPv6 loopback fixture endpoint should parse")
                .as_str(),
            "http://[::1]:43124/mcp"
        );

        for output in [
            b"0.0.0.0:43123\n".as_slice(),
            b"192.0.2.10:43123\n".as_slice(),
            b"not-an-address\n".as_slice(),
            b"\n".as_slice(),
        ] {
            assert!(
                parse_conformance_fixture_endpoint(output).is_err(),
                "unsafe or invalid Compose output must be rejected: {output:?}"
            );
        }
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
            spec_version: "2026-07-28".to_owned(),
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
                repository:
                    cf_integration_compliance::conformance_fixture::OFFICIAL_CONFORMANCE_REPOSITORY
                        .to_owned(),
                revision:
                    cf_integration_compliance::conformance_fixture::OFFICIAL_CONFORMANCE_REVISION
                        .to_owned(),
                server_id:
                    cf_integration_compliance::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID
                        .to_owned(),
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
                    cf_integration_compliance::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID
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
    async fn combined_workflow_routes_alpha9_july_conformance_through_dataplane_gateway() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let api_state = Arc::new(FixtureApiState {
            events: Arc::clone(&events),
            fail_cleanup: false,
            expected_scoped_server_id: Some(
                cf_integration_compliance::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID
                    .to_owned(),
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
            spec_version: "2026-07-28".to_owned(),
            results_dir: Some(reports.path().to_owned()),
        };

        runtime
            .run_compliance_workflow(&common, Some(ConformanceSuite::Active), true, None)
            .await
            .expect_err("intentional official and gateway responses should fail");

        let events = events.lock().expect("event lock");
        assert!(
            events
                .iter()
                .any(|event| event == "official fixture direct"),
            "the official oracle must first exercise the fixture directly: {events:?}"
        );
        let npx = events
            .iter()
            .position(|event| event == "npx")
            .expect("npx event");
        let official_complete = events
            .iter()
            .position(|event| event == "official proxy complete")
            .expect("official proxy boundary");
        assert!(events.iter().any(|event| event == "npx spec 2026-07-28"));
        let expected_path = format!(
            "/servers/{}/mcp",
            cf_integration_compliance::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID
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
                cf_integration_compliance::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID
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
            cf_integration_compliance::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID
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
            cf_integration_compliance::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID,
        );

        let error = runtime
                .run_official_conformance_mode(
                    &OfficialConformanceRun {
                        mode: StackMode::Controlplane,
                        server_id: cf_integration_compliance::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID,
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
            cf_integration_compliance::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID,
        );

        let error = runtime
                .run_official_conformance_mode(
                    &OfficialConformanceRun {
                        mode: StackMode::Controlplane,
                        server_id: cf_integration_compliance::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID,
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
            cf_integration_compliance::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID,
        );
        for target in [
            ConformanceTarget::Fixture,
            ConformanceTarget::Controlplane,
            ConformanceTarget::Dataplane,
        ] {
            let mode_paths = paths.conformance_mode(target);
            write_official_results(
                &mode_paths.official_results,
                scenarios.iter().copied(),
                "SUCCESS",
            );
            if target == ConformanceTarget::Controlplane {
                write_fixture_failure_result(&mode_paths.official_results, fixture_scenario);
            }
            fs::write(&mode_paths.rich_baseline, "server: []\n")
                .expect("empty rich baseline should be written");
            write_run_metadata(
                &mode_paths.metadata,
                &ConformanceRunMetadata {
                    oracle: cf_integration_compliance::conformance::OFFICIAL_CONFORMANCE_PACKAGE
                        .to_owned(),
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
            "| {fixture_scenario} | compliant | failure | compliant | control-plane only failure |"
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
            cf_integration_compliance::conformance_fixture::OFFICIAL_CONFORMANCE_SERVER_ID,
        );
        for target in [
            ConformanceTarget::Fixture,
            ConformanceTarget::Controlplane,
            ConformanceTarget::Dataplane,
        ] {
            let mode_paths = paths.conformance_mode(target);
            write_official_results(
                &mode_paths.official_results,
                scenarios.iter().copied(),
                "SUCCESS",
            );
            if target == ConformanceTarget::Controlplane {
                write_fixture_failure_result(&mode_paths.official_results, fixture_scenario);
            }
            fs::write(&mode_paths.rich_baseline, "server: []\n")
                .expect("empty rich baseline should be written");
            write_run_metadata(
                &mode_paths.metadata,
                &ConformanceRunMetadata {
                    oracle: cf_integration_compliance::conformance::OFFICIAL_CONFORMANCE_PACKAGE
                        .to_owned(),
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
            "| {fixture_scenario} | compliant | fixture failure | compliant | fixture failure |"
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
                "oracle": cf_integration_compliance::conformance::OFFICIAL_CONFORMANCE_PACKAGE,
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
                oracle: cf_integration_compliance::conformance::OFFICIAL_CONFORMANCE_PACKAGE
                    .to_owned(),
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
                oracle: cf_integration_compliance::conformance::OFFICIAL_CONFORMANCE_PACKAGE
                    .to_owned(),
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
            oracle: cf_integration_compliance::conformance::OFFICIAL_CONFORMANCE_PACKAGE.to_owned(),
            target: "control-plane".to_owned(),
            spec_version: "2025-11-25".to_owned(),
            suite: "active".to_owned(),
            fixture: None,
        };
        let mut dataplane = controlplane.clone();
        dataplane.target = "dataplane".to_owned();
        dataplane.suite = "all".to_owned();

        let error = compatible_metadata(None, Some(&controlplane), Some(&dataplane), None)
            .expect_err("mixed suites must not be reported as one comparison");
        assert!(error.to_string().contains("incompatible runs"));
    }

    fn paired_metadata(fixture: Option<ConformanceFixtureMetadata>) -> ConformanceRunMetadata {
        ConformanceRunMetadata {
            oracle: cf_integration_compliance::conformance::OFFICIAL_CONFORMANCE_PACKAGE.to_owned(),
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

        compatible_metadata(None, Some(&controlplane), Some(&dataplane), None)
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

        compatible_metadata(None, Some(&controlplane), Some(&dataplane), None)
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

        let error = compatible_metadata(None, Some(&controlplane), Some(&dataplane), None)
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

            let error = compatible_metadata(None, Some(&controlplane), Some(&dataplane), None)
                .expect_err("different fixture provenance must not be compared");

            let message = error.to_string();
            assert!(message.contains("fixture provenance mismatch"));
            for value in [repository, revision, server_id] {
                assert!(!message.contains(value));
            }
        }
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
