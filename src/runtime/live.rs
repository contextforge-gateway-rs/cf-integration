//! Upstream live-gateway test orchestration.

use super::*;

const FAST_TEST_SERVER_ID: &str = "b8e3f1a2c4d5e6f7a1b2c3d4e5f6a7b8";

impl<R: ProcessRunner> RuntimeExecutor<R> {
    pub(super) async fn run_live(&self, topology: StackMode, group: LiveGroup) -> AppResult<()> {
        let server_id = self.default_server_id().to_owned();
        self.with_managed_test_target(topology, &server_id, || async {
            if live_group_needs_fast_test(group) {
                self.ensure_fast_test_fixture(topology).await?;
            }
            match group {
                LiveGroup::Mcp => self.run_controlplane_make(topology, "test-mcp-protocol-e2e"),
                LiveGroup::Rbac => self.run_controlplane_make(topology, "test-mcp-rbac"),
                LiveGroup::Protocol => {
                    self.run_controlplane_make(topology, "test-protocol-compliance-gateway")
                }
                LiveGroup::All => self.run_live_all(topology),
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

    fn run_controlplane_make(&self, topology: StackMode, target: &str) -> AppResult<()> {
        let command = CommandSpec::new("make")
            .arg("-C")
            .arg(self.config.controlplane_dir().as_os_str())
            .arg(target);
        Ok(self
            .runner
            .run(&self.compose_environment(command, topology, false)?)?)
    }

    fn run_live_all(&self, topology: StackMode) -> AppResult<()> {
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
}
