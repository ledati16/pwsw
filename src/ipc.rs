//! IPC infrastructure for daemon communication
//!
//! Provides Unix socket-based IPC for CLI commands to communicate with the daemon.
//! Uses length-prefixed JSON messages for protocol framing.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, error, warn};

// ============================================================================
// Message Types
// ============================================================================

/// Requests sent from CLI to daemon
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Request {
    /// Query daemon status
    Status,
    /// Get list of currently tracked windows
    ListWindows,
    /// Test a rule pattern against current windows
    TestRule { pattern: String },
    /// Gracefully shutdown the daemon
    Shutdown,
}

/// Responses sent from daemon to CLI
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Response {
    /// Status information
    Status {
        version: String,
        uptime_secs: u64,
        current_sink: String,
        active_window: Option<String>,
        tracked_windows: usize,
    },
    /// Generic success response
    Ok { message: String },
    /// Error response
    Error { message: String },
    /// List of tracked windows
    Windows { windows: Vec<WindowInfo> },
    /// Rule test results
    RuleMatches {
        pattern: String,
        matches: Vec<WindowInfo>,
    },
}

/// Window information for IPC responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    pub app_id: String,
    pub title: String,
    /// For test-rule: which fields matched ("app_id", "title", or "both")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_on: Option<String>,
    /// For list-windows: tracking status and sink info
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tracked: Option<TrackedInfo>,
}

/// Information about a tracked window (matched a rule)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackedInfo {
    pub sink_name: String,
    pub sink_desc: String,
}

// ============================================================================
// Socket Path Management
// ============================================================================

/// Get the IPC socket path
/// Prefers $XDG_RUNTIME_DIR/pwsw.sock, falls back to /tmp/pwsw-$UID.sock
pub fn get_socket_path() -> Result<PathBuf> {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        Ok(PathBuf::from(runtime_dir).join("pwsw.sock"))
    } else {
        // Fallback to /tmp with UID for security (parsed from XDG_RUNTIME_DIR if available,
        // otherwise use a fixed safe location)
        // XDG_RUNTIME_DIR typically has the pattern /run/user/$UID
        if let Ok(user) = std::env::var("USER") {
            Ok(PathBuf::from(format!("/tmp/pwsw-{}.sock", user)))
        } else {
            // Last resort: use process ID (not ideal but better than world-writable)
            let pid = std::process::id();
            Ok(PathBuf::from(format!("/tmp/pwsw-{}.sock", pid)))
        }
    }
}

/// Check if a daemon is currently running
/// Returns true if a daemon is active (socket exists and accepts connections)
pub async fn is_daemon_running() -> bool {
    let socket_path = match get_socket_path() {
        Ok(path) => path,
        Err(_) => return false,
    };

    if !socket_path.exists() {
        return false;
    }

    // Try to connect - if it succeeds, daemon is running
    let connect_result = tokio::time::timeout(
        Duration::from_millis(100),
        tokio::net::UnixStream::connect(&socket_path)
    ).await;

    matches!(connect_result, Ok(Ok(_)))
}

/// Clean up stale socket file
/// Checks if socket exists and if daemon is actually running
pub async fn cleanup_stale_socket() -> Result<()> {
    let socket_path = get_socket_path()?;

    if !socket_path.exists() {
        return Ok(());
    }

    // Try to connect - if it fails, the socket is stale
    let connect_result = tokio::time::timeout(
        Duration::from_millis(100),
        tokio::net::UnixStream::connect(&socket_path)
    ).await;

    let is_stale = match connect_result {
        Ok(Ok(_stream)) => {
            // Successfully connected - socket is alive
            false
        }
        Ok(Err(_connect_err)) => {
            // Connection failed - socket is stale
            true
        }
        Err(_timeout) => {
            // Timeout - assume socket is stale
            true
        }
    };

    if is_stale {
        debug!("Removing stale socket: {:?}", socket_path);
        std::fs::remove_file(&socket_path)
            .with_context(|| format!("Failed to remove stale socket: {:?}", socket_path))?;
    }

    Ok(())
}

// ============================================================================
// Protocol Helpers
// ============================================================================

