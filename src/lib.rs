//! `PWSW` - `PipeWire` Switcher
//!
//! Automatically switches audio sinks based on active windows in Wayland compositors.
//! Uses `PipeWire` native tools (`pw-dump`, `pw-metadata`, `pw-cli`) for audio control.
//!
//! # Features
//! - Automatic sink switching based on window rules
//! - Profile switching for analog/digital outputs on the same card
//! - Status bar integration with JSON output
//! - Smart toggle between configured sinks
//! - Multi-compositor support via Wayland protocols (wlr-foreign-toplevel, plasma-window-management)
//!
//! # Supported Compositors
//! - Sway, Hyprland, Niri, River, Wayfire, labwc, dwl, hikari (via wlr-foreign-toplevel)
//! - KDE Plasma/KWin (via plasma-window-management)

pub mod cli;
pub mod config;
pub mod pipewire;
pub mod notification;
pub mod state;
pub mod commands;
pub mod daemon;
pub mod compositor;
pub mod ipc;

// Re-export commonly used types for convenience
pub use cli::Args;
pub use config::Config;
pub use state::State;
