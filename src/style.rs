//! Terminal styling utilities
//!
//! Provides consistent color scheme across all CLI commands using the "Moderate+" approach:
//! - Semantic colors for status (green/yellow/red)
//! - Cyan for headers and technical terms
//! - Bold for important identifiers
//! - Dim for secondary information

use crossterm::style::Stylize;

/// Extension trait for consistent PWSW styling
///
/// This trait extends crossterm's `Stylize` with semantic styling methods
/// that enforce our color scheme. Use these methods instead of direct color
/// calls to ensure consistency across all CLI output.
///
/// # Examples
///
/// ```
/// use crossterm::style::Stylize;
/// use pwsw::style::PwswStyle;
///
/// println!("{}", "Section Header".header());
/// println!("{}", "Success message".success());
/// println!("{}", "/path/to/config".technical());
/// ```
pub trait PwswStyle: Stylize {
    /// Style for section headers (cyan bold)
    ///
    /// Use for main section titles like "ACTIVE SINKS:", "Daemon", etc.
    fn header(self) -> <<Self as Stylize>::Styled as Stylize>::Styled
    where
        Self: Sized,
        <Self as Stylize>::Styled: Stylize,
    {
        self.cyan().bold()
    }

    /// Style for success/active status (green)
    ///
    /// Use for positive states: "Running", "active", success messages, etc.
    fn success(self) -> <Self as Stylize>::Styled
    where
        Self: Sized,
    {
        self.green()
    }

    /// Style for error/missing status (red)
    ///
    /// Use for problems: "not found", "Not running", error messages, etc.
    fn error(self) -> <Self as Stylize>::Styled
    where
        Self: Sized,
    {
        self.red()
    }

    /// Style for warning/partial status (yellow)
    ///
    /// Use for warnings or partial states: "profile switch", warnings, etc.
    fn warning(self) -> <Self as Stylize>::Styled
    where
        Self: Sized,
    {
        self.yellow()
    }

    /// Style for technical terms and identifiers (cyan)
    ///
    /// Use for technical content: regex patterns, counts, paths, etc.
    fn technical(self) -> <Self as Stylize>::Styled
    where
        Self: Sized,
    {
        self.cyan()
    }
}

// Implement for all types that implement Stylize (String, &str, etc.)
impl<T: Stylize> PwswStyle for T {}
