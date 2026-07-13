//! Official conformance, gateway compliance, coverage, and report primitives.

pub mod conformance;
pub mod conformance_fixture;
pub mod coverage;
pub mod gateway_compliance;
pub mod profile;
pub mod suite;

pub use profile::{
    DEFAULT_MCP_SPEC_VERSION, LEGACY_MCP_SPEC_VERSION, OFFICIAL_CONFORMANCE_PACKAGE,
    OFFICIAL_CONFORMANCE_REPOSITORY, OFFICIAL_CONFORMANCE_REVISION, STABLE_MCP_SPEC_VERSION,
};
pub use suite::ConformanceSuite;
