//! Locust settings precedence and Docker Compose invocation.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::cli::{LoadArgs, LoadEngine, StackMode};
use crate::compose::ComposeProject;
use crate::config::{AppConfig, SourcedValue, ValueOrigin};
use crate::mcp::PROTOCOL_VERSION;
use crate::process::CommandSpec;

const SMOKE_USERS: &str = "1";
const SMOKE_SPAWN_RATE: &str = "1";
const SMOKE_RUN_TIME: &str = "10s";
const LOCUST_ADAPTER_NAME: &str = "locustfile_mcp.py";
const LOCUST_ADAPTER_CONTAINER_PATH: &str = "/mnt/locust-cf/locustfile_mcp.py";
const REQUEST_TIMEOUT_DEFAULT_SECONDS: &str = "60";
const REQUEST_TIMEOUT_ENV: &str = "LOCUST_REQUEST_TIMEOUT_SECONDS";
const REQUEST_TIMEOUT_ERROR: &str =
    "LOCUST_REQUEST_TIMEOUT_SECONDS must be a finite number greater than zero";
const RUN_TIME_ERROR: &str = "LOCUST_RUN_TIME must be a positive Locust duration using h, m, and s at most once in that order";

/// Validated load settings after applying CLI, process, dotenv, and default precedence.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadSettings {
    users: NonZeroUsize,
    spawn_rate: f64,
    run_time: String,
}

impl LoadSettings {
    /// Resolves settings using CLI > process > dotenv/default precedence.
    ///
    /// Smoke mode replaces only dotenv and built-in values. Explicit process
    /// values remain authoritative, including invalid empty values, which are
    /// reported rather than silently replaced.
    ///
    /// # Errors
    ///
    /// Returns an error for a zero or malformed user count, a non-finite or
    /// non-positive spawn rate, or an invalid Locust run-time expression.
    pub fn resolve(config: &AppConfig, arguments: &LoadArgs) -> Result<Self> {
        let users = match arguments.users {
            Some(users) => users,
            None => parse_users(selected_value(
                &config.locust_users,
                arguments.smoke,
                SMOKE_USERS,
            ))?,
        };
        let users = NonZeroUsize::new(users)
            .context("LOCUST_USERS must be an integer greater than zero")?;

        let spawn_rate = match arguments.spawn_rate {
            Some(spawn_rate) if spawn_rate.is_finite() && spawn_rate > 0.0 => spawn_rate,
            Some(_) => bail!("LOCUST_SPAWN_RATE must be a finite number greater than zero"),
            None => parse_spawn_rate(selected_value(
                &config.locust_spawn_rate,
                arguments.smoke,
                SMOKE_SPAWN_RATE,
            ))?,
        };

        let run_time = arguments.run_time.as_deref().map_or_else(
            || {
                selected_value(&config.locust_run_time, arguments.smoke, SMOKE_RUN_TIME)
                    .to_str()
                    .map(str::to_owned)
                    .context("LOCUST_RUN_TIME must be valid UTF-8")
            },
            |run_time| Ok(run_time.to_owned()),
        )?;
        match arguments.engine {
            LoadEngine::Locust => validate_locust_run_time(&run_time)?,
            LoadEngine::Goose => validate_grouped_run_time(&run_time)?,
        }

        Ok(Self {
            users,
            spawn_rate,
            run_time,
        })
    }

    /// Returns the concurrent user count.
    #[must_use]
    pub fn users(&self) -> NonZeroUsize {
        self.users
    }

    /// Returns the users spawned per second.
    #[must_use]
    pub fn spawn_rate(&self) -> f64 {
        self.spawn_rate
    }

    /// Returns the validated Locust duration expression.
    #[must_use]
    pub fn run_time(&self) -> &str {
        &self.run_time
    }
}

/// Prepared Docker Compose Locust invocation and its host report directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocustCommand {
    command: CommandSpec,
    report_dir: PathBuf,
}

