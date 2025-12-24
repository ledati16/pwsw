//! Command-line interface definitions
//!
//! Uses clap for argument parsing with derive macros.

use clap::{Parser, Subcommand};

/// `PWSW` - `PipeWire` Switcher
///
/// Automatically switch audio sinks based on active windows.
#[derive(Parser)]
#[command(name = "pwsw")]
#[command(version)]
#[command(about = "PipeWire Switcher - Automatically switch audio sinks based on active windows")]
#[command(after_help = "\
GETTING STARTED:
  1. pwsw validate         Check config file syntax
  2. pwsw list-sinks       See available audio outputs
  3. pwsw daemon           Start the daemon
  4. pwsw                  Check status

DAEMON CONTROL:
  daemon               Start daemon in background
  daemon --foreground  Start with logs visible (for debugging)
  shutdown             Stop the daemon

QUERYING (requires running daemon):
  [no subcommand]     Show status, uptime, and current audio output
  status              Same as above (supports --json)
  list-windows        Show all open windows (tracked vs untracked)
  test-rule PATTERN   Test regex against windows (checks app_id & title)

QUERYING (no daemon needed):
  list-sinks          List available PipeWire audio outputs
  validate            Check config file syntax

MANUAL SINK CONTROL (no daemon needed):
  set-sink SINK       Switch to specific sink (by desc, name, or position 1/2/3)
  next-sink           Cycle to next configured sink (wraps around)
  prev-sink           Cycle to previous configured sink (wraps around)

HOW IT WORKS:
  The daemon monitors Wayland windows and switches audio outputs based on
  rules in your config file. Most recently opened matching window wins.
  When all matching windows close, returns to default output.

CONFIG & IPC:
  Config: ~/.config/pwsw/config.toml
  Socket: $XDG_RUNTIME_DIR/pwsw.sock

  After editing config, restart the daemon:
    pwsw shutdown && pwsw daemon")]
pub struct Args {
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

    /// Show daemon status, uptime, and current audio output
    Status {
        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },

    /// Stop the daemon gracefully
    Shutdown,

    /// List available `PipeWire` audio outputs
    ListSinks {
        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },

    /// Show all open windows (tracked vs untracked)
    ListWindows {
        /// Output in `JSON` format
        #[arg(long)]
        json: bool,
    },

    /// Check config file syntax (no daemon needed)
    Validate,

    /// Test regex pattern against all windows (`app_id` & title)
    TestRule {
        /// Regex pattern to test
        pattern: String,

        /// Output in `JSON` format
        #[arg(long)]
        json: bool,
    },

    /// Set audio output (by desc, node name, or position like "1", "2")
    SetSink {
        /// Sink reference (description, node name, or position)
        sink: String,
    },

    /// Cycle to next configured sink
    NextSink,

    /// Cycle to previous configured sink
    PrevSink,

    /// Terminal UI for configuration and monitoring
    Tui,
}
