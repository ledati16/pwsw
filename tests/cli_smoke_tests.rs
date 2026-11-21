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
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

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
        self.cmd_for(env!("CARGO_BIN_EXE_pwsw"))
    }

    /// Any program (e.g. `sh` wrapping `pwsw`) with the same sandboxed environment.
    fn cmd_for(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut cmd = Command::new(program);
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
        // Under `cargo llvm-cov` the instrumented binary writes its profile here;
        // without it, it would drop a `default_*.profraw` into the sandbox
        if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
            cmd.env("LLVM_PROFILE_FILE", profile);
        }
        cmd
    }
}

/// Longest a single `pwsw` invocation may take before the test fails
const RUN_TIMEOUT: Duration = Duration::from_secs(60);

/// Run `cmd` to completion, capturing its output; fails (and kills it) after
/// [`RUN_TIMEOUT`] instead of hanging the test
fn run(cmd: &mut Command) -> Output {
    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to run pwsw binary");
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    if let Ok(output) = rx.recv_timeout(RUN_TIMEOUT) {
        return output.expect("failed to collect pwsw output");
    }
    // The waiting thread still owns the child and reaps it once it dies
    if let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
    }
    panic!("pwsw did not exit within {RUN_TIMEOUT:?}: {cmd:?}");
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
    assert_eq!(stdout(&output).trim(), format!("pwsw {}", pwsw::VERSION));

    // `<crate version>` or `<crate version>+<short hash>[-dirty]`, same as the daemon
    let base = env!("CARGO_PKG_VERSION");
    let rest = pwsw::VERSION
        .strip_prefix(base)
        .expect("version starts with the crate version");
    if let Some(revision) = rest.strip_prefix('+') {
        let hash = revision.strip_suffix("-dirty").unwrap_or(revision);
        assert!(
            hash.len() >= 7 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
            "bad commit hash in {:?}",
            pwsw::VERSION
        );
    } else {
        assert!(rest.is_empty(), "unexpected version suffix {rest:?}");
    }
    assert_eq!(pwsw::version_string(), format!("v{}", pwsw::VERSION));
}

/// Whether `bytes` contain an `ESC` (start of any terminal escape sequence)
fn has_escape(bytes: &[u8]) -> bool {
    bytes.contains(&0x1b)
}

#[test]
fn cli_piped_output_has_no_escape_codes_by_default() {
    let sb = Sandbox::new();
    let config = sb.write_file("valid.toml", VALID_CONFIG);
    let missing = sb.dir("files").join("missing.toml");

    // No `NO_COLOR`: piped output alone must turn styling off
    let ok = run(sb
        .cmd()
        .env_remove("NO_COLOR")
        .arg("--config")
        .arg(&config)
        .arg("validate"));
    assert_exit(&ok, 0);
    let failed = run(sb
        .cmd()
        .env_remove("NO_COLOR")
        .arg("--config")
        .arg(&missing)
        .arg("validate"));
    assert_exit(&failed, 1);
    for output in [&ok, &failed] {
        assert!(!has_escape(&output.stdout), "stdout: {:?}", stdout(output));
        assert!(!has_escape(&output.stderr), "stderr: {:?}", stderr(output));
    }
}

#[test]
fn cli_color_always_styles_piped_output() {
    let sb = Sandbox::new();
    let config = sb.write_file("valid.toml", VALID_CONFIG);
    let missing = sb.dir("files").join("missing.toml");

    // `--color always` wins over the sandbox's `NO_COLOR=1`
    let ok = run(sb
        .cmd()
        .args(["--color", "always", "--config"])
        .arg(&config)
        .arg("validate"));
    assert_exit(&ok, 0);
    assert!(has_escape(&ok.stdout), "stdout: {:?}", stdout(&ok));

    let failed = run(sb
        .cmd()
        .args(["validate", "--color", "always", "--config"])
        .arg(&missing));
    assert_exit(&failed, 1);
    assert!(has_escape(&failed.stderr), "stderr: {:?}", stderr(&failed));
}

#[test]
fn cli_no_color_beats_clicolor_force() {
    let sb = Sandbox::new();
    let config = sb.write_file("valid.toml", VALID_CONFIG);

    let forced = run(sb
        .cmd()
        .env_remove("NO_COLOR")
        .env("CLICOLOR_FORCE", "1")
        .arg("--config")
        .arg(&config)
        .arg("validate"));
    assert_exit(&forced, 0);
    assert!(has_escape(&forced.stdout), "CLICOLOR_FORCE=1 forces colour");

    let output = run(sb
        .cmd()
        .env("NO_COLOR", "1")
        .env("CLICOLOR_FORCE", "1")
        .args(["--color", "auto", "--config"])
        .arg(&config)
        .arg("validate"));
    assert_exit(&output, 0);
    assert!(!has_escape(&output.stdout), "stdout: {:?}", stdout(&output));
    assert!(!has_escape(&output.stderr), "stderr: {:?}", stderr(&output));

    let never = run(sb
        .cmd()
        .env_remove("NO_COLOR")
        .env("CLICOLOR_FORCE", "1")
        .args(["--color", "never", "--config"])
        .arg(&config)
        .arg("validate"));
    assert!(!has_escape(&never.stdout), "--color never wins");
}

