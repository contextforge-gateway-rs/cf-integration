//! Environment loading and repository path resolution.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

const ROOT_OVERRIDE: &str = "CF_INTEGRATION_ROOT";
const COMPOSE_FILE: &str = "docker/docker-compose.cf-integration.yaml";
const REDACTED: &str = "<redacted>";

/// Environment values supplied without mutating the process environment.
pub type Environment = HashMap<OsString, OsString>;

/// Source used for a loaded configuration value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueOrigin {
    /// Value supplied by the process environment.
    Process,
    /// Value loaded from the repository `.env` file.
    Dotenv,
    /// Value supplied by a configuration default.
    Default,
}

/// Environment value paired with its source.
#[derive(Clone, PartialEq, Eq)]
pub struct SourcedValue {
    /// Raw environment value.
    pub value: OsString,
    /// Source that supplied the value.
    pub origin: ValueOrigin,
}

/// Environment values loaded from the process and optional `.env` file.
#[derive(Clone, PartialEq, Eq)]
pub struct LoadedEnvironment {
    values: HashMap<OsString, SourcedValue>,
    warnings: Vec<String>,
}

/// Resolved container image and whether the process explicitly overrode it.
#[derive(Clone, PartialEq, Eq)]
pub struct ImageSetting {
    resolved: OsString,
    explicitly_set: bool,
}

impl ImageSetting {
    /// Returns the image selected after applying shell-compatible fallbacks.
    #[must_use]
    pub fn resolved(&self) -> &OsStr {
        &self.resolved
    }

    /// Returns whether the process supplied the image override, including empty values.
    #[must_use]
    pub fn is_explicitly_set(&self) -> bool {
        self.explicitly_set
    }
}

/// Derived configuration used by integration commands.
#[derive(Clone)]
#[allow(dead_code)] // Retained for same-crate command modules added in later migration tasks.
pub struct AppConfig {
    root: PathBuf,
    integration_dir: SourcedValue,
    controlplane_dir: SourcedValue,
    pub(crate) controlplane_repo: SourcedValue,
    pub(crate) controlplane_ref: SourcedValue,
    dataplane_dir: SourcedValue,
    pub(crate) dataplane_repo: SourcedValue,
    pub(crate) dataplane_ref: SourcedValue,
    pub(crate) integration_project: SourcedValue,
    pub(crate) controlplane_project: SourcedValue,
    pub(crate) jwt_secret_key: SourcedValue,
    pub(crate) jwt_subject: SourcedValue,
    controlplane_image: ImageSetting,
    dataplane_image: ImageSetting,
    pub(crate) dataplane_platform: SourcedValue,
    pub(crate) compose_build: SourcedValue,
    pub(crate) fast_time_server_id: SourcedValue,
    pub(crate) fast_time_expected_image: SourcedValue,
    pub(crate) base_url: SourcedValue,
    pub(crate) platform_admin_email: SourcedValue,
    pub(crate) key_file_password: SourcedValue,
    pub(crate) locust_users: SourcedValue,
    pub(crate) locust_spawn_rate: SourcedValue,
    pub(crate) locust_run_time: SourcedValue,
    environment: LoadedEnvironment,
}

/// Configuration plus non-fatal environment loading warnings.
#[derive(Debug, Clone)]
pub struct ConfigLoad {
    /// Fully derived application configuration.
    pub config: AppConfig,
    /// Non-fatal `.env` parsing warnings.
    pub warnings: Vec<String>,
}

impl fmt::Debug for SourcedValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourcedValue")
            .field("value", &REDACTED)
            .field("origin", &self.origin)
            .finish()
    }
}

impl fmt::Debug for LoadedEnvironment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoadedEnvironment")
            .field("values", &self.values)
            .field("warnings", &self.warnings)
            .finish()
    }
}

impl fmt::Debug for ImageSetting {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImageSetting")
            .field("resolved", &REDACTED)
            .field("explicitly_set", &self.explicitly_set)
            .finish()
    }
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppConfig")
            .field("root", &REDACTED)
            .field("controlplane_image", &self.controlplane_image)
            .field("dataplane_image", &self.dataplane_image)
            .field("environment", &self.environment)
            .finish_non_exhaustive()
    }
}

