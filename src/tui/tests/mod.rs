//! TUI subsystem integration tests
//!
//! This directory exists for TUI integration tests that need access to `pub(crate)` internals.
//!
//! ## Why not top-level `tests/` directory?
//!
//! Tests in `tests/` are compiled as separate crates and can ONLY access public (`pub`) items.
//! The TUI subsystem exposes several internal APIs marked `pub(crate)` that are essential for
//! testing but should not be part of the public API:
//!
//! - `simulate_key_event()` - Test helper for simulating keyboard input
//! - `BgCommand`, `DaemonAction` - Internal async message types
//!
//! ## When to use this directory vs `tests/`:
//!
//! **Use `src/tui/tests/` when:**
//! - Testing TUI subsystem integration that requires `pub(crate)` access
//! - Testing internal async message passing or state management
//! - Testing TUI components that aren't exposed in the public API
//!
//! **Use top-level `tests/` when:**
//! - Testing public API behavior (CLI commands, config loading)
//! - Testing cross-module integration through public interfaces only
//! - Writing smoke tests or end-to-end tests
//!
//! This follows a valid Rust pattern for subsystem testing. See:
//! <https://doc.rust-lang.org/book/ch11-03-test-organization.html#integration-tests>

mod app_tests;
mod keymap_tests;
mod render_tests;
mod worker_tests;

#[cfg(test)]
mod daemon_log_tests {
    use pretty_assertions::assert_eq;

    use crate::config::Config;
    use crate::tui::app::{App, AppUpdate};

    fn app() -> App {
        App::with_config(Config::new(
            crate::config::Settings::default(),
            vec![],
            vec![],
        ))
    }

    #[test]
    fn test_daemon_logs_bounded_on_large_burst() {
        let mut app = app();
        let burst: Vec<String> = (0..1000).map(|i| format!("line {i}")).collect();
        app.handle_update(AppUpdate::DaemonLogs(burst));

        assert_eq!(app.daemon_log_lines.len(), 500);
        assert_eq!(app.daemon_log_lines[0], "line 500");
        assert_eq!(app.daemon_log_lines[499], "line 999");
    }

    #[test]
    fn test_daemon_logs_incremental_growth() {
        let mut app = app();
        for i in 0..300 {
            app.handle_update(AppUpdate::DaemonLogs(vec![format!("line {i}")]));
        }
        assert_eq!(app.daemon_log_lines.len(), 300);

        for i in 300..600 {
            app.handle_update(AppUpdate::DaemonLogs(vec![format!("line {i}")]));
        }
        assert_eq!(app.daemon_log_lines.len(), 500);
        assert_eq!(app.daemon_log_lines[0], "line 100");
        assert_eq!(app.daemon_log_lines[499], "line 599");
    }
}
