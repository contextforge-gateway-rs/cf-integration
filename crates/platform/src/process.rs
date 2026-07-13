//! Child-process construction and execution.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::OpenOptions;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Command, ExitStatus, Stdio};

use anyhow::Context;

use crate::error::PlatformError;

/// An owned child-process command description.
#[must_use = "a command specification does nothing until a process runner executes it"]
#[derive(Clone, PartialEq, Eq)]
pub struct CommandSpec {
    program: OsString,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
    environment: BTreeMap<OsString, OsString>,
    inherit_environment: bool,
}

impl CommandSpec {
    /// Creates a command specification for `program`.
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            environment: BTreeMap::new(),
            inherit_environment: true,
        }
    }

    /// Appends one argument.
    pub fn arg(mut self, argument: impl Into<OsString>) -> Self {
        self.args.push(argument.into());
        self
    }

    /// Appends multiple arguments.
    pub fn args<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(arguments.into_iter().map(Into::into));
        self
    }

    /// Sets the child working directory.
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Adds or replaces one child environment override.
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.environment.insert(key.into(), value.into());
        self
    }

    /// Prevents the child from inheriting unspecified parent environment values.
    pub fn clear_environment(mut self) -> Self {
        self.inherit_environment = false;
        self
    }

    /// Returns the program name or path.
    #[must_use]
    pub fn program(&self) -> &OsStr {
        &self.program
    }

    /// Returns the ordered argument list.
    #[must_use]
    pub fn arguments(&self) -> &[OsString] {
        &self.args
    }

    /// Returns the configured child working directory.
    #[must_use]
    pub fn working_directory(&self) -> Option<&Path> {
        self.cwd.as_deref()
    }

    /// Returns deterministic child environment overrides.
    #[must_use]
    pub fn environment(&self) -> &BTreeMap<OsString, OsString> {
        &self.environment
    }

    /// Returns whether unspecified parent environment values are inherited.
    #[must_use]
    pub fn inherits_environment(&self) -> bool {
        self.inherit_environment
    }
}

impl fmt::Debug for CommandSpec {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommandSpec")
            .field("program", &self.program)
            .field("arg_count", &self.args.len())
            .field("cwd", &self.cwd)
            .field("inherit_environment", &self.inherit_environment)
            .field(
                "environment_keys",
                &self.environment.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Bytes captured from both child output streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl CapturedOutput {
    /// Creates captured output from owned stream bytes.
    #[must_use]
    pub fn new(stdout: Vec<u8>, stderr: Vec<u8>) -> Self {
        Self { stdout, stderr }
    }

    /// Returns captured standard output bytes.
    #[must_use]
    pub fn stdout(&self) -> &[u8] {
        &self.stdout
    }

    /// Returns captured standard error bytes.
    #[must_use]
    pub fn stderr(&self) -> &[u8] {
        &self.stderr
    }

    /// Splits captured output into owned standard output and error bytes.
    #[must_use]
    pub fn into_parts(self) -> (Vec<u8>, Vec<u8>) {
        (self.stdout, self.stderr)
    }
}

/// Injectable child-process execution boundary.
pub trait ProcessRunner {
    /// Runs with inherited standard output and error.
    fn run(&self, spec: &CommandSpec) -> Result<(), PlatformError>;

    /// Runs without blocking the calling async executor thread.
    fn run_async<'a>(
        &'a self,
        spec: &'a CommandSpec,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + 'a>> {
        Box::pin(async move { self.run(spec) })
    }

    /// Runs asynchronously and returns after cancellation is observed.
    ///
    /// The default is intended for fake runners without owned OS children.
    fn run_async_cancellable<'a>(
        &'a self,
        spec: &'a CommandSpec,
        mut cancellation: tokio::sync::watch::Receiver<bool>,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + 'a>> {
        Box::pin(async move {
            let process = self.run_async(spec);
            tokio::pin!(process);
            tokio::select! {
                result = &mut process => result,
                () = wait_for_cancellation(&mut cancellation) => {
                    Err(cancelled_failure(spec))
                }
            }
        })
    }

    /// Captures standard output while inheriting standard error.
    fn capture_stdout(&self, spec: &CommandSpec) -> Result<Vec<u8>, PlatformError>;

    /// Captures standard output and error separately.
    fn capture_output(&self, spec: &CommandSpec) -> Result<CapturedOutput, PlatformError>;

    /// Appends standard output and error to one log file.
    fn run_to_log(&self, spec: &CommandSpec, log_path: &Path) -> Result<(), PlatformError>;
}

/// Operating-system-backed process runner.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemProcessRunner;

impl ProcessRunner for SystemProcessRunner {
    fn run(&self, spec: &CommandSpec) -> Result<(), PlatformError> {
        let mut command = command(spec);
        command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        let mut child = command
            .spawn()
            .with_context(|| operation_context("spawn", spec))?;
        let status = child
            .wait()
            .with_context(|| operation_context("wait for", spec))?;
        require_success(spec, status)
    }

