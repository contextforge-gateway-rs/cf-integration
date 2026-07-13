//! Official conformance and gateway-compliance orchestration.

use super::*;

impl<R: ProcessRunner> RuntimeExecutor<R> {
    pub(super) fn caller_managed_server_id<'a>(
        &'a self,
        explicit: Option<&'a str>,
    ) -> Option<&'a str> {
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

    pub(super) fn require_loopback_fixture_base_url(&self) -> AppResult<()> {
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

    pub(super) async fn complete_compliance_setup(
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

    pub(super) fn conformance_compose_project(&self, mode: StackMode) -> ComposeProject {
        self.compose_project(mode)
            .with_conformance_fixture(self.config.root())
    }

    pub(super) async fn start_conformance_service(&self, mode: StackMode) -> AppResult<()> {
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

    pub(super) fn conformance_fixture_endpoint(&self, mode: StackMode) -> AppResult<url::Url> {
        let command = self.conformance_compose_project(mode).command([
            "port",
            OFFICIAL_CONFORMANCE_SERVICE,
            "3000",
        ]);
        let command = self.compose_environment(command, mode, true)?;
        let output = self.runner.capture_stdout(&command)?;
        parse_conformance_fixture_endpoint(&output).map_err(AppFailure::from)
    }

    pub(super) async fn stop_conformance_service(&self, mode: StackMode) -> AppResult<()> {
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

    pub(super) async fn execute_compliance(&self, action: ComplianceAction) -> AppResult<()> {
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

    pub(super) async fn run_compliance_workflow(
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

    pub(super) async fn run_compliance_workflow_with_interrupt<I>(
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
        let suite_name = conformance_suite.map(ConformanceSuite::label);
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

        let selected_modes = selected_modes(common.mode);
        let direct_fixture_mode = selected_modes.first().copied();
        for mode in selected_modes {
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
            let mut fixture_endpoint = None;
            let mut selected_server_id = base_server_id;

            if mode_failure.is_none() && auto_fixture {
                let (start_result, start_interrupted) = finish_phase_after_interrupt(
                    self.start_conformance_service(mode),
                    interrupt.as_mut(),
                )
                .await;
                interrupted |= start_interrupted;
                match start_result {
                    Ok(()) => match self
                        .conformance_fixture_endpoint(mode)
                        .and_then(|endpoint| {
                            fixture_endpoint = Some(endpoint);
                            self.admin_token().and_then(|token| {
                                ConformanceFixtureClient::builder(self.base_url()?, token)
                                    .build()
                                    .map_err(AppFailure::from)
                            })
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
                    if direct_fixture_mode == Some(mode)
                        && let (Some(suite_name), Some(endpoint), Some(fixture)) = (
                            suite_name,
                            fixture_endpoint.as_ref(),
                            fixture_metadata.as_ref(),
                        )
                        && let Err(error) = self
                            .run_official_conformance_direct(
                                &DirectConformanceRun {
                                    endpoint,
                                    spec_version: &common.spec_version,
                                    suite: suite_name,
                                    fixture,
                                    cancellation: cancellation_receiver.clone(),
                                },
                                &paths,
                            )
                            .await
                    {
                        failure = Some(error);
                    }
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
                        let gateway_spec_version = if suite_name.is_some() {
                            GATEWAY_SPEC_VERSION
                        } else {
                            &common.spec_version
                        };
                        let gateway_result = tokio::select! {
                            result = self.run_gateway_compliance_mode(
                                mode,
                                &selected_server_id,
                                &token,
                                gateway_spec_version,
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
        if !interrupted
            && run_gateway
            && suite_name.is_some()
            && common.spec_version == COVERAGE_SPEC_VERSION
        {
            match self.write_spec_coverage_report(&paths.report_output, &paths) {
                Ok(path) => println!("Specification coverage: {}", path.display()),
                Err(error) => last_failure = Some(error),
            }
        } else if !interrupted && run_gateway && suite_name.is_some() {
            println!(
                "Specification coverage skipped: inventory is pinned to {COVERAGE_SPEC_VERSION}, official conformance used {}",
                common.spec_version
            );
        }

        last_failure.map_or(Ok(()), Err)
    }

    pub(super) async fn run_official_conformance_mode(
        &self,
        run: &OfficialConformanceRun<'_>,
        paths: &CompliancePaths,
    ) -> AppResult<()> {
        let baseline_target = baseline_target(run.mode);
        let baseline_path = run
            .custom_baseline
            .map(Path::to_owned)
            .unwrap_or_else(|| default_baseline_path(self.config.root(), baseline_target));
        let baseline = load_baseline(&baseline_path, baseline_target).map_err(AppFailure::from)?;
        let target = conformance_target(run.mode);
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
        let result = self
            .run_official_conformance_target(
                &ConformanceTargetRun {
                    target,
                    endpoint: proxy.url(),
                    spec_version: run.spec_version,
                    suite: run.suite,
                    baseline: &baseline,
                    baseline_target: Some(baseline_target),
                    fixture: run.fixture,
                    cancellation: run.cancellation.clone(),
                },
                paths,
            )
            .await;
        let shutdown = proxy
            .shutdown()
            .await
            .context("failed to stop the conformance authentication proxy")
            .map_err(AppFailure::from);
        finish_with_cleanup(result.err(), shutdown)
    }

    pub(super) async fn run_official_conformance_direct(
        &self,
        run: &DirectConformanceRun<'_>,
        paths: &CompliancePaths,
    ) -> AppResult<()> {
        let baseline = Baseline::default();
        self.run_official_conformance_target(
            &ConformanceTargetRun {
                target: ConformanceTarget::Fixture,
                endpoint: run.endpoint,
                spec_version: run.spec_version,
                suite: run.suite,
                baseline: &baseline,
                baseline_target: None,
                fixture: Some(run.fixture),
                cancellation: run.cancellation.clone(),
            },
            paths,
        )
        .await
    }

    async fn run_official_conformance_target(
        &self,
        run: &ConformanceTargetRun<'_>,
        paths: &CompliancePaths,
    ) -> AppResult<()> {
        expected_server_scenarios(run.suite, run.spec_version).map_err(AppFailure::from)?;
        let mode_paths = paths.conformance_mode(run.target);
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
        match run.baseline_target {
            Some(target) => {
                write_official_baseline_projection(run.baseline, target, &mode_paths.projection)
                    .map_err(AppFailure::from)?;
            }
            None => fs::write(&mode_paths.projection, "server: []\n")
                .with_context(|| {
                    format!(
                        "failed to write direct-fixture expected-failure projection {:?}",
                        mode_paths.projection
                    )
                })
                .map_err(AppFailure::from)?,
        }
        write_rich_baseline(&mode_paths.rich_baseline, run.baseline)?;
        write_run_metadata(
            &mode_paths.metadata,
            &ConformanceRunMetadata {
                oracle: cf_integration_compliance::conformance::OFFICIAL_CONFORMANCE_PACKAGE
                    .to_owned(),
                target: run.target.label().to_owned(),
                spec_version: run.spec_version.to_owned(),
                suite: run.suite.to_owned(),
                fixture: run.fixture.cloned(),
            },
        )?;

        let command = allowlisted_npx_environment(
            official_server_command(
                run.endpoint.as_str(),
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

        let results = load_server_results(&mode_paths.official_results).map_err(AppFailure::from);
        let audit = results
            .as_ref()
            .ok()
            .map(|results| audit_baseline(results, run.baseline));
        if let Some(audit) = audit.as_ref()
            && !audit.is_clean()
        {
            eprintln!("{}", format_baseline_audit(run.target, audit));
        }

        if !conformance_process_completed(&process_result) {
            return process_result;
        }
        let results = results?;
        if !is_trusted_official_fixture(run.fixture) {
            validate_no_fixture_failures(&results).map_err(AppFailure::from)?;
        }
        mark_conformance_complete(
            &process_result,
            &results,
            run.target,
            run.suite,
            run.spec_version,
            &mode_paths.completion,
        )?;
        let audit = audit.ok_or_else(|| {
            AppFailure::from(anyhow!(
                "failed to audit parsed official conformance results for {target}",
                target = run.target
            ))
        })?;
        if !audit.is_clean() {
            return Err(AppFailure::from(anyhow!(format_baseline_audit(
                run.target, &audit
            ))));
        }
        process_result?;
        println!(
            "Official conformance artifacts ({}): {}",
            run.target,
            mode_paths.root.display()
        );
        Ok(())
    }

    pub(super) async fn run_gateway_compliance_mode(
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

    pub(super) fn wrong_scope_token(&self, server_id: &str) -> AppResult<Option<String>> {
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
}

pub(super) const fn uses_automatic_conformance_fixture(
    has_conformance_suite: bool,
    server_id: Option<&str>,
) -> bool {
    has_conformance_suite && server_id.is_none()
}

pub(super) fn selected_compliance_server_id<'a>(
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

pub(super) fn parse_conformance_fixture_endpoint(output: &[u8]) -> anyhow::Result<url::Url> {
    let output = std::str::from_utf8(output).context("Compose fixture port output is not UTF-8")?;
    let address = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .ok_or_else(|| anyhow!("Compose did not publish the conformance fixture port"))?
        .parse::<std::net::SocketAddr>()
        .context("Compose returned an invalid conformance fixture address")?;
    if !address.ip().is_loopback() {
        return Err(anyhow!(
            "Compose published the conformance fixture on non-loopback address {}",
            address.ip()
        ));
    }
    url::Url::parse(&format!("http://{address}/mcp"))
        .context("failed to construct the direct conformance fixture URL")
}

pub(super) struct OfficialConformanceRun<'a> {
    pub(super) mode: StackMode,
    pub(super) server_id: &'a str,
    pub(super) token: &'a str,
    pub(super) spec_version: &'a str,
    pub(super) suite: &'a str,
    pub(super) custom_baseline: Option<&'a Path>,
    pub(super) fixture: Option<&'a ConformanceFixtureMetadata>,
    pub(super) cancellation: tokio::sync::watch::Receiver<bool>,
}

pub(super) struct DirectConformanceRun<'a> {
    pub(super) endpoint: &'a url::Url,
    pub(super) spec_version: &'a str,
    pub(super) suite: &'a str,
    pub(super) fixture: &'a ConformanceFixtureMetadata,
    pub(super) cancellation: tokio::sync::watch::Receiver<bool>,
}

struct ConformanceTargetRun<'a> {
    target: ConformanceTarget,
    endpoint: &'a url::Url,
    spec_version: &'a str,
    suite: &'a str,
    baseline: &'a Baseline,
    baseline_target: Option<BaselineTarget>,
    fixture: Option<&'a ConformanceFixtureMetadata>,
    cancellation: tokio::sync::watch::Receiver<bool>,
}

pub(super) fn combine_cleanup_results(
    first: AppResult<()>,
    second: AppResult<()>,
) -> AppResult<()> {
    match (first, second) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(first), Err(second)) => Err(AppFailure::from(anyhow!(
            "{first}; additionally conformance service cleanup failed: {second}"
        ))),
    }
}

pub(super) async fn finish_phase_after_interrupt<F, I, T>(
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

pub(super) async fn wait_for_runtime_cancellation(
    cancellation: &mut tokio::sync::watch::Receiver<bool>,
) {
    while !*cancellation.borrow_and_update() {
        if cancellation.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

pub(super) fn interrupted_conformance_failure() -> AppFailure {
    AppFailure::from(anyhow!("conformance workflow interrupted by Ctrl-C"))
}

pub(super) fn finish_with_cleanup(
    primary: Option<AppFailure>,
    cleanup: AppResult<()>,
) -> AppResult<()> {
    match (primary, cleanup) {
        (None, Ok(())) => Ok(()),
        (Some(primary), Ok(())) => Err(primary),
        (None, Err(cleanup)) => Err(cleanup),
        (Some(primary), Err(cleanup)) => Err(AppFailure::from(anyhow!(
            "{primary}; additionally conformance cleanup failed: {cleanup}"
        ))),
    }
}