impl LocustCommand {
    /// Builds a mode-specific Locust invocation and creates its report directory.
    ///
    /// The caller supplies an already-generated bearer token. This boundary
    /// never mints credentials or writes them to disk.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty token, a missing dataplane server ID, an
    /// invalid request timeout, or when the report directory cannot be created.
    pub fn new(
        config: &AppConfig,
        mode: StackMode,
        settings: &LoadSettings,
        bearer_token: &str,
        server_id: Option<&str>,
    ) -> Result<Self> {
        if bearer_token.trim().is_empty() {
            bail!("Locust bearer token must not be empty");
        }
        if mode == StackMode::Dataplane && server_id.is_none_or(|value| value.trim().is_empty()) {
            bail!("dataplane Locust server ID must not be empty");
        }
        let request_timeout = request_timeout_seconds(config)?;

        let mode_name = match mode {
            StackMode::Controlplane => "controlplane",
            StackMode::Dataplane => "dataplane",
        };
        let report_dir = config
            .integration_dir()
            .join("reports/load")
            .join(mode_name)
            .join("locust");
        fs::create_dir_all(&report_dir).with_context(|| {
            format!("failed to create Locust report directory {:?}", report_dir)
        })?;

        let volume = volume_argument(&report_dir);
        let project = match mode {
            StackMode::Dataplane => ComposeProject::dataplane(
                config.root(),
                config.controlplane_dir(),
                config.integration_project.value.clone(),
                !config.dataplane_ref.value.is_empty(),
            ),
            StackMode::Controlplane => ComposeProject::controlplane(
                config.root(),
                config.controlplane_dir(),
                config.controlplane_project.value.clone(),
                controlplane_sso_enabled(config),
            ),
        };

        let command = match mode {
            StackMode::Dataplane => project.command([
                OsString::from("--profile"),
                OsString::from("testing"),
                OsString::from("run"),
                OsString::from("--rm"),
                OsString::from("--no-deps"),
                OsString::from("--volume"),
                volume,
                OsString::from("locust"),
            ]),
            StackMode::Controlplane => {
                let adapter_volume = adapter_volume_argument(config.root());
                let arguments = vec![
                    OsString::from("run"),
                    OsString::from("--rm"),
                    OsString::from("--no-deps"),
                    OsString::from("--volume"),
                    volume,
                    OsString::from("--volume"),
                    adapter_volume,
                    OsString::from("-e"),
                    OsString::from("MCPGATEWAY_BEARER_TOKEN"),
                    OsString::from("-e"),
                    OsString::from("MCP_STACK_MODE"),
                    OsString::from("-e"),
                    OsString::from(REQUEST_TIMEOUT_ENV),
                    OsString::from("-e"),
                    OsString::from("MCP_PROTOCOL_VERSION"),
                    OsString::from("-e"),
                    OsString::from("MCP_TOOL_NAMES"),
                    OsString::from("--entrypoint"),
                    OsString::from("locust"),
                    OsString::from("locust"),
                    OsString::from("-f"),
                    OsString::from(LOCUST_ADAPTER_CONTAINER_PATH),
                    OsString::from("--host=http://nginx:80"),
                    OsString::from(format!("--users={}", settings.users.get())),
                    OsString::from(format!("--spawn-rate={}", settings.spawn_rate)),
                    OsString::from(format!("--run-time={}", settings.run_time)),
                    OsString::from("--headless"),
                    OsString::from("--html=/mnt/reports/locust_report.html"),
                    OsString::from("--csv=/mnt/reports/locust"),
                    OsString::from("--only-summary"),
                ];
                project.command(arguments)
            }
        };

        let mut command = command
            .env("CF_INTEGRATION_ROOT", config.root().as_os_str())
            .env("CF_INTEGRATION_DIR", config.integration_dir().as_os_str())
            .env("MCP_STACK_MODE", mode_name)
            .env("MCPGATEWAY_BEARER_TOKEN", bearer_token)
            .env("LOCUST_LOCUSTFILE", LOCUST_ADAPTER_NAME)
            .env("LOCUST_MODE", "headless")
            .env("LOCUST_USERS", settings.users.get().to_string())
            .env("LOCUST_SPAWN_RATE", settings.spawn_rate.to_string())
            .env("LOCUST_RUN_TIME", settings.run_time.as_str())
            .env(REQUEST_TIMEOUT_ENV, request_timeout.to_string());
        match mode {
            StackMode::Dataplane => {
                command = command.env("MCP_SERVER_ID", server_id.unwrap_or_default());
            }
            StackMode::Controlplane => {
                let protocol_version =
                    configured_text(config, "MCP_PROTOCOL_VERSION").unwrap_or(PROTOCOL_VERSION);
                let tool_names = configured_text(config, "MCP_TOOL_NAMES").unwrap_or("");
                command = command
                    .env("MCP_PROTOCOL_VERSION", protocol_version)
                    .env("MCP_TOOL_NAMES", tool_names);
            }
        }

        Ok(Self {
            command,
            report_dir,
        })
    }

