//! `pwsw doctor`: check the whole setup and say, in plain language, what to fix.
//!
//! Strictly read-only: it never changes audio state, the config or services. It
//! queries the compositor's registry (binding nothing), runs the `PipeWire` tools
//! with `--version` and one `pw-dump`, loads the config, asks a running daemon for
//! its status over `IPC`, asks `systemctl --user` about the unit and reads the log.
//!
//! Gathering ([`run`]) is kept apart from judging (the `*_checks` functions, which
//! are pure) and from printing ([`Report::render`]), so the verdicts and the
//! output can be tested without a session.

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::Result;

use crate::compositor::{Backend, ProtocolPreference, ProtocolProbe, probe_protocols};
use crate::config::{Config, Endpoint, PositionalRef};
use crate::daemon_manager::{DaemonManager, SYSTEMD_UNIT, StartHint, output_with_timeout};
use crate::ipc::{self, Request, Response};
use crate::pipewire::{GraphInfo, PipeWire, Reachability, Snapshot};
use crate::style::PwswStyle;

/// How long the compositor may take to answer the registry roundtrip
const WAYLAND_TIMEOUT: Duration = Duration::from_secs(5);
/// How long one `systemctl --user` query may take
const SYSTEMCTL_TIMEOUT: Duration = Duration::from_secs(3);
/// Longest log line shown
const MAX_LOG_LINE_CHARS: usize = 300;

// ============================================================================
// Report
// ============================================================================

/// How a check came out
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Works
    Ok,
    /// A fact worth knowing (a path, how something is set up)
    Info,
    /// Works, but probably not as intended
    Warn,
    /// Broken: `pwsw` will not work until this is fixed
    Fail,
}

impl Level {
    const fn mark(self) -> &'static str {
        match self {
            Self::Ok => "✓",
            Self::Info => "·",
            Self::Warn => "⚠",
            Self::Fail => "✗",
        }
    }
}

/// One line of the report, with an optional one-line fix
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub level: Level,
    pub message: String,
    pub hint: Option<String>,
}

impl Check {
    fn new(level: Level, message: impl Into<String>) -> Self {
        Self {
            level,
            message: message.into(),
            hint: None,
        }
    }

    fn ok(message: impl Into<String>) -> Self {
        Self::new(Level::Ok, message)
    }

    fn info(message: impl Into<String>) -> Self {
        Self::new(Level::Info, message)
    }

    fn warn(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::new(Level::Warn, message).hint(hint)
    }

    fn fail(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::new(Level::Fail, message).hint(hint)
    }

    fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// A titled group of checks
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub title: &'static str,
    pub checks: Vec<Check>,
}

/// Everything `pwsw doctor` found
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    pub sections: Vec<Section>,
}

impl Report {
    fn count(&self, level: Level) -> usize {
        self.sections
            .iter()
            .flat_map(|s| &s.checks)
            .filter(|c| c.level == level)
            .count()
    }

    /// Whether nothing is broken (no ✗); decides the exit status
    #[must_use]
    pub fn passed(&self) -> bool {
        self.count(Level::Fail) == 0
    }

    /// The report as printed: one block per section, then a summary line.
    /// Styled for stdout (plain when colour is off there).
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        for section in &self.sections {
            let _ = writeln!(out, "{}", section.title.header());
            for check in &section.checks {
                let mark = match check.level {
                    Level::Ok => check.level.mark().success(),
                    Level::Info => check.level.mark().dim(),
                    Level::Warn => check.level.mark().warning(),
                    Level::Fail => check.level.mark().error(),
                };
                let mut lines = check.message.lines();
                let _ = writeln!(out, "  {mark} {}", lines.next().unwrap_or_default());
                for line in lines {
                    let _ = writeln!(out, "    {line}");
                }
                if let Some(hint) = &check.hint {
                    let _ = writeln!(out, "    {} {hint}", "→".dim());
                }
            }
            out.push('\n');
        }
        let (fails, warns) = (self.count(Level::Fail), self.count(Level::Warn));
        let summary = match (fails, warns) {
            (0, 0) => "Everything looks fine.".success().to_string(),
            (0, w) => format!("No problems, {}.", plural(w, "warning"))
                .warning()
                .to_string(),
            (f, w) => format!("{}, {}.", plural(f, "problem"), plural(w, "warning"))
                .error()
                .to_string(),
        };
        let _ = writeln!(out, "{summary}");
        out
    }
}

/// `1 sink`, `2 sinks`
fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

// ============================================================================
// Running the checks
// ============================================================================

/// Run every check, print the report and return whether nothing is broken
/// (the exit status: 0 without ✗ lines, 1 otherwise). `config_flag`: whether
/// `--config` was given (to say where the config path comes from).
pub async fn run(config_flag: bool) -> bool {
    let local = tokio::task::spawn_blocking(move || local_sections(config_flag)).await;
    let (sections, monitor, config_ok, has_sinks) = match local {
        Ok(local) => (
            local.sections,
            local.monitor,
            local.config_ok,
            local.has_sinks,
        ),
        Err(e) => (
            vec![Section {
                title: "Doctor",
                checks: vec![Check::fail(
                    format!("The checks stopped unexpectedly: {e}"),
                    "Please report this bug",
                )],
            }],
            MonitorRules::default(),
            true,
            false,
        ),
    };
    let mut report = Report { sections };
    let facts = DaemonFacts::query().await;
    let (service, unit_installed) = tokio::task::spawn_blocking(service_sections)
        .await
        .unwrap_or_default();
    let start = StartHint::new(config_ok, has_sinks, unit_installed);
    report.sections.push(daemon_checks(&facts, monitor, start));
    report.sections.extend(service);
    crate::style::write_stdout(format_args!("{}", report.render()));
    report.passed()
}

/// What the config and the compositor say about rules with `output`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct MonitorRules {
    /// The config has rules with `output`
    configured: bool,
    /// The compositor offers `wlr` (monitor info); `None` when it could not be asked
    available: Option<bool>,
}

/// The locally gathered sections, and what they found out about monitor rules
struct Local {
    sections: Vec<Section>,
    monitor: MonitorRules,
    /// The config loaded, or does not exist yet (`pwsw tui` creates it); `false` when
    /// it exists but fails to load (or its path cannot be determined)
    config_ok: bool,
    /// The config loaded and has sinks (the daemon can start)
    has_sinks: bool,
}

