//! `PWSW` - `PipeWire` Switcher
//!
//! Automatically switches audio sinks based on active windows in Wayland compositors.
//! Uses `PipeWire` native tools (`pw-dump`, `pw-metadata`, `pw-cli`) for audio control.
//!
//! # Features
//! - Automatic sink switching based on window rules
//! - Profile switching for analog/digital outputs on the same card
//! - Status bar integration with `JSON` output
//! - Smart toggle between configured sinks
//! - Multi-compositor support via Wayland protocols (`ext-foreign-toplevel-list-v1`, `wlr-foreign-toplevel`)
//!
//! # Supported Compositors
//! - Any compositor implementing `ext-foreign-toplevel-list-v1` (preferred) or
//!   `wlr-foreign-toplevel-management`, e.g. Sway, Hyprland, niri, river, Wayfire, labwc
//!
//! # Unsupported Compositors
//! - GNOME/Mutter, KDE Plasma 6 (no supported window management protocols exposed)

pub mod cli;
pub mod commands;
pub mod compositor;
pub mod config;
pub mod daemon;
pub mod daemon_manager;
pub mod ipc;
pub mod logging;
pub mod manual;
pub mod notification;
pub mod pipewire;
pub mod sanitize;
pub mod state;
pub mod style;

#[cfg(feature = "tui")]
pub mod tui;

// Test-only utilities
#[cfg(test)]
pub mod test_utils;

/// Full version: crate version, plus `+<short commit>` and `-dirty` when built from a
/// git checkout (e.g. `0.7.0+abc1234-dirty`; just `0.7.0` without git information).
/// Set by `build.rs`; printed by `pwsw --version`.
pub const VERSION: &str = env!("PWSW_VERSION");

/// The version as the daemon and TUI report it: [`VERSION`] with a `v` prefix
/// (e.g. `v0.7.0+abc1234`)
#[must_use]
pub fn version_string() -> String {
    format!("v{VERSION}")
}

/// Format uptime in human-readable form (e.g., "1h 30m", "45s")
#[must_use]
pub fn format_uptime(secs: u64) -> String {
    const SECS_PER_MINUTE: u64 = 60;
    const SECS_PER_HOUR: u64 = 3600;

    if secs < SECS_PER_MINUTE {
        return format!("{secs}s");
    }
    if secs < SECS_PER_HOUR {
        return format!("{mins}m", mins = secs / SECS_PER_MINUTE);
    }
    let hours = secs / SECS_PER_HOUR;
    let mins = (secs % SECS_PER_HOUR) / SECS_PER_MINUTE;
    if mins > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{hours}h")
    }
}

// Re-export commonly used types for convenience
pub use cli::Args;
pub use config::Config;
pub use state::State;
