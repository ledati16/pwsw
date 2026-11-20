//! CLI smoke tests - verify basic command-line interface functionality
//!
//! These tests run the actual compiled binary to ensure:
//! - Help and version flags work
//! - Commands parse correctly
//! - Error messages are helpful
//! - `--config` / `PWSW_CONFIG` select the config file
//!
//! Isolation: every invocation runs in its own [`Sandbox`] with a cleared environment.
//! `HOME` and all XDG directories point into a fresh temp dir, so the binary can never
//! read or create the user's real config, reach the live daemon socket, or talk to the
//! session's Wayland compositor / D-Bus (those variables are not passed through).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

/// A valid config with a single default sink.
const VALID_CONFIG: &str = r#"
[settings]
log_level = "info"

[[sinks]]
name = "alsa_output.test-sink"
desc = "Smoke Test Speakers"
default = true
"#;

/// A config that parses but fails validation (two default sinks).
const TWO_DEFAULTS_CONFIG: &str = r#"
[[sinks]]
name = "sink_a"
desc = "Sink A"
default = true

[[sinks]]
name = "sink_b"
desc = "Sink B"
default = true
"#;

/// A config whose sinks list is empty (valid, but the daemon refuses to start).
const NO_SINKS_CONFIG: &str = "[settings]\nlog_level = \"info\"\n";

/// Per-test isolated environment for running the `pwsw` binary.
struct Sandbox {
    root: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("failed to create sandbox tempdir");
        for sub in [
            "home", "config", "data", "state", "cache", "runtime", "files",
        ] {
            std::fs::create_dir(root.path().join(sub)).expect("failed to create sandbox dir");
        }
        Self { root }
    }

    fn dir(&self, sub: &str) -> PathBuf {
        self.root.path().join(sub)
    }

    /// Write `contents` to a file outside every default config location.
    fn write_file(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.dir("files").join(name);
        std::fs::write(&path, contents).expect("failed to write sandbox file");
        path
    }

    /// Where `pwsw` would put its config if no override were given.
    fn default_config_path(&self) -> PathBuf {
        self.dir("config").join("pwsw").join("config.toml")
    }

    /// Assert that no default-location `pwsw` config/data was created in this sandbox.
    fn assert_default_locations_untouched(&self) {
        for path in [
            self.dir("config").join("pwsw"),
            self.dir("home").join(".config").join("pwsw"),
            self.dir("data").join("pwsw"),
            self.dir("home").join(".local").join("share").join("pwsw"),
        ] {
            assert!(
                !path.exists(),
                "{} was created even though an explicit config was given",
                path.display()
            );
        }
    }

    /// A `pwsw` command with a cleared, sandboxed environment.
    fn cmd(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_pwsw"));
        cmd.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.dir("home"))
            .env("XDG_CONFIG_HOME", self.dir("config"))
            .env("XDG_DATA_HOME", self.dir("data"))
            .env("XDG_STATE_HOME", self.dir("state"))
            .env("XDG_CACHE_HOME", self.dir("cache"))
            .env("XDG_RUNTIME_DIR", self.dir("runtime"))
            .env("NO_COLOR", "1")
            .current_dir(self.dir("files"));
        cmd
    }
}