/// Wayland, `PipeWire`, config, sinks and sources (blocking)
fn local_sections(config_flag: bool) -> Local {
    let path = Config::get_config_path();
    let source = if config_flag {
        "--config"
    } else if Config::config_path_override().ok().flatten().is_some() {
        "PWSW_CONFIG"
    } else {
        "default location"
    };
    let exists = path.as_ref().is_ok_and(|p| p.exists());
    let config = exists.then(Config::load);
    let loaded = config.as_ref().and_then(|c| c.as_ref().ok());
    let output_rules = loaded.is_some_and(Config::has_output_rules);

    let display = std::env::var_os("WAYLAND_DISPLAY").filter(|d| !d.is_empty());
    let probe = display.as_ref().map(|_| probe_protocols(WAYLAND_TIMEOUT));
    let monitor = MonitorRules {
        configured: output_rules,
        available: probe
            .as_ref()
            .and_then(|p| p.as_ref().ok())
            .map(monitor_info_offered),
    };
    let wayland = wayland_checks(display.as_deref(), probe, output_rules);

    let tools: Vec<(&str, Result<String>)> = ["pw-dump", "pw-cli", "pw-metadata"]
        .into_iter()
        .map(|tool| (tool, PipeWire::tool_version(tool)))
        .collect();
    let dump = PipeWire::dump_with_info();
    let wireplumber = PipeWire::tool_version("wireplumber");
    let pipewire = pipewire_checks(&tools, dump.as_ref(), wireplumber.as_ref().ok());

    let config_section = config_checks(path.as_deref().ok(), source, config.as_ref());

    let snapshot = dump.as_ref().ok().map(|(snapshot, _)| snapshot);
    let mut sections = vec![wayland, pipewire, config_section];
    sections.extend(sink_checks(loaded, snapshot));
    sections.extend(source_checks(loaded, snapshot));
    Local {
        sections,
        monitor,
        config_ok: match &config {
            Some(result) => result.is_ok(),
            None => path.is_ok(),
        },
        has_sinks: loaded.is_some_and(|c| !c.sinks.is_empty()),
    }
}

/// Whether the compositor offers a protocol that reports monitors (`wlr`)
fn monitor_info_offered(probe: &ProtocolProbe) -> bool {
    probe
        .capabilities(ProtocolPreference::PreferWlr)
        .is_some_and(crate::compositor::Capabilities::monitor_info)
}

/// systemd unit and log file (blocking), and whether the unit is installed
fn service_sections() -> (Vec<Section>, bool) {
    let mut installed = false;
    let systemd = if in_path("systemctl") {
        let query = |verb: &str| {
            let mut cmd = Command::new("systemctl");
            cmd.args(["--user", verb, SYSTEMD_UNIT]);
            output_with_timeout(cmd, SYSTEMCTL_TIMEOUT).map(|o| UnitQuery {
                stdout: o.stdout.trim().to_string(),
                stderr: o.stderr.trim().to_string(),
            })
        };
        let enabled = query("is-enabled");
        installed = enabled
            .as_ref()
            .is_some_and(|q| !matches!(q.stdout.as_str(), "" | "not-found"));
        let active = query("is-active");
        systemd_checks(Some((enabled, active)))
    } else {
        systemd_checks(None)
    };

    let log_path = crate::daemon::get_log_file_path().ok();
    let contents = log_path
        .as_deref()
        .and_then(|p| std::fs::read(p).ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    (
        vec![
            systemd,
            log_checks(log_path.as_deref(), contents.as_deref()),
        ],
        installed,
    )
}

/// Whether an executable `name` is in `PATH`
fn in_path(name: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            std::fs::metadata(dir.join(name))
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    })
}

// ============================================================================
// Wayland
// ============================================================================

/// Judge the Wayland session: `display` is `WAYLAND_DISPLAY`, `probe` the
/// registry scan (`None` when there is no display to connect to), `output_rules`
/// whether the config has rules with `output` (the daemon then prefers `wlr`)
fn wayland_checks(
    display: Option<&OsStr>,
    probe: Option<Result<ProtocolProbe>>,
    output_rules: bool,
) -> Section {
    let mut checks = Vec::new();
    match (display, probe) {
        (None, _) | (_, None) => checks.push(Check::fail(
            "WAYLAND_DISPLAY is not set: not running inside a Wayland session",
            "Run pwsw from your Wayland session; under systemd the compositor must export it \
             (systemctl --user import-environment WAYLAND_DISPLAY)",
        )),
        (Some(display), Some(Err(e))) => checks.push(Check::fail(
            format!(
                "Cannot connect to Wayland display {}: {e:#}",
                display.to_string_lossy()
            ),
            "Is the compositor running? WAYLAND_DISPLAY and XDG_RUNTIME_DIR must point at its socket",
        )),
        (Some(display), Some(Ok(probe))) => {
            checks.push(Check::ok(format!(
                "Connected to Wayland display {}",
                display.to_string_lossy()
            )));
            if probe.advertised.is_empty() {
                checks.push(Check::fail(
                    "The compositor advertises no window protocol pwsw supports \
                     (ext_foreign_toplevel_list_v1, zwlr_foreign_toplevel_manager_v1)",
                    "pwsw cannot see windows here; GNOME and KDE Plasma 6 offer neither (see \
                     COMPATIBILITY in pwsw(1))",
                ));
            } else {
                let list: Vec<String> = probe
                    .advertised
                    .iter()
                    .map(|g| format!("{} v{}", g.interface, g.version))
                    .collect();
                checks.push(Check::ok(format!(
                    "Compositor advertises {}",
                    list.join(", ")
                )));
            }
            let preference = if output_rules {
                ProtocolPreference::PreferWlr
            } else {
                ProtocolPreference::Auto
            };
            if let Some(capabilities) = probe.capabilities(preference) {
                checks.push(Check::ok(format!(
                    "The daemon will use {}",
                    capabilities.backend().interface()
                )));
                // The daemon is on wlr when the config has `output` rules, or when
                // the compositor offers nothing else
                checks.push(if monitor_info_offered(&probe)
                    && (output_rules || capabilities.monitor_info())
                {
                    Check::ok(format!(
                        "Monitor rules: available ({})",
                        Backend::Wlr.name()
                    ))
                } else if monitor_info_offered(&probe) {
                    // Without `output` rules the daemon takes ext when offered, which
                    // reports no monitors (the TUI's rule form says so and points here)
                    Check::ok(format!(
                        "Monitor rules: available: the daemon uses {} once a rule has \
                         `output` (restart it after adding the first)",
                        Backend::Wlr.name()
                    ))
                } else if output_rules {
                    Check::warn(
                        "Monitor rules: unavailable: the compositor offers only ext, so rules \
                         with `output` will never match",
                        "Monitor rules need wlr-foreign-toplevel-management (e.g. Sway, \
                         Hyprland); remove `output` from the rules or use such a compositor",
                    )
                } else {
                    Check::info("Monitor rules: unavailable: the compositor offers only ext")
                });
            }
        }
    }
    Section {
        title: "Wayland",
        checks,
    }
}

// ============================================================================
// PipeWire
// ============================================================================

/// The version in a `--version` output: the `Linked with libX <version>` line's
/// last word, else the last non-empty line
fn version_of(output: &str) -> Option<&str> {
    let lines = || output.lines().map(str::trim).filter(|l| !l.is_empty());
    lines()
        .find(|l| l.starts_with("Linked with"))
        .and_then(|l| l.split_whitespace().last())
        .or_else(|| lines().next_back())
}