#[test]
fn cli_errors_are_short_without_backtrace_request() {
    let sb = Sandbox::new();
    let missing = sb.dir("files").join("missing.toml");

    let output = run(sb.cmd().arg("--config").arg(&missing).arg("validate"));
    assert_exit(&output, 1);
    let err = stderr(&output);
    assert!(err.starts_with("error: Config file not found"), "{err}");
    for noise in ["Location:", "RUST_BACKTRACE", "BACKTRACE", "src/"] {
        assert!(
            !err.contains(noise),
            "stderr should not contain {noise:?}:\n{err}"
        );
    }

    // Asking for backtraces brings the full report back: `anyhow`'s `Debug` form
    let output = run(sb
        .cmd()
        .env("RUST_BACKTRACE", "1")
        .arg("--config")
        .arg(&missing)
        .arg("validate"));
    assert_exit(&output, 1);
    let err = stderr(&output);
    assert!(err.starts_with("Error:"), "{err}");
    assert!(err.contains("Config file not found"), "{err}");
    assert!(err.contains("Stack backtrace:"), "{err}");
}

#[test]
fn cli_completions_print_a_script() {
    let sb = Sandbox::new();
    for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
        let output = run(sb.cmd().args(["completions", shell]));
        assert_exit(&output, 0);
        let script = stdout(&output);
        assert!(
            script.contains("pwsw") && script.contains("list-sinks"),
            "{shell} completions look wrong:\n{script}"
        );
    }
    let output = run(sb.cmd().args(["completions", "tcsh"]));
    assert_exit(&output, 2);
}