fn run(cmd: &mut Command) -> Output {
    cmd.output().expect("failed to run pwsw binary")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_exit(output: &Output, code: i32) {
    assert_eq!(
        output.status.code(),
        Some(code),
        "unexpected exit status\nstdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

fn assert_stderr_contains(output: &Output, needle: &str) {
    let err = stderr(output);
    assert!(
        err.contains(needle),
        "stderr should contain {needle:?}\nstderr:\n{err}"
    );
}

#[test]
fn cli_help_works() {
    let sb = Sandbox::new();
    let output = run(sb.cmd().arg("--help"));

    assert_exit(&output, 0);
    let out = stdout(&output);
    for needle in [
        "Usage",
        "daemon",
        "status",
        "tui",
        "--config <PATH>",
        "PWSW_CONFIG",
    ] {
        assert!(
            out.contains(needle),
            "help should mention {needle:?}:\n{out}"
        );
    }
}

#[test]
fn cli_version_works() {
    let sb = Sandbox::new();
    let output = run(sb.cmd().arg("--version"));

    assert_exit(&output, 0);
    let expected = format!("pwsw {}", env!("CARGO_PKG_VERSION"));
    assert_eq!(stdout(&output).trim(), expected);
}

#[test]
fn cli_invalid_command_shows_error() {
    let sb = Sandbox::new();
    let output = run(sb.cmd().arg("nonexistent-command"));

    // clap usage errors exit with status 2
    assert_exit(&output, 2);
    assert_stderr_contains(&output, "unrecognized subcommand 'nonexistent-command'");
}

#[test]
fn cli_validate_creates_default_config_in_sandbox() {
    let sb = Sandbox::new();
    let output = run(sb.cmd().arg("validate"));

    assert_exit(&output, 0);
    // The shipped default has no sinks: validate warns but succeeds without a daemon
    assert_stderr_contains(&output, "No sinks configured - daemon will not start");
    assert!(
        sb.default_config_path().is_file(),
        "default config should be created at the sandbox XDG path"
    );
    let out = stdout(&output);
    assert!(
        out.contains(&sb.default_config_path().display().to_string()),
        "validate should print the config path in use:\n{out}"
    );
}

#[test]
fn cli_validate_honours_config_flag() {
    let sb = Sandbox::new();
    let config = sb.write_file("flag.toml", VALID_CONFIG);

    let output = run(sb.cmd().arg("--config").arg(&config).arg("validate"));

    assert_exit(&output, 0);
    let out = stdout(&output);
    assert!(
        out.contains("Smoke Test Speakers"),
        "summary should list sink:\n{out}"
    );
    assert!(
        out.contains(&config.display().to_string()),
        "summary should show the --config path:\n{out}"
    );
    sb.assert_default_locations_untouched();
}

#[test]
fn cli_validate_honours_relative_config_flag_after_subcommand() {
    let sb = Sandbox::new();
    let config = sb.write_file("relative.toml", VALID_CONFIG);

    // Global flag after the subcommand, relative to the working directory
    let output = run(sb.cmd().args(["validate", "--config", "relative.toml"]));

    assert_exit(&output, 0);
    let out = stdout(&output);
    assert!(out.contains("Smoke Test Speakers"));
    // The relative path is resolved to an absolute one
    assert!(
        out.contains(&config.display().to_string()),
        "summary should show the absolute config path:\n{out}"
    );
    sb.assert_default_locations_untouched();
}

#[test]
fn cli_validate_honours_pwsw_config_env() {
    let sb = Sandbox::new();
    let config = sb.write_file("env.toml", VALID_CONFIG);

    let output = run(sb.cmd().env("PWSW_CONFIG", &config).arg("validate"));

    assert_exit(&output, 0);
    let out = stdout(&output);
    assert!(
        out.contains("Smoke Test Speakers"),
        "summary should list sink:\n{out}"
    );
    assert!(out.contains(&config.display().to_string()));
    sb.assert_default_locations_untouched();
}

#[test]
fn cli_config_flag_takes_precedence_over_env() {
    let sb = Sandbox::new();
    let valid = sb.write_file("valid.toml", VALID_CONFIG);
    let invalid = sb.write_file("invalid.toml", TWO_DEFAULTS_CONFIG);

    let output = run(sb
        .cmd()
        .env("PWSW_CONFIG", &invalid)
        .arg("--config")
        .arg(&valid)
        .arg("validate"));

    assert_exit(&output, 0);
    assert!(stdout(&output).contains("Smoke Test Speakers"));
    sb.assert_default_locations_untouched();
}

#[test]
fn cli_validate_rejects_invalid_config() {
    let sb = Sandbox::new();
    let config = sb.write_file("two_defaults.toml", TWO_DEFAULTS_CONFIG);

    let output = run(sb.cmd().arg("--config").arg(&config).arg("validate"));

    assert_exit(&output, 1);
    assert_stderr_contains(&output, "2 default sinks found. Only one allowed.");
    sb.assert_default_locations_untouched();
}

#[test]
fn cli_validate_rejects_unparseable_config() {
    let sb = Sandbox::new();
    let config = sb.write_file("broken.toml", "[[sinks]\nname = ");

    let output = run(sb.cmd().arg("--config").arg(&config).arg("validate"));

    assert_exit(&output, 1);
    assert_stderr_contains(
        &output,
        &format!("Failed to parse config: {}", config.display()),
    );
    sb.assert_default_locations_untouched();
}

#[test]
fn cli_daemon_refuses_to_start_without_sinks() {
    let sb = Sandbox::new();
    let config = sb.write_file("empty.toml", NO_SINKS_CONFIG);

    let output = run(sb.cmd().arg("--config").arg(&config).arg("daemon"));

    assert_exit(&output, 1);
    assert_stderr_contains(&output, "No sinks configured. The daemon cannot start");
    // The guidance must point at the file actually in use
    assert_stderr_contains(&output, &config.display().to_string());
    sb.assert_default_locations_untouched();
    assert!(
        !sb.dir("runtime").join("pwsw.sock").exists(),
        "no IPC socket should be created"
    );
}

#[test]
fn cli_ipc_commands_report_missing_daemon() {
    let sb = Sandbox::new();
    for (args, message) in [
        (&["shutdown"][..], "Daemon is not running"),
        (
            &["list-windows"][..],
            "Daemon is not running. Start it with: pwsw daemon",
        ),
        (
            &["test-rule", "^firefox$"][..],
            "Daemon is not running. Start it with: pwsw daemon",
        ),
    ] {
        let output = run(sb.cmd().args(args));
        assert_exit(&output, 1);
        assert_stderr_contains(&output, message);
    }
}

#[test]
fn cli_status_reports_missing_pipewire_tools() {
    let sb = Sandbox::new();
    let config = sb.write_file("status.toml", VALID_CONFIG);
    // An empty PATH dir makes the outcome independent of whether `PipeWire` is installed
    let empty_bin = sb.dir("cache");

    let output = run(sb
        .cmd()
        .env("PATH", &empty_bin)
        .arg("--config")
        .arg(&config)
        .arg("status"));

    assert_exit(&output, 1);
    assert_stderr_contains(&output, "PipeWire tool 'pw-dump' not found or failed");
    sb.assert_default_locations_untouched();
}

/// Sanity check for the helper itself: the sandbox must not inherit session variables.
#[test]
fn sandbox_environment_is_isolated() {
    let sb = Sandbox::new();
    let cmd = sb.cmd();
    let envs: Vec<_> = cmd.get_envs().collect();
    for var in ["WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS", "PWSW_CONFIG"] {
        assert!(
            !envs
                .iter()
                .any(|(k, v)| k.to_str() == Some(var) && v.is_some()),
            "{var} must not be passed to the binary"
        );
    }
    let home = envs
        .iter()
        .find(|(k, _)| k.to_str() == Some("HOME"))
        .and_then(|(_, v)| *v)
        .map(Path::new)
        .expect("HOME must be set");
    assert!(home.starts_with(sb.root.path()));
}