/// Judge the `PipeWire` side: each tool's `--version` result, the `pw-dump`
/// snapshot, and `wireplumber --version` (if it ran)
fn pipewire_checks(
    tools: &[(&str, Result<String>)],
    dump: Result<&(Snapshot, GraphInfo), &anyhow::Error>,
    wireplumber: Option<&String>,
) -> Section {
    const INSTALL_HINT: &str = "Install your distribution's PipeWire tools (pipewire-tools, \
                                pipewire-utils or pipewire-bin)";
    let mut checks = Vec::new();
    let broken: Vec<_> = tools.iter().filter(|(_, r)| r.is_err()).collect();
    if broken.is_empty() {
        let names: Vec<&str> = tools.iter().map(|(t, _)| *t).collect();
        checks.push(Check::ok(format!("{} are installed", names.join(", "))));
    }
    for (tool, result) in broken {
        if let Err(e) = result {
            checks.push(Check::fail(
                format!("{tool} does not work: {e:#}"),
                INSTALL_HINT,
            ));
        }
    }
    let dump_tool_works = tools
        .iter()
        .any(|(tool, result)| *tool == "pw-dump" && result.is_ok());

    let wireplumber_version = wireplumber.and_then(|o| version_of(o));
    match dump {
        Ok((_, info)) => {
            let library = tools
                .iter()
                .find(|(tool, _)| *tool == "pw-cli")
                .and_then(|(_, r)| r.as_ref().ok())
                .and_then(|o| version_of(o));
            match (&info.server_version, library) {
                (Some(version), _) => {
                    checks.push(Check::ok(format!("PipeWire {version} is running")));
                }
                (None, Some(version)) => checks.push(Check::ok(format!(
                    "PipeWire is running (library {version})"
                ))),
                (None, None) => checks.push(Check::ok("PipeWire is running (version unknown)")),
            }
            match &info.session_manager {
                Some(name) => checks.push(Check::ok(match wireplumber_version {
                    Some(version) => format!("{name} {version} is connected"),
                    None => format!("{name} is connected"),
                })),
                None => checks.push(Check::warn(
                    "No WirePlumber client is connected to PipeWire",
                    "pwsw relies on WirePlumber to keep the default sink and profiles: \
                     systemctl --user enable --now wireplumber",
                )),
            }
        }
        Err(e) => {
            if dump_tool_works {
                checks.push(Check::fail(
                    format!("Cannot read the PipeWire graph: {e:#}"),
                    "Is PipeWire running? Check: systemctl --user status pipewire",
                ));
            }
            match wireplumber_version {
                Some(version) => checks.push(Check::info(format!(
                    "WirePlumber {version} is installed (whether it runs is unknown)"
                ))),
                None => checks.push(Check::warn(
                    "WirePlumber not found",
                    "Install WirePlumber, PipeWire's session manager (package wireplumber)",
                )),
            }
        }
    }
    Section {
        title: "PipeWire",
        checks,
    }
}

// ============================================================================
// Config
// ============================================================================

/// Judge the config: `path` in use (from `source`), and the load result
/// (`None`: the file does not exist)
fn config_checks(path: Option<&Path>, source: &str, config: Option<&Result<Config>>) -> Section {
    let mut checks = Vec::new();
    match path {
        Some(path) => checks.push(Check::info(format!(
            "Config file: {} ({source})",
            path.display()
        ))),
        None => checks.push(Check::fail(
            "Cannot tell where the config file is",
            "Set XDG_CONFIG_HOME or HOME, or pass --config PATH",
        )),
    }
    match config {
        None if let Some(path) = path => checks.push(Check::fail(
            "The config file does not exist",
            format!(
                "Create it: {} (or {}, which also helps you add sinks)",
                crate::config::pwsw_command(path, "init"),
                crate::config::pwsw_command(path, "tui")
            ),
        )),
        None => {}
        Some(Err(e)) => checks.push(Check::fail(
            format!("The config is invalid: {e:#}"),
            "Fix the file; pwsw validate shows the same error",
        )),
        Some(Ok(config)) => {
            checks.push(Check::ok(format!(
                "The config is valid: {}, {}",
                plural(config.sinks.len(), "sink"),
                plural(config.rules.len(), "rule")
            )));
            if config.sinks.is_empty() {
                checks.push(Check::fail(
                    "No sinks are configured, so the daemon will not start",
                    "Add sinks: pwsw tui (2 Sinks → a); pwsw list-sinks shows your outputs",
                ));
            }
            if let Some(check) = positional_check(&config.positional_refs()) {
                checks.push(check);
            }
        }
    }
    Section {
        title: "Config",
        checks,
    }
}

/// The warning about rules that refer to sinks or mics by position (`None`: no
/// such rule); the hint names what to write instead when every position resolves
fn positional_check(refs: &[PositionalRef]) -> Option<Check> {
    if refs.is_empty() {
        return None;
    }
    let mut rules: Vec<String> = refs.iter().map(|r| r.rule.to_string()).collect();
    rules.dedup();
    let sinks = refs.iter().any(|r| r.kind == Endpoint::Sink);
    let mics = refs.iter().any(|r| r.kind == Endpoint::Source);
    let what = match (sinks, mics) {
        (true, true) => "a sink or mic",
        (false, true) => "a mic",
        _ => "a sink",
    };
    let message = format!(
        "Rule(s) {} refer to {what} by position, which is deprecated",
        rules.join(", ")
    );
    let replacements: Option<Vec<String>> = refs
        .iter()
        .map(|r| {
            r.replacement().map(|text| {
                // An element of a fallback list is named by its rule even when only
                // one rule has positions
                if rules.len() == 1 && !r.in_list {
                    text
                } else {
                    format!("rule {}: {text}", r.rule)
                }
            })
        })
        .collect();
    let hint = match replacements {
        Some(list) => format!(
            "Use the desc: {} (saving the rule in pwsw tui does that)",
            list.join(", ")
        ),
        None => "Use the desc; saving the rule in pwsw tui does that".to_string(),
    };
    Some(Check::warn(message, hint))
}

/// Judge each configured sink against one snapshot (`None` when there is no valid
/// config with sinks)
fn sink_checks(config: Option<&Config>, snapshot: Option<&Snapshot>) -> Option<Section> {
    endpoint_checks(Endpoint::Sink, config, snapshot)
}

/// Judge each configured source (microphone) against one snapshot (`None` when there
/// is no valid config with sources)
fn source_checks(config: Option<&Config>, snapshot: Option<&Snapshot>) -> Option<Section> {
    endpoint_checks(Endpoint::Source, config, snapshot)
}

