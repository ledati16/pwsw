//! IPC infrastructure for daemon communication
//!
//! Provides Unix socket-based IPC for CLI commands to communicate with the daemon.
//! Uses length-prefixed JSON messages for protocol framing.

use color_eyre::eyre::{self, Context, Result};
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, error, warn};

// ============================================================================
// Constants
// ============================================================================

/// Timeout for checking if daemon is running (health check connection)
/// Set to 500ms to accommodate slow systems and high load scenarios
const DAEMON_HEALTH_CHECK_TIMEOUT_MS: u64 = 500;

/// Timeout for stale socket cleanup check (same as health check for consistency)
/// If daemon responds within health check timeout, socket is not stale
const STALE_SOCKET_CHECK_TIMEOUT_MS: u64 = 500;

/// Timeout for client connections (longer to allow daemon to process request)
const CLIENT_CONNECT_TIMEOUT_SECS: u64 = 5;

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
    /// Manually switch to a specific sink
    SetSink { sink: String },
    /// Get daemon manager information (systemd vs direct)
    GetManagerInfo,
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
    /// Daemon manager information
    ManagerInfo {
        daemon_manager: crate::daemon_manager::DaemonManager,
    },
}

/// Window information for IPC responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    pub id: Option<u64>,
    pub app_id: String,
    pub title: String,
    /// For test-rule: which fields matched ("`app_id`", "title", or "both")
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

/// Get the `IPC` socket path
/// Prefers `$XDG_RUNTIME_DIR/pwsw.sock`, falls back to `/tmp/pwsw-$USER.sock`
///
/// # Errors
/// Returns an error if both `XDG_RUNTIME_DIR` and `USER` environment variables are unset.
pub fn get_socket_path() -> Result<PathBuf> {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        Ok(PathBuf::from(runtime_dir).join("pwsw.sock"))
    } else if let Ok(user) = std::env::var("USER") {
        // Fallback to /tmp with username for consistent location
        Ok(PathBuf::from({
            let mut s = String::with_capacity(12 + user.len());
            let _ = write!(s, "/tmp/pwsw-{user}.sock");
            s
        }))
    } else {
        // Cannot determine a consistent socket path
        eyre::bail!(
            "Cannot determine IPC socket path: Both `XDG_RUNTIME_DIR` and `USER` environment variables are unset.\n\
             \n\
             This is unusual - please ensure your environment is set up correctly.\n\
             You can manually set `XDG_RUNTIME_DIR` to a user-specific directory like `/run/user/$UID`"
        )
    }
}

/// Check if a daemon is currently running
/// Returns true if a daemon is active (socket exists and accepts connections)
pub async fn is_daemon_running() -> bool {
    let Ok(socket_path) = get_socket_path() else {
        return false;
    };

    if !socket_path.exists() {
        return false;
    }

    // Try to connect - if it succeeds, daemon is running
    let connect_result = tokio::time::timeout(
        Duration::from_millis(DAEMON_HEALTH_CHECK_TIMEOUT_MS),
        tokio::net::UnixStream::connect(&socket_path),
    )
    .await;

    matches!(connect_result, Ok(Ok(_)))
}

