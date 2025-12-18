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