/// Judge each configured sink or source against one snapshot
fn endpoint_checks(
    kind: Endpoint,
    config: Option<&Config>,
    snapshot: Option<&Snapshot>,
) -> Option<Section> {
    let config = config.filter(|c| !c.endpoints(kind).is_empty())?;
    let (title, compare) = match kind {
        Endpoint::Sink => (
            "Sinks",
            "Compare the configured node names with pwsw list-sinks (pwsw tui → 2 Sinks → e, \
             Space on Node picks one)",
        ),
        Endpoint::Source => (
            "Mics",
            "Compare the configured [[sources]] node names with the sources pwsw list-sinks \
             shows",
        ),
    };
    let mut checks = Vec::new();
    let Some(snapshot) = snapshot else {
        checks.push(Check::info("Not checked: PipeWire could not be queried"));
        return Some(Section { title, checks });
    };
    let mut reachable = 0;
    for entry in config.endpoints(kind) {
        let label = if entry.default {
            format!("{} (default)", entry.desc)
        } else {
            entry.desc.clone()
        };
        match PipeWire::reachability_of(snapshot, kind, &entry.name) {
            Ok(Reachability::Present { node_name }) => {
                reachable += 1;
                checks.push(Check::ok(if node_name == entry.name {
                    format!("{label}: present")
                } else {
                    format!("{label}: present as {node_name}")
                }));
            }
            Ok(Reachability::ProfileSwitch {
                device_name,
                profile_name,
            }) => {
                reachable += 1;
                checks.push(Check::ok(format!(
                    "{label}: available by switching {device_name} to profile {profile_name}"
                )));
            }
            Err(e) => checks.push(Check::warn(
                format!("{label}: not available now: {e:#}"),
                "Expected while the device is off or disconnected; otherwise compare the node \
                 name with pwsw list-sinks",
            )),
        }
    }
    if reachable == 0 {
        checks.push(Check::fail(
            format!(
                "None of the configured {}s can be activated right now",
                kind.human_noun()
            ),
            compare,
        ));
    }
    Some(Section { title, checks })
}

// ============================================================================
// Daemon
// ============================================================================

/// What a running daemon said about itself
#[derive(Debug, Clone, PartialEq, Eq)]
struct DaemonStatus {
    version: String,
    protocol: u32,
    uptime_secs: u64,
    current_sink: Option<String>,
    current_sink_fallback: bool,
    /// Compositor backend the daemon uses (`None` from older daemons)
    backend: Option<String>,
}

/// What the daemon checks found out
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct DaemonFacts {
    running: bool,
    /// The `Status` answer, or why there was none (only when `running`)
    status: Option<Result<DaemonStatus, String>>,
    manager: Option<DaemonManager>,
}

impl DaemonFacts {
    /// Ask the daemon (if one answers on the socket); read-only requests only
    async fn query() -> Self {
        if !ipc::is_daemon_running().await {
            return Self::default();
        }
        let status = match ipc::send_request(Request::Status).await {
            Ok(Response::Status {
                version,
                protocol,
                uptime_secs,
                current_sink,
                current_sink_fallback,
                backend,
                ..
            }) => Ok(DaemonStatus {
                version,
                protocol,
                uptime_secs,
                current_sink,
                current_sink_fallback,
                backend,
            }),
            Ok(Response::Error { message }) => Err(message),
            Ok(_) => Err("unexpected answer to a status request".to_string()),
            Err(e) => Err(format!("{e:#}")),
        };
        let manager = match ipc::send_request(Request::GetManagerInfo).await {
            Ok(Response::ManagerInfo { daemon_manager }) => Some(daemon_manager),
            _ => None,
        };
        Self {
            running: true,
            status: Some(status),
            manager,
        }
    }
}

/// Judge the daemon (`monitor`: whether monitor rules are configured and possible, to
/// notice a daemon started before they were added; `start`: how to start a stopped
/// one)
fn daemon_checks(facts: &DaemonFacts, monitor: MonitorRules, start: StartHint) -> Section {
    let mut checks = Vec::new();
    let restart = "Restart it: systemctl --user restart pwsw (or pwsw shutdown, then pwsw daemon)";
    match &facts.status {
        _ if !facts.running => checks.push(Check::warn("The daemon is not running", start.text())),
        None => {}
        Some(Err(message)) => checks.push(Check::fail(
            format!("The daemon runs but could not report its status: {message}"),
            restart,
        )),
        Some(Ok(status)) => {
            if status.protocol == ipc::PROTOCOL_VERSION {
                checks.push(Check::ok(format!(
                    "The daemon {} is running (uptime {})",
                    status.version,
                    crate::format_uptime(status.uptime_secs)
                )));
                let mine = crate::version_string();
                if status.version != mine {
                    checks.push(Check::warn(
                        format!("The daemon is {}, this pwsw is {mine}", status.version),
                        restart,
                    ));
                }
            } else {
                checks.push(Check::fail(
                    format!(
                        "The daemon {} speaks IPC protocol {}, this pwsw speaks {}",
                        status.version,
                        status.protocol,
                        ipc::PROTOCOL_VERSION
                    ),
                    restart,
                ));
            }
            checks.push(Check::info(match &status.current_sink {
                Some(sink) if status.current_sink_fallback => {
                    format!("Current sink: {sink} (a rule's fallback)")
                }
                Some(sink) => format!("Current sink: {sink}"),
                None => "The daemon does not know the current sink yet".to_string(),
            }));
            // Started without `output` rules it chose ext; a restart picks wlr
            if monitor.configured
                && monitor.available != Some(false)
                && status.backend.as_deref() == Some(Backend::Ext.name())
            {
                checks.push(Check::warn(
                    "The daemon uses ext, which reports no monitors, but the config has rules \
                     with `output`",
                    restart,
                ));
            }
        }
    }
    match facts.manager {
        Some(DaemonManager::Systemd) => {
            checks.push(Check::info(format!("Managed by systemd ({SYSTEMD_UNIT})")));
        }
        Some(DaemonManager::Direct) => {
            checks.push(Check::info("Started directly (not by systemd)"));
        }
        None => {}
    }
    Section {
        title: "Daemon",
        checks,
    }
}

// ============================================================================
// systemd
// ============================================================================

/// What `systemctl --user <verb> pwsw.service` printed (`None`: it did not finish)
#[derive(Debug, Clone, PartialEq, Eq)]
struct UnitQuery {
    stdout: String,
    stderr: String,
}

