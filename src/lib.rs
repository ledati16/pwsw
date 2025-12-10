//! NASW - Niri Audio Switcher
//!
//! Automatically switches audio sinks based on active windows in Wayland compositors.
//! Uses PipeWire native tools (pw-dump, pw-metadata, pw-cli) for audio control.
//!
//! # Features
//! - Automatic sink switching based on window rules
//! - Profile switching for analog/digital outputs on the same card
//! - Status bar integration with JSON output
//! - Smart toggle between configured sinks
//! - Multi-compositor support (Niri, with Sway/Hyprland planned)

pub mod cli;
pub mod config;
pub mod pipewire;
pub mod notification;
pub mod state;
pub mod commands;
pub mod daemon;
pub mod compositor;

// Re-export commonly used types for convenience
pub use cli::Args;
pub use config::Config;
pub use state::State;