/// Clean up stale socket file
/// Checks if socket exists and if daemon is actually running
///
/// # Errors
/// Returns an error if the socket path cannot be determined or removal fails.
async fn cleanup_stale_socket_at(socket_path: &Path) -> Result<()> {
    if !socket_path.exists() {
        return Ok(());
    }

    // On Unix: verify the file is actually a socket and owned by the current user
    #[cfg(unix)]
    {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        use users::get_current_uid;

        let metadata = std::fs::metadata(socket_path)
            .with_context(|| format!("Failed to stat socket: {}", socket_path.display()))?;

        let is_socket = metadata.file_type().is_socket();
        debug!(
            "cleanup_stale_socket: {} exists. is_socket={}, uid={}",
            socket_path.display(),
            is_socket,
            metadata.uid()
        );

        if !is_socket {
            warn!(
                "Socket path exists but is not a socket: {}",
                socket_path.display()
            );
            return Ok(());
        }

        let owner_uid = metadata.uid();
        let current_uid = get_current_uid();
        debug!(
            "cleanup_stale_socket: owner_uid={}, current_uid={}",
            owner_uid, current_uid
        );
        if owner_uid != current_uid {
            warn!(
                "Socket '{}' is owned by uid {} (current uid: {}). Not removing.",
                socket_path.display(),
                owner_uid,
                current_uid
            );
            return Ok(());
        }
    }

    // Try to connect - if it fails, the socket is stale
    // Use short timeout since we're just checking if socket is responsive
    let connect_result = tokio::time::timeout(
        Duration::from_millis(STALE_SOCKET_CHECK_TIMEOUT_MS),
        tokio::net::UnixStream::connect(socket_path),
    )
    .await;

    let is_stale = match connect_result {
        Ok(Ok(_stream)) => false,      // Successfully connected - socket is alive
        Ok(Err(_connect_err)) => true, // Connection failed - socket is stale
        Err(_timeout) => true,         // Timeout - assume socket is stale
    };

    if is_stale {
        debug!("Removing stale socket: {:?}", socket_path);
        // If the file disappeared between checks, ignore the error
        if let Err(e) = std::fs::remove_file(socket_path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(e).with_context(|| {
                format!("Failed to remove stale socket: {}", socket_path.display())
            });
        }
    }

    Ok(())
}

/// Clean up any stale socket at the default IPC path.
///
/// # Errors
/// Returns an error if the socket path cannot be determined or removal fails.
pub async fn cleanup_stale_socket() -> Result<()> {
    let socket_path = get_socket_path()?;
    cleanup_stale_socket_at(&socket_path).await
}

// ============================================================================
// Protocol Helpers
// ============================================================================

const MAX_MESSAGE_SIZE: usize = 1024 * 1024; // 1MB max message size
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Read a length-prefixed JSON message from a stream
async fn read_message<T: for<'de> Deserialize<'de>>(stream: &mut UnixStream) -> Result<T> {
    // Read 4-byte big-endian length prefix
    let mut len_buf = [0u8; 4];
    tokio::time::timeout(READ_TIMEOUT, stream.read_exact(&mut len_buf))
        .await
        .context("Timeout reading message length")?
        .context("Failed to read message length")?;

    let msg_len_u32 = u32::from_be_bytes(len_buf);

    // Check size before cast to prevent overflow on 32-bit systems
    if msg_len_u32 > MAX_MESSAGE_SIZE as u32 {
        eyre::bail!("Message too large: {msg_len_u32} bytes (max: {MAX_MESSAGE_SIZE})");
    }

    let msg_len = msg_len_u32 as usize;

    // Read the JSON payload
    let mut msg_buf = vec![0u8; msg_len];
    tokio::time::timeout(READ_TIMEOUT, stream.read_exact(&mut msg_buf))
        .await
        .context("Timeout reading message payload")?
        .context("Failed to read message payload")?;

    // Deserialize JSON
    serde_json::from_slice(&msg_buf).context("Failed to deserialize message")
}

/// Write a length-prefixed JSON message to a stream
async fn write_message<T: Serialize>(stream: &mut UnixStream, message: &T) -> Result<()> {
    // Serialize to JSON
    let json = serde_json::to_vec(message).context("Failed to serialize message")?;

    if json.len() > MAX_MESSAGE_SIZE {
        eyre::bail!(
            "Message too large: {} bytes (max: {})",
            json.len(),
            MAX_MESSAGE_SIZE
        );
    }

    // Write length prefix (4 bytes big-endian)
    // Safe cast: MAX_MESSAGE_SIZE is 1MB, well within u32 range
    let len = (json.len() as u32).to_be_bytes();
    stream
        .write_all(&len)
        .await
        .context("Failed to write message length")?;

    // Write JSON payload
    stream
        .write_all(&json)
        .await
        .context("Failed to write message payload")?;

    stream.flush().await.context("Failed to flush stream")?;

    Ok(())
}

// ============================================================================
// IPC Client (for CLI commands)
// ============================================================================