    fn run_async<'a>(
        &'a self,
        spec: &'a CommandSpec,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + 'a>> {
        Box::pin(async move {
            let mut command = tokio::process::Command::from(command(spec));
            command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
            let mut child = command
                .spawn()
                .with_context(|| operation_context("spawn", spec))?;
            let status = child
                .wait()
                .await
                .with_context(|| operation_context("wait for", spec))?;
            require_success(spec, status)
        })
    }

    fn run_async_cancellable<'a>(
        &'a self,
        spec: &'a CommandSpec,
        mut cancellation: tokio::sync::watch::Receiver<bool>,
    ) -> Pin<Box<dyn Future<Output = Result<(), PlatformError>> + 'a>> {
        Box::pin(async move {
            let mut command = tokio::process::Command::from(command(spec));
            command
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .kill_on_drop(true);
            let mut child = command
                .spawn()
                .with_context(|| operation_context("spawn", spec))?;
            tokio::select! {
                status = child.wait() => {
                    let status = status.with_context(|| operation_context("wait for", spec))?;
                    require_success(spec, status)
                }
                () = wait_for_cancellation(&mut cancellation) => {
                    let _ = child.start_kill();
                    child
                        .wait()
                        .await
                        .with_context(|| operation_context("reap cancelled", spec))?;
                    Err(cancelled_failure(spec))
                }
            }
        })
    }

    fn capture_stdout(&self, spec: &CommandSpec) -> Result<Vec<u8>, PlatformError> {
        let mut command = command(spec);
        command.stdout(Stdio::piped()).stderr(Stdio::inherit());
        let child = command
            .spawn()
            .with_context(|| operation_context("spawn", spec))?;
        let output = child
            .wait_with_output()
            .with_context(|| operation_context("wait for", spec))?;
        require_success(spec, output.status)?;
        Ok(output.stdout)
    }

    fn capture_output(&self, spec: &CommandSpec) -> Result<CapturedOutput, PlatformError> {
        let mut command = command(spec);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let child = command
            .spawn()
            .with_context(|| operation_context("spawn", spec))?;
        let output = child
            .wait_with_output()
            .with_context(|| operation_context("wait for", spec))?;
        require_success(spec, output.status)?;
        Ok(CapturedOutput::new(output.stdout, output.stderr))
    }

    fn run_to_log(&self, spec: &CommandSpec, log_path: &Path) -> Result<(), PlatformError> {
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .with_context(|| log_context("open", log_path, spec))?;
        let stderr_log = log
            .try_clone()
            .with_context(|| log_context("clone handle for", log_path, spec))?;
        let mut command = command(spec);
        command
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(stderr_log));
        let mut child = command
            .spawn()
            .with_context(|| operation_context("spawn", spec))?;
        let status = child
            .wait()
            .with_context(|| operation_context("wait for", spec))?;
        require_success(spec, status)
    }
}

fn command(spec: &CommandSpec) -> Command {
    let mut command = Command::new(spec.program());
    command.args(spec.arguments());
    if !spec.inherits_environment() {
        command.env_clear();
    }
    if let Some(cwd) = spec.working_directory() {
        command.current_dir(cwd);
    }
    command.envs(spec.environment());
    command
}

fn require_success(spec: &CommandSpec, status: ExitStatus) -> Result<(), PlatformError> {
    if status.success() {
        Ok(())
    } else {
        Err(PlatformError::child_exit(spec.program.to_owned(), status))
    }
}

async fn wait_for_cancellation(cancellation: &mut tokio::sync::watch::Receiver<bool>) {
    while !*cancellation.borrow_and_update() {
        if cancellation.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

fn cancelled_failure(spec: &CommandSpec) -> PlatformError {
    PlatformError::from(anyhow::anyhow!(
        "program {:?} cancelled and reaped",
        spec.program()
    ))
}

fn operation_context(operation: &str, spec: &CommandSpec) -> String {
    match spec.working_directory() {
        Some(cwd) => format!(
            "failed to {operation} program {:?} in cwd {cwd:?}",
            spec.program()
        ),
        None => format!(
            "failed to {operation} program {:?} in inherited cwd",
            spec.program()
        ),
    }
}

fn log_context(operation: &str, log_path: &Path, spec: &CommandSpec) -> String {
    match spec.working_directory() {
        Some(cwd) => format!(
            "failed to {operation} log file {log_path:?} for program {:?} in cwd {cwd:?}",
            spec.program()
        ),
        None => format!(
            "failed to {operation} log file {log_path:?} for program {:?} in inherited cwd",
            spec.program()
        ),
    }
}