impl LoadedEnvironment {
    /// Returns the loaded value for `key`.
    #[must_use]
    pub fn get(&self, key: &OsStr) -> Option<&SourcedValue> {
        self.values.get(key)
    }

    /// Returns non-fatal `.env` parsing warnings.
    #[must_use]
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Iterates loaded values and their origins in unspecified order.
    pub fn iter(&self) -> impl Iterator<Item = (&OsString, &SourcedValue)> {
        self.values.iter()
    }
}

impl AppConfig {
    /// Loads the environment and derives configuration without global mutation.
    ///
    /// # Errors
    ///
    /// Returns an error when the repository root cannot be resolved or its
    /// existing `.env` file cannot be read.
    pub fn load(process: &Environment, executable: &Path, cwd: &Path) -> Result<ConfigLoad> {
        let root = resolve_repository_root(process, executable, cwd)?;
        let environment = load_environment(&root, process)?;

        let integration_dir = resolved_path(
            &root,
            shell_value(
                &environment,
                "CF_INTEGRATION_DIR",
                root.join(".integration").into_os_string(),
            ),
        );
        let controlplane_dir = resolved_path(
            &root,
            shell_value(
                &environment,
                "CF_CONTROLPLANE_DIR",
                Path::new(&integration_dir.value)
                    .join("mcp-context-forge")
                    .into_os_string(),
            ),
        );
        let dataplane_dir = resolved_path(
            &root,
            shell_value(
                &environment,
                "CF_DATAPLANE_DIR",
                Path::new(&integration_dir.value)
                    .join("contextforge-gateway-rs")
                    .into_os_string(),
            ),
        );
        let controlplane_repo = shell_value(
            &environment,
            "CF_CONTROLPLANE_REPO",
            OsString::from("https://github.com/IBM/mcp-context-forge.git"),
        );
        let controlplane_ref =
            shell_value(&environment, "CF_CONTROLPLANE_REF", OsString::from("main"));
        let dataplane_repo = shell_value(
            &environment,
            "CF_DATAPLANE_REPO",
            OsString::from(
                "https://github.com/contextforge-gateway-rs/contextforge-gateway-rs.git",
            ),
        );
        let dataplane_ref = shell_value(&environment, "CF_DATAPLANE_REF", OsString::new());
        let integration_project = shell_value(
            &environment,
            "CF_INTEGRATION_PROJECT",
            OsString::from("cf-integration"),
        );
        let controlplane_project = shell_value(
            &environment,
            "CF_CONTROLPLANE_PROJECT",
            OsString::from("cf-controlplane-only"),
        );
        let jwt_secret_key = shell_value(
            &environment,
            "JWT_SECRET_KEY",
            OsString::from("my-test-key-but-now-longer-than-32-bytes"),
        );
        let jwt_subject = shell_value(
            &environment,
            "MCP_JWT_SUBJECT",
            OsString::from("admin@example.com"),
        );
        let controlplane_image = controlplane_image(&environment);
        let dataplane_image = dataplane_image(&environment, &dataplane_ref);
        let dataplane_platform = shell_value(
            &environment,
            "CF_DATAPLANE_PLATFORM",
            OsString::from("auto"),
        );
        let compose_build = shell_value(&environment, "CF_COMPOSE_BUILD", OsString::from("auto"));
        let fast_time_server_id = shell_value(
            &environment,
            "CF_FAST_TIME_SERVER_ID",
            OsString::from("9779b6698cbd4b4995ee04a4fab38737"),
        );
        let fast_time_expected_image = first_nonempty(&environment, "CF_FAST_TIME_EXPECTED_IMAGE")
            .or_else(|| first_nonempty(&environment, "FAST_TIME_IMAGE"))
            .cloned()
            .unwrap_or_else(|| default_value("ghcr.io/ibm/cfex-mcp-fast-time-server:latest"));
        let base_url = base_url(&environment);
        let platform_admin_email = first_nonempty(&environment, "PLATFORM_ADMIN_EMAIL")
            .cloned()
            .unwrap_or_else(|| jwt_subject.clone());
        let key_file_password = shell_value(&environment, "KEY_FILE_PASSWORD", OsString::new());
        let locust_users = present_value(&environment, "LOCUST_USERS", "100");
        let locust_spawn_rate = present_value(&environment, "LOCUST_SPAWN_RATE", "10");
        let locust_run_time = present_value(&environment, "LOCUST_RUN_TIME", "5m");
        let warnings = environment.warnings.clone();

        Ok(ConfigLoad {
            config: Self {
                root,
                integration_dir,
                controlplane_dir,
                controlplane_repo,
                controlplane_ref,
                dataplane_dir,
                dataplane_repo,
                dataplane_ref,
                integration_project,
                controlplane_project,
                jwt_secret_key,
                jwt_subject,
                controlplane_image,
                dataplane_image,
                dataplane_platform,
                compose_build,
                fast_time_server_id,
                fast_time_expected_image,
                base_url,
                platform_admin_email,
                key_file_password,
                locust_users,
                locust_spawn_rate,
                locust_run_time,
                environment,
            },
            warnings,
        })
    }