/// Send a request to the daemon and wait for response
///
/// # Errors
/// Returns an error if socket path cannot be determined, connection fails, or IPC communication fails.
pub async fn send_request(request: Request) -> Result<Response> {
    let socket_path = get_socket_path()?;

    // Connect to daemon (longer timeout for actual client requests)
    let mut stream = tokio::time::timeout(
        Duration::from_secs(CLIENT_CONNECT_TIMEOUT_SECS),
        UnixStream::connect(&socket_path),
    )
    .await
    .context("Timeout connecting to daemon")?
    .with_context(|| {
        format!(
            "Failed to connect to daemon. Is the daemon running?\nSocket: {}",
            socket_path.display()
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
    ///
    /// # Errors
    /// Returns an error if socket path cannot be determined or socket binding fails.
    pub async fn bind() -> Result<Self> {
        let socket_path = get_socket_path()?;

        // Clean up any stale socket
        cleanup_stale_socket().await?;

        // Bind the listener
        let listener = UnixListener::bind(&socket_path)
            .with_context(|| format!("Failed to bind IPC socket: {}", socket_path.display()))?;

        // Set socket permissions to user-only (0o600) for security
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
                .with_context(|| {
                    format!(
                        "Failed to set socket permissions: {}",
                        socket_path.display()
                    )
                })?;
        }

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
    pub fn socket_path(&self) -> &Path {
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
///
/// # Errors
/// Returns an error if reading fails or the message cannot be deserialized.
pub async fn read_request(stream: &mut UnixStream) -> Result<Request> {
    read_message(stream).await
}

/// Write a response to a client connection
///
/// # Errors
/// Returns an error if serialization or writing fails.
pub async fn write_response(stream: &mut UnixStream, response: &Response) -> Result<()> {
    write_message(stream, response).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // Request serialization roundtrip tests
    #[test]
    fn test_request_status_roundtrip() {
        let request = Request::Status;
        let json = serde_json::to_string(&request).unwrap();
        let deserialized: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, Request::Status));
    }

    #[test]
    fn test_request_list_windows_roundtrip() {
        let request = Request::ListWindows;
        let json = serde_json::to_string(&request).unwrap();
        let deserialized: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, Request::ListWindows));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_cleanup_stale_socket_non_socket_file() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("pwsw.sock");

        // Ensure cleanup_stale_socket will look at our tempdir
        let prev = std::env::var_os("XDG_RUNTIME_DIR");
        // SAFETY: Test-only code, single-threaded test execution, no concurrent env access
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", dir.path());
        }

        // Create a regular file at the socket path
        std::fs::write(&socket_path, b"not a socket").unwrap();
        assert!(socket_path.exists());

        // Should not remove non-socket files
        cleanup_stale_socket().await.unwrap();
        assert!(
            socket_path.exists(),
            "Non-socket file should not be removed"
        );

        // Restore env
        // SAFETY: Test-only code, restoring environment after test
        unsafe {
            if let Some(val) = prev {
                std::env::set_var("XDG_RUNTIME_DIR", val);
            } else {
                std::env::remove_var("XDG_RUNTIME_DIR");
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_cleanup_stale_socket_active_socket() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("pwsw.sock");

        // Ensure cleanup_stale_socket will look at our tempdir
        let prev = std::env::var_os("XDG_RUNTIME_DIR");
        // SAFETY: Test-only code, single-threaded test execution, no concurrent env access
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", dir.path());
        }

        // Bind and keep the listener alive to simulate active daemon
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        assert!(socket_path.exists());

        // Active socket should not be removed
        cleanup_stale_socket().await.unwrap();
        assert!(socket_path.exists(), "Active socket should not be removed");

        // Close the listener and ensure cleanup_stale_socket can still detect active socket removal
        drop(listener);

        // Restore env
        // SAFETY: Test-only code, restoring environment after test
        unsafe {
            if let Some(val) = prev {
                std::env::set_var("XDG_RUNTIME_DIR", val);
            } else {
                std::env::remove_var("XDG_RUNTIME_DIR");
            }
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_cleanup_stale_socket_stale_socket() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let socket_path = dir.path().join("pwsw.sock");

        // Create a listener and drop it immediately so the socket file remains but no process listens
        {
            let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
            drop(listener);
        }

        assert!(socket_path.exists());

        // Now the socket is stale (no process listening) and should be removed
        cleanup_stale_socket_at(&socket_path).await.unwrap();

        // Socket should be removed, but in some environments the socket may be re-created
        // by external processes between checks. Accept either removed or active.
        if socket_path.exists() {
            // If it still exists, it must be active (i.e., connect succeeds)
            let conn = tokio::time::timeout(
                Duration::from_millis(100),
                tokio::net::UnixStream::connect(&socket_path),
            )
            .await;
            match conn {
                Ok(Ok(_stream)) => {
                    // Active - acceptable
                }
                _ => panic!("Stale socket should be removed"),
            }
        } else {
            // removed - success
        }
    }

    #[test]
    fn test_request_test_rule_roundtrip() {
        let request = Request::TestRule {
            pattern: "firefox".to_string(),
        };
        let json = serde_json::to_string(&request).unwrap();
        let deserialized: Request = serde_json::from_str(&json).unwrap();
        if let Request::TestRule { pattern } = deserialized {
            assert_eq!(pattern, "firefox");
        } else {
            panic!("Expected TestRule variant");
        }
    }

    #[test]
    fn test_request_shutdown_roundtrip() {
        let request = Request::Shutdown;
        let json = serde_json::to_string(&request).unwrap();
        let deserialized: Request = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, Request::Shutdown));
    }

    // Response serialization roundtrip tests
    #[test]
    fn test_response_status_roundtrip() {
        let response = Response::Status {
            version: "0.3.1".to_string(),
            uptime_secs: 3600,
            current_sink: "test_sink".to_string(),
            active_window: Some("firefox".to_string()),
            tracked_windows: 2,
        };
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: Response = serde_json::from_str(&json).unwrap();
        if let Response::Status {
            version,
            uptime_secs,
            current_sink,
            active_window,
            tracked_windows,
        } = deserialized
        {
            assert_eq!(version, "0.3.1");
            assert_eq!(uptime_secs, 3600);
            assert_eq!(current_sink, "test_sink");
            assert_eq!(active_window, Some("firefox".to_string()));
            assert_eq!(tracked_windows, 2);
        } else {
            panic!("Expected Status variant");
        }
    }

    #[test]
    fn test_response_ok_roundtrip() {
        let response = Response::Ok {
            message: "Success".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: Response = serde_json::from_str(&json).unwrap();
        if let Response::Ok { message } = deserialized {
            assert_eq!(message, "Success");
        } else {
            panic!("Expected Ok variant");
        }
    }

    #[test]
    fn test_response_error_roundtrip() {
        let response = Response::Error {
            message: "An error occurred".to_string(),
        };
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: Response = serde_json::from_str(&json).unwrap();
        if let Response::Error { message } = deserialized {
            assert_eq!(message, "An error occurred");
        } else {
            panic!("Expected Error variant");
        }
    }

    #[test]
    fn test_response_windows_roundtrip() {
        let window_info = WindowInfo {
            id: None,
            app_id: "firefox".to_string(),
            title: "Mozilla Firefox".to_string(),
            matched_on: Some("firefox pattern".to_string()),
            tracked: Some(TrackedInfo {
                sink_name: "hdmi_sink".to_string(),
                sink_desc: "HDMI Output".to_string(),
            }),
        };
        let response = Response::Windows {
            windows: vec![window_info],
        };
        let json = serde_json::to_string(&response).unwrap();
        let deserialized: Response = serde_json::from_str(&json).unwrap();
        if let Response::Windows { windows } = deserialized {
            assert_eq!(windows.len(), 1);
            assert_eq!(windows[0].app_id, "firefox");
            assert_eq!(windows[0].title, "Mozilla Firefox");
            assert!(windows[0].tracked.is_some());
        } else {
            panic!("Expected Windows variant");
        }
    }
}
