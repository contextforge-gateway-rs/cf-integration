use std::process::ExitCode;

use cf_integration::app::dispatch;
use cf_integration::cli::Cli;
use cf_integration::runtime::RuntimeExecutor;
use cf_integration_platform::config::{AppConfig, Environment};
use cf_integration_platform::process::SystemProcessRunner;
use clap::Parser;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let environment: Environment = std::env::vars_os().collect();
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("failed to locate cf-integration executable: {error}");
            return ExitCode::FAILURE;
        }
    };
    let cwd = match std::env::current_dir() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("failed to determine current directory: {error}");
            return ExitCode::FAILURE;
        }
    };
    let loaded = match AppConfig::load(&environment, &executable, &cwd) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("{error:#}");
            return ExitCode::FAILURE;
        }
    };
    for warning in &loaded.warnings {
        eprintln!("warning: {warning}");
    }

    let effective_environment = loaded
        .config
        .environment()
        .iter()
        .map(|(key, value)| (key.clone(), value.value.clone()))
        .collect::<Environment>();
    let mut runtime = RuntimeExecutor::new(loaded.config, SystemProcessRunner);
    match dispatch(cli, &effective_environment, &mut runtime).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            exit_code(error.exit_code())
        }
    }
}

fn exit_code(code: i32) -> ExitCode {
    u8::try_from(code)
        .map(ExitCode::from)
        .unwrap_or(ExitCode::FAILURE)
}