/// Judge the user unit from `is-enabled` and `is-active` (`None`: no `systemctl`)
fn systemd_checks(queries: Option<(Option<UnitQuery>, Option<UnitQuery>)>) -> Section {
    let mut checks = Vec::new();
    let title = "systemd user unit";
    let Some((enabled, active)) = queries else {
        checks.push(Check::info("systemctl not found: skipped"));
        return Section { title, checks };
    };
    let Some(enabled) = enabled else {
        checks.push(Check::warn(
            format!(
                "systemctl --user did not answer within {} s",
                SYSTEMCTL_TIMEOUT.as_secs()
            ),
            "Check the user manager: systemctl --user status",
        ));
        return Section { title, checks };
    };
    let is_enabled = match enabled.stdout.as_str() {
        "enabled" | "enabled-runtime" | "linked" | "linked-runtime" | "alias" => {
            checks.push(Check::ok(format!(
                "{SYSTEMD_UNIT} is installed and enabled"
            )));
            true
        }
        "disabled" | "indirect" => {
            checks.push(
                Check::info(format!("{SYSTEMD_UNIT} is installed, not enabled"))
                    .hint("To start pwsw with your session: systemctl --user enable --now pwsw"),
            );
            false
        }
        "" | "not-found" => {
            let unreachable = enabled.stdout.is_empty()
                && (enabled.stderr.contains("Failed to connect")
                    || !(enabled.stderr.contains("not found")
                        || enabled.stderr.contains("No such file")));
            if unreachable {
                let why = enabled.stderr.lines().next().unwrap_or("no answer");
                checks.push(Check::info(format!(
                    "The systemd user manager is not reachable: {why}"
                )));
            } else {
                checks.push(
                    Check::info(format!("{SYSTEMD_UNIT} is not installed")).hint(
                        "Optional: install contrib/pwsw.service to start pwsw with your session \
                     (see the README)",
                    ),
                );
            }
            return Section { title, checks };
        }
        other => {
            checks.push(Check::warn(
                format!("{SYSTEMD_UNIT} is {other}"),
                "See: systemctl --user status pwsw",
            ));
            false
        }
    };
    match active.as_ref().map(|q| q.stdout.as_str()) {
        Some("active" | "reloading" | "refreshing") => {
            checks.push(Check::ok(format!("{SYSTEMD_UNIT} is active")));
        }
        Some("failed") => checks.push(Check::fail(
            format!("{SYSTEMD_UNIT} has failed"),
            "See why: journalctl --user -u pwsw -e; then systemctl --user restart pwsw",
        )),
        Some("activating") => checks.push(Check::info(format!("{SYSTEMD_UNIT} is starting"))),
        Some(_) if is_enabled => checks.push(Check::warn(
            format!("{SYSTEMD_UNIT} is enabled but not running"),
            "Start it: systemctl --user start pwsw",
        )),
        Some(_) => checks.push(Check::info(format!("{SYSTEMD_UNIT} is not running"))),
        None => {}
    }
    Section { title, checks }
}

// ============================================================================
// Log
// ============================================================================

/// The last `ERROR` or `WARN` line of a daemon log, as (level, timestamp, message)
fn last_problem(contents: &str) -> Option<(&str, &str, &str)> {
    contents.lines().rev().find_map(|line| {
        // `<timestamp> <level> <target>: <message>`, the level padded with spaces
        let (timestamp, rest) = line.trim_start().split_once(char::is_whitespace)?;
        let (level, message) = rest.trim_start().split_once(char::is_whitespace)?;
        matches!(level, "ERROR" | "WARN").then(|| (level, timestamp, message.trim_start()))
    })
}

/// `2026-11-20T23:50:13.788054Z` → `2026-11-20 23:50:13 UTC` (others unchanged)
fn readable_timestamp(timestamp: &str) -> String {
    match timestamp.strip_suffix('Z').and_then(|t| t.split_once('T')) {
        Some((date, time)) => {
            let time = time.split('.').next().unwrap_or(time);
            format!("{date} {time} UTC")
        }
        None => timestamp.to_string(),
    }
}

