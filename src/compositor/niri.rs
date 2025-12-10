//! Niri compositor implementation
//!
//! Connects to Niri via its Unix socket IPC and parses window events
//! from the EventStream.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::env;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;
use tracing::{info, trace};

use super::{Compositor, WindowEvent};

/// Niri IPC event from EventStream
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
enum NiriEvent {
    WindowOpenedOrChanged { window: WindowProps },
    WindowClosed { id: u64 },
    #[serde(untagged)]
    Other(#[allow(dead_code)] serde_json::Value),
}

#[derive(Debug, Deserialize)]
struct WindowProps {
    id: u64,
    title: String,
    app_id: String,
}

/// Niri compositor implementation
pub struct NiriCompositor {
    socket_path: String,
    lines: Option<Lines<BufReader<OwnedReadHalf>>>,
    #[allow(dead_code)]
    writer: Option<OwnedWriteHalf>,
}

impl NiriCompositor {
    /// Create a new Niri compositor instance
    ///
    /// Reads NIRI_SOCKET from environment.
    pub fn new() -> Result<Self> {
        let socket_path = env::var("NIRI_SOCKET")
            .context("NIRI_SOCKET not set. Is Niri running?")?;

        Ok(Self {
            socket_path,
            lines: None,
            writer: None,
        })
    }
}

impl Compositor for NiriCompositor {
    fn name(&self) -> &'static str {
        "Niri"
    }

    async fn connect(&mut self) -> Result<()> {
        info!("Connecting to Niri: {}", self.socket_path);

        let stream = UnixStream::connect(&self.socket_path).await
            .with_context(|| format!("Failed to connect to Niri socket: {}", self.socket_path))?;

        let (reader, mut writer) = stream.into_split();

        // Request event stream
        writer.write_all(b"\"EventStream\"\n").await?;

        self.lines = Some(BufReader::new(reader).lines());
        self.writer = Some(writer);

        Ok(())
    }

    async fn next_event(&mut self) -> Result<Option<WindowEvent>> {
        let lines = self.lines.as_mut()
            .context("Not connected to Niri")?;

        loop {
            match lines.next_line().await? {
                Some(line) => {
                    // Try to parse as a window event
                    match serde_json::from_str::<NiriEvent>(&line) {
                        Ok(NiriEvent::WindowOpenedOrChanged { window }) => {
                            // Niri combines open and change into one event type
                            // We'll treat all as potential opens/changes and let state handle it
                            return Ok(Some(WindowEvent::Opened {
                                id: window.id,
                                app_id: window.app_id,
                                title: window.title,
                            }));
                        }
                        Ok(NiriEvent::WindowClosed { id }) => {
                            return Ok(Some(WindowEvent::Closed { id }));
                        }
                        Ok(NiriEvent::Other(_)) => {
                            // Skip non-window events
                            trace!("Skipping non-window event");
                            continue;
                        }
                        Err(e) => {
                            trace!("Skipping unknown event: {} ({})",
                                   line.chars().take(50).collect::<String>(), e);
                            continue;
                        }
                    }
                }
                None => return Ok(None), // Stream ended
            }
        }
    }
}