    /// Returns the resolved integration repository root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the resolved integration runtime directory.
    #[must_use]
    pub fn integration_dir(&self) -> &Path {
        Path::new(&self.integration_dir.value)
    }

    /// Returns the resolved control-plane checkout directory.
    #[must_use]
    pub fn controlplane_dir(&self) -> &Path {
        Path::new(&self.controlplane_dir.value)
    }

    /// Returns the resolved dataplane checkout directory.
    #[must_use]
    pub fn dataplane_dir(&self) -> &Path {
        Path::new(&self.dataplane_dir.value)
    }

    /// Returns the configured control-plane repository.
    #[must_use]
    pub fn controlplane_repo(&self) -> &SourcedValue {
        &self.controlplane_repo
    }

    /// Returns the configured control-plane revision.
    #[must_use]
    pub fn controlplane_ref(&self) -> &SourcedValue {
        &self.controlplane_ref
    }

    /// Returns the configured dataplane repository.
    #[must_use]
    pub fn dataplane_repo(&self) -> &SourcedValue {
        &self.dataplane_repo
    }

    /// Returns the configured dataplane revision.
    #[must_use]
    pub fn dataplane_ref(&self) -> &SourcedValue {
        &self.dataplane_ref
    }

    /// Returns the integration Compose project name.
    #[must_use]
    pub fn integration_project(&self) -> &SourcedValue {
        &self.integration_project
    }

    /// Returns the control-plane Compose project name.
    #[must_use]
    pub fn controlplane_project(&self) -> &SourcedValue {
        &self.controlplane_project
    }

    /// Returns the JWT signing secret setting.
    #[must_use]
    pub fn jwt_secret_key(&self) -> &SourcedValue {
        &self.jwt_secret_key
    }

    /// Returns the JWT subject setting.
    #[must_use]
    pub fn jwt_subject(&self) -> &SourcedValue {
        &self.jwt_subject
    }

    /// Returns the resolved control-plane image setting.
    #[must_use]
    pub fn controlplane_image(&self) -> &ImageSetting {
        &self.controlplane_image
    }

    /// Returns the resolved dataplane image setting.
    #[must_use]
    pub fn dataplane_image(&self) -> &ImageSetting {
        &self.dataplane_image
    }

    /// Returns the configured dataplane container platform.
    #[must_use]
    pub fn dataplane_platform(&self) -> &SourcedValue {
        &self.dataplane_platform
    }

    /// Returns the Compose build-mode setting.
    #[must_use]
    pub fn compose_build(&self) -> &SourcedValue {
        &self.compose_build
    }

    /// Returns the Fast Time server identifier setting.
    #[must_use]
    pub fn fast_time_server_id(&self) -> &SourcedValue {
        &self.fast_time_server_id
    }

    /// Returns the expected Fast Time image setting.
    #[must_use]
    pub fn fast_time_expected_image(&self) -> &SourcedValue {
        &self.fast_time_expected_image
    }

    /// Returns the public integration base URL setting.
    #[must_use]
    pub fn base_url(&self) -> &SourcedValue {
        &self.base_url
    }

