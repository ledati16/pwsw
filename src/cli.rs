//! Command-line interface definitions
//!
//! Uses clap for argument parsing with derive macros.

use clap::{Parser, Subcommand};
use clap_complete::Shell;

use crate::style::ColorChoice;

/// `PWSW` - `PipeWire` Switcher
///
/// Automatically switch audio sinks based on active windows.
#[derive(Parser)]
#[command(name = "pwsw")]
#[command(version = crate::VERSION)]
#[command(about = "PipeWire Switcher - Automatically switch audio sinks based on active windows")]
#[command(long_about = None)]
#[command(after_help = "\
GETTING STARTED:
  1. pwsw init             Create a commented config (or use: pwsw tui)
  2. pwsw list-sinks       See available audio outputs
  3. pwsw validate         Check config file syntax
  4. pwsw daemon           Start the daemon
  5. pwsw                  Check status

DAEMON CONTROL:
  daemon               Start daemon in background
  daemon --foreground  Start with logs visible (for debugging)
  shutdown             Stop the daemon

PAUSING (requires running daemon):
  pause               Stop rule-driven switching (manual switches still work)
  resume              Resume it, switching at once if the rules want another sink
  toggle-pause        Pause or resume

QUERYING (requires running daemon):
  list-windows        Show all open windows (tracked vs untracked)
  test-rule PATTERN   Test regex against windows (checks app_id & title;
                      --output REGEX also or only matches the monitor)

QUERYING (no daemon needed):
  [no subcommand]     Show current audio output and daemon status
  status              Same as above (supports --json)
  status --watch      Follow the daemon's status, one line per change (--json
                      for status bars; keeps running while the daemon is down)
  list-sinks          List available PipeWire audio outputs
  validate            Check config file syntax
  init [--force]      Write the commented default config

MANUAL CONTROL (through the daemon when it runs, otherwise directly):
  set-sink SINK       Switch to specific sink (by desc or node name)
  next-sink           Cycle to next configured sink (wraps around)
  prev-sink           Cycle to previous configured sink (wraps around)
  set-source SOURCE   Switch the microphone (needs [[sources]] in the config)

HOW IT WORKS:
  The daemon monitors Wayland windows and switches audio outputs based on
  rules in your config file; a rule matches a window's app_id, title and/or
  the monitor it is on (output). The most recently opened matching window
  wins (or the first matching rule, with match_by_index). When all matching
  windows close, it returns to the default output. With [[sources]], rules
  can switch the microphone the same way.

CONFIG & IPC:
  Config: $XDG_CONFIG_HOME/pwsw/config.toml (normally ~/.config/pwsw/config.toml)
          (override with --config PATH or the PWSW_CONFIG env var)
  Socket: $XDG_RUNTIME_DIR/pwsw.sock

  The daemon picks up config changes automatically when the file is saved.

  Shell completions: pwsw completions <bash|zsh|fish|elvish|powershell>")]
pub struct Args {
    /// Use this config file instead of `$XDG_CONFIG_HOME/pwsw/config.toml`
    /// (overrides `PWSW_CONFIG`)
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<std::path::PathBuf>,

    /// When to use colours (`auto` honours `NO_COLOR` and `CLICOLOR_FORCE`)
    #[arg(long, global = true, value_name = "WHEN", value_enum, default_value_t)]
    pub color: ColorChoice,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Available subcommands
#[derive(Subcommand)]
pub enum Command {
    /// Start the daemon (background by default)
    Daemon {
        /// Run in foreground with logs visible
        #[arg(short, long)]
        foreground: bool,
    },

    /// Show the current audio output and daemon status (no daemon needed)
    Status {
        /// Output in `JSON` format (always valid `JSON`, errors included; see pwsw(1))
        #[arg(long)]
        json: bool,
        /// Follow the daemon's status: print a line now and on every change (with
        /// `--json`, one `JSON` object per line for status bars; see STATUS STREAM in
        /// pwsw(1)). Keeps running while the daemon is stopped.
        #[arg(long)]
        watch: bool,
    },

    /// Stop the daemon gracefully
    Shutdown,

    /// List available `PipeWire` audio outputs, then input devices (no daemon needed)
    ListSinks {
        /// Output in `JSON` format
        #[arg(long)]
        json: bool,
    },

    /// Show all open windows, tracked (matching a rule) or not (requires daemon)
    ListWindows {
        /// Output in `JSON` format
        #[arg(long)]
        json: bool,
    },

    /// Check the config file (syntax, sinks, rules, regexes); does not create one
    Validate,

    /// Write the commented default config file (refuses to overwrite without `--force`)
    Init {
        /// Replace an existing config (the old file is kept as `<name>.bak`)
        #[arg(long)]
        force: bool,
    },

    /// Test a regex against all open windows' `app_id` and title (requires daemon)
    TestRule {
        /// Regex pattern to test against `app_id` and title
        #[arg(required_unless_present = "output")]
        pattern: Option<String>,

        /// Also (or only) require a monitor matching this regex, like a rule's `output`
        #[arg(long, value_name = "REGEX")]
        output: Option<String>,

        /// Output in `JSON` format
        #[arg(long)]
        json: bool,
    },

    /// Set audio output (by desc or node name; positions like "2" are deprecated)
    ///
    /// With `set_smart_toggle`, asking for the sink that is already active switches
    /// back to the default sink. Goes through the daemon when it runs.
    SetSink {
        /// Sink reference (description or node name; a position still works but is deprecated)
        sink: String,
    },

    /// Set the default microphone to a configured source (by desc or node name)
    ///
    /// Needs `[[sources]]` in the config. With `set_smart_toggle`, asking for the
    /// source that is already active switches back to the default source. Goes through
    /// the daemon when it runs (the choice holds until the rules want another source).
    SetSource {
        /// Source reference (description or node name; a position still works but is
        /// deprecated)
        source: String,
    },

    /// Cycle to the next configured sink (wraps around)
    NextSink,

    /// Cycle to the previous configured sink (wraps around)
    PrevSink,

    // --- Pause / resume (requires daemon) ---
    /// Stop rule-driven switching until `resume` (requires daemon)
    ///
    /// Windows are still tracked and manual switches still work. The pause lasts until
    /// `resume` or a daemon restart.
    Pause,

    /// Resume rule-driven switching (requires daemon)
    ///
    /// Switches at once if the rules want a different sink than the current one, also
    /// over a manual choice.
    Resume,

    /// Pause when running, resume when paused (requires daemon)
    TogglePause,
    // --- end pause / resume ---
    /// Terminal UI for configuration and monitoring
    Tui,

    /// Check the setup (Wayland, `PipeWire`, config, sinks, daemon, systemd, log) and say
    /// what to fix; changes nothing. Exits 1 if anything is broken
    Doctor,

    /// Print a shell completion script to stdout
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
}