/// Run `cmd` (with stdout and stderr already set up by the caller) to completion,
/// failing after [`RUN_TIMEOUT`]
fn wait_status(cmd: &mut Command) -> std::process::ExitStatus {
    let mut child = cmd
        .stdin(Stdio::null())
        .spawn()
        .expect("failed to run pwsw");
    let deadline = std::time::Instant::now() + RUN_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("wait for pwsw") {
            return status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("pwsw did not exit within {RUN_TIMEOUT:?}: {cmd:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The process exited by itself with `code`: not killed by a signal (`SIGABRT` from
/// a panic under `panic = "abort"`), and not a panic exit (101) in test builds
fn assert_clean_exit(status: std::process::ExitStatus, code: i32, what: &str) {
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(
        status.signal(),
        None,
        "{what}: killed by signal {:?}",
        status.signal()
    );
    assert_eq!(status.code(), Some(code), "{what}: unexpected exit status");
}

/// A pipe whose reading end is already closed: every write to it fails with `EPIPE`
fn broken_pipe() -> Stdio {
    let (reader, writer) = std::io::pipe().expect("pipe");
    drop(reader);
    Stdio::from(writer)
}

/// Shell snippet that closes stderr, then runs `$0` with the remaining arguments
const CLOSE_STDERR: &str = "exec 2>&-; exec \"$0\" \"$@\"";

#[test]
fn cli_survives_closed_stderr() {
    let sb = Sandbox::new();
    let config = sb.write_file("config.toml", VALID_CONFIG);
    for (args, code) in [
        (vec!["validate"], 1), // no config: the error goes to the closed stderr
        (vec!["nosuchcmd"], 2),
    ] {
        let status = wait_status(
            sb.cmd_for("sh")
                .args(["-c", CLOSE_STDERR, env!("CARGO_BIN_EXE_pwsw")])
                .args(&args)
                .stdout(Stdio::null()),
        );
        assert_clean_exit(status, code, &format!("pwsw {args:?} with stderr closed"));
    }
    // Success output to stdout, warnings and errors to a closed stderr
    let status = wait_status(
        sb.cmd_for("sh")
            .args(["-c", CLOSE_STDERR, env!("CARGO_BIN_EXE_pwsw"), "--config"])
            .arg(&config)
            .arg("validate")
            .stdout(Stdio::null()),
    );
    assert_clean_exit(status, 0, "pwsw validate (valid) with stderr closed");
}

#[test]
fn cli_survives_broken_pipes() {
    let sb = Sandbox::new();
    let config = sb.write_file("config.toml", VALID_CONFIG);

    // Nobody reads stdout at all
    let status = wait_status(
        sb.cmd()
            .args(["completions", "bash"])
            .stdout(broken_pipe())
            .stderr(Stdio::null()),
    );
    assert_clean_exit(status, 0, "completions into a closed pipe");
    let status = wait_status(
        sb.cmd()
            .arg("--config")
            .arg(&config)
            .arg("validate")
            .stdout(broken_pipe())
            .stderr(Stdio::null()),
    );
    assert_clean_exit(status, 0, "validate into a closed pipe");

    // Nobody reads stderr: an error report must not panic either
    let status = wait_status(
        sb.cmd()
            .arg("validate")
            .stdout(Stdio::null())
            .stderr(broken_pipe()),
    );
    assert_clean_exit(status, 1, "validate error into a closed stderr pipe");

    // `| head -n 1`: the reader goes away after the first line
    let mut child = sb
        .cmd()
        .args(["completions", "bash"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to run pwsw");
    let mut first = String::new();
    std::io::BufRead::read_line(
        &mut std::io::BufReader::new(child.stdout.take().expect("stdout")),
        &mut first,
    )
    .expect("read first line");
    assert!(!first.is_empty(), "completions printed nothing");
    // The reader was dropped above; pwsw may or may not have finished writing by then
    let deadline = std::time::Instant::now() + RUN_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait for pwsw") {
            break status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "pwsw did not exit after its reader closed"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_clean_exit(status, 0, "completions | head -n 1");
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
fn cli_validate_reports_missing_config_without_creating_it() {
    let sb = Sandbox::new();
    let output = run(sb.cmd().arg("validate"));

    assert_exit(&output, 1);
    assert_stderr_contains(&output, "Config file not found");
    assert_stderr_contains(&output, &sb.default_config_path().display().to_string());
    assert_stderr_contains(&output, "pwsw init");
    assert!(
        !sb.dir("config").join("pwsw").exists(),
        "validate must not create a config"
    );
}

#[test]
fn cli_status_and_bare_invocation_do_not_create_config() {
    let sb = Sandbox::new();
    for args in [&[][..], &["status"][..]] {
        let output = run(sb.cmd().args(args));
        assert_exit(&output, 1);
        assert_stderr_contains(&output, "Config file not found");
    }
    assert!(!sb.dir("config").join("pwsw").exists());
}

#[test]
fn cli_init_writes_template_and_refuses_overwrite() {
    let sb = Sandbox::new();
    let path = sb.default_config_path();

    let output = run(sb.cmd().arg("init"));
    assert_exit(&output, 0);
    assert_stderr_contains(&output, "Created default config at");
    let template = std::fs::read_to_string(&path).expect("init should write the config");
    assert!(
        template.contains("# [[sinks]]"),
        "template keeps its examples"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path)
            .expect("stat file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    // The template is a valid (sink-less) config
    let output = run(sb.cmd().arg("validate"));
    assert_exit(&output, 0);
    assert_stderr_contains(&output, "No sinks configured - daemon will not start");
    assert!(stdout(&output).contains(&path.display().to_string()));

    // A second init refuses and leaves the file alone
    std::fs::write(&path, VALID_CONFIG).expect("write file");
    let output = run(sb.cmd().arg("init"));
    assert_exit(&output, 1);
    assert_stderr_contains(&output, "Config already exists");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read file"),
        VALID_CONFIG
    );

    // --force replaces it and keeps a backup
    let output = run(sb.cmd().args(["init", "--force"]));
    assert_exit(&output, 0);
    assert_eq!(std::fs::read_to_string(&path).expect("read file"), template);
    assert_eq!(
        std::fs::read_to_string(path.with_file_name("config.toml.bak")).expect("read file"),
        VALID_CONFIG
    );
}

#[test]
fn cli_init_honours_config_flag() {
    let sb = Sandbox::new();
    let path = sb.dir("files").join("custom.toml");

    let output = run(sb.cmd().arg("--config").arg(&path).arg("init"));
    assert_exit(&output, 0);
    assert!(path.is_file());
    sb.assert_default_locations_untouched();
}

#[test]
fn cli_validate_reports_unknown_key() {
    let sb = Sandbox::new();
    let config = sb.write_file("typo.toml", "[settings]\nlog_levle = \"info\"\n");

    let output = run(sb.cmd().arg("--config").arg(&config).arg("validate"));

    assert_exit(&output, 1);
    assert_stderr_contains(&output, "unknown field `log_levle`");
    assert_stderr_contains(&output, &config.display().to_string());
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

/// Help text is plain: no rustdoc backticks; the getting-started list ends with
/// `pwsw doctor`
#[test]
fn cli_help_text_has_no_backticks() {
    let sb = Sandbox::new();
    let output = run(sb.cmd().arg("--help"));
    assert_exit(&output, 0);
    let out = stdout(&output);
    assert!(
        out.contains("6. pwsw doctor           If something does not work"),
        "{out}"
    );
    for args in [
        &["--help"][..],
        &["status", "--help"],
        &["init", "--help"],
        &["test-rule", "--help"],
        &["set-source", "--help"],
        &["pause", "--help"],
        &["doctor", "--help"],
    ] {
        let output = run(sb.cmd().args(args));
        assert_exit(&output, 0);
        let out = stdout(&output);
        assert!(!out.contains('`'), "{args:?} prints a backtick:\n{out}");
    }
}

/// `validate` without sinks says so instead of an empty `Sinks (0):` header
#[test]
fn cli_validate_without_sinks_says_so() {
    let sb = Sandbox::new();
    let config = sb.write_file("empty.toml", NO_SINKS_CONFIG);
    let output = run(sb.cmd().arg("--config").arg(&config).arg("validate"));
    assert_exit(&output, 0);
    let out = stdout(&output);
    assert!(out.contains("No sinks configured."), "{out}");
    assert!(!out.contains("Sinks (0)"), "{out}");
}

/// `validate` calls sources mics, shows a rule's desc, and the deprecation
/// warning for positions is one short line per reference (no timestamp, no
/// module path)
#[test]
fn cli_validate_shows_mics_rule_descs_and_short_warnings() {
    let sb = Sandbox::new();
    let config = sb.write_file(
        "mics.toml",
        r#"
[[sinks]]
name = "alsa_output.a"
desc = "Speakers"
default = true

[[sinks]]
name = "alsa_output.b"
desc = "Headphones"

[[sources]]
name = "alsa_input.m"
desc = "Desk Mic"
default = true

[[rules]]
app_id = "^firefox$"
sink = 2
desc = "Browser"

[[rules]]
app_id = "^discord$"
sink = "Speakers"
source = 1
"#,
    );
    let output = run(sb.cmd().arg("--config").arg(&config).arg("validate"));
    assert_exit(&output, 0);
    let out = stdout(&output);
    for needle in ["Mics (1):", "Desk Mic [DEFAULT]", "desc: Browser"] {
        assert!(out.contains(needle), "{needle:?} in:\n{out}");
    }
    assert!(!out.contains("Sources ("), "{out}");
    let err = stderr(&output);
    let lines: Vec<&str> = err.lines().map(str::trim).collect();
    assert_eq!(lines, [
        "WARN rule 1 refers to its sink by position (sink = 2); use the sink's desc (sink = \
         \"Headphones\")",
        "WARN rule 2 refers to its mic by position (source = 1); use the mic's desc (source = \
         \"Desk Mic\")",
    ]);

    // An unknown sink lists the descs to use, not the deprecated positions
    let output = run(sb
        .cmd()
        .arg("--config")
        .arg(&config)
        .args(["set-sink", "Nope"]));
    assert_exit(&output, 1);
    assert_stderr_contains(
        &output,
        "error: Unknown sink 'Nope'. Available: 'Speakers', 'Headphones'",
    );
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
        (
            &["pause"][..],
            "Daemon is not running. Start it with: pwsw daemon",
        ),
        (
            &["resume"][..],
            "Daemon is not running. Start it with: pwsw daemon",
        ),
        (
            &["toggle-pause"][..],
            "Daemon is not running. Start it with: pwsw daemon",
        ),
    ] {
        let output = run(sb.cmd().args(args));
        assert_exit(&output, 1);
        assert_stderr_contains(&output, message);
    }
}

/// `test-rule` needs a pattern or `--output`, and rejects a bad `--output` regex before
/// asking the daemon
#[test]
fn cli_test_rule_checks_its_arguments() {
    let sb = Sandbox::new();
    let output = run(sb.cmd().arg("test-rule"));
    assert_exit(&output, 2);
    assert_stderr_contains(&output, "<PATTERN>");

    let output = run(sb.cmd().args(["test-rule", "--output", "(HDMI"]));
    assert_exit(&output, 1);
    assert_stderr_contains(&output, "Invalid output regex");

    let output = run(sb.cmd().args(["test-rule", "--output", "^HDMI-"]));
    assert_exit(&output, 1);
    assert_stderr_contains(&output, "Daemon is not running");
    sb.assert_default_locations_untouched();
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
    assert_stderr_contains(&output, "Failed to run `pw-dump`");
    // The rest of the status is still shown
    let out = stdout(&output);
    assert!(out.contains("unavailable"), "{out}");
    assert!(out.contains("Not running"), "{out}");
    // No systemd unit to start (no systemctl at all here); the config in use is named
    assert!(
        out.contains(&format!(
            "  Start it: pwsw --config {} daemon",
            config.display()
        )),
        "{out}"
    );
    sb.assert_default_locations_untouched();
}

/// A config that does not load is not "no sinks": the start hint sends the user to
/// `pwsw validate` (the TUI cannot open it); a missing one still says to add a sink,
/// unless it is a symlink into a missing directory (the TUI cannot create it there
/// either). The hints name the config in use.
#[test]
fn cli_start_hint_for_a_broken_or_missing_config() {
    let sb = Sandbox::new();
    let broken = sb.write_file("broken.toml", TWO_DEFAULTS_CONFIG);
    let missing = sb.dir("files").join("missing.toml");
    let dangling = sb.dir("files").join("dangling.toml");
    std::os::unix::fs::symlink(sb.dir("files").join("gone").join("pwsw.toml"), &dangling)
        .expect("create symlink");
    let empty_bin = sb.dir("cache");
    let fix = |config: &std::path::Path| {
        format!(
            "Fix the config first: pwsw --config {} validate shows the error",
            config.display()
        )
    };
    let add = |config: &std::path::Path| {
        format!("Add a sink first (pwsw --config {} tui)", config.display())
    };

    for (config, expected, unexpected) in [
        (&broken, fix(&broken), add(&broken)),
        (&missing, add(&missing), fix(&missing)),
        (&dangling, fix(&dangling), add(&dangling)),
    ] {
        let output = run(sb
            .cmd()
            .env("PATH", &empty_bin)
            .arg("--config")
            .arg(config)
            .arg("status"));
        assert_exit(&output, 1);
        let out = stdout(&output);
        assert!(out.contains(&format!("  {expected}")), "{out}");
        assert!(!out.contains(&unexpected), "{out}");

        let output = run(sb
            .cmd()
            .env("PATH", &empty_bin)
            .arg("--config")
            .arg(config)
            .arg("doctor"));
        assert_exit(&output, 1);
        let out = stdout(&output);
        assert!(
            out.contains(&format!("⚠ The daemon is not running\n    → {expected}")),
            "{out}"
        );
    }
    sb.assert_default_locations_untouched();
}

#[test]
fn cli_status_json_is_valid_without_pipewire_or_daemon() {
    let sb = Sandbox::new();
    let config = sb.write_file("status.toml", VALID_CONFIG);
    let empty_bin = sb.dir("cache");

    let output = run(sb
        .cmd()
        .env("PATH", &empty_bin)
        .arg("--config")
        .arg(&config)
        .args(["status", "--json"]));

    // Errors are reported inside the document, so status bars keep working
    assert_exit(&output, 0);
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("status --json must print valid JSON");
    assert_eq!(json["daemon"]["running"], false);
    assert!(json["daemon"]["error"].is_null());
    assert!(json["current_sink"].is_null());
    assert!(json["pipewire"]["current_sink"].is_null());
    let pw_error = json["pipewire"]["error"].as_str().expect("pipewire error");
    assert!(pw_error.contains("pw-dump"), "{pw_error}");
    assert!(json["config"]["error"].is_null());
    assert_eq!(
        json["config"]["path"].as_str(),
        Some(config.display().to_string().as_str())
    );

    // A missing config is reported the same way
    let missing = sb.dir("files").join("missing.toml");
    let output = run(sb
        .cmd()
        .env("PATH", &empty_bin)
        .arg("--config")
        .arg(&missing)
        .args(["status", "--json"]));
    assert_exit(&output, 0);
    let json: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("status --json must print valid JSON");
    let config_error = json["config"]["error"].as_str().expect("config error");
    assert!(
        config_error.contains("Config file not found"),
        "{config_error}"
    );
    sb.assert_default_locations_untouched();
}

/// Start `pwsw status --watch` with `args`, wait (bounded) for its first line, and
/// return the still-running child with that line
fn watch_first_line(sb: &Sandbox, args: &[&str]) -> (std::process::Child, String) {
    let mut child = sb
        .cmd()
        .args(["status", "--watch"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to run pwsw");
    let out = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let mut reader = std::io::BufReader::new(out);
        let _ = tx.send(std::io::BufRead::read_line(&mut reader, &mut line).map(|_| line));
        // Keep reading (and the pipe open) until the child is gone
        let _ = std::io::copy(&mut reader, &mut std::io::sink());
    });
    let Ok(line) = rx.recv_timeout(RUN_TIMEOUT) else {
        let _ = child.kill();
        let _ = child.wait();
        panic!("status --watch printed nothing within {RUN_TIMEOUT:?}");
    };
    (child, line.expect("read first line"))
}

/// Without a daemon, `status --watch --json` prints one `stopped` line and keeps
/// running (it waits for the daemon to come back)
#[test]
fn cli_status_watch_without_daemon_prints_stopped_and_keeps_running() {
    let sb = Sandbox::new();
    let (mut child, line) = watch_first_line(&sb, &["--json"]);
    let still_running = child.try_wait().expect("try_wait").is_none();
    // Only the child spawned above, by its handle
    let _ = child.kill();
    let _ = child.wait();
    assert!(line.ends_with('\n'), "a whole line: {line:?}");
    let json: serde_json::Value = serde_json::from_str(&line).expect("valid JSON line");
    assert_eq!(json["class"], "stopped");
    assert_eq!(json["alt"], "stopped");
    assert_eq!(json["daemon"], "stopped");
    assert_eq!(json["text"], "pwsw stopped");
    assert!(json["sink"].is_null() && json["source"].is_null());
    assert!(still_running, "status --watch exited without a daemon");

    let (mut child, line) = watch_first_line(&sb, &[]);
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(line, "pwsw stopped\n");
    sb.assert_default_locations_untouched();
}

/// `status --watch` ends quietly (status 0) when nobody reads its output any more,
/// even while it has nothing new to print
#[test]
fn cli_status_watch_exits_cleanly_on_broken_pipe() {
    let sb = Sandbox::new();
    let status = wait_status(
        sb.cmd()
            .args(["status", "--watch", "--json"])
            .stdout(broken_pipe())
            .stderr(Stdio::null()),
    );
    assert_clean_exit(status, 0, "status --watch into a closed pipe");

    // The reader goes away after the first line (no daemon: nothing more is printed)
    let mut child = sb
        .cmd()
        .args(["status", "--watch", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to run pwsw");
    let out = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let mut reader = std::io::BufReader::new(out);
        let _ = std::io::BufRead::read_line(&mut reader, &mut line);
        let _ = tx.send(line);
        // `reader` (the only read end) is dropped here
    });
    let first = rx.recv_timeout(RUN_TIMEOUT).unwrap_or_default();
    let deadline = std::time::Instant::now() + RUN_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait for pwsw") {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("status --watch did not exit after its reader closed");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(first.contains("\"stopped\""), "{first:?}");
    assert_clean_exit(status, 0, "status --watch | head -n 1");
}

#[test]
fn cli_list_sinks_warns_when_config_fails_to_load() {
    let sb = Sandbox::new();
    let config = sb.write_file("bad.toml", TWO_DEFAULTS_CONFIG);
    // No `PipeWire` tools on PATH: the command fails afterwards, but the warning comes first
    let empty_bin = sb.dir("cache");

    let output = run(sb
        .cmd()
        .env("PATH", &empty_bin)
        .arg("--config")
        .arg(&config)
        .arg("list-sinks"));

    assert_stderr_contains(&output, "Config not loaded");
    assert_stderr_contains(&output, "2 default sinks found");
}

/// Every path under `dir`, recursively, sorted (coverage profiles excluded: an
/// instrumented binary may write them wherever `LLVM_PROFILE_FILE` points)
fn tree(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("readable sandbox dir") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                stack.push(path.clone());
            } else if path.extension().is_some_and(|ext| ext == "profraw") {
                continue;
            }
            out.push(path);
        }
    }
    out.sort();
    out
}

#[test]
fn cli_doctor_reports_what_is_missing_and_changes_nothing() {
    let sb = Sandbox::new();
    // No Wayland, no `PipeWire` tools, no `systemctl`, no daemon, no config
    let empty_bin = sb.dir("cache");
    let before = tree(sb.root.path());

    let output = run(sb.cmd().env("PATH", &empty_bin).arg("doctor"));

    assert_exit(&output, 1);
    let out = stdout(&output);
    for line in [
        "✗ WAYLAND_DISPLAY is not set",
        "→ Run pwsw from your Wayland session",
        "✗ pw-dump does not work: Failed to run `pw-dump --version`",
        "✗ pw-cli does not work",
        "✗ pw-metadata does not work",
        "→ Install your distribution's PipeWire tools",
        "⚠ WirePlumber not found",
        &format!(
            "· Config file: {} (default location)",
            sb.default_config_path().display()
        ),
        "✗ The config file does not exist",
        "→ Create it: pwsw init",
        "⚠ The daemon is not running",
        // No config, so no sinks: starting it would fail
        "→ Add a sink first (pwsw tui)",
        "· systemctl not found: skipped",
        "· No log yet (the daemon has not run)",
        "5 problems, 2 warnings.",
    ] {
        assert!(out.contains(line), "missing {line:?} in:\n{out}");
    }
    assert!(!has_escape(&output.stdout), "NO_COLOR is honoured");
    assert_eq!(tree(sb.root.path()), before, "doctor must not create files");

    // With a valid config: it is checked, the sinks are not (no PipeWire)
    let config = sb.write_file("doctor.toml", VALID_CONFIG);
    let before = tree(sb.root.path());
    let output = run(sb
        .cmd()
        .env("PATH", &empty_bin)
        .arg("--config")
        .arg(&config)
        .arg("doctor"));
    assert_exit(&output, 1);
    let out = stdout(&output);
    for line in [
        &format!("· Config file: {} (--config)", config.display()),
        "✓ The config is valid: 1 sink, 0 rules",
        "Sinks\n  · Not checked: PipeWire could not be queried",
        // Sinks, but no systemd unit (no systemctl): start it directly, with the config
        &format!(
            "⚠ The daemon is not running\n    → Start it: pwsw --config {} daemon",
            config.display()
        ),
    ] {
        assert!(out.contains(line), "missing {line:?} in:\n{out}");
    }
    assert_eq!(tree(sb.root.path()), before, "doctor must not create files");
}

/// Set (to anything) in the re-run of [`sandbox_environment_is_isolated`] that
/// reports what the sandbox hands a child
const PROBE_VAR: &str = "PWSW_SANDBOX_PROBE";
/// Session variables the outer run sets so the probe can prove they are dropped
const LEAKY_VARS: [&str; 5] = [
    "WAYLAND_DISPLAY",
    "DBUS_SESSION_BUS_ADDRESS",
    "PIPEWIRE_REMOTE",
    "PWSW_CONFIG",
    "XDG_CONFIG_DIRS",
];
/// Everything the sandbox may pass to a child
const ALLOWED_VARS: [&str; 9] = [
    "PATH",
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "XDG_RUNTIME_DIR",
    "NO_COLOR",
    "LLVM_PROFILE_FILE",
];

/// The helper itself: a child started by the sandbox sees only the sandbox's
/// variables, even when the test process has session variables set.
///
/// `Command::get_envs` only lists explicit changes, never inherited variables, so the
/// check runs `env` inside the sandbox. The test re-runs itself with session variables
/// set (setting them in this multi-threaded process would be unsound); the re-run
/// prints the child's environment and the sandbox root between markers.
#[test]
fn sandbox_environment_is_isolated() {
    if std::env::var_os(PROBE_VAR).is_some() {
        let sb = Sandbox::new();
        let output = run(&mut sb.cmd_for("env"));
        assert_exit(&output, 0);
        println!("\n<<<ROOT {}>>>", sb.root.path().display());
        println!("<<<ENV>>>\n{}<<<END>>>", stdout(&output));
        return;
    }

    let mut probe = Command::new(std::env::current_exe().expect("test binary path"));
    probe
        .args([
            "--exact",
            "sandbox_environment_is_isolated",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(PROBE_VAR, "1");
    for var in LEAKY_VARS {
        probe.env(var, format!("leaked-{var}"));
    }
    let output = run(&mut probe);
    assert_exit(&output, 0);
    let out = stdout(&output);
    let root = out
        .split_once("<<<ROOT ")
        .and_then(|(_, rest)| rest.split_once(">>>"))
        .map_or_else(
            || panic!("probe printed no sandbox root:\n{out}"),
            |(root, _)| PathBuf::from(root),
        );
    let env = out
        .split_once("<<<ENV>>>\n")
        .and_then(|(_, rest)| rest.split_once("<<<END>>>"))
        .map_or_else(
            || panic!("probe printed no environment:\n{out}"),
            |(env, _)| env,
        );

    let vars: Vec<(&str, &str)> = env
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect();
    for (name, value) in &vars {
        assert!(
            ALLOWED_VARS.contains(name),
            "{name}={value} must not reach the binary:\n{env}"
        );
        assert!(!value.starts_with("leaked-"), "{name} leaked:\n{env}");
    }
    for var in LEAKY_VARS {
        assert!(
            vars.iter().all(|(name, _)| *name != var),
            "{var} must not be passed to the binary:\n{env}"
        );
    }
    for var in [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
        "XDG_RUNTIME_DIR",
    ] {
        let value = vars.iter().find(|(name, _)| *name == var).map_or_else(
            || panic!("{var} must be set:\n{env}"),
            |(_, value)| Path::new(value),
        );
        assert!(
            value.starts_with(&root),
            "{var}={} is outside the sandbox {}",
            value.display(),
            root.display()
        );
    }
}

/// A directory with a fake `pw-dump` that prints `dump` (and no other `PipeWire` tool)
fn fake_pw_dump(sb: &Sandbox, dump: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let bin = sb.dir("cache").join("bin");
    std::fs::create_dir_all(&bin).expect("create fake bin dir");
    let json = sb.write_file("dump.json", dump);
    let script = bin.join("pw-dump");
    // Shell builtins only: `PATH` holds nothing but this directory
    let body = format!(
        "#!/bin/sh\nwhile IFS= read -r line || [ -n \"$line\" ]; do printf '%s\\n' \"$line\"; \
         done < '{}'\n",
        json.display()
    );
    std::fs::write(&script, body).expect("write fake pw-dump");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("make fake pw-dump executable");
    bin
}

#[test]
fn cli_list_sinks_escapes_pipewire_and_config_text() {
    let sb = Sandbox::new();
    let dump = include_str!("fixtures/pwdump-synthetic.json").replace(
        "\"node.description\": \"Built-in Audio Analog Stereo\"",
        "\"node.description\": \"\\u001b[31mEVIL\\u001b]0;pwned\\u0007\\u202e\"",
    );
    let bin = fake_pw_dump(&sb, &dump);
    let config = sb.write_file(
        "evil.toml",
        "[[sinks]]\nname = \"alsa_output.pci-0000_00_1f.3.analog-stereo\"\n\
         desc = \"\\u001b[2JRED\"\ndefault = true\n",
    );

    let output = run(sb
        .cmd()
        .env("PATH", &bin)
        .arg("--config")
        .arg(&config)
        .arg("list-sinks"));
    assert_exit(&output, 0);
    assert!(!output.stdout.contains(&0x1b), "{:?}", stdout(&output));
    let out = stdout(&output);
    assert!(!out.contains('\u{202e}'), "{out:?}");
    assert!(
        out.contains("\\u{1b}[31mEVIL\\u{1b}]0;pwned\\u{7}\\u{202e}"),
        "{out}"
    );
    assert!(out.contains("[\\u{1b}[2JRED]"), "configured marker: {out}");

    // `validate` prints configured strings the same way
    let output = run(sb.cmd().arg("--config").arg(&config).arg("validate"));
    assert_exit(&output, 0);
    assert!(
        stdout(&output).contains("\\u{1b}[2JRED"),
        "{}",
        stdout(&output)
    );
    assert!(!output.stdout.contains(&0x1b));
}

#[test]
fn cli_list_sinks_says_when_there_is_no_config_or_no_sink() {
    let sb = Sandbox::new();
    let bin = fake_pw_dump(&sb, include_str!("fixtures/pwdump-synthetic.json"));

    let output = run(sb.cmd().env("PATH", &bin).arg("list-sinks"));
    assert_exit(&output, 0);
    let out = stdout(&output);
    let path = sb.default_config_path().display().to_string();
    assert!(
        out.contains(&format!("Config: {path} (not created yet: pwsw init)")),
        "{out}"
    );

    let config = sb.write_file("empty.toml", NO_SINKS_CONFIG);
    let output = run(sb
        .cmd()
        .env("PATH", &bin)
        .arg("--config")
        .arg(&config)
        .arg("list-sinks"));
    assert_exit(&output, 0);
    let out = stdout(&output);
    assert!(
        out.contains(&format!(
            "CONFIGURED SINKS:\n-----------------\n  (none; add one with pwsw --config {} tui)",
            config.display()
        )),
        "{out}"
    );
    assert!(
        out.ends_with(&format!("Config: {}\n", config.display())),
        "{out}"
    );
}

#[test]
fn cli_init_writes_through_a_dangling_symlink() {
    let sb = Sandbox::new();
    let target = sb.dir("files").join("dotfiles").join("pwsw.toml");
    std::fs::create_dir(target.parent().expect("parent")).expect("create target dir");
    let link = sb.dir("files").join("config.toml");
    std::os::unix::fs::symlink(&target, &link).expect("create symlink");

    let output = run(sb.cmd().arg("--config").arg(&link).arg("validate"));
    assert_exit(&output, 1);
    assert_stderr_contains(
        &output,
        &format!(
            "Config file not found: {} (a symlink to missing {})",
            link.display(),
            target.display()
        ),
    );
    assert_stderr_contains(&output, "init");

    let output = run(sb.cmd().arg("--config").arg(&link).arg("init"));
    assert_exit(&output, 0);
    assert!(
        target.is_file(),
        "the template is written to the link's target"
    );
    assert!(
        std::fs::symlink_metadata(&link)
            .expect("stat link")
            .file_type()
            .is_symlink(),
        "the link stays a link"
    );
    let output = run(sb.cmd().arg("--config").arg(&link).arg("validate"));
    assert_exit(&output, 0);
}

#[test]
fn cli_init_explains_a_dangling_symlink_into_a_missing_directory() {
    let sb = Sandbox::new();
    let target = sb.dir("files").join("gone").join("pwsw.toml");
    let link = sb.dir("files").join("config.toml");
    std::os::unix::fs::symlink(&target, &link).expect("create symlink");

    let output = run(sb.cmd().arg("--config").arg(&link).arg("validate"));
    assert_exit(&output, 1);
    assert_stderr_contains(
        &output,
        &format!(
            "Create the directory {} (then",
            target.parent().expect("parent").display()
        ),
    );

    for args in [&["init"][..], &["init", "--force"][..]] {
        let output = run(sb.cmd().arg("--config").arg(&link).args(args));
        assert_exit(&output, 1);
        assert_stderr_contains(
            &output,
            &format!(
                "{} is a symlink to missing {}, whose directory",
                link.display(),
                target.display()
            ),
        );
        assert!(
            !target.parent().expect("parent").exists(),
            "nothing created"
        );
    }
}

#[test]
fn cli_init_into_an_unwritable_directory_names_the_directory() {
    use std::os::unix::fs::PermissionsExt;
    // Root writes anywhere: nothing to test
    if rustix::process::geteuid().is_root() {
        return;
    }
    let sb = Sandbox::new();
    let dir = sb.dir("files").join("readonly");
    std::fs::create_dir(&dir).expect("create dir");
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).expect("chmod");

    let output = run(sb
        .cmd()
        .arg("--config")
        .arg(dir.join("config.toml"))
        .arg("init"));
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).expect("chmod back");
    assert_exit(&output, 1);
    assert_stderr_contains(
        &output,
        &format!("Cannot write to {}: Permission denied", dir.display()),
    );
    // Not `tempfile`'s "... at path .../.tmpXXXXXX"
    assert!(!stderr(&output).contains("at path"), "{}", stderr(&output));
}
