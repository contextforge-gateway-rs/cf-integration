//! Shared support for the `cf-integration` executable.

pub mod app;
pub mod cli;
pub mod error;
mod output;
pub mod runtime;
pub mod token;

pub use output::OutputStyle;
