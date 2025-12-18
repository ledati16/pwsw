//! Terminal styling utilities
//!
//! Provides consistent color scheme across all CLI commands and TUI using the "Moderate+" approach:
//! - Semantic colors for status (green/yellow/red)
//! - Cyan for headers and technical terms
//! - Bold for important identifiers
//! - Dim for secondary information
//!
//! This module defines a unified color palette used by both CLI output and the TUI,
//! ensuring consistent visual language throughout the application.

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

    // ========================================================================
    // TUI Log Styling
    // ========================================================================

    /// Style for log timestamps (dark gray, dim)
    ///
    /// Use for log line timestamps to keep them subtle and unobtrusive.
    fn log_timestamp(self) -> <Self as Stylize>::Styled
    where
        Self: Sized,
    {
        self.dark_grey()
    }

    /// Style for INFO log level (green)
    ///
    /// Use for INFO level indicator in log lines.
    fn log_level_info(self) -> <Self as Stylize>::Styled
    where
        Self: Sized,
    {
        self.green()
    }

    /// Style for DEBUG log level (cyan)
    ///
    /// Use for DEBUG level indicator in log lines.
    fn log_level_debug(self) -> <Self as Stylize>::Styled
    where
        Self: Sized,
    {
        self.cyan()
    }

    /// Style for TRACE log level (magenta)
    ///
    /// Use for TRACE level indicator in log lines.
    fn log_level_trace(self) -> <Self as Stylize>::Styled
    where
        Self: Sized,
    {
        self.magenta()
    }

    /// Style for WARN log level (yellow bold)
    ///
    /// Use for WARN level indicator in log lines to draw attention.
    fn log_level_warn(self) -> <<Self as Stylize>::Styled as Stylize>::Styled
    where
        Self: Sized,
        <Self as Stylize>::Styled: Stylize,
    {
        self.yellow().bold()
    }

    /// Style for ERROR log level (red bold)
    ///
    /// Use for ERROR level indicator in log lines to draw immediate attention.
    fn log_level_error(self) -> <<Self as Stylize>::Styled as Stylize>::Styled
    where
        Self: Sized,
        <Self as Stylize>::Styled: Stylize,
    {
        self.red().bold()
    }

    /// Style for log keywords (cyan)
    ///
    /// Use for technical markers in logs: `app_id=`, `title=`, `id=`, etc.
    fn log_keyword(self) -> <Self as Stylize>::Styled
    where
        Self: Sized,
    {
        self.cyan()
    }

    /// Style for log events (green bold)
    ///
    /// Use for important event markers: "Rule matched:", "Switching:", etc.
    fn log_event(self) -> <<Self as Stylize>::Styled as Stylize>::Styled
    where
        Self: Sized,
        <Self as Stylize>::Styled: Stylize,
    {
        self.green().bold()
    }

    /// Style for log close events (yellow bold)
    ///
    /// Use for closing/removal events: "Window closed:", "Tracked window closed:", etc.
    fn log_event_close(self) -> <<Self as Stylize>::Styled as Stylize>::Styled
    where
        Self: Sized,
        <Self as Stylize>::Styled: Stylize,
    {
        self.yellow().bold()
    }

    // ========================================================================
    // TUI UI Element Styling
    // ========================================================================

    /// Style for active/live UI borders (green)
    ///
    /// Use for borders of active elements: live log viewer, running daemon status, etc.
    fn ui_border_active(self) -> <Self as Stylize>::Styled
    where
        Self: Sized,
    {
        self.green()
    }

    /// Style for inactive/stopped UI borders (gray)
    ///
    /// Use for borders of inactive elements: stopped daemon status, disabled features, etc.
    fn ui_border_inactive(self) -> <Self as Stylize>::Styled
    where
        Self: Sized,
    {
        self.grey()
    }

    /// Style for selected/focused UI elements (cyan bold on dark gray background)
    ///
    /// Use for currently selected items in lists, menus, etc.
    /// Returns a styled value - use with ratatui's `Style::default()` wrapper.
    fn ui_selected(self) -> <<Self as Stylize>::Styled as Stylize>::Styled
    where
        Self: Sized,
        <Self as Stylize>::Styled: Stylize,
    {
        self.cyan().bold()
    }

    /// Style for UI element highlights (cyan)
    ///
    /// Use for highlighted UI elements: sink names, rule descriptions, card borders, etc.
    fn ui_highlight(self) -> <Self as Stylize>::Styled
    where
        Self: Sized,
    {
        self.cyan()
    }

    /// Style for statistics/counts (yellow)
    ///
    /// Use for numeric stats and counts in cards: window count, rule count, etc.
    fn ui_stat(self) -> <Self as Stylize>::Styled
    where
        Self: Sized,
    {
        self.yellow()
    }
}