    /// Returns the child-process specification.
    pub fn command(&self) -> &CommandSpec {
        &self.command
    }

    /// Returns the host directory receiving HTML and CSV reports.
    #[must_use]
    pub fn report_dir(&self) -> &Path {
        &self.report_dir
    }
}

/// Inspects every regular Locust artifact and removes all files containing the bearer token.
///
/// # Errors
///
/// Returns an error after attempting all removals when an artifact cannot be inspected,
/// a tainted artifact cannot be removed, or at least one credential-bearing artifact was
/// removed.
pub fn audit_reports(report_dir: &Path, bearer_token: &str) -> Result<()> {
    let mut files = Vec::new();
    let mut first_inspection_error = None;
    collect_report_files(report_dir, &mut files, &mut first_inspection_error);
    let mut tainted = Vec::new();
    for path in files {
        match fs::read(&path) {
            Ok(contents) if contains_bytes(&contents, bearer_token.as_bytes()) => {
                tainted.push(path);
            }
            Ok(_) => {}
            Err(error) if first_inspection_error.is_none() => {
                first_inspection_error = Some((path, error));
            }
            Err(_) => {}
        }
    }

    let tainted_count = tainted.len();
    let mut first_cleanup_error = None;
    for path in tainted {
        if let Err(error) = fs::remove_file(&path)
            && first_cleanup_error.is_none()
        {
            first_cleanup_error = Some((path, error));
        }
    }
    if let Some((path, error)) = first_cleanup_error {
        return Err(error)
            .with_context(|| format!("failed to remove tainted Locust report {path:?}"));
    }
    if tainted_count > 0 {
        bail!(
            "removed {tainted_count} Locust report artifact(s) because they contained a bearer credential"
        );
    }
    if let Some((path, error)) = first_inspection_error {
        return Err(error).with_context(|| format!("failed to inspect Locust report {path:?}"));
    }
    Ok(())
}

fn collect_report_files(
    directory: &Path,
    files: &mut Vec<PathBuf>,
    first_error: &mut Option<(PathBuf, io::Error)>,
) {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) => {
            if first_error.is_none() {
                *first_error = Some((directory.to_path_buf(), error));
            }
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                if first_error.is_none() {
                    *first_error = Some((directory.to_path_buf(), error));
                }
                continue;
            }
        };
        let path = entry.path();
        match entry.file_type() {
            Ok(file_type) if file_type.is_dir() => {
                collect_report_files(&path, files, first_error);
            }
            Ok(file_type) if file_type.is_file() => files.push(path),
            Ok(_) => {}
            Err(error) if first_error.is_none() => {
                *first_error = Some((path, error));
            }
            Err(_) => {}
        }
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn selected_value<'a>(
    configured: &'a SourcedValue,
    smoke: bool,
    smoke_default: &'static str,
) -> &'a OsStr {
    if smoke && configured.origin != ValueOrigin::Process {
        OsStr::new(smoke_default)
    } else {
        &configured.value
    }
}

fn parse_users(value: &OsStr) -> Result<usize> {
    let value = value.to_str().context("LOCUST_USERS must be valid UTF-8")?;
    let users = value
        .parse::<usize>()
        .context("LOCUST_USERS must be an integer greater than zero")?;
    if users == 0 {
        bail!("LOCUST_USERS must be an integer greater than zero");
    }
    Ok(users)
}

fn parse_spawn_rate(value: &OsStr) -> Result<f64> {
    let value = value
        .to_str()
        .context("LOCUST_SPAWN_RATE must be valid UTF-8")?;
    let spawn_rate = value
        .parse::<f64>()
        .context("LOCUST_SPAWN_RATE must be a finite number greater than zero")?;
    if !spawn_rate.is_finite() || spawn_rate <= 0.0 {
        bail!("LOCUST_SPAWN_RATE must be a finite number greater than zero");
    }
    Ok(spawn_rate)
}

