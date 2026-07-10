//! Load-test configuration and execution boundaries.

mod goose;
mod locust;

pub use goose::{GooseLoadConfig, GooseReportPaths, GooseRunError, GooseRunOutcome};
pub use locust::{LoadSettings, LocustCommand, audit_reports as audit_locust_reports};