    /// Returns the platform administrator email setting.
    #[must_use]
    pub fn platform_admin_email(&self) -> &SourcedValue {
        &self.platform_admin_email
    }

    /// Returns the private-key password setting.
    #[must_use]
    pub fn key_file_password(&self) -> &SourcedValue {
        &self.key_file_password
    }

    /// Returns the Locust user-count setting.
    #[must_use]
    pub fn locust_users(&self) -> &SourcedValue {
        &self.locust_users
    }

    /// Returns the Locust spawn-rate setting.
    #[must_use]
    pub fn locust_spawn_rate(&self) -> &SourcedValue {
        &self.locust_spawn_rate
    }

    /// Returns the Locust run-time setting.
    #[must_use]
    pub fn locust_run_time(&self) -> &SourcedValue {
        &self.locust_run_time
    }

    /// Returns the environment loaded before deriving fallback values.
    #[must_use]
    pub fn environment(&self) -> &LoadedEnvironment {
        &self.environment
    }
}

fn shell_value(environment: &LoadedEnvironment, key: &str, default: OsString) -> SourcedValue {
    first_nonempty(environment, key)
        .cloned()
        .unwrap_or(SourcedValue {
            value: default,
            origin: ValueOrigin::Default,
        })
}

fn present_value(environment: &LoadedEnvironment, key: &str, default: &str) -> SourcedValue {
    environment
        .get(OsStr::new(key))
        .cloned()
        .unwrap_or_else(|| default_value(default))
}

fn first_nonempty<'a>(environment: &'a LoadedEnvironment, key: &str) -> Option<&'a SourcedValue> {
    environment
        .get(OsStr::new(key))
        .filter(|sourced| !sourced.value.is_empty())
}

fn default_value(value: &str) -> SourcedValue {
    SourcedValue {
        value: OsString::from(value),
        origin: ValueOrigin::Default,
    }
}

fn resolved_path(root: &Path, mut value: SourcedValue) -> SourcedValue {
    value.value = absolute_path(root, &value.value).into_os_string();
    value
}

fn is_configured_value(environment: &LoadedEnvironment, key: &str) -> bool {
    environment.get(OsStr::new(key)).is_some()
}

fn prefixed_value(prefix: &str, suffix: &OsStr) -> OsString {
    let mut value = OsString::from(prefix);
    value.push(suffix);
    value
}

fn controlplane_image(environment: &LoadedEnvironment) -> ImageSetting {
    let explicitly_set = is_configured_value(environment, "CF_CONTROLPLANE_IMAGE")
        || is_configured_value(environment, "IMAGE_LOCAL");
    let resolved = first_nonempty(environment, "CF_CONTROLPLANE_IMAGE")
        .or_else(|| first_nonempty(environment, "IMAGE_LOCAL"))
        .map(|value| value.value.clone())
        .unwrap_or_else(|| {
            let version = shell_value(
                environment,
                "CF_CONTROLPLANE_VERSION",
                OsString::from("latest"),
            );
            prefixed_value("mcpgateway/mcpgateway:", &version.value)
        });

    ImageSetting {
        resolved,
        explicitly_set,
    }
}

fn dataplane_image(environment: &LoadedEnvironment, dataplane_ref: &SourcedValue) -> ImageSetting {
    let explicitly_set = is_configured_value(environment, "CF_DATAPLANE_IMAGE");
    let resolved = if let Some(image) = first_nonempty(environment, "CF_DATAPLANE_IMAGE") {
        image.value.clone()
    } else if !dataplane_ref.value.is_empty() {
        shell_value(
            environment,
            "CF_DATAPLANE_LOCAL_IMAGE",
            OsString::from("contextforge-gateway-rs/contextforge-gateway-rs:local"),
        )
        .value
    } else {
        let version = shell_value(environment, "CF_DATAPLANE_VERSION", OsString::from("0.1.0"));
        prefixed_value(
            "ghcr.io/contextforge-gateway-rs/contextforge-gateway-rs:",
            &version.value,
        )
    };

    ImageSetting {
        resolved,
        explicitly_set,
    }
}

