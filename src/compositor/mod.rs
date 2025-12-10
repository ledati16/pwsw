//! Compositor abstraction layer
//!
//! Provides a trait-based interface for different Wayland compositors.
//! Currently supports Niri and Sway, with Hyprland planned.

mod niri;
mod sway;

pub use niri::NiriCompositor;
pub use sway::SwayCompositor;

use anyhow::{Context, Result};
use std::env;

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

// ============================================================================
// Compositor Enum (for runtime dispatch)
// ============================================================================

/// Enum wrapper for compositor implementations
///
/// Since async traits aren't dyn-compatible, we use an enum for runtime dispatch.
/// This is efficient (no heap allocation) and type-safe.
pub enum AnyCompositor {
    Niri(NiriCompositor),
    Sway(SwayCompositor),
}

impl AnyCompositor {
    /// Get the compositor name for logging
    pub fn name(&self) -> &'static str {
        match self {
            AnyCompositor::Niri(c) => c.name(),
            AnyCompositor::Sway(c) => c.name(),
        }
    }

    /// Connect to the compositor's IPC socket
    pub async fn connect(&mut self) -> Result<()> {
        match self {
            AnyCompositor::Niri(c) => c.connect().await,
            AnyCompositor::Sway(c) => c.connect().await,
        }
    }

    /// Get the next window event
    pub async fn next_event(&mut self) -> Result<Option<WindowEvent>> {
        match self {
            AnyCompositor::Niri(c) => c.next_event().await,
            AnyCompositor::Sway(c) => c.next_event().await,
        }
    }
}

/// Detect which compositor is running and create the appropriate instance
///
/// Checks environment variables to determine the running compositor:
/// - NIRI_SOCKET → Niri
/// - SWAYSOCK → Sway
pub fn detect() -> Result<AnyCompositor> {
    // Check for Niri first (more specific)
    if env::var("NIRI_SOCKET").is_ok() {
        let compositor = NiriCompositor::new()?;
        return Ok(AnyCompositor::Niri(compositor));
    }

    // Check for Sway
    if env::var("SWAYSOCK").is_ok() {
        let compositor = SwayCompositor::new()?;
        return Ok(AnyCompositor::Sway(compositor));
    }

    // No supported compositor found
    Err(anyhow::anyhow!(
        "No supported compositor detected.\n\
         Set NIRI_SOCKET (Niri) or SWAYSOCK (Sway) environment variable.\n\
         Supported compositors: Niri, Sway"
    ))
    .context("Compositor detection failed")
}
