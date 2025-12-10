//! Compositor abstraction layer
//!
//! Provides a trait-based interface for different Wayland compositors.
//! Currently supports Niri, with Sway and Hyprland planned.

mod niri;

pub use niri::NiriCompositor;

use anyhow::Result;

/// Window event from a compositor
#[derive(Debug, Clone)]
pub enum WindowEvent {
    /// A new window was opened
    Opened {
        id: u64,
        app_id: String,
        title: String,
    },
    /// An existing window's properties changed
    Changed {
        id: u64,
        app_id: String,
        title: String,
    },
    /// A window was closed
    Closed {
        id: u64,
    },
}

/// Trait for compositor implementations
///
/// Each compositor (Niri, Sway, Hyprland) implements this trait
/// to provide window event streams in a unified format.
#[allow(async_fn_in_trait)] // We control all implementations
pub trait Compositor {
    /// Get the compositor name for logging
    fn name(&self) -> &'static str;

    /// Connect to the compositor's IPC socket
    async fn connect(&mut self) -> Result<()>;

    /// Get the next window event (async)
    ///
    /// Returns Ok(None) when the event stream ends.
    async fn next_event(&mut self) -> Result<Option<WindowEvent>>;
}