// Implement for all types that implement Stylize (String, &str, etc.)
impl<T: Stylize> PwswStyle for T {}

// ============================================================================
// Ratatui TUI Color Helpers
// ============================================================================
//
// These functions provide semantic color values for use with ratatui's Style API.
// They mirror the semantic intent of the PwswStyle trait methods above but return
// raw Color values instead of styled strings.

/// Semantic color palette for TUI use with ratatui
pub mod colors {
    use ratatui::style::Color;

    // Log styling colors

    /// Color for log timestamps (dark gray)
    pub const LOG_TIMESTAMP: Color = Color::DarkGray;

    /// Color for INFO log level (green)
    pub const LOG_LEVEL_INFO: Color = Color::Green;

    /// Color for DEBUG log level (cyan)
    pub const LOG_LEVEL_DEBUG: Color = Color::Cyan;

    /// Color for TRACE log level (magenta)
    pub const LOG_LEVEL_TRACE: Color = Color::Magenta;

    /// Color for WARN log level (yellow, use with bold modifier)
    pub const LOG_LEVEL_WARN: Color = Color::Yellow;

    /// Color for ERROR log level (red, use with bold modifier)
    pub const LOG_LEVEL_ERROR: Color = Color::Red;

    /// Color for log keywords (`app_id=`, `title=`, etc.)
    pub const LOG_KEYWORD: Color = Color::Cyan;

    /// Color for log events ("Rule matched:", "Switching:", use with bold)
    pub const LOG_EVENT: Color = Color::Green;

    /// Color for log close events ("Window closed:", use with bold)
    pub const LOG_EVENT_CLOSE: Color = Color::Yellow;

    /// Color for log message text (default readable)
    pub const LOG_MESSAGE: Color = Color::White;

    // UI element colors

    /// Color for active/live UI borders (green)
    pub const UI_BORDER_ACTIVE: Color = Color::Green;

    /// Color for inactive/stopped UI borders (gray)
    pub const UI_BORDER_INACTIVE: Color = Color::Gray;

    /// Color for selected UI elements (cyan, use with bold)
    pub const UI_SELECTED: Color = Color::Cyan;

    /// Background color for selected UI elements (dark gray)
    pub const UI_SELECTED_BG: Color = Color::DarkGray;

    /// Color for UI highlights (cyan)
    pub const UI_HIGHLIGHT: Color = Color::Cyan;

    /// Color for statistics/counts (yellow, use with bold)
    pub const UI_STAT: Color = Color::Yellow;

    /// Color for success states (green)
    pub const UI_SUCCESS: Color = Color::Green;

    /// Color for error states (red)
    pub const UI_ERROR: Color = Color::Red;

    /// Color for warning states (yellow)
    pub const UI_WARNING: Color = Color::Yellow;

    /// Color for secondary/dimmed text (gray)
    pub const UI_SECONDARY: Color = Color::Gray;

    /// Color for normal UI text (white)
    pub const UI_TEXT: Color = Color::White;
}
