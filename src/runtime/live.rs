//! Upstream live-gateway test orchestration.

use super::*;

const FAST_TEST_SERVER_ID: &str = "b8e3f1a2c4d5e6f7a1b2c3d4e5f6a7b8";

impl<R: ProcessRunner> RuntimeExecutor<R> {
    pub(super) async fn run_live(
        &self,
        lane: LiveLane,
        group: LiveGroup,
        protocol_version: &ProtocolVersion,
    ) -> AppResult<()> {
        match lane {
            LiveLane::Fixture => {
                if group != LiveGroup::Protocol {
                    return Err(AppFailure::from(anyhow!(
                        "fixture-direct live lane requires the protocol group"
                    )));
                }
                self.ensure_controlplane()?;
                self.run_controlplane_make(
                    StackMode::Controlplane,
                    "test-protocol-compliance-reference",
                    protocol_version,
                )
            }
            LiveLane::Controlplane => {
                self.run_routed_live(StackMode::Controlplane, group, protocol_version)
                    .await
            }
            LiveLane::Dataplane => {
                self.run_routed_live(StackMode::Dataplane, group, protocol_version)
                    .await
            }
        }
    }

    async fn run_routed_live(
        &self,
        topology: StackMode,
        group: LiveGroup,
        protocol_version: &ProtocolVersion,
    ) -> AppResult<()> {
        let server_id = self.default_server_id().to_owned();
        self.with_managed_test_target(topology, &server_id, || async {
            if live_group_needs_fast_test(group) {
                self.ensure_fast_test_fixture(topology).await?;
            }
            match group {
                LiveGroup::Mcp => {
                    self.run_controlplane_make(topology, "test-mcp-protocol-e2e", protocol_version)
                }
                LiveGroup::Rbac => {
                    self.run_controlplane_make(topology, "test-mcp-rbac", protocol_version)
                }
                LiveGroup::Protocol => self.run_controlplane_make(
                    topology,
                    "test-protocol-compliance-gateway",
                    protocol_version,
                ),
                LiveGroup::All => self.run_live_all(topology, protocol_version),
            }
        })
        .await
    }

    async fn ensure_fast_test_fixture(&self, topology: StackMode) -> AppResult<()> {
        let project = self.compose_project(topology);
        for plan in [
            StackCommandPlan::fast_test_up(project.clone()),
            StackCommandPlan::fast_test_register(project),
        ] {
            self.runner.run(&self.compose_environment(
                plan.command().clone(),
                topology,
                true,
            )?)?;
        }
        if topology == StackMode::Dataplane {
            self.wait_for_publisher_snapshot(FAST_TEST_SERVER_ID)
                .await?;
        }
        Ok(())
    }

    fn run_controlplane_make(
        &self,
        topology: StackMode,
        target: &str,
        protocol_version: &ProtocolVersion,
    ) -> AppResult<()> {
        let command = CommandSpec::new("make")
            .arg("-C")
            .arg(self.config.controlplane_dir().as_os_str())
            .arg(target);
        let command = self.live_protocol_environment(command, protocol_version)?;
        Ok(self
            .runner
            .run(&self.compose_environment(command, topology, false)?)?)
    }

    fn run_live_all(
        &self,
        topology: StackMode,
        protocol_version: &ProtocolVersion,
    ) -> AppResult<()> {
        let pass_one = CommandSpec::new("uv")
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
                "-v",
                "--tb=short",
            ])
            .cwd(self.config.controlplane_dir());
        let pass_one = self.live_protocol_environment(pass_one, protocol_version)?;
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
        let pass_two = self.live_protocol_environment(pass_two, protocol_version)?;

        let first = self
            .runner
            .run(&self.compose_environment(pass_one, topology, false)?)
            .map_err(AppFailure::from);
        let second = self
            .runner
            .run(&self.compose_environment(pass_two, topology, false)?)
            .map_err(AppFailure::from);
        combine_live_results(first, second)
    }

    fn live_protocol_environment(
        &self,
        command: CommandSpec,
        protocol_version: &ProtocolVersion,
    ) -> AppResult<CommandSpec> {
        let inherited_python_path = self
            .config
            .environment()
            .get(OsStr::new("PYTHONPATH"))
            .map(|value| value.value.as_os_str());
        add_live_protocol_environment(
            command,
            &self.config.root().join("scripts/live_protocol"),
            inherited_python_path,
            protocol_version.as_str(),
        )
    }
}

fn add_live_protocol_environment(
    command: CommandSpec,
    hook_directory: &Path,
    inherited_python_path: Option<&OsStr>,
    protocol_version: &str,
) -> AppResult<CommandSpec> {
    let python_paths = std::iter::once(hook_directory.to_path_buf()).chain(
        inherited_python_path
            .into_iter()
            .flat_map(std::env::split_paths),
    );
    let python_path = std::env::join_paths(python_paths)
        .context("failed to construct live-test PYTHONPATH")
        .map_err(AppFailure::from)?;
    Ok(command
        .env("PYTHONPATH", python_path)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("MCP_PROTOCOL_VERSION", protocol_version)
        .env("CF_LIVE_MCP_PROTOCOL_VERSION", protocol_version))
}

const fn live_group_needs_fast_test(group: LiveGroup) -> bool {
    matches!(group, LiveGroup::Mcp | LiveGroup::All)
}

fn combine_live_results(first: AppResult<()>, second: AppResult<()>) -> AppResult<()> {
    match (first, second) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(first), Err(second)) => Err(AppFailure::from(anyhow!(
            "first live-test pass failed: {first}; second live-test pass also failed: {second}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_test_is_limited_to_live_groups_that_exercise_its_tools() {
        assert!(live_group_needs_fast_test(LiveGroup::Mcp));
        assert!(live_group_needs_fast_test(LiveGroup::All));
        assert!(!live_group_needs_fast_test(LiveGroup::Rbac));
        assert!(!live_group_needs_fast_test(LiveGroup::Protocol));
    }

    #[test]
    fn both_live_all_failures_are_preserved() {
        let first = Err(AppFailure::from(anyhow!("first failure")));
        let second = Err(AppFailure::from(anyhow!("second failure")));

        let error = combine_live_results(first, second)
            .expect_err("both failures should fail the live workflow")
            .to_string();

        assert!(error.contains("first failure"));
        assert!(error.contains("second failure"));
    }

    #[test]
    fn live_protocol_environment_prepends_hook_and_sets_selected_version() {
        let command = add_live_protocol_environment(
            CommandSpec::new("pytest"),
            Path::new("/harness/live-protocol"),
            Some(OsStr::new("/existing/python")),
            "2025-06-18",
        )
        .expect("live protocol environment should be valid");

        let environment = command.environment();
        assert_eq!(
            environment.get(OsStr::new("MCP_PROTOCOL_VERSION")),
            Some(&OsString::from("2025-06-18"))
        );
        assert_eq!(
            environment.get(OsStr::new("CF_LIVE_MCP_PROTOCOL_VERSION")),
            Some(&OsString::from("2025-06-18"))
        );
        assert_eq!(
            environment.get(OsStr::new("PYTHONDONTWRITEBYTECODE")),
            Some(&OsString::from("1"))
        );
        let paths = std::env::split_paths(
            environment
                .get(OsStr::new("PYTHONPATH"))
                .expect("PYTHONPATH should be set"),
        )
        .collect::<Vec<_>>();
        assert_eq!(
            paths,
            [
                PathBuf::from("/harness/live-protocol"),
                PathBuf::from("/existing/python")
            ]
        );
    }
}
