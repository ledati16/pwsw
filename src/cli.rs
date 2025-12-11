//! Command-line interface definitions
//!
//! Uses clap for argument parsing with derive macros.

use clap::{Parser, Subcommand};

/// PWSW - PipeWire Switcher
///
/// Automatically switch audio sinks based on active windows.
#[derive(Parser)]
#[command(name = "pwsw")]
#[command(version)]
#[command(about = "PipeWire Switcher - Automatically switch audio sinks based on active windows")]
#[command(after_help = "\
BEHAVIOR:
  - The daemon monitors window events and switches audio sinks based on configured rules
  - When a matching window opens, switches to that rule's sink
  - When multiple windows match rules, the most recently opened takes priority
  - When all matching windows close, returns to the default sink
  - Supports profile switching for analog/digital outputs on the same card

DAEMON MANAGEMENT:
  pwsw daemon              Run the daemon in background (detached)
  pwsw daemon --foreground Run in foreground with logs to stderr
  pwsw status              Query daemon status (or just: pwsw)
  pwsw reload              Tell daemon to reload config
  pwsw shutdown            Gracefully stop the daemon

QUERY COMMANDS:
  pwsw list-sinks          List available PipeWire sinks
  pwsw list-windows        Show windows currently tracked by daemon
  pwsw validate            Validate config file (local, no daemon needed)

TEST COMMANDS:
  pwsw test-rule PATTERN   Test a regex pattern against current windows

FUTURE:
  pwsw tui                 Terminal UI (not yet implemented)

IPC SOCKET:
  $XDG_RUNTIME_DIR/pwsw.sock (or /tmp/pwsw-$UID.sock)

PIPEWIRE INTEGRATION:
  Uses pw-dump for JSON queries, pw-metadata for setting defaults.
  Supports profile switching via pw-cli for analog/digital outputs.
  Node names are stable across reboots (unlike numeric IDs).")]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Available subcommands
#[derive(Subcommand)]
pub enum Command {
    /// Run the daemon (monitors windows and switches audio)
    Daemon {
        /// Run in foreground with logs to stderr
        #[arg(short, long)]
        foreground: bool,
    },
    
    /// Query daemon status via IPC
    Status {
        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },
    
    /// Tell daemon to reload config file
    Reload,
    
    /// Gracefully shutdown the daemon
    Shutdown,
    
    /// List available PipeWire sinks
    ListSinks {
        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },
    
    /// Show windows currently tracked by daemon
    ListWindows {
        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },
    
    /// Validate config file (local, no daemon needed)
    Validate,
    
    /// Test a regex pattern against current windows
    TestRule {
        /// The regex pattern to test
        pattern: String,
        
        /// Output in JSON format
        #[arg(long)]
        json: bool,
    },
    
    /// Terminal UI (not yet implemented)
    Tui,
}