/// Judge the log: where it is and its last warning or error
fn log_checks(path: Option<&Path>, contents: Option<&str>) -> Section {
    let mut checks = Vec::new();
    match path {
        None => checks.push(Check::info(
            "Cannot tell where the daemon log is (no HOME?)",
        )),
        Some(path) => checks.push(Check::info(format!("Log file: {}", path.display()))),
    }
    match contents {
        None if path.is_some() => checks.push(Check::info("No log yet (the daemon has not run)")),
        None => {}
        Some(contents) => {
            let clean = crate::sanitize::strip_ansi(contents);
            match last_problem(&clean) {
                Some((level, timestamp, message)) => {
                    let what = if level == "ERROR" { "error" } else { "warning" };
                    let message = crate::sanitize::sanitize_line(message, MAX_LOG_LINE_CHARS);
                    checks.push(Check::info(format!(
                        "Last {what} ({}): {message}",
                        readable_timestamp(timestamp)
                    )));
                }
                None => checks.push(Check::info("No warnings or errors in the log")),
            }
        }
    }
    Section {
        title: "Log",
        checks,
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use anyhow::anyhow;
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::compositor::AdvertisedGlobal;
    use crate::test_utils::fixtures::{
        make_config, make_rule, make_rule_with_fallbacks, make_sink,
    };

    fn section(title: &'static str, checks: Vec<Check>) -> Section {
        Section { title, checks }
    }

    #[test]
    fn test_render_marks_hints_and_summary() {
        let report = Report {
            sections: vec![
                section("Wayland", vec![
                    Check::ok("Connected"),
                    Check::fail("No protocol", "Use another compositor"),
                ]),
                section("Log", vec![
                    Check::info("Log file: /x\nsecond line"),
                    Check::warn("Old daemon", "Restart it"),
                ]),
            ],
        };
        assert_eq!(
            report.render(),
            "Wayland\n  ✓ Connected\n  ✗ No protocol\n    → Use another compositor\n\n\
             Log\n  · Log file: /x\n    second line\n  ⚠ Old daemon\n    → Restart it\n\n\
             1 problem, 1 warning.\n"
        );
        assert!(!report.passed());
    }

    #[test]
    fn test_exit_status_ignores_warnings() {
        let mut report = Report {
            sections: vec![section("A", vec![Check::ok("fine"), Check::info("fact")])],
        };
        assert!(report.passed());
        assert!(report.render().ends_with("Everything looks fine.\n"));
        report.sections[0]
            .checks
            .push(Check::warn("odd", "look at it"));
        assert!(report.passed());
        assert!(report.render().ends_with("No problems, 1 warning.\n"));
        report.sections[0].checks.push(Check::fail("a", "b"));
        report.sections[0].checks.push(Check::fail("c", "d"));
        assert!(!report.passed());
        assert!(report.render().ends_with("2 problems, 1 warning.\n"));
    }

    #[test]
    fn test_wayland_checks() {
        let missing = wayland_checks(None, None, false);
        assert_eq!(missing.checks.len(), 1);
        assert_eq!(missing.checks[0].level, Level::Fail);
        assert!(
            missing.checks[0]
                .message
                .contains("WAYLAND_DISPLAY is not set")
        );

        let display = OsString::from("wayland-1");
        let broken = wayland_checks(Some(&display), Some(Err(anyhow!("refused"))), false);
        assert_eq!(broken.checks[0].level, Level::Fail);
        assert!(broken.checks[0].message.contains("wayland-1: refused"));

        let probe = ProtocolProbe {
            advertised: vec![
                AdvertisedGlobal {
                    interface: "zwlr_foreign_toplevel_manager_v1".to_string(),
                    version: 3,
                },
                AdvertisedGlobal {
                    interface: "ext_foreign_toplevel_list_v1".to_string(),
                    version: 1,
                },
            ],
        };
        let good = wayland_checks(Some(&display), Some(Ok(probe.clone())), false);
        let messages: Vec<&str> = good.checks.iter().map(|c| c.message.as_str()).collect();
        assert_eq!(messages, [
            "Connected to Wayland display wayland-1",
            "Compositor advertises zwlr_foreign_toplevel_manager_v1 v3, \
             ext_foreign_toplevel_list_v1 v1",
            "The daemon will use ext_foreign_toplevel_list_v1",
            "Monitor rules: available: the daemon uses wlr once a rule has `output` \
             (restart it after adding the first)",
        ]);
        assert!(good.checks.iter().all(|c| c.level == Level::Ok));

        // With `output` rules the daemon prefers wlr
        let monitor = wayland_checks(Some(&display), Some(Ok(probe)), true);
        assert_eq!(
            monitor.checks[2].message,
            "The daemon will use zwlr_foreign_toplevel_manager_v1"
        );
        assert_eq!(monitor.checks[3].level, Level::Ok);
        assert_eq!(monitor.checks[3].message, "Monitor rules: available (wlr)");

        let gnome = wayland_checks(
            Some(&display),
            Some(Ok(ProtocolProbe {
                advertised: Vec::new(),
            })),
            false,
        );
        assert_eq!(gnome.checks[1].level, Level::Fail);
        assert_eq!(gnome.checks.len(), 2, "no daemon or monitor line");
    }

    /// A wlr-only compositor: the daemon is on wlr even without `output` rules, so
    /// monitor rules simply work (no "restart after adding the first")
    #[test]
    fn test_wayland_checks_wlr_only_monitor_rules() {
        let display = OsString::from("wayland-1");
        let wlr_only = || {
            Some(Ok(ProtocolProbe {
                advertised: vec![AdvertisedGlobal {
                    interface: "zwlr_foreign_toplevel_manager_v1".to_string(),
                    version: 3,
                }],
            }))
        };
        for output_rules in [false, true] {
            let section = wayland_checks(Some(&display), wlr_only(), output_rules);
            let messages: Vec<&str> = section.checks.iter().map(|c| c.message.as_str()).collect();
            assert_eq!(messages[2..], [
                "The daemon will use zwlr_foreign_toplevel_manager_v1",
                "Monitor rules: available (wlr)",
            ]);
            assert!(section.checks.iter().all(|c| c.level == Level::Ok));
        }
    }

    /// An ext-only compositor: monitor rules are just unavailable, unless the config
    /// has some (then it warns that they never match)
    #[test]
    fn test_wayland_checks_ext_only_monitor_rules() {
        let display = OsString::from("wayland-1");
        let ext_only = || {
            Some(Ok(ProtocolProbe {
                advertised: vec![AdvertisedGlobal {
                    interface: "ext_foreign_toplevel_list_v1".to_string(),
                    version: 1,
                }],
            }))
        };
        let plain = wayland_checks(Some(&display), ext_only(), false);
        let last = plain.checks.last().expect("checks");
        assert_eq!(last.level, Level::Info);
        assert_eq!(
            last.message,
            "Monitor rules: unavailable: the compositor offers only ext"
        );

        let with_rules = wayland_checks(Some(&display), ext_only(), true);
        let last = with_rules.checks.last().expect("checks");
        assert_eq!(last.level, Level::Warn);
        assert!(
            last.message.contains("will never match"),
            "{}",
            last.message
        );
        assert!(
            last.hint
                .as_deref()
                .is_some_and(|h| h.contains("wlr-foreign-toplevel-management")),
            "{last:?}"
        );
        assert!(!monitor_info_offered(&ProtocolProbe {
            advertised: Vec::new()
        }));
    }

    #[test]
    fn test_pipewire_checks() {
        let tools =
            |dump: Result<String>| {
                vec![
                ("pw-dump", dump),
                (
                    "pw-cli",
                    Ok("pw-cli\nCompiled with libpipewire 1.2.7\nLinked with libpipewire 1.2.7\n"
                        .to_string()),
                ),
                ("pw-metadata", Ok(String::new())),
            ]
            };
        let info = GraphInfo {
            server_version: Some("1.2.7".to_string()),
            session_manager: Some("WirePlumber".to_string()),
        };
        let wp = "wireplumber\nLinked with libwireplumber 0.5.6\n".to_string();
        let ok = (Snapshot::default(), info);
        let good = pipewire_checks(&tools(Ok(String::new())), Ok(&ok), Some(&wp));
        let messages: Vec<&str> = good.checks.iter().map(|c| c.message.as_str()).collect();
        assert_eq!(messages, [
            "pw-dump, pw-cli, pw-metadata are installed",
            "PipeWire 1.2.7 is running",
            "WirePlumber 0.5.6 is connected",
        ]);

        let no_wp = (Snapshot::default(), GraphInfo::default());
        let checks = pipewire_checks(&tools(Ok(String::new())), Ok(&no_wp), None).checks;
        assert_eq!(checks[1].message, "PipeWire is running (library 1.2.7)");
        assert_eq!(checks[2].level, Level::Warn);

        // No server: the dump fails although the tool runs
        let err = anyhow!("can't connect");
        let checks = pipewire_checks(&tools(Ok(String::new())), Err(&err), Some(&wp)).checks;
        assert_eq!(checks[1].level, Level::Fail);
        assert!(checks[1].message.contains("can't connect"), "{checks:?}");
        assert_eq!(checks[2].level, Level::Info);

        // No tools: one ✗ per missing tool, the dump failure is not repeated
        let checks = pipewire_checks(&tools(Err(anyhow!("not found"))), Err(&err), None).checks;
        let fails: Vec<&str> = checks
            .iter()
            .filter(|c| c.level == Level::Fail)
            .map(|c| c.message.as_str())
            .collect();
        assert_eq!(fails, ["pw-dump does not work: not found"]);
        assert!(
            checks
                .iter()
                .all(|c| c.level == Level::Ok || c.hint.is_some())
        );
    }

    #[test]
    fn test_config_checks() {
        let path = PathBuf::from("/c/config.toml");
        let missing = config_checks(Some(&path), "default location", None);
        assert_eq!(
            missing.checks[0].message,
            "Config file: /c/config.toml (default location)"
        );
        assert_eq!(missing.checks[1].level, Level::Fail);

        let invalid = config_checks(
            Some(&path),
            "--config",
            Some(&Err(anyhow!("2 default sinks"))),
        );
        assert!(invalid.checks[1].message.contains("2 default sinks"));

        let config = make_config(vec![make_sink("a", "A", true)], vec![
            make_rule("x", None, "A"),
            make_rule("y", None, "1"),
        ]);
        let valid = config_checks(Some(&path), "PWSW_CONFIG", Some(&Ok(config)));
        assert_eq!(
            valid.checks[1].message,
            "The config is valid: 1 sink, 2 rules"
        );
        assert_eq!(valid.checks[2].level, Level::Warn);
        assert_eq!(
            valid.checks[2].message,
            "Rule(s) 2 refer to a sink by position, which is deprecated"
        );
        assert_eq!(
            valid.checks[2].hint.as_deref(),
            Some("Use the desc: sink = \"A\" (saving the rule in pwsw tui does that)")
        );

        let empty = config_checks(
            Some(&path),
            "--config",
            Some(&Ok(make_config(vec![], vec![]))),
        );
        assert_eq!(empty.checks[2].level, Level::Fail);
    }

    #[test]
    fn test_sink_checks() {
        let json = include_bytes!("../tests/fixtures/pwdump-synthetic.json");
        let snapshot = crate::pipewire::parse_snapshot(json).expect("fixture");
        let present = PipeWire::get_active_sinks(&snapshot);
        let present = present.first().expect("an active sink");
        let config = make_config(
            vec![
                make_sink(&present.name, "Here", true),
                make_sink("bluez_output.00_00_00_00_00_00.1", "Gone", false),
            ],
            vec![],
        );
        let section = sink_checks(Some(&config), Some(&snapshot)).expect("a section");
        assert_eq!(section.checks[0].message, "Here (default): present");
        assert_eq!(section.checks[1].level, Level::Warn);
        assert!(
            section.checks[1].message.contains("Bluetooth"),
            "{section:?}"
        );
        assert_eq!(section.checks.len(), 2, "one sink is reachable");

        let none = make_config(vec![make_sink("nope", "Nope", true)], vec![]);
        let section = sink_checks(Some(&none), Some(&snapshot)).expect("a section");
        assert_eq!(section.checks.last().map(|c| c.level), Some(Level::Fail));

        assert!(sink_checks(None, Some(&snapshot)).is_none());
        let section = sink_checks(Some(&config), None).expect("a section");
        assert_eq!(section.checks[0].level, Level::Info);
    }

    /// `[[sources]]` get their own section, judged like sinks; none without sources
    #[test]
    fn test_source_checks() {
        let json = include_bytes!("../tests/fixtures/pwdump-synthetic.json");
        let snapshot = crate::pipewire::parse_snapshot(json).expect("fixture");
        let present = PipeWire::get_active(&snapshot, Endpoint::Source);
        let present = present.first().expect("an active source");
        let sinks = vec![make_sink("nope", "Nope", true)];
        assert!(
            source_checks(Some(&make_config(sinks.clone(), vec![])), Some(&snapshot)).is_none()
        );

        let config = make_config(sinks, vec![]).with_sources(vec![
            make_sink(&present.name, "Desk Mic", true),
            make_sink("bluez_input.00_00_00_00_00_00.0", "Headset Mic", false),
        ]);
        let section = source_checks(Some(&config), Some(&snapshot)).expect("a section");
        assert_eq!(section.title, "Mics");
        assert_eq!(section.checks[0].message, "Desk Mic (default): present");
        assert_eq!(section.checks[1].level, Level::Warn);
        assert_eq!(section.checks.len(), 2, "one source is reachable");

        let none = make_config(vec![make_sink("nope", "Nope", true)], vec![])
            .with_sources(vec![make_sink("alsa_input.nope", "Nope Mic", true)]);
        let section = source_checks(Some(&none), Some(&snapshot)).expect("a section");
        let last = section.checks.last().expect("checks");
        assert_eq!(last.level, Level::Fail);
        assert!(last.message.contains("configured mics"), "{last:?}");
    }

    #[test]
    fn test_daemon_checks() {
        let none = MonitorRules::default();
        let stopped = daemon_checks(&DaemonFacts::default(), none, StartHint::Systemd);
        assert_eq!(stopped.checks.len(), 1);
        assert_eq!(stopped.checks[0].level, Level::Warn);

        let status = |protocol, version: &str| DaemonFacts {
            running: true,
            status: Some(Ok(DaemonStatus {
                version: version.to_string(),
                protocol,
                uptime_secs: 3700,
                current_sink: Some("Speakers".to_string()),
                current_sink_fallback: true,
                backend: Some("ext".to_string()),
            })),
            manager: Some(DaemonManager::Systemd),
        };
        let mine = crate::version_string();
        let good = daemon_checks(
            &status(ipc::PROTOCOL_VERSION, &mine),
            none,
            StartHint::Systemd,
        );
        let messages: Vec<&str> = good.checks.iter().map(|c| c.message.as_str()).collect();
        assert_eq!(messages, [
            format!("The daemon {mine} is running (uptime 1h 1m)").as_str(),
            "Current sink: Speakers (a rule's fallback)",
            "Managed by systemd (pwsw.service)",
        ]);

        let old = daemon_checks(
            &status(ipc::PROTOCOL_VERSION, "v0.6.0"),
            none,
            StartHint::Systemd,
        );
        assert_eq!(old.checks[1].level, Level::Warn);
        let other_protocol = daemon_checks(
            &status(ipc::PROTOCOL_VERSION + 1, "v9"),
            none,
            StartHint::Systemd,
        );
        assert_eq!(other_protocol.checks[0].level, Level::Fail);

        let mute = daemon_checks(
            &DaemonFacts {
                running: true,
                status: Some(Err("timed out".to_string())),
                manager: None,
            },
            none,
            StartHint::Systemd,
        );
        assert_eq!(mute.checks[0].level, Level::Fail);
    }

    /// A stopped daemon gets a start hint that can work: without sinks it would
    /// refuse to start, without the unit `systemctl` cannot start it
    #[test]
    fn test_daemon_checks_start_hint_fits_the_setup() {
        let hint = |start| {
            daemon_checks(&DaemonFacts::default(), MonitorRules::default(), start).checks[0]
                .hint
                .clone()
        };
        assert_eq!(
            hint(StartHint::new(true, false, true)).as_deref(),
            Some("Add a sink first (pwsw tui)")
        );
        // A config that does not load: the TUI cannot open it, so do not send them there
        for unit in [false, true] {
            assert_eq!(
                hint(StartHint::new(false, false, unit)).as_deref(),
                Some("Fix the config first: pwsw validate shows the error")
            );
        }
        assert_eq!(
            hint(StartHint::new(true, true, false)).as_deref(),
            Some("Start it: pwsw daemon")
        );
        assert_eq!(
            hint(StartHint::new(true, true, true)).as_deref(),
            Some("Start it: systemctl --user enable --now pwsw (or pwsw daemon)")
        );
    }

    /// Positional references: one warning naming sinks and/or mics, and what to
    /// write instead when every position names an entry
    #[test]
    fn test_positional_check() {
        let config = make_config(
            vec![
                make_sink("a", "Speakers", true),
                make_sink("b", "Headphones", false),
            ],
            vec![make_rule("x", None, "2"), make_rule("y", None, "Speakers")],
        );
        let check = positional_check(&config.positional_refs()).expect("a warning");
        assert_eq!(
            check.message,
            "Rule(s) 1 refer to a sink by position, which is deprecated"
        );
        assert_eq!(
            check.hint.as_deref(),
            Some("Use the desc: sink = \"Headphones\" (saving the rule in pwsw tui does that)")
        );

        let mut rule = make_rule("z", None, "Speakers");
        rule.source_refs = vec!["1".to_string()];
        let config = make_config(
            vec![
                make_sink("a", "Speakers", true),
                make_sink("b", "Headphones", false),
            ],
            vec![make_rule("x", None, "2"), rule],
        )
        .with_sources(vec![make_sink("m", "Desk Mic", true)]);
        let check = positional_check(&config.positional_refs()).expect("a warning");
        assert_eq!(
            check.message,
            "Rule(s) 1, 2 refer to a sink or mic by position, which is deprecated"
        );
        assert_eq!(
            check.hint.as_deref(),
            Some(
                "Use the desc: rule 1: sink = \"Headphones\", rule 2: source = \"Desk Mic\" \
                 (saving the rule in pwsw tui does that)"
            )
        );

        // A position that names nothing: no concrete advice
        let unresolved = [PositionalRef {
            rule: 3,
            kind: Endpoint::Source,
            position: "9".to_string(),
            desc: None,
            in_list: false,
            written: None,
        }];
        let check = positional_check(&unresolved).expect("a warning");
        assert_eq!(
            check.message,
            "Rule(s) 3 refer to a mic by position, which is deprecated"
        );
        assert_eq!(
            check.hint.as_deref(),
            Some("Use the desc; saving the rule in pwsw tui does that")
        );
        assert!(positional_check(&[]).is_none());
    }

    /// A position among fallbacks: the advice replaces that element, not the whole
    /// list (`sink = "Headphones"` would drop the fallbacks)
    #[test]
    fn test_positional_check_in_a_fallback_list() {
        let config = make_config(
            vec![
                make_sink("a", "Speakers", true),
                make_sink("b", "Headphones", false),
            ],
            vec![make_rule_with_fallbacks("x", None, &["2", "Speakers"])],
        );
        let check = positional_check(&config.positional_refs()).expect("a warning");
        assert_eq!(
            check.hint.as_deref(),
            Some(
                "Use the desc: rule 1: \"2\" → \"Headphones\" (saving the rule in pwsw tui \
                 does that)"
            )
        );
    }

    /// A daemon on ext while the config has `output` rules and the compositor offers
    /// wlr: restart it. Nothing to say when wlr is not offered (the Wayland section
    /// warns) or the daemon already uses wlr.
    #[test]
    fn test_daemon_checks_monitor_rules_need_a_restart() {
        let facts = |backend: Option<&str>| DaemonFacts {
            running: true,
            status: Some(Ok(DaemonStatus {
                version: crate::version_string(),
                protocol: ipc::PROTOCOL_VERSION,
                uptime_secs: 5,
                current_sink: None,
                current_sink_fallback: false,
                backend: backend.map(str::to_string),
            })),
            manager: None,
        };
        let restart_warning = |facts: &DaemonFacts, monitor| {
            daemon_checks(facts, monitor, StartHint::Systemd)
                .checks
                .into_iter()
                .find(|c| c.message.contains("rules with `output`"))
        };
        let wanted = MonitorRules {
            configured: true,
            available: Some(true),
        };
        let warning = restart_warning(&facts(Some("ext")), wanted).expect("a warning");
        assert_eq!(warning.level, Level::Warn);
        assert!(
            warning
                .hint
                .as_deref()
                .is_some_and(|h| h.contains("restart pwsw")),
            "{warning:?}"
        );
        // Could not ask the compositor: still worth saying
        let unknown = MonitorRules {
            available: None,
            ..wanted
        };
        assert!(restart_warning(&facts(Some("ext")), unknown).is_some());

        assert!(restart_warning(&facts(Some("wlr")), wanted).is_none());
        assert!(
            restart_warning(&facts(None), wanted).is_none(),
            "older daemon"
        );
        let ext_only = MonitorRules {
            available: Some(false),
            ..wanted
        };
        assert!(restart_warning(&facts(Some("ext")), ext_only).is_none());
        assert!(restart_warning(&facts(Some("ext")), MonitorRules::default()).is_none());
    }

    #[test]
    fn test_systemd_checks() {
        let q = |stdout: &str, stderr: &str| {
            Some(UnitQuery {
                stdout: stdout.to_string(),
                stderr: stderr.to_string(),
            })
        };
        let levels = |s: Section| s.checks.iter().map(|c| c.level).collect::<Vec<_>>();

        assert_eq!(levels(systemd_checks(None)), [Level::Info]);
        assert_eq!(
            levels(systemd_checks(Some((q("enabled", ""), q("active", ""))))),
            [Level::Ok, Level::Ok]
        );
        assert_eq!(
            levels(systemd_checks(Some((q("enabled", ""), q("failed", ""))))),
            [Level::Ok, Level::Fail]
        );
        assert_eq!(
            levels(systemd_checks(Some((q("enabled", ""), q("inactive", ""))))),
            [Level::Ok, Level::Warn]
        );
        assert_eq!(
            levels(systemd_checks(Some((q("disabled", ""), q("inactive", ""))))),
            [Level::Info, Level::Info]
        );
        let missing = systemd_checks(Some((
            q(
                "",
                "Failed to get unit file state for pwsw.service: No such file or directory",
            ),
            q("inactive", ""),
        )));
        assert!(missing.checks[0].message.contains("not installed"));
        assert_eq!(missing.checks.len(), 1);
        for stderr in [
            "Failed to connect to bus: No medium found",
            "Failed to connect to user scope bus via local transport: No such file or directory",
        ] {
            let no_bus = systemd_checks(Some((q("", stderr), q("", ""))));
            assert!(
                no_bus.checks[0]
                    .message
                    .contains("not reachable: Failed to connect"),
                "{no_bus:?}"
            );
        }
        let not_found = systemd_checks(Some((q("not-found", ""), q("inactive", ""))));
        assert!(not_found.checks[0].message.contains("not installed"));
        assert_eq!(levels(systemd_checks(Some((None, None)))), [Level::Warn]);
    }

    #[test]
    fn test_log_checks_find_the_last_problem() {
        let log = "\
2026-11-20T10:00:00.000001Z  INFO pwsw::daemon: Starting
2026-11-20T10:00:01.000001Z ERROR pwsw::daemon: first
2026-11-20T10:00:02.000001Z  WARN pwsw::state: Could not \u{1b}[31mquery\u{1b}[0m default sink
2026-11-20T10:00:03.000001Z  INFO pwsw::daemon: fine
";
        assert_eq!(
            last_problem(log),
            Some((
                "WARN",
                "2026-11-20T10:00:02.000001Z",
                "pwsw::state: Could not \u{1b}[31mquery\u{1b}[0m default sink"
            ))
        );
        let path = PathBuf::from("/d/daemon.log");
        let section = log_checks(Some(&path), Some(log));
        let messages: Vec<&str> = section.checks.iter().map(|c| c.message.as_str()).collect();
        assert_eq!(messages, [
            "Log file: /d/daemon.log",
            "Last warning (2026-11-20 10:00:02 UTC): pwsw::state: Could not query default sink",
        ]);
        assert_eq!(
            log_checks(Some(&path), Some("x INFO fine\n")).checks[1].message,
            "No warnings or errors in the log"
        );
        assert_eq!(
            log_checks(Some(&path), None).checks[1].message,
            "No log yet (the daemon has not run)"
        );
    }
}
