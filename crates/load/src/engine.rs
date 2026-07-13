//! Load-test engine selection.

/// Load-test implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadEngine {
    /// Python Locust adapter.
    Locust,
    /// Native Rust Goose runner.
    Goose,
}
