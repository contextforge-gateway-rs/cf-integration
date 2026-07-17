//! Terminal-aware styling for human-readable CLI output.

use std::ffi::OsStr;
use std::io::IsTerminal as _;

const ANSI_RESET: &str = "\x1b[0m";
const ANSI_CYAN: &str = "\x1b[36m";
const ANSI_BOLD_CYAN: &str = "\x1b[1;36m";
const ANSI_GREEN: &str = "\x1b[32m";
const ANSI_BOLD_GREEN: &str = "\x1b[1;32m";
const ANSI_RED: &str = "\x1b[31m";
const ANSI_BOLD_RED: &str = "\x1b[1;31m";
const ANSI_YELLOW: &str = "\x1b[33m";
const ANSI_MAGENTA: &str = "\x1b[35m";
const ANSI_BOLD_MAGENTA: &str = "\x1b[1;35m";

/// Styles human-readable output according to the target terminal and color environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputStyle {
    color: bool,
}

impl OutputStyle {
    /// Resolves styling for standard output.
    #[must_use]
    pub fn stdout() -> Self {
        Self::resolve(std::io::stdout().is_terminal())
    }

    /// Resolves styling for standard error.
    #[must_use]
    pub fn stderr() -> Self {
        Self::resolve(std::io::stderr().is_terminal())
    }

    /// Styles informational text.
    #[must_use]
    pub fn info(self, text: &str) -> String {
        self.ansi(text, ANSI_CYAN)
    }

    /// Styles a prominent informational heading.
    #[must_use]
    pub fn heading(self, text: &str) -> String {
        self.ansi(text, ANSI_BOLD_CYAN)
    }

    /// Styles successful output.
    #[must_use]
    pub fn success(self, text: &str) -> String {
        self.ansi(text, ANSI_GREEN)
    }

    /// Styles a prominent success summary.
    #[must_use]
    pub fn success_heading(self, text: &str) -> String {
        self.ansi(text, ANSI_BOLD_GREEN)
    }

    /// Styles failure output.
    #[must_use]
    pub fn failure(self, text: &str) -> String {
        self.ansi(text, ANSI_RED)
    }

    /// Styles a prominent failure summary.
    #[must_use]
    pub fn failure_heading(self, text: &str) -> String {
        self.ansi(text, ANSI_BOLD_RED)
    }

    /// Styles warning or skipped output.
    #[must_use]
    pub fn warning(self, text: &str) -> String {
        self.ansi(text, ANSI_YELLOW)
    }

    /// Styles output whose result is unknown.
    #[must_use]
    pub fn unknown(self, text: &str) -> String {
        self.ansi(text, ANSI_MAGENTA)
    }

    /// Styles a prominent summary whose result is unknown.
    #[must_use]
    pub fn unknown_heading(self, text: &str) -> String {
        self.ansi(text, ANSI_BOLD_MAGENTA)
    }

    #[cfg(test)]
    pub(crate) const fn plain() -> Self {
        Self { color: false }
    }

    #[cfg(test)]
    pub(crate) const fn colored() -> Self {
        Self { color: true }
    }

    fn resolve(stream_is_terminal: bool) -> Self {
        Self {
            color: resolve_color(
                stream_is_terminal,
                std::env::var_os("NO_COLOR").is_some(),
                std::env::var_os("CARGO_TERM_COLOR").as_deref(),
            ),
        }
    }

    fn ansi(self, text: &str, style: &str) -> String {
        if self.color {
            format!("{style}{text}{ANSI_RESET}")
        } else {
            text.to_owned()
        }
    }
}

fn resolve_color(
    stream_is_terminal: bool,
    no_color: bool,
    cargo_term_color: Option<&OsStr>,
) -> bool {
    if no_color {
        return false;
    }
    match cargo_term_color.and_then(OsStr::to_str) {
        Some("always") => true,
        Some("never") => false,
        _ => stream_is_terminal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_policy_honors_terminal_cargo_and_no_color_settings() {
        assert!(resolve_color(true, false, None));
        assert!(!resolve_color(false, false, None));
        assert!(resolve_color(false, false, Some(OsStr::new("always"))));
        assert!(!resolve_color(true, false, Some(OsStr::new("never"))));
        assert!(!resolve_color(true, true, Some(OsStr::new("always"))));
    }

    #[test]
    fn plain_style_leaves_human_readable_output_unchanged() {
        let style = OutputStyle::plain();

        assert_eq!(style.info("Waiting"), "Waiting");
        assert_eq!(style.success("PASS"), "PASS");
        assert_eq!(style.failure("FAIL"), "FAIL");
        assert_eq!(style.warning("warning"), "warning");
        assert_eq!(style.unknown("UNKNOWN"), "UNKNOWN");
    }

    #[test]
    fn colored_style_uses_consistent_semantic_colors() {
        let style = OutputStyle::colored();

        assert_eq!(style.heading("Lane"), "\x1b[1;36mLane\x1b[0m");
        assert_eq!(style.success("PASS"), "\x1b[32mPASS\x1b[0m");
        assert_eq!(style.failure("FAIL"), "\x1b[31mFAIL\x1b[0m");
        assert_eq!(style.warning("SKIP"), "\x1b[33mSKIP\x1b[0m");
        assert_eq!(style.unknown("UNKNOWN"), "\x1b[35mUNKNOWN\x1b[0m");
    }
}
