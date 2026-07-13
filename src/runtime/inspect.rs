//! Official MCP Inspector composition.

use super::*;

const INSPECTOR_PACKAGE: &str = "@modelcontextprotocol/inspector@0.22.0";
pub(super) const NPM_ENV_ALLOWLIST: &[&str] = &[
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

impl<R: ProcessRunner> RuntimeExecutor<R> {
    pub(super) async fn inspect(
        &self,
        mode: StackMode,
        method: &str,
        server_id: Option<&str>,
    ) -> AppResult<()> {
        self.require_mode_sources(mode)?;
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

pub(super) fn inspector_command(endpoint: &str, method: &str) -> CommandSpec {
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

pub(super) fn allowlisted_npx_environment(mut command: CommandSpec) -> CommandSpec {
    command = command.clear_environment();
    for key in NPM_ENV_ALLOWLIST {
        if let Some(value) = std::env::var_os(key) {
            command = command.env(key, value);
        }
    }
    command
}