const MAX_MESSAGE_SIZE: usize = 1024 * 1024; // 1MB max message size
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Read a length-prefixed JSON message from a stream
async fn read_message<T: for<'de> Deserialize<'de>>(
    stream: &mut UnixStream,
) -> Result<T> {
    // Read 4-byte big-endian length prefix
    let mut len_buf = [0u8; 4];
    tokio::time::timeout(READ_TIMEOUT, stream.read_exact(&mut len_buf))
        .await
        .context("Timeout reading message length")?
        .context("Failed to read message length")?;
    
    let msg_len = u32::from_be_bytes(len_buf) as usize;
    
    if msg_len > MAX_MESSAGE_SIZE {
        anyhow::bail!("Message too large: {} bytes (max: {})", msg_len, MAX_MESSAGE_SIZE);
    }
    
    // Read the JSON payload
    let mut msg_buf = vec![0u8; msg_len];
    tokio::time::timeout(READ_TIMEOUT, stream.read_exact(&mut msg_buf))
        .await
        .context("Timeout reading message payload")?
        .context("Failed to read message payload")?;
    
    // Deserialize JSON
    serde_json::from_slice(&msg_buf)
        .context("Failed to deserialize message")
}

/// Write a length-prefixed JSON message to a stream
async fn write_message<T: Serialize>(
    stream: &mut UnixStream,
    message: &T,
) -> Result<()> {
    // Serialize to JSON
    let json = serde_json::to_vec(message)
        .context("Failed to serialize message")?;
    
    if json.len() > MAX_MESSAGE_SIZE {
        anyhow::bail!("Message too large: {} bytes (max: {})", json.len(), MAX_MESSAGE_SIZE);
    }
    
    // Write length prefix (4 bytes big-endian)
    let len = (json.len() as u32).to_be_bytes();
    stream.write_all(&len).await
        .context("Failed to write message length")?;
    
    // Write JSON payload
    stream.write_all(&json).await
        .context("Failed to write message payload")?;
    
    stream.flush().await
        .context("Failed to flush stream")?;
    
    Ok(())
}

// ============================================================================
// IPC Client (for CLI commands)
// ============================================================================

/// Send a request to the daemon and wait for response
pub async fn send_request(request: Request) -> Result<Response> {
    let socket_path = get_socket_path()?;
    
    // Connect to daemon
    let mut stream = tokio::time::timeout(
        Duration::from_secs(5),
        UnixStream::connect(&socket_path),
    )
    .await
    .context("Timeout connecting to daemon")?
    .with_context(|| {
        format!(
            "Failed to connect to daemon. Is the daemon running?\nSocket: {:?}",
            socket_path
        )
    })?;
    
    debug!("Connected to daemon at {:?}", socket_path);
    
    // Send request
    write_message(&mut stream, &request).await?;
    
    // Read response
    let response: Response = read_message(&mut stream).await?;
    
    Ok(response)
}

// ============================================================================
// IPC Server (for daemon)
// ============================================================================

/// Handle for the IPC server running in the daemon
pub struct IpcServer {
    listener: UnixListener,
    socket_path: PathBuf,
}

impl IpcServer {
    /// Create and bind a new IPC server
    pub async fn bind() -> Result<Self> {
        let socket_path = get_socket_path()?;
        
        // Clean up any stale socket
        cleanup_stale_socket().await?;
        
        // Bind the listener
        let listener = UnixListener::bind(&socket_path)
            .with_context(|| format!("Failed to bind IPC socket: {:?}", socket_path))?;
        
        debug!("IPC server listening on {:?}", socket_path);
        
        Ok(Self {
            listener,
            socket_path,
        })
    }
    
    /// Accept the next incoming connection
    /// Returns None if accept fails (non-fatal)
    pub async fn accept(&self) -> Option<UnixStream> {
        match self.listener.accept().await {
            Ok((stream, _addr)) => Some(stream),
            Err(e) => {
                error!("Failed to accept IPC connection: {}", e);
                None
            }
        }
    }
    
    /// Get the socket path
    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        // Clean up socket on drop
        if let Err(e) = std::fs::remove_file(&self.socket_path) {
            warn!("Failed to remove IPC socket on shutdown: {}", e);
        } else {
            debug!("Removed IPC socket: {:?}", self.socket_path);
        }
    }
}

/// Read a request from a client connection
pub async fn read_request(stream: &mut UnixStream) -> Result<Request> {
    read_message(stream).await
}

/// Write a response to a client connection
pub async fn write_response(stream: &mut UnixStream, response: &Response) -> Result<()> {
    write_message(stream, response).await
}
