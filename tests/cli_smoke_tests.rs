//! CLI smoke tests - verify basic command-line interface functionality
//!
//! These tests run the actual compiled binary to ensure:
//! - Help and version flags work
//! - Commands parse correctly
//! - Error messages are helpful

use std::process::Command;

/// Helper to get the path to the compiled pwsw binary
fn pwsw_bin() -> Command {
    // Use the test binary path - cargo test compiles to target/debug
    Command::new(env!("CARGO_BIN_EXE_pwsw"))
}

#[test]
fn cli_help_works() {
    let output = pwsw_bin()
        .arg("--help")
        .output()
        .expect("Failed to run pwsw --help");

    assert!(
        output.status.success(),
        "pwsw --help should exit successfully"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage"), "Help should show usage");
    assert!(stdout.contains("daemon"), "Help should list daemon command");
    assert!(stdout.contains("status"), "Help should list status command");
    assert!(stdout.contains("tui"), "Help should list tui command");
}

#[test]
fn cli_version_works() {
    let output = pwsw_bin()
        .arg("--version")
        .output()
        .expect("Failed to run pwsw --version");

    assert!(
        output.status.success(),
        "pwsw --version should exit successfully"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("pwsw"), "Version should mention pwsw");
    // Version output should contain something like "pwsw 0.3.1" or similar
    assert!(
        stdout.split_whitespace().count() >= 2,
        "Version should show name and version number"
    );
}

#[test]
fn cli_validate_requires_no_daemon() {
    let output = pwsw_bin()
        .arg("validate")
        .output()
        .expect("Failed to run pwsw validate");

    // validate command should work even without a running daemon
    // It reads config and validates it
    assert!(
        output.status.success() || !output.stderr.is_empty(),
        "validate should either succeed or show config error"
    );

    // Should not complain about missing daemon
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("daemon") || !stderr.contains("not running"),
        "validate should not require daemon to be running"
    );
}

#[test]
fn cli_invalid_command_shows_error() {
    let output = pwsw_bin()
        .arg("nonexistent-command")
        .output()
        .expect("Failed to run pwsw with invalid command");

    assert!(
        !output.status.success(),
        "Invalid command should fail with non-zero exit"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    // clap should show an error about unrecognized subcommand
    assert!(
        stderr.contains("unrecognized")
            || stderr.contains("unexpected")
            || stderr.contains("error"),
        "Should show error for invalid command"
    );
}

#[test]
fn cli_status_shows_helpful_error_when_daemon_not_running() {
    let output = pwsw_bin()
        .arg("status")
        .output()
        .expect("Failed to run pwsw status");

    // Status command requires daemon/pipewire - should fail gracefully with helpful error
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let combined = format!("{stdout}{stderr}");

        // Should show an error message (not be empty)
        assert!(
            !combined.trim().is_empty(),
            "Should show error message when command fails"
        );

        // Error should be somewhat descriptive (contains "Error:" or similar)
        assert!(
            combined.contains("Error") || combined.contains("Failed") || combined.contains("error"),
            "Error message should be clear: {combined}"
        );
    }
    // If daemon is actually running (unlikely in test), that's fine too
}
