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
//! - `windows_fingerprint()` - Internal hash function for window list comparison
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

mod forwarder;
mod input_integration_tests;
mod windows_fp;

#[cfg(test)]
mod daemon_log_tests {
    use crate::config::Config;
    use crate::tui::app::App;

    #[test]
    fn test_daemon_logs_bounded_on_large_burst() {
        const MAX_LOG_LINES: usize = 500;

        // Create a minimal config for App
        let config = Config {
            sinks: vec![],
            rules: vec![],
            settings: crate::config::Settings {
                default_on_startup: true,
                set_smart_toggle: true,
                notify_manual: true,
                notify_rules: true,
                match_by_index: false,
                log_level: "info".to_string(),
            },
        };
        let mut app = App::with_config(config);

        // Simulate 1000-line burst
        let burst: Vec<String> = (0..1000).map(|i| format!("line {i}")).collect();

        // Apply the bounding logic from mod.rs:578-593
        let available_space = MAX_LOG_LINES.saturating_sub(app.daemon_log_lines.len());
        let safe_new_lines = if burst.len() > available_space {
            // Take last N lines if burst is too large
            &burst[burst.len().saturating_sub(MAX_LOG_LINES)..]
        } else {
            &burst
        };

        app.daemon_log_lines.extend_from_slice(safe_new_lines);

        // Keep only last 500 lines to avoid unbounded growth (defensive)
        if app.daemon_log_lines.len() > MAX_LOG_LINES {
            let excess = app.daemon_log_lines.len() - MAX_LOG_LINES;
            app.daemon_log_lines.drain(0..excess);
        }

        // Verify logs are bounded
        assert_eq!(app.daemon_log_lines.len(), 500);
        // Verify we kept the last 500 lines from the burst
        assert_eq!(app.daemon_log_lines[0], "line 500");
        assert_eq!(app.daemon_log_lines[499], "line 999");
    }

    #[test]
    fn test_daemon_logs_incremental_growth() {
        const MAX_LOG_LINES: usize = 500;

        let config = Config {
            sinks: vec![],
            rules: vec![],
            settings: crate::config::Settings {
                default_on_startup: true,
                set_smart_toggle: true,
                notify_manual: true,
                notify_rules: true,
                match_by_index: false,
                log_level: "info".to_string(),
            },
        };
        let mut app = App::with_config(config);

        // Add 300 lines
        for i in 0..300 {
            let new_lines = vec![format!("line {i}")];
            let available_space = MAX_LOG_LINES.saturating_sub(app.daemon_log_lines.len());
            let safe_new_lines = if new_lines.len() > available_space {
                &new_lines[new_lines.len().saturating_sub(MAX_LOG_LINES)..]
            } else {
                &new_lines
            };
            app.daemon_log_lines.extend_from_slice(safe_new_lines);

            if app.daemon_log_lines.len() > MAX_LOG_LINES {
                let excess = app.daemon_log_lines.len() - MAX_LOG_LINES;
                app.daemon_log_lines.drain(0..excess);
            }
        }

        assert_eq!(app.daemon_log_lines.len(), 300);

        // Add 300 more lines (total 600, should trim to 500)
        for i in 300..600 {
            let new_lines = vec![format!("line {i}")];
            let available_space = MAX_LOG_LINES.saturating_sub(app.daemon_log_lines.len());
            let safe_new_lines = if new_lines.len() > available_space {
                &new_lines[new_lines.len().saturating_sub(MAX_LOG_LINES)..]
            } else {
                &new_lines
            };
            app.daemon_log_lines.extend_from_slice(safe_new_lines);

            if app.daemon_log_lines.len() > MAX_LOG_LINES {
                let excess = app.daemon_log_lines.len() - MAX_LOG_LINES;
                app.daemon_log_lines.drain(0..excess);
            }
        }

        assert_eq!(app.daemon_log_lines.len(), 500);
        // Verify we kept lines 100-599
        assert_eq!(app.daemon_log_lines[0], "line 100");
        assert_eq!(app.daemon_log_lines[499], "line 599");
    }
}
