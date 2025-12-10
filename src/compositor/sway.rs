//! Sway compositor implementation
//!
//! Connects to Sway via its i3-ipc compatible Unix socket and parses window events.
//! Handles both native Wayland windows (app_id) and XWayland windows (window class).

use anyhow::{Context, Result};
use serde::Deserialize;
use std::env;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tracing::{debug, info, trace};

use super::{Compositor, WindowEvent};

// ============================================================================
// i3-ipc Protocol Constants
// ============================================================================

const IPC_MAGIC: &[u8] = b"i3-ipc";

// Message types
const IPC_SUBSCRIBE: u32 = 2;

// Event types (high bit set)
const IPC_EVENT_WINDOW: u32 = 0x80000003;

// ============================================================================
// Sway IPC JSON Structures
// ============================================================================

/// Window event from Sway
#[derive(Debug, Deserialize)]
struct SwayWindowEvent {
    change: String,
    container: SwayContainer,
}

/// Container (window) properties from Sway
#[derive(Debug, Deserialize)]
struct SwayContainer {
    id: i64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    window_properties: Option<WindowProperties>,
}

/// X11 window properties (XWayland only)
#[derive(Debug, Deserialize)]
struct WindowProperties {
    #[serde(default)]
    class: Option<String>,
    #[serde(default)]
    instance: Option<String>,
    #[serde(default)]
    title: Option<String>,
}

/// Subscribe response
#[derive(Debug, Deserialize)]
struct SubscribeResponse {
    success: bool,
}

impl SwayContainer {
    /// Get the effective app_id (native Wayland) or class (XWayland)
    fn get_app_id(&self) -> String {
        // Native Wayland: use app_id
        if let Some(ref app_id) = self.app_id {
            return app_id.clone();
        }

        // XWayland: use window class as app_id equivalent
        if let Some(ref props) = self.window_properties {
            if let Some(ref class) = props.class {
                return class.clone();
            }
            // Fall back to instance if class not available
            if let Some(ref instance) = props.instance {
                return instance.clone();
            }
        }

        String::new()
    }

    /// Get the window title
    fn get_title(&self) -> String {
        // Prefer top-level name
        if let Some(ref name) = self.name {
            return name.clone();
        }

        // XWayland fallback
        if let Some(ref props) = self.window_properties {
            if let Some(ref title) = props.title {
                return title.clone();
            }
        }

        String::new()
    }
}

// ============================================================================
// Sway Compositor
// ============================================================================

/// Sway compositor implementation using i3-ipc protocol
pub struct SwayCompositor {
    socket_path: String,
    stream: Option<UnixStream>,
}

impl SwayCompositor {
    /// Create a new Sway compositor instance
    ///
    /// Reads SWAYSOCK from environment.
    pub fn new() -> Result<Self> {
        let socket_path = env::var("SWAYSOCK")
            .context("SWAYSOCK not set. Is Sway running?")?;

        Ok(Self {
            socket_path,
            stream: None,
        })
    }

    /// Send an i3-ipc message
    async fn send_message(&mut self, msg_type: u32, payload: &[u8]) -> Result<()> {
        let stream = self.stream.as_mut()
            .context("Not connected to Sway")?;

        // Build message: magic + length + type + payload
        let mut message = Vec::with_capacity(14 + payload.len());
        message.extend_from_slice(IPC_MAGIC);
        message.extend_from_slice(&(payload.len() as u32).to_ne_bytes());
        message.extend_from_slice(&msg_type.to_ne_bytes());
        message.extend_from_slice(payload);

        stream.write_all(&message).await
            .context("Failed to send IPC message")?;

        Ok(())
    }

    /// Receive an i3-ipc message
    async fn recv_message(&mut self) -> Result<(u32, Vec<u8>)> {
        let stream = self.stream.as_mut()
            .context("Not connected to Sway")?;

        // Read header: magic (6) + length (4) + type (4) = 14 bytes
        let mut header = [0u8; 14];
        stream.read_exact(&mut header).await
            .context("Failed to read IPC header")?;

        // Verify magic
        if &header[0..6] != IPC_MAGIC {
            anyhow::bail!("Invalid IPC magic bytes");
        }

        // Parse length and type
        let length = u32::from_ne_bytes([header[6], header[7], header[8], header[9]]) as usize;
        let msg_type = u32::from_ne_bytes([header[10], header[11], header[12], header[13]]);

        // Read payload
        let mut payload = vec![0u8; length];
        if length > 0 {
            stream.read_exact(&mut payload).await
                .context("Failed to read IPC payload")?;
        }

        Ok((msg_type, payload))
    }
}

impl Compositor for SwayCompositor {
    fn name(&self) -> &'static str {
        "Sway"
    }

    async fn connect(&mut self) -> Result<()> {
        info!("Connecting to Sway: {}", self.socket_path);

        let stream = UnixStream::connect(&self.socket_path).await
            .with_context(|| format!("Failed to connect to Sway socket: {}", self.socket_path))?;

        self.stream = Some(stream);

        // Subscribe to window events
        let subscribe_payload = b"[\"window\"]";
        self.send_message(IPC_SUBSCRIBE, subscribe_payload).await?;

        // Read subscribe response
        let (msg_type, payload) = self.recv_message().await?;

        // Response type should be IPC_SUBSCRIBE (2)
        if msg_type != IPC_SUBSCRIBE {
            anyhow::bail!("Unexpected response type: {}", msg_type);
        }

        let response: SubscribeResponse = serde_json::from_slice(&payload)
            .context("Failed to parse subscribe response")?;

        if !response.success {
            anyhow::bail!("Failed to subscribe to window events");
        }

        debug!("Subscribed to Sway window events");
        Ok(())
    }

    async fn next_event(&mut self) -> Result<Option<WindowEvent>> {
        loop {
            let (msg_type, payload) = self.recv_message().await?;

            // Only process window events
            if msg_type != IPC_EVENT_WINDOW {
                trace!("Skipping non-window event type: 0x{:x}", msg_type);
                continue;
            }

            let event: SwayWindowEvent = match serde_json::from_slice(&payload) {
                Ok(e) => e,
                Err(e) => {
                    trace!("Failed to parse window event: {}", e);
                    continue;
                }
            };

            let id = event.container.id as u64;
            let app_id = event.container.get_app_id();
            let title = event.container.get_title();

            match event.change.as_str() {
                "new" => {
                    debug!("Sway window new: id={}, app_id='{}', title='{}'", id, app_id, title);
                    return Ok(Some(WindowEvent::Opened { id, app_id, title }));
                }
                "close" => {
                    debug!("Sway window close: id={}", id);
                    return Ok(Some(WindowEvent::Closed { id }));
                }
                "title" => {
                    debug!("Sway window title: id={}, app_id='{}', title='{}'", id, app_id, title);
                    // Title change - treat as potential state change
                    return Ok(Some(WindowEvent::Changed { id, app_id, title }));
                }
                "focus" => {
                    // Focus events are frequent - only log at trace level
                    trace!("Sway window focus: id={}, app_id='{}'", id, app_id);
                    // We don't emit focus events as they'd cause too much churn
                    // The window is already tracked from "new" event
                    continue;
                }
                other => {
                    trace!("Skipping window change type: {}", other);
                    continue;
                }
            }
        }
    }
}
