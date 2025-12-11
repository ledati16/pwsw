//! Command-line interface definitions
//!
//! Uses clap for argument parsing with derive macros.

use clap::Parser;

/// PWSW - PipeWire Switcher
///
/// Automatically switch audio sinks based on active windows.
#[derive(Parser)]
#[command(name = "pwsw")]
#[command(version)]
#[command(about = "PipeWire Switcher - Automatically switch audio sinks based on active windows")]
#[command(after_help = "\
BEHAVIOR:
  - On startup, queries the current system default sink
  - When a matching window opens, switches to that rule's sink
  - When multiple windows match rules, the most recently opened takes priority
  - When all matching windows close, returns to the default sink
  - Supports profile switching for analog/digital outputs on the same card

ONE-SHOT COMMANDS:
  --check-config     Validate configuration and view settings
  --set-sink SINK    Switch audio output (toggles back to default if already active)
  --next-sink        Cycle to next configured sink
  --prev-sink        Cycle to previous configured sink
  --get-sink         Display current output (add --json for icon)
  --list-sinks       Discover available audio outputs including inactive profiles

SINK REFERENCES:
  Sinks can be referenced by description, node name, or position (1, 2, 3...).

PIPEWIRE INTEGRATION:
  Uses pw-dump for JSON queries, pw-metadata for setting defaults.
  Supports profile switching via pw-cli for analog/digital outputs.
  Node names are stable across reboots (unlike numeric IDs).

  Sinks marked with ~ require profile switching to activate.")]
pub struct Args {
    /// Validate configuration file and exit
    #[arg(long, group = "command")]
    pub check_config: bool,

    /// List available audio sinks (including those requiring profile switch)
    #[arg(long, group = "command")]
    pub list_sinks: bool,

    /// Set the default sink (by desc, node name, or position)
    #[arg(long, value_name = "SINK", group = "command")]
    pub set_sink: Option<String>,

    /// Get current default sink (plain text, or JSON with --json for icon)
    #[arg(long, group = "command")]
    pub get_sink: bool,

    /// Switch to next configured sink (wraps around)
    #[arg(long, group = "command")]
    pub next_sink: bool,

    /// Switch to previous configured sink (wraps around)
    #[arg(long, group = "command")]
    pub prev_sink: bool,

    /// Output in JSON format (for --get-sink and --list-sinks)
    #[arg(long)]
    pub json: bool,
}