fn base_url(environment: &LoadedEnvironment) -> SourcedValue {
    if let Some(url) = first_nonempty(environment, "MCP_CLI_BASE_URL") {
        return url.clone();
    }

    let port = shell_value(environment, "NGINX_PORT", OsString::from("8080"));
    SourcedValue {
        value: prefixed_value("http://127.0.0.1:", &port.value),
        origin: port.origin,
    }
}

/// Loads process values and supplements missing keys from `root/.env`.
///
/// # Errors
///
/// Returns an error when an existing `.env` file cannot be read.
pub fn load_environment(root: &Path, process: &Environment) -> Result<LoadedEnvironment> {
    let mut values = process
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                SourcedValue {
                    value: value.clone(),
                    origin: ValueOrigin::Process,
                },
            )
        })
        .collect();
    let mut warnings = Vec::new();
    let dotenv_path = root.join(".env");

    let contents = match fs::read_to_string(&dotenv_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(LoadedEnvironment { values, warnings });
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read dotenv file {}", dotenv_path.display()));
        }
    };

    for (index, raw_line) in contents.lines().enumerate() {
        let line_number = index + 1;
        let line = raw_line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let assignment = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = assignment.split_once('=') else {
            warnings.push(format!(
                "invalid .env line {line_number}: expected KEY=value"
            ));
            continue;
        };

        if !is_valid_key(key) {
            warnings.push(format!("invalid .env key on line {line_number}: {key}"));
            continue;
        }

        if values.contains_key(OsStr::new(key)) {
            continue;
        }

        values.insert(
            OsString::from(key),
            SourcedValue {
                value: OsString::from(strip_outer_quotes(value)),
                origin: ValueOrigin::Dotenv,
            },
        );
    }

    Ok(LoadedEnvironment { values, warnings })
}

/// Resolves the integration repository root in documented precedence order.
///
/// # Errors
///
/// Returns an error when none of the candidate paths contains both repository
/// marker files.
pub fn resolve_repository_root(
    process: &Environment,
    executable: &Path,
    cwd: &Path,
) -> Result<PathBuf> {
    resolve_repository_root_with_manifest(
        process,
        executable,
        cwd,
        Path::new(env!("CARGO_MANIFEST_DIR")),
    )
}

fn resolve_repository_root_with_manifest(
    process: &Environment,
    executable: &Path,
    cwd: &Path,
    manifest_root: &Path,
) -> Result<PathBuf> {
    if let Some(raw) = process.get(OsStr::new(ROOT_OVERRIDE))
        && !raw.is_empty()
    {
        let candidate = absolute_path(cwd, raw);
        if is_repository_root(&candidate) {
            return Ok(candidate);
        }
    }

    if let Some(candidate) = executable.ancestors().find(|path| is_repository_root(path)) {
        return Ok(candidate.to_path_buf());
    }

    if let Some(candidate) = cwd.ancestors().find(|path| is_repository_root(path)) {
        return Ok(candidate.to_path_buf());
    }

    if let Some(candidate) = manifest_root
        .ancestors()
        .find(|path| is_repository_root(path))
    {
        return Ok(candidate.to_path_buf());
    }

    bail!(
        "failed to resolve cf-integration repository root: no candidate contains Cargo.toml and {COMPOSE_FILE}"
    )
}