fn validate_locust_run_time(value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    let mut position = 0;
    if bytes.is_empty() {
        bail!(RUN_TIME_ERROR);
    }
    let mut previous_unit = None;
    while position < bytes.len() {
        let number_start = position;
        while position < bytes.len() && bytes[position].is_ascii_digit() {
            position += 1;
        }
        if number_start == position
            || value[number_start..position]
                .parse::<u64>()
                .ok()
                .is_none_or(|amount| amount == 0)
        {
            bail!(RUN_TIME_ERROR);
        }
        let unit = match bytes.get(position) {
            Some(b'h') => 0,
            Some(b'm') => 1,
            Some(b's') => 2,
            _ => {
                bail!(RUN_TIME_ERROR);
            }
        };
        if previous_unit.is_some_and(|previous| unit <= previous) {
            bail!(RUN_TIME_ERROR);
        }
        previous_unit = Some(unit);
        position += 1;
    }
    Ok(())
}

fn validate_grouped_run_time(value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    let mut position = 0;
    if bytes.is_empty() {
        bail!("LOCUST_RUN_TIME must be one or more positive integer+unit groups");
    }
    while position < bytes.len() {
        let number_start = position;
        while position < bytes.len() && bytes[position].is_ascii_digit() {
            position += 1;
        }
        if number_start == position
            || value[number_start..position]
                .parse::<u64>()
                .ok()
                .is_none_or(|amount| amount == 0)
        {
            bail!("LOCUST_RUN_TIME must be one or more positive integer+unit groups");
        }
        if bytes[position..].starts_with(b"ms") {
            position += 2;
        } else if matches!(bytes.get(position), Some(b's' | b'm' | b'h' | b'd')) {
            position += 1;
        } else {
            bail!("LOCUST_RUN_TIME must be one or more positive integer+unit groups");
        }
    }
    Ok(())
}

fn configured_text<'a>(config: &'a AppConfig, key: &str) -> Option<&'a str> {
    config
        .environment()
        .get(OsStr::new(key))
        .and_then(|value| value.value.to_str())
}

fn request_timeout_seconds(config: &AppConfig) -> Result<f64> {
    let value = config
        .environment()
        .get(OsStr::new(REQUEST_TIMEOUT_ENV))
        .map_or(OsStr::new(REQUEST_TIMEOUT_DEFAULT_SECONDS), |value| {
            value.value.as_os_str()
        });
    let timeout = value
        .to_str()
        .context(REQUEST_TIMEOUT_ERROR)?
        .parse::<f64>()
        .context(REQUEST_TIMEOUT_ERROR)?;
    if !timeout.is_finite() || timeout <= 0.0 {
        bail!(REQUEST_TIMEOUT_ERROR);
    }
    Ok(timeout)
}

fn volume_argument(report_dir: &Path) -> OsString {
    let mut argument = report_dir.as_os_str().to_owned();
    argument.push(":/mnt/reports");
    argument
}

fn adapter_volume_argument(root: &Path) -> OsString {
    let mut argument = root
        .join("scripts")
        .join(LOCUST_ADAPTER_NAME)
        .into_os_string();
    argument.push(":");
    argument.push(LOCUST_ADAPTER_CONTAINER_PATH);
    argument.push(":ro");
    argument
}

fn controlplane_sso_enabled(config: &AppConfig) -> bool {
    config
        .environment()
        .get(OsStr::new("CONTROLPLANE_ENABLE_SSO"))
        .and_then(|value| value.value.to_str())
        .is_some_and(|value| matches!(value, "true" | "1"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_audit_removes_every_tainted_artifact_before_failing() {
        let directory = tempfile::tempdir().expect("temporary report directory");
        let nested = directory.path().join("nested");
        fs::create_dir(&nested).expect("nested report directory should be created");
        let first = directory.path().join("locust.html");
        let second = nested.join("locust_failures.csv");
        let safe = directory.path().join("locust_stats.csv");
        let token = "credential-present-in-every-tainted-report";
        fs::write(&first, format!("html {token}")).expect("first report should be written");
        fs::write(&second, format!("csv {token}")).expect("second report should be written");
        fs::write(&safe, "safe aggregate").expect("safe report should be written");

        let error = audit_reports(directory.path(), token)
            .expect_err("credential-bearing reports must fail closed");

        assert!(error.to_string().contains("removed 2 Locust report"));
        assert!(!first.exists());
        assert!(!second.exists());
        assert!(safe.is_file());
    }
}
