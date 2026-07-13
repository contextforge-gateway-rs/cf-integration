//! Official conformance suite selection.

/// Official server scenario selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConformanceSuite {
    /// Stable scenarios, excluding upstream pending scenarios.
    Active,
    /// Every scenario tagged for the selected revision.
    All,
}

impl ConformanceSuite {
    /// Stable argument and report label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::All => "all",
        }
    }
}