/// Returns `raw` unchanged when absolute, otherwise joined to `root`.
#[must_use]
pub fn absolute_path(root: &Path, raw: &OsStr) -> PathBuf {
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn is_valid_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn strip_outer_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && matches!(
            (bytes.first(), bytes.last()),
            (Some(b'\''), Some(b'\'')) | (Some(b'"'), Some(b'"'))
        )
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn is_repository_root(path: &Path) -> bool {
    path.join("Cargo.toml").is_file() && path.join(COMPOSE_FILE).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(values: &[(&str, &str)]) -> Environment {
        values
            .iter()
            .map(|(key, value)| (OsString::from(key), OsString::from(value)))
            .collect()
    }

    fn repository_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("temporary repository root should be created");
        fs::write(root.path().join("Cargo.toml"), "[package]\n")
            .expect("temporary Cargo manifest should be written");
        fs::create_dir_all(root.path().join("docker"))
            .expect("temporary docker directory should be created");
        fs::write(root.path().join(COMPOSE_FILE), "services: {}\n")
            .expect("temporary Compose file should be written");
        root
    }

    fn load_app_config(root: &Path, process: &Environment) -> ConfigLoad {
        AppConfig::load(process, &root.join("target/debug/cf-integration"), root)
            .expect("application config should load")
    }

    fn assert_sourced(actual: &SourcedValue, expected: &OsStr, origin: ValueOrigin) {
        assert_eq!(actual.value, expected);
        assert_eq!(actual.origin, origin);
    }

    #[test]
    fn repository_root_failure_describes_required_markers() {
        let outside = tempfile::tempdir().expect("temporary directory should be created");
        let executable = outside.path().join("bin/cf-integration");
        let cwd = outside.path().join("work/nested");
        let invalid_manifest_root = outside.path().join("manifest");

        let error = resolve_repository_root_with_manifest(
            &Environment::new(),
            &executable,
            &cwd,
            &invalid_manifest_root,
        )
        .expect_err("invalid candidates should fail root resolution");

        let message = error.to_string();
        assert!(message.contains("Cargo.toml"));
        assert!(message.contains(COMPOSE_FILE));
    }

    #[test]
    fn app_config_uses_documented_defaults() {
        let root = repository_root();

        let config = load_app_config(root.path(), &Environment::new()).config;

        assert_eq!(config.root, root.path());
        assert_sourced(
            &config.integration_dir,
            root.path().join(".integration").as_os_str(),
            ValueOrigin::Default,
        );
        assert_sourced(
            &config.controlplane_dir,
            root.path()
                .join(".integration/mcp-context-forge")
                .as_os_str(),
            ValueOrigin::Default,
        );
        assert_sourced(
            &config.controlplane_repo,
            OsStr::new("https://github.com/IBM/mcp-context-forge.git"),
            ValueOrigin::Default,
        );
        assert_sourced(
            &config.controlplane_ref,
            OsStr::new("main"),
            ValueOrigin::Default,
        );
        assert_sourced(
            &config.dataplane_dir,
            root.path()
                .join(".integration/contextforge-gateway-rs")
                .as_os_str(),
            ValueOrigin::Default,
        );
        assert_sourced(
            &config.dataplane_repo,
            OsStr::new("https://github.com/contextforge-gateway-rs/contextforge-gateway-rs.git"),
            ValueOrigin::Default,
        );
        assert_sourced(&config.dataplane_ref, OsStr::new(""), ValueOrigin::Default);
        assert_sourced(
            &config.integration_project,
            OsStr::new("cf-integration"),
            ValueOrigin::Default,
        );
        assert_sourced(
            &config.controlplane_project,
            OsStr::new("cf-controlplane-only"),
            ValueOrigin::Default,
        );
        assert_sourced(
            &config.jwt_secret_key,
            OsStr::new("my-test-key-but-now-longer-than-32-bytes"),
            ValueOrigin::Default,
        );
        assert_sourced(
            &config.jwt_subject,
            OsStr::new("admin@example.com"),
            ValueOrigin::Default,
        );
        assert_eq!(
            config.controlplane_image.resolved,
            OsStr::new("mcpgateway/mcpgateway:latest")
        );
        assert!(!config.controlplane_image.explicitly_set);
        assert_eq!(
            config.dataplane_image.resolved,
            OsStr::new("ghcr.io/contextforge-gateway-rs/contextforge-gateway-rs:0.1.0")
        );
        assert!(!config.dataplane_image.explicitly_set);
        assert_sourced(
            &config.dataplane_platform,
            OsStr::new("auto"),
            ValueOrigin::Default,
        );
        assert_sourced(
            &config.compose_build,
            OsStr::new("auto"),
            ValueOrigin::Default,
        );
        assert_sourced(
            &config.fast_time_server_id,
            OsStr::new("9779b6698cbd4b4995ee04a4fab38737"),
            ValueOrigin::Default,
        );
        assert_sourced(
            &config.fast_time_expected_image,
            OsStr::new("ghcr.io/ibm/cfex-mcp-fast-time-server:latest"),
            ValueOrigin::Default,
        );
        assert_sourced(
            &config.base_url,
            OsStr::new("http://127.0.0.1:8080"),
            ValueOrigin::Default,
        );
        assert_sourced(
            &config.platform_admin_email,
            OsStr::new("admin@example.com"),
            ValueOrigin::Default,
        );
        assert_sourced(
            &config.key_file_password,
            OsStr::new(""),
            ValueOrigin::Default,
        );
        assert_sourced(
            &config.locust_users,
            OsStr::new("100"),
            ValueOrigin::Default,
        );
        assert_sourced(
            &config.locust_spawn_rate,
            OsStr::new("10"),
            ValueOrigin::Default,
        );
        assert_sourced(
            &config.locust_run_time,
            OsStr::new("5m"),
            ValueOrigin::Default,
        );
    }

    #[test]
    fn app_config_preserves_process_precedence_and_dotenv_origins() {
        let root = repository_root();
        fs::write(
            root.path().join(".env"),
            concat!(
                "CF_CONTROLPLANE_REPO=dotenv-controlplane\n",
                "CF_CONTROLPLANE_REF=dotenv-ref\n",
                "CF_DATAPLANE_REPO=dotenv-dataplane\n",
                "CF_INTEGRATION_PROJECT=dotenv-project\n",
                "CF_CONTROLPLANE_IMAGE=dotenv/image:tag\n",
            ),
        )
        .expect("dotenv should be written");
        let process = environment(&[
            ("CF_CONTROLPLANE_REPO", "process-controlplane"),
            ("CF_CONTROLPLANE_REF", ""),
        ]);

        let config = load_app_config(root.path(), &process).config;

        assert_sourced(
            &config.controlplane_repo,
            OsStr::new("process-controlplane"),
            ValueOrigin::Process,
        );
        assert_sourced(
            &config.controlplane_ref,
            OsStr::new("main"),
            ValueOrigin::Default,
        );
        assert_sourced(
            &config.dataplane_repo,
            OsStr::new("dotenv-dataplane"),
            ValueOrigin::Dotenv,
        );
        assert_sourced(
            &config.integration_project,
            OsStr::new("dotenv-project"),
            ValueOrigin::Dotenv,
        );
        assert_eq!(
            config.controlplane_image.resolved,
            OsStr::new("dotenv/image:tag")
        );
        assert!(config.controlplane_image.explicitly_set);
    }

    #[test]
    fn controlplane_image_uses_primary_then_image_local_then_version_default() {
        let root = repository_root();
        let primary = environment(&[
            ("CF_CONTROLPLANE_IMAGE", "primary/image:tag"),
            ("IMAGE_LOCAL", "legacy/image:tag"),
        ]);
        let image_local = environment(&[("IMAGE_LOCAL", "legacy/image:tag")]);
        let empty_image_local =
            environment(&[("IMAGE_LOCAL", ""), ("CF_CONTROLPLANE_VERSION", "edge")]);

        let primary_config = load_app_config(root.path(), &primary).config;
        let local_config = load_app_config(root.path(), &image_local).config;
        let empty_config = load_app_config(root.path(), &empty_image_local).config;

        assert_eq!(
            primary_config.controlplane_image.resolved,
            OsStr::new("primary/image:tag")
        );
        assert!(primary_config.controlplane_image.explicitly_set);
        assert_eq!(
            local_config.controlplane_image.resolved,
            OsStr::new("legacy/image:tag")
        );
        assert!(local_config.controlplane_image.explicitly_set);
        assert_eq!(
            empty_config.controlplane_image.resolved,
            OsStr::new("mcpgateway/mcpgateway:edge")
        );
        assert!(empty_config.controlplane_image.explicitly_set);
    }

    #[test]
    fn dataplane_image_switches_between_source_and_published_defaults() {
        let root = repository_root();
        let source = environment(&[("CF_DATAPLANE_REF", "feature")]);
        let local = environment(&[
            ("CF_DATAPLANE_REF", "feature"),
            ("CF_DATAPLANE_LOCAL_IMAGE", "local/image:tag"),
        ]);
        let published = environment(&[("CF_DATAPLANE_VERSION", "2.0.0")]);
        let explicit = environment(&[("CF_DATAPLANE_IMAGE", "direct/image:tag")]);

        let source_config = load_app_config(root.path(), &source).config;
        let local_config = load_app_config(root.path(), &local).config;
        let published_config = load_app_config(root.path(), &published).config;
        let explicit_config = load_app_config(root.path(), &explicit).config;

        assert_eq!(
            source_config.dataplane_image.resolved,
            OsStr::new("contextforge-gateway-rs/contextforge-gateway-rs:local")
        );
        assert_eq!(
            local_config.dataplane_image.resolved,
            OsStr::new("local/image:tag")
        );
        assert_eq!(
            published_config.dataplane_image.resolved,
            OsStr::new("ghcr.io/contextforge-gateway-rs/contextforge-gateway-rs:2.0.0")
        );
        assert_eq!(
            explicit_config.dataplane_image.resolved,
            OsStr::new("direct/image:tag")
        );
        assert!(explicit_config.dataplane_image.explicitly_set);
    }

    #[test]
    fn fast_time_image_uses_new_override_then_legacy_override_then_default() {
        let root = repository_root();
        let expected = environment(&[
            ("CF_FAST_TIME_EXPECTED_IMAGE", "expected/image:tag"),
            ("FAST_TIME_IMAGE", "legacy/image:tag"),
        ]);
        let legacy = environment(&[("FAST_TIME_IMAGE", "legacy/image:tag")]);
        let empty = environment(&[("CF_FAST_TIME_EXPECTED_IMAGE", ""), ("FAST_TIME_IMAGE", "")]);

        let expected_config = load_app_config(root.path(), &expected).config;
        let legacy_config = load_app_config(root.path(), &legacy).config;
        let empty_config = load_app_config(root.path(), &empty).config;

        assert_sourced(
            &expected_config.fast_time_expected_image,
            OsStr::new("expected/image:tag"),
            ValueOrigin::Process,
        );
        assert_sourced(
            &legacy_config.fast_time_expected_image,
            OsStr::new("legacy/image:tag"),
            ValueOrigin::Process,
        );
        assert_sourced(
            &empty_config.fast_time_expected_image,
            OsStr::new("ghcr.io/ibm/cfex-mcp-fast-time-server:latest"),
            ValueOrigin::Default,
        );
    }

    #[test]
    fn base_url_uses_direct_value_then_port_and_admin_email_uses_subject() {
        let root = repository_root();
        let direct = environment(&[
            ("MCP_CLI_BASE_URL", "https://example.test"),
            ("NGINX_PORT", "9191"),
        ]);
        let port_and_subject = environment(&[
            ("MCP_CLI_BASE_URL", ""),
            ("NGINX_PORT", "9191"),
            ("MCP_JWT_SUBJECT", "operator@example.test"),
        ]);

        let direct_config = load_app_config(root.path(), &direct).config;
        let fallback_config = load_app_config(root.path(), &port_and_subject).config;

        assert_sourced(
            &direct_config.base_url,
            OsStr::new("https://example.test"),
            ValueOrigin::Process,
        );
        assert_sourced(
            &fallback_config.base_url,
            OsStr::new("http://127.0.0.1:9191"),
            ValueOrigin::Process,
        );
        assert_sourced(
            &fallback_config.platform_admin_email,
            OsStr::new("operator@example.test"),
            ValueOrigin::Process,
        );
    }

    #[test]
    fn locust_values_preserve_process_empty_and_dotenv_origins() {
        let root = repository_root();
        fs::write(root.path().join(".env"), "LOCUST_SPAWN_RATE=25\n")
            .expect("dotenv should be written");
        let process = environment(&[("LOCUST_USERS", "")]);

        let config = load_app_config(root.path(), &process).config;

        assert_sourced(&config.locust_users, OsStr::new(""), ValueOrigin::Process);
        assert_sourced(
            &config.locust_spawn_rate,
            OsStr::new("25"),
            ValueOrigin::Dotenv,
        );
        assert_sourced(
            &config.locust_run_time,
            OsStr::new("5m"),
            ValueOrigin::Default,
        );
        assert_sourced(
            config
                .environment
                .get(OsStr::new("LOCUST_USERS"))
                .expect("raw process value should remain loaded"),
            OsStr::new(""),
            ValueOrigin::Process,
        );
    }
}
