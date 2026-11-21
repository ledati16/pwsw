//! IPC infrastructure for daemon communication
//!
//! Provides Unix socket-based IPC for CLI commands to communicate with the daemon.
//! Uses length-prefixed JSON messages for protocol framing.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, error, warn};

// ============================================================================
// Constants
// ============================================================================

/// Timeout for checking if daemon is running (health check connection).
///
/// Set to 500ms to accommodate slow systems and high load scenarios
/// while still being fast enough for typical daemon startup checks.
/// This balances responsiveness with reliability across different system loads.
const DAEMON_HEALTH_CHECK_TIMEOUT_MS: u64 = 500;

/// Timeout for stale socket cleanup check (same as health check for consistency).
///
/// If daemon responds within health check timeout, socket is not stale.
const STALE_SOCKET_CHECK_TIMEOUT_MS: u64 = 500;

/// Timeout for client connections (longer to allow daemon to process request)
const CLIENT_CONNECT_TIMEOUT_SECS: u64 = 5;

/// Version of the `IPC` protocol (the set of [`Request`] / [`Response`] messages).
///
/// Bump it whenever a message is added or changed incompatibly. The daemon reports it
/// in [`Response::Status`]; daemons from before versioning report nothing, which
/// reads as `0`.
pub const PROTOCOL_VERSION: u32 = 2;

/// How to get a daemon that speaks the current protocol, appended to `IPC` errors
pub const RESTART_HINT: &str = "if pwsw was upgraded, restart the daemon: \
     `systemctl --user restart pwsw` (or `pwsw shutdown && pwsw daemon`)";

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
    /// Manually switch to a specific sink (by position, description or node name).
    /// Honours `set_smart_toggle`: asking for the current sink switches to the default.
    SetSink { sink: String },
    /// Manually switch to the next configured sink (wrapping)
    NextSink,
    /// Manually switch to the previous configured sink (wrapping)
    PrevSink,
    /// Manually switch the default source (microphone) to a configured source (by
    /// description, node name or position); honours `set_smart_toggle` like
    /// [`Request::SetSink`]. Answered with [`Response::SinkSwitched`] (protocol 2).
    SetSource { source: String },
    /// Get daemon manager information (systemd vs direct)
    GetManagerInfo,
    /// Gracefully shutdown the daemon
    Shutdown,
    // --- Pause / resume (answered with [`Response::PauseState`]) ---
    /// Stop rule-driven switching (windows are still tracked; manual switches work)
    Pause,
    /// Resume rule-driven switching, switching at once if the rules want another sink
    Resume,
    /// Pause when running, resume when paused
    TogglePause,
    /// Follow the daemon's status: the connection stays open and the daemon writes a
    /// [`Response::StatusUpdate`] at once and then on every change (identical states
    /// are not repeated), or one [`Response::Error`] when too many clients follow it
    /// already. The client sends nothing more; hanging up ends the subscription.
    /// Protocol 2.
    Subscribe,
}

/// Responses sent from daemon to CLI
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Response {
    /// Status information
    Status {
        /// Daemon version (see [`crate::version_string`])
        version: String,
        /// `IPC` protocol the daemon speaks ([`PROTOCOL_VERSION`]; `0` from daemons
        /// that predate versioning)
        #[serde(default)]
        protocol: u32,
        uptime_secs: u64,
        /// The sink the daemon last applied (or found at startup), by configured
        /// description; `None` while it does not know the default sink
        #[serde(default)]
        current_sink: Option<String>,
        /// `current_sink` is a fallback: a rule-driven switch found its rule's
        /// earlier sinks unreachable
        #[serde(default)]
        current_sink_fallback: bool,
        active_window: Option<String>,
        tracked_windows: usize,
        /// Rule-driven switching is paused (`false` from daemons without pause support)
        #[serde(default)]
        paused: bool,
        /// The source (microphone) the daemon last applied (or found at startup), by
        /// configured description; `None` while unknown or when no sources are
        /// configured (and from daemons without source support)
        #[serde(default)]
        current_source: Option<String>,
        /// `current_source` is a fallback a rule-driven switch settled for
        #[serde(default)]
        current_source_fallback: bool,
        /// Whether `[[sources]]` are configured (pwsw manages the default source)
        #[serde(default)]
        sources_configured: bool,
        /// The compositor protocol the daemon uses: `"ext"` or `"wlr"` (`None` from
        /// daemons that predate reporting it)
        #[serde(default)]
        backend: Option<String>,
        /// Whether the backend reports which monitors windows are on (rules with
        /// `output` can match); `false` from older daemons
        #[serde(default)]
        monitor_info: bool,
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
    /// A manual switch finished: the configured sink (or source, for
    /// [`Request::SetSource`]) and the node made default
    SinkSwitched {
        /// Configured sink (source) description
        desc: String,
        /// `PipeWire` node name that was made the default
        node_name: String,
        /// Smart toggle: the requested sink (source) was the default already, so the
        /// default one was activated instead; its description (absent otherwise, and
        /// from older daemons)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        toggled_from: Option<String>,
    },
    /// Answer to [`Request::Pause`], [`Request::Resume`] and [`Request::TogglePause`]
    PauseState {
        /// Whether rule-driven switching is paused now
        paused: bool,
        /// Whether the request changed the state (`false`: it already was so)
        changed: bool,
    },
    /// One message of a [`Request::Subscribe`] stream
    StatusUpdate(StatusUpdate),
}

/// The daemon's status as a [`Request::Subscribe`] stream reports it: everything a
/// status bar shows
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusUpdate {
    /// Daemon version (see [`crate::version_string`])
    pub version: String,
    /// `IPC` protocol the daemon speaks ([`PROTOCOL_VERSION`])
    pub protocol: u32,
    /// How the daemon is run
    pub daemon_manager: crate::daemon_manager::DaemonManager,
    /// Rule-driven switching is paused
    pub paused: bool,
    /// The default sink pwsw last applied (or found at startup); `None` while unknown
    pub sink: Option<EndpointStatus>,
    /// The default source pwsw last applied; `None` while unknown or when no sources
    /// are configured
    pub source: Option<EndpointStatus>,
    /// Whether `[[sources]]` are configured (pwsw manages the default source)
    pub sources_configured: bool,
}

/// The sink (or source) a [`StatusUpdate`] reports
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EndpointStatus {
    /// Configured description (the node name when it is not in the config)
    pub desc: String,
    /// `PipeWire` node name
    pub node: String,
    /// It is a fallback: a rule-driven switch found the rule's earlier choices
    /// unreachable
    pub fallback: bool,
    /// The window whose rule chose it; `None` when no rule decides it (the default,
    /// a manual choice, or a default found at startup)
    pub window: Option<RuleWindow>,
}

/// The window behind a rule's choice, and the rule
///
/// Carries no window title: titles change all the time (browser tabs) and would leak
/// page titles into status bars. All text is sanitised (control characters escaped,
/// length capped), since `app_id`s and descriptions are untrusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleWindow {
    pub app_id: String,
    /// The rule's `desc`, else `#<rule_index>`
    pub rule: String,
    /// The rule's position in the config, 1-based (`0` from older daemons)
    #[serde(default)]
    pub rule_index: usize,
}

/// Window information for IPC responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    pub id: Option<u64>,
    pub app_id: String,
    pub title: String,
    /// Connector names of the monitors the window is on (`HDMI-A-1`); empty when
    /// unknown (the `ext` backend, or older daemons)
    #[serde(default)]
    pub outputs: Vec<String>,
    /// Whether the window is minimized (`false` when unknown)
    #[serde(default)]
    pub minimized: bool,
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
    /// The rule's first-choice sink
    pub sink_name: String,
    pub sink_desc: String,
    /// The rule's fallback sinks, in priority order (absent from older daemons)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallbacks: Vec<FallbackSink>,
    /// The rule's sources (microphones), first choice then fallbacks; empty when the
    /// rule has no `source` (absent from older daemons)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<FallbackSink>,
    /// For the window deciding the daemon's sink: the fallback it settled for, when
    /// that is what plays instead of `sink_name` (absent otherwise, and from older
    /// daemons)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback_in_use: Option<FallbackSink>,
}

/// A configured sink (or source) a rule names: a fallback sink, or one of its sources
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FallbackSink {
    pub name: String,
    pub desc: String,
}

// ============================================================================
// Socket Path Management
// ============================================================================

/// File name of the `IPC` socket inside its directory.
const SOCKET_FILE_NAME: &str = "pwsw.sock";

/// Shared temporary directory holding the per-user fallback directory.
///
/// Deliberately not `$TMPDIR`: the daemon and its clients must agree on the path
/// even when their environments differ.
const FALLBACK_SOCKET_BASE: &str = "/tmp";

/// Where the `IPC` socket lives.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SocketLocation {
    /// Full path of the socket file.
    path: PathBuf,
    /// The per-user private directory (`/tmp/pwsw-<uid>`) holding the socket
    /// when `XDG_RUNTIME_DIR` is unusable. `None` when the socket lives in
    /// `XDG_RUNTIME_DIR`, which the session manager already keeps private.
    private_dir: Option<PathBuf>,
}

/// Resolve the socket location from the current environment.
fn socket_location() -> SocketLocation {
    socket_location_from(
        std::env::var_os("XDG_RUNTIME_DIR").as_deref(),
        Path::new(FALLBACK_SOCKET_BASE),
        current_uid(),
    )
}

/// The real UID of this process.
fn current_uid() -> u32 {
    rustix::process::getuid().as_raw()
}

/// Pure socket location derivation.
///
/// Uses `<runtime_dir>/pwsw.sock` when `runtime_dir` is an absolute path. An
/// unset, empty or relative `XDG_RUNTIME_DIR` is ignored (a relative value would
/// put the socket in whatever the current directory happens to be) and the socket
/// goes into the private per-user directory `<fallback_base>/pwsw-<uid>/`.
fn socket_location_from(
    runtime_dir: Option<&OsStr>,
    fallback_base: &Path,
    uid: u32,
) -> SocketLocation {
    if let Some(dir) = runtime_dir.map(Path::new).filter(|d| d.is_absolute()) {
        return SocketLocation {
            path: dir.join(SOCKET_FILE_NAME),
            private_dir: None,
        };
    }
    let private_dir = fallback_base.join(format!("pwsw-{uid}"));
    SocketLocation {
        path: private_dir.join(SOCKET_FILE_NAME),
        private_dir: Some(private_dir),
    }
}

/// Get the `IPC` socket path
///
/// Prefers `$XDG_RUNTIME_DIR/pwsw.sock`. When `XDG_RUNTIME_DIR` is unset, empty
/// or not absolute, falls back to `/tmp/pwsw-<uid>/pwsw.sock`, inside a directory
/// the daemon creates with mode `0700` and that both sides verify before use (a
/// predictable name directly in `/tmp` could be squatted by another local user).
#[must_use]
pub fn get_socket_path() -> PathBuf {
    socket_location().path
}

/// Path of a daemon runtime file next to the socket (e.g. `pwsw.lock`)
///
/// Same location rules as the socket: `$XDG_RUNTIME_DIR/<file_name>` when that is an
/// absolute path, else `/tmp/pwsw-<uid>/<file_name>`, creating and verifying the
/// private `0700` directory first so the file cannot be squatted.
///
/// # Errors
/// Returns an error if the private fallback directory cannot be created or verified.
pub(crate) fn runtime_file_path(file_name: &str) -> Result<PathBuf> {
    let location = socket_location();
    if let Some(dir) = &location.private_dir {
        ensure_private_dir(dir, current_uid())?;
    }
    Ok(location.path.with_file_name(file_name))
}

/// Create (if missing) and verify the private fallback directory.
///
/// # Errors
/// Returns an error if the directory cannot be created or fails
/// [`verify_private_dir`].
fn ensure_private_dir(dir: &Path, uid: u32) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {
            // The umask can only narrow the mode; make it exactly 0700 so the
            // verification below does not reject a directory we just created.
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
                .with_context(|| format!("Failed to set permissions on {}", dir.display()))?;
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => {
            return Err(e)
                .with_context(|| format!("Failed to create socket directory {}", dir.display()));
        }
    }
    verify_private_dir(dir, uid)
}

/// Verify that `dir` is a real directory (not a symlink), owned by `uid`, with
/// mode exactly `0700`.
///
/// # Errors
/// Returns an error describing the first check that fails.
fn verify_private_dir(dir: &Path, uid: u32) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let hint = "Set XDG_RUNTIME_DIR to a private directory, or remove the offending path";
    let meta = std::fs::symlink_metadata(dir)
        .with_context(|| format!("Failed to stat socket directory {}", dir.display()))?;
    if meta.file_type().is_symlink() {
        anyhow::bail!(
            "Refusing to use socket directory {}: it is a symlink.\n{hint}",
            dir.display()
        );
    }
    if !meta.is_dir() {
        anyhow::bail!(
            "Refusing to use socket directory {}: it is not a directory.\n{hint}",
            dir.display()
        );
    }
    if meta.uid() != uid {
        anyhow::bail!(
            "Refusing to use socket directory {}: it is owned by uid {} (expected {uid}).\n{hint}",
            dir.display(),
            meta.uid()
        );
    }
    let mode = meta.permissions().mode() & 0o7777;
    if mode != 0o700 {
        anyhow::bail!(
            "Refusing to use socket directory {}: it has mode {mode:04o} (expected 0700).\n{hint}",
            dir.display()
        );
    }
    Ok(())
}

/// Client-side check of the fallback directory: if it exists it must pass
/// [`verify_private_dir`]; if it does not exist no daemon can be listening there.
///
/// # Errors
/// Returns an error if the directory exists but is not safe to use.
fn verify_client_dir(location: &SocketLocation, uid: u32) -> Result<()> {
    let Some(dir) = &location.private_dir else {
        return Ok(());
    };
    match std::fs::symlink_metadata(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => verify_private_dir(dir, uid),
    }
}

/// Check that the process at the other end of `stream` runs as `expected_uid`.
///
/// # Errors
/// Returns an error if the peer credentials cannot be read or the UID differs.
fn verify_peer_uid(stream: &UnixStream, expected_uid: u32) -> Result<()> {
    let cred = stream
        .peer_cred()
        .context("Failed to read IPC peer credentials")?;
    if cred.uid() != expected_uid {
        anyhow::bail!(
            "IPC peer runs as uid {} (expected uid {expected_uid})",
            cred.uid()
        );
    }
    Ok(())
}

/// Connect to the daemon socket and make sure it is served by our own UID.
///
/// # Errors
/// Returns an error if the fallback directory is unsafe, the socket file belongs
/// to another user, the connection fails or times out, or the server runs as a
/// different UID.
async fn connect_verified(location: &SocketLocation, timeout: Duration) -> Result<UnixStream> {
    use std::os::unix::fs::MetadataExt;

    let uid = current_uid();
    verify_client_dir(location, uid)?;

    let path = &location.path;
    if let Ok(meta) = std::fs::symlink_metadata(path)
        && meta.uid() != uid
    {
        anyhow::bail!(
            "Refusing to talk to {}: the socket is owned by uid {} (expected {uid}). \
             Another user may be impersonating the daemon.",
            path.display(),
            meta.uid()
        );
    }

    let stream = tokio::time::timeout(timeout, UnixStream::connect(path))
        .await
        .with_context(|| format!("Timeout connecting to daemon at {}", path.display()))?
        .with_context(|| {
            format!(
                "Failed to connect to daemon. Is the daemon running?\nSocket: {}",
                path.display()
            )
        })?;

    verify_peer_uid(&stream, uid).with_context(|| {
        format!(
            "Refusing to talk to {}: it is not served by the current user",
            path.display()
        )
    })?;
    Ok(stream)
}

/// Check if a daemon is currently running
/// Returns true if a daemon is active (socket exists, accepts connections and is
/// served by the current user)
pub async fn is_daemon_running() -> bool {
    let location = socket_location();

    if !location.path.exists() {
        return false;
    }

    match connect_verified(
        &location,
        Duration::from_millis(DAEMON_HEALTH_CHECK_TIMEOUT_MS),
    )
    .await
    {
        Ok(_stream) => true,
        Err(e) => {
            debug!("Daemon health check failed: {e:#}");
            false
        }
    }
}

/// Wait until the daemon at the default socket has gone away.
///
/// Returns once the socket file has disappeared, or once it no longer has a
/// listener (`ECONNREFUSED`, i.e. a stale socket left behind by a crash).
///
/// # Errors
/// Returns an error if the daemon is still listening after `timeout`.
pub async fn wait_for_daemon_exit(timeout: Duration) -> Result<()> {
    const POLL_INTERVAL: Duration = Duration::from_millis(50);

    let path = get_socket_path();
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Err(e) = std::fs::symlink_metadata(&path)
            && e.kind() == std::io::ErrorKind::NotFound
        {
            return Ok(());
        }
        if let Ok(Err(e)) = tokio::time::timeout(POLL_INTERVAL, UnixStream::connect(&path)).await
            && e.kind() == std::io::ErrorKind::ConnectionRefused
        {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!(
                "Daemon still running after {:.1}s (socket {} is still accepting connections)",
                timeout.as_secs_f64(),
                path.display()
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Remove `path` only if it is still the file identified by `(dev, ino)`.
///
/// Returns `Ok(true)` if the file was removed, `Ok(false)` if it is gone or has
/// been replaced by a different file (e.g. a socket bound by another daemon).
/// A tiny window remains between the `stat` and the `unlink`; POSIX offers no
/// unlink-by-inode, and both happen within microseconds.
///
/// # Errors
/// Returns an error if the file cannot be inspected or removed.
fn remove_if_same_inode(path: &Path, (dev, ino): (u64, u64)) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    if meta.dev() != dev || meta.ino() != ino {
        return Ok(false);
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// What [`cleanup_stale_socket_at`] found at the socket path, and what it did
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StaleSocketCheck {
    /// Nothing at the path
    Missing,
    /// The path is not a socket; left alone
    NotASocket,
    /// A socket owned by another user; left alone
    ForeignOwner,
    /// A listener accepted the probe connection: a live daemon; left alone
    Live,
    /// Nobody listening (`ECONNREFUSED`): the stale socket was removed
    Removed,
    /// Nobody listening, but the path was replaced before removal; left alone
    Replaced,
    /// The probe failed with another error (e.g. `EACCES`); left alone
    ProbeFailed(std::io::ErrorKind),
    /// The probe got no answer within [`STALE_SOCKET_CHECK_TIMEOUT_MS`]; left alone
    TimedOut,
}

/// Clean up a stale socket file at `socket_path`.
///
/// The file is removed only when it is a socket owned by the current user and a
/// connection attempt is refused with `ECONNREFUSED` (nobody is listening). A
/// timeout or any other error leaves the file alone: a daemon may be starting
/// up or busy, and binding will then fail with a clear "address in use" error
/// instead of stealing a live daemon's socket.
///
/// # Errors
/// Returns an error if the socket cannot be inspected or removal fails.
async fn cleanup_stale_socket_at(socket_path: &Path) -> Result<StaleSocketCheck> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let metadata = match std::fs::symlink_metadata(socket_path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StaleSocketCheck::Missing);
        }
        Err(e) => {
            return Err(e)
                .with_context(|| format!("Failed to stat socket: {}", socket_path.display()));
        }
    };

    if !metadata.file_type().is_socket() {
        warn!(
            "Socket path exists but is not a socket: {}",
            socket_path.display()
        );
        return Ok(StaleSocketCheck::NotASocket);
    }

    let owner_uid = metadata.uid();
    let current_uid = current_uid();
    if owner_uid != current_uid {
        warn!(
            "Socket '{}' is owned by uid {} (current uid: {}). Not removing.",
            socket_path.display(),
            owner_uid,
            current_uid
        );
        return Ok(StaleSocketCheck::ForeignOwner);
    }

    let connect_result = tokio::time::timeout(
        Duration::from_millis(STALE_SOCKET_CHECK_TIMEOUT_MS),
        UnixStream::connect(socket_path),
    )
    .await;

    match connect_result {
        // A listener answered: the socket is live
        Ok(Ok(_stream)) => Ok(StaleSocketCheck::Live),
        // Nobody is listening: the socket is stale
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            debug!("Removing stale socket: {}", socket_path.display());
            let removed = remove_if_same_inode(socket_path, (metadata.dev(), metadata.ino()))
                .with_context(|| {
                    format!("Failed to remove stale socket: {}", socket_path.display())
                })?;
            Ok(if removed {
                StaleSocketCheck::Removed
            } else {
                StaleSocketCheck::Replaced
            })
        }
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(StaleSocketCheck::Missing),
        Ok(Err(e)) => {
            warn!(
                "Could not probe socket {} ({e}); leaving it in place",
                socket_path.display()
            );
            Ok(StaleSocketCheck::ProbeFailed(e.kind()))
        }
        Err(_timeout) => {
            warn!(
                "Socket {} did not answer within {}ms; leaving it in place",
                socket_path.display(),
                STALE_SOCKET_CHECK_TIMEOUT_MS
            );
            Ok(StaleSocketCheck::TimedOut)
        }
    }
}

// ============================================================================
// Protocol Helpers
// ============================================================================

const MAX_MESSAGE_SIZE: usize = 1024 * 1024; // 1MB max message size
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Read a length-prefixed JSON message from a stream
///
/// Wire Format:
/// [ Length (4 bytes, u32 BE) ] [ JSON Payload (Length bytes) ]
///
/// Example:
/// [ 0x00, 0x00, 0x00, 0x0D ] [ {"type":"Status"} ]
async fn read_message<T: for<'de> Deserialize<'de>>(stream: &mut UnixStream) -> Result<T> {
    let frame = read_frame(stream).await?;
    serde_json::from_slice(&frame).context("Failed to deserialize message")
}

/// Read one length-prefixed frame (the raw `JSON` payload), see [`read_message`]
async fn read_frame(stream: &mut UnixStream) -> Result<Vec<u8>> {
    // Read 4-byte big-endian length prefix
    let mut len_buf = [0u8; 4];
    tokio::time::timeout(READ_TIMEOUT, stream.read_exact(&mut len_buf))
        .await
        .context("Timeout reading message length")?
        .context("Failed to read message length")?;
    read_frame_body(stream, len_buf).await
}

/// The rest of a frame whose length prefix `len_buf` was read
async fn read_frame_body(stream: &mut UnixStream, len_buf: [u8; 4]) -> Result<Vec<u8>> {
    let msg_len_u32 = u32::from_be_bytes(len_buf);

    // Check size before cast to prevent overflow on 32-bit systems
    if msg_len_u32 > u32::try_from(MAX_MESSAGE_SIZE).unwrap_or(u32::MAX) {
        anyhow::bail!("Message too large: {msg_len_u32} bytes (max: {MAX_MESSAGE_SIZE})");
    }

    let msg_len = msg_len_u32 as usize;

    // Read the JSON payload
    let mut msg_buf = vec![0u8; msg_len];
    tokio::time::timeout(READ_TIMEOUT, stream.read_exact(&mut msg_buf))
        .await
        .context("Timeout reading message payload")?
        .context("Failed to read message payload")?;

    Ok(msg_buf)
}

/// Write a length-prefixed JSON message to a stream
///
/// Wire Format:
/// [ Length (4 bytes, u32 BE) ] [ JSON Payload (Length bytes) ]
///
/// Example:
/// [ 0x00, 0x00, 0x00, 0x0D ] [ {"type":"Status"} ]
async fn write_message<T: Serialize>(stream: &mut UnixStream, message: &T) -> Result<()> {
    // Serialize to JSON
    let json = serde_json::to_vec(message).context("Failed to serialize message")?;

    if json.len() > MAX_MESSAGE_SIZE {
        anyhow::bail!(
            "Message too large: {} bytes (max: {})",
            json.len(),
            MAX_MESSAGE_SIZE
        );
    }

    // Write length prefix (4 bytes big-endian)
    // Safe cast: MAX_MESSAGE_SIZE is 1MB, well within u32 range
    let len = u32::try_from(json.len())
        .context("Message too large for u32 length prefix")?
        .to_be_bytes();
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
/// Returns an error if connection fails, the socket is not served by the current
/// user, or IPC communication fails.
pub async fn send_request(request: Request) -> Result<Response> {
    exchange(request)
        .await
        .with_context(|| format!("No valid reply from the pwsw daemon; {RESTART_HINT}"))
}

async fn exchange(request: Request) -> Result<Response> {
    let location = socket_location();

    // Connect to daemon (longer timeout for actual client requests)
    let mut stream =
        connect_verified(&location, Duration::from_secs(CLIENT_CONNECT_TIMEOUT_SECS)).await?;

    debug!("Connected to daemon at {}", location.path.display());

    // Send request
    write_message(&mut stream, &request).await?;

    // Read response
    let response: Response = read_message(&mut stream).await?;

    Ok(response)
}

/// A [`Request::Subscribe`] connection: the daemon's status updates as they come
#[derive(Debug)]
pub struct StatusStream {
    stream: UnixStream,
}

impl StatusStream {
    /// Connect to the daemon and subscribe to its status
    ///
    /// # Errors
    /// Returns an error if the connection fails, the socket is not served by the
    /// current user, or the request cannot be sent.
    pub async fn connect() -> Result<Self> {
        let location = socket_location();
        let mut stream =
            connect_verified(&location, Duration::from_secs(CLIENT_CONNECT_TIMEOUT_SECS)).await?;
        write_message(&mut stream, &Request::Subscribe).await?;
        Ok(Self { stream })
    }

    /// Wait for the next message: a [`Response::StatusUpdate`], or a
    /// [`Response::Error`] after which the daemon hangs up
    ///
    /// There is no time limit for the next update to begin (the status may not change
    /// for hours); once it has, the rest must arrive within the usual read timeout.
    ///
    /// # Errors
    /// Returns an error when the daemon hung up (it stopped) or sent something
    /// unreadable.
    pub async fn next(&mut self) -> Result<Response> {
        let mut len_buf = [0u8; 4];
        self.stream
            .read_exact(&mut len_buf)
            .await
            .context("The daemon closed the status stream")?;
        let frame = read_frame_body(&mut self.stream, len_buf).await?;
        serde_json::from_slice(&frame).context("Failed to deserialize status update")
    }
}

// ============================================================================
// IPC Server (for daemon)
// ============================================================================

/// Handle for the IPC server running in the daemon
pub struct IpcServer {
    listener: UnixListener,
    socket_path: PathBuf,
    /// `(dev, ino)` of the socket file we bound, so `Drop` only removes our own
    socket_id: (u64, u64),
    /// UID that clients must run as
    uid: u32,
}

impl IpcServer {
    /// Create and bind a new IPC server
    ///
    /// # Errors
    /// Returns an error if the socket directory is unsafe or socket binding fails.
    pub async fn bind() -> Result<Self> {
        Self::bind_at(&socket_location()).await
    }

    /// Bind the server at an explicit location.
    ///
    /// # Errors
    /// Returns an error if the private directory cannot be created or verified,
    /// stale-socket cleanup fails, or binding/permission setting fails.
    async fn bind_at(location: &SocketLocation) -> Result<Self> {
        use std::os::unix::fs::MetadataExt;

        let uid = current_uid();
        let socket_path = location.path.clone();

        if let Some(dir) = &location.private_dir {
            ensure_private_dir(dir, uid)?;
        }

        // Clean up any stale socket
        let check = cleanup_stale_socket_at(&socket_path).await?;
        debug!("Stale socket check: {check:?}");

        let listener = bind_private(&socket_path, uid)?;

        let meta = std::fs::symlink_metadata(&socket_path)
            .with_context(|| format!("Failed to stat IPC socket: {}", socket_path.display()))?;

        debug!("IPC server listening on {}", socket_path.display());

        Ok(Self {
            listener,
            socket_path,
            socket_id: (meta.dev(), meta.ino()),
            uid,
        })
    }

    /// Accept the next incoming connection
    ///
    /// Returns `None` if accept fails (non-fatal) or the peer runs as a different
    /// user (the connection is dropped and logged).
    pub async fn accept(&self) -> Option<UnixStream> {
        match self.listener.accept().await {
            Ok((stream, _addr)) => match verify_peer_uid(&stream, self.uid) {
                Ok(()) => Some(stream),
                Err(e) => {
                    warn!("Rejected IPC connection: {e:#}");
                    None
                }
            },
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

/// Whether `dir` is a real directory owned by `uid` with no group or other
/// permission bits, i.e. nobody else can reach anything inside it.
fn dir_is_private(dir: &Path, uid: u32) -> bool {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    use rustix::fs::Mode;

    std::fs::symlink_metadata(dir).is_ok_and(|meta| {
        let mode = Mode::from_raw_mode(meta.permissions().mode());
        meta.is_dir() && meta.uid() == uid && !mode.intersects(Mode::RWXG | Mode::RWXO)
    })
}

/// Name prefix of the transient staging directory used by [`bind_private`]
const STAGING_DIR_PREFIX: &str = ".pwsw-bind-";

/// Bind a listening socket at `socket_path` that no other user can connect to at
/// any instant.
///
/// `bind` creates the socket file with a mode derived from the process umask. The
/// umask is process-wide, so changing it around `bind` races with every other
/// thread creating files (and with concurrent binds restoring it in the wrong
/// order); it is deliberately left alone. Instead:
/// - if the parent directory is private ([`dir_is_private`]: the verified
///   `/tmp/pwsw-<uid>` fallback, or a normal `0700` `$XDG_RUNTIME_DIR`), nobody
///   else can even reach the path, so the socket is bound in place and then set
///   to `0600`;
/// - otherwise it is bound inside a fresh `0700` staging directory next to
///   `socket_path`, set to `0600` there, and only then hard-linked to
///   `socket_path` (which fails if the name is taken: an existing file is never
///   replaced). The staging directory is removed afterwards. A socket is found by
///   its inode, so connecting through the final name works.
///
/// # Errors
/// Returns an error if binding, setting permissions, or linking fails.
fn bind_private(socket_path: &Path, uid: u32) -> Result<UnixListener> {
    use std::os::unix::fs::PermissionsExt;

    let user_only = || std::fs::Permissions::from_mode(0o600);
    let bind = |path: &Path| {
        UnixListener::bind(path)
            .with_context(|| format!("Failed to bind IPC socket: {}", path.display()))
    };
    let chmod = |path: &Path| {
        std::fs::set_permissions(path, user_only())
            .with_context(|| format!("Failed to set socket permissions: {}", path.display()))
    };

    let parent = socket_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if dir_is_private(parent, uid) {
        let listener = bind(socket_path)?;
        chmod(socket_path)?;
        return Ok(listener);
    }

    debug!(
        "{} is reachable by other users; binding via a private staging directory",
        parent.display()
    );
    let staging = tempfile::Builder::new()
        .prefix(STAGING_DIR_PREFIX)
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir_in(parent)
        .with_context(|| {
            format!(
                "Failed to create a staging directory in {}",
                parent.display()
            )
        })?;
    // mkdir applies the umask, which can only narrow 0700; make it exact
    std::fs::set_permissions(staging.path(), std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("Failed to set permissions on {}", staging.path().display()))?;
    let staged = staging.path().join(SOCKET_FILE_NAME);
    let listener = bind(&staged)?;
    chmod(&staged)?;
    std::fs::hard_link(&staged, socket_path).with_context(|| {
        format!(
            "Failed to bind IPC socket: {} (is another daemon starting?)",
            socket_path.display()
        )
    })?;
    if let Err(e) = staging.close() {
        warn!("Failed to remove socket staging directory: {e}");
    }
    Ok(listener)
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        // Only remove the socket if it is still the one we bound: another daemon
        // may have replaced it after we stopped listening.
        match remove_if_same_inode(&self.socket_path, self.socket_id) {
            Ok(true) => debug!("Removed IPC socket: {}", self.socket_path.display()),
            Ok(false) => debug!(
                "IPC socket {} was already removed or replaced; leaving it",
                self.socket_path.display()
            ),
            Err(e) => warn!("Failed to remove IPC socket on shutdown: {}", e),
        }
    }
}

/// Read a request from a client connection
///
/// The inner `Err` is a well-framed message this daemon does not understand (most
/// likely from a newer client); answer it with [`unsupported_request`] instead of
/// dropping the connection.
///
/// # Errors
/// Returns an error if reading the frame fails.
pub async fn read_request(
    stream: &mut UnixStream,
) -> Result<std::result::Result<Request, serde_json::Error>> {
    let frame = read_frame(stream).await?;
    Ok(serde_json::from_slice(&frame))
}

/// The reply to a request this daemon (`daemon_version`) cannot parse
#[must_use]
pub fn unsupported_request(daemon_version: &str, error: &serde_json::Error) -> Response {
    Response::Error {
        message: format!(
            "Request not understood ({error}): daemon is pwsw {daemon_version} \
             (protocol {PROTOCOL_VERSION}); restart it with `systemctl --user restart pwsw` \
             (or `pwsw shutdown && pwsw daemon`)"
        ),
    }
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
        let json = serde_json::to_string(&request).expect("serialize");
        let deserialized: Request = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(deserialized, Request::Status));
    }

    #[test]
    fn test_request_list_windows_roundtrip() {
        let request = Request::ListWindows;
        let json = serde_json::to_string(&request).expect("serialize");
        let deserialized: Request = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(deserialized, Request::ListWindows));
    }

    #[test]
    fn test_socket_path_prefers_runtime_dir() {
        let loc = socket_location_from(Some(OsStr::new("/run/user/1000")), Path::new("/tmp"), 1000);
        assert_eq!(loc.path, PathBuf::from("/run/user/1000/pwsw.sock"));
        assert_eq!(loc.private_dir, None);
    }

    #[test_case::test_case(None ; "unset")]
    #[test_case::test_case(Some("") ; "empty")]
    #[test_case::test_case(Some("relative/dir") ; "relative")]
    #[test_case::test_case(Some(".") ; "dot")]
    fn test_socket_path_falls_back_to_private_dir(runtime_dir: Option<&str>) {
        let loc = socket_location_from(runtime_dir.map(OsStr::new), Path::new("/base"), 4242);
        assert_eq!(loc.path, PathBuf::from("/base/pwsw-4242/pwsw.sock"));
        assert_eq!(loc.private_dir, Some(PathBuf::from("/base/pwsw-4242")));
    }

    #[test]
    fn test_ensure_private_dir_creates_0700() {
        use std::os::unix::fs::PermissionsExt;

        let base = tempfile::tempdir().expect("create temp dir");
        let dir = base.path().join("pwsw-test");
        ensure_private_dir(&dir, current_uid()).expect("fresh directory should be accepted");
        let mode = std::fs::metadata(&dir)
            .expect("stat file")
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(mode, 0o700);
        // Idempotent on an existing, correct directory
        ensure_private_dir(&dir, current_uid()).expect("existing 0700 directory is fine");
    }

    #[test]
    fn test_verify_private_dir_rejects_symlink() {
        let base = tempfile::tempdir().expect("create temp dir");
        let real = base.path().join("real");
        std::fs::DirBuilder::new()
            .create(&real)
            .expect("`create` succeeds");
        std::fs::set_permissions(&real, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .expect("set permissions");
        let link = base.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("create symlink");

        let err = ensure_private_dir(&link, current_uid()).expect_err("`ensure_private_dir` fails");
        assert!(format!("{err:#}").contains("symlink"), "{err:#}");
    }

    #[test]
    fn test_verify_private_dir_rejects_wrong_mode() {
        use std::os::unix::fs::PermissionsExt;

        let base = tempfile::tempdir().expect("create temp dir");
        let dir = base.path().join("open");
        std::fs::create_dir(&dir).expect("create dir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755))
            .expect("set permissions");

        let err = ensure_private_dir(&dir, current_uid()).expect_err("`ensure_private_dir` fails");
        assert!(format!("{err:#}").contains("0755"), "{err:#}");
    }

    #[test]
    fn test_verify_private_dir_rejects_wrong_owner_and_files() {
        let base = tempfile::tempdir().expect("create temp dir");
        let dir = base.path().join("d");
        ensure_private_dir(&dir, current_uid()).expect("`ensure_private_dir` succeeds");
        let other_uid = current_uid().wrapping_add(1);
        let err = verify_private_dir(&dir, other_uid).expect_err("`verify_private_dir` fails");
        assert!(format!("{err:#}").contains("owned by uid"), "{err:#}");

        let file = base.path().join("f");
        std::fs::write(&file, b"").expect("write file");
        let err = verify_private_dir(&file, current_uid()).expect_err("`verify_private_dir` fails");
        assert!(format!("{err:#}").contains("not a directory"), "{err:#}");
    }

    #[test]
    fn test_verify_client_dir_missing_is_ok() {
        let base = tempfile::tempdir().expect("create temp dir");
        let loc = socket_location_from(None, base.path(), 7);
        verify_client_dir(&loc, current_uid()).expect("missing dir means no daemon, not an error");
    }

    #[tokio::test]
    async fn test_verify_peer_uid() {
        let (a, b) = UnixStream::pair().expect("socket pair");
        verify_peer_uid(&a, current_uid()).expect("same process, same uid");
        verify_peer_uid(&b, current_uid()).expect("same process, same uid");
        let err = verify_peer_uid(&a, current_uid().wrapping_add(1))
            .expect_err("`verify_peer_uid` fails");
        assert!(format!("{err:#}").contains("expected uid"), "{err:#}");
    }

    #[test]
    fn test_remove_if_same_inode() {
        use std::os::unix::fs::MetadataExt;

        let dir = tempfile::tempdir().expect("create temp dir");
        let path = dir.path().join("f");
        std::fs::write(&path, b"one").expect("write file");
        let meta = std::fs::metadata(&path).expect("stat file");
        let id = (meta.dev(), meta.ino());

        // Replace the file: same path, different inode (keep the old inode alive
        // via a hard link so it cannot be reused)
        let keep = dir.path().join("keep");
        std::fs::hard_link(&path, &keep).expect("create hard link");
        std::fs::remove_file(&path).expect("remove file");
        std::fs::write(&path, b"two").expect("write file");
        assert!(!remove_if_same_inode(&path, id).expect("`remove_if_same_inode` succeeds"));
        assert!(path.exists(), "replaced file must not be removed");

        let meta = std::fs::metadata(&path).expect("stat file");
        assert!(
            remove_if_same_inode(&path, (meta.dev(), meta.ino()))
                .expect("`remove_if_same_inode` succeeds")
        );
        assert!(!path.exists());
        assert!(
            !remove_if_same_inode(&path, id).expect("`remove_if_same_inode` succeeds"),
            "missing file is not an error"
        );
    }

    #[tokio::test]
    async fn test_server_bind_mode_and_drop_cleanup() {
        use std::os::unix::fs::PermissionsExt;

        let base = tempfile::tempdir().expect("create temp dir");
        let loc = socket_location_from(None, base.path(), current_uid());
        let server = IpcServer::bind_at(&loc).await.expect("bind in private dir");
        let mode = std::fs::symlink_metadata(&loc.path)
            .expect("stat file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        drop(server);
        assert!(!loc.path.exists(), "own socket removed on drop");
    }

    #[tokio::test]
    async fn test_server_drop_keeps_replaced_socket() {
        let base = tempfile::tempdir().expect("create temp dir");
        let loc = socket_location_from(None, base.path(), current_uid());
        let server = IpcServer::bind_at(&loc).await.expect("bind in private dir");

        // Another daemon takes over the path after we stopped listening
        std::fs::remove_file(&loc.path).expect("remove file");
        let other = std::os::unix::net::UnixListener::bind(&loc.path).expect("bind socket");

        drop(server);
        assert!(
            loc.path.exists(),
            "a socket we did not bind must survive our drop"
        );
        drop(other);
    }

    #[tokio::test]
    async fn test_server_rejects_unsafe_private_dir() {
        use std::os::unix::fs::PermissionsExt;

        let base = tempfile::tempdir().expect("create temp dir");
        let loc = socket_location_from(None, base.path(), current_uid());
        let dir = loc.private_dir.clone().expect("`clone` succeeds");
        std::fs::create_dir(&dir).expect("create dir");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777))
            .expect("set permissions");

        let Err(err) = IpcServer::bind_at(&loc).await else {
            panic!("bound in a world-writable directory");
        };
        assert!(
            err.to_string()
                .contains("it has mode 0777 (expected 0700)."),
            "{err}"
        );
        assert!(!loc.path.exists());
    }

    /// Bound for every wait in these tests, so a bug fails fast instead of hanging
    const TEST_TIMEOUT: Duration = Duration::from_secs(10);

    /// Connect a client to `server` at `loc` and send one request through
    async fn roundtrip(server: &IpcServer, loc: &SocketLocation) {
        let client = connect_verified(loc, TEST_TIMEOUT);
        let (client, accepted) = tokio::time::timeout(TEST_TIMEOUT, async {
            tokio::join!(client, server.accept())
        })
        .await
        .expect("connect/accept timed out");
        let mut client = client.expect("client accepts a socket served by our uid");
        let mut accepted = accepted.expect("server accepts a peer with our uid");

        write_message(&mut client, &Request::Status)
            .await
            .expect("write message");
        let req: Request = read_request(&mut accepted)
            .await
            .expect("read request")
            .expect("a known request");
        assert!(matches!(req, Request::Status));
    }

    #[tokio::test]
    async fn test_client_roundtrip_with_peer_check() {
        let base = tempfile::tempdir().expect("create temp dir");
        let loc = socket_location_from(None, base.path(), current_uid());
        let server = IpcServer::bind_at(&loc).await.expect("bind server");
        roundtrip(&server, &loc).await;
    }

    /// A peer running as another user is dropped by `accept`: the server is told
    /// to expect a different uid than the (same-process) client has
    #[tokio::test]
    async fn test_server_accept_drops_peer_with_foreign_uid() {
        let base = tempfile::tempdir().expect("create temp dir");
        let loc = socket_location_from(None, base.path(), current_uid());
        let mut server = IpcServer::bind_at(&loc).await.expect("bind in private dir");
        server.uid = current_uid().wrapping_add(1);

        let (client, accepted) = tokio::time::timeout(TEST_TIMEOUT, async {
            tokio::join!(UnixStream::connect(&loc.path), server.accept())
        })
        .await
        .expect("connect/accept timed out");
        let mut client = client.expect("the kernel accepts the connection");
        assert!(accepted.is_none(), "a foreign peer must be rejected");

        // The rejected connection is closed: the client reads end-of-file at once
        let mut buf = [0u8; 1];
        let n = tokio::time::timeout(TEST_TIMEOUT, client.read(&mut buf))
            .await
            .expect("rejected connection is closed, not left hanging")
            .expect("read from a closed connection");
        assert_eq!(n, 0, "nothing is sent to a rejected peer");

        // The same server still serves its own user
        server.uid = current_uid();
        roundtrip(&server, &loc).await;
    }

    /// A length prefix above [`MAX_MESSAGE_SIZE`] is refused before any payload
    /// buffer is allocated or read; exactly the maximum is read
    #[tokio::test]
    async fn test_read_frame_rejects_oversized_length_prefix() {
        let max = u32::try_from(MAX_MESSAGE_SIZE).expect("max fits in u32");
        for len in [max + 1, u32::MAX] {
            let (mut tx, mut rx) = UnixStream::pair().expect("socket pair");
            tx.write_all(&len.to_be_bytes()).await.expect("send prefix");
            // The sender stays open and sends no payload: reading one would time out
            let err = tokio::time::timeout(TEST_TIMEOUT, read_frame(&mut rx))
                .await
                .expect("rejected without waiting for a payload")
                .expect_err("oversized frame");
            assert_eq!(
                err.to_string(),
                format!("Message too large: {len} bytes (max: {MAX_MESSAGE_SIZE})")
            );
            drop(tx);
        }

        // At the limit the payload is read (here: the sender hangs up early)
        let (mut tx, mut rx) = UnixStream::pair().expect("socket pair");
        tx.write_all(&max.to_be_bytes()).await.expect("send prefix");
        drop(tx);
        let err = read_frame(&mut rx).await.expect_err("truncated payload");
        assert_eq!(err.to_string(), "Failed to read message payload");

        // Through the server's entry point too
        let (mut tx, mut rx) = UnixStream::pair().expect("socket pair");
        tx.write_all(&(max + 1).to_be_bytes())
            .await
            .expect("send prefix");
        let err = tokio::time::timeout(TEST_TIMEOUT, read_request(&mut rx))
            .await
            .expect("rejected without waiting for a payload")
            .expect_err("oversized request");
        assert!(err.to_string().starts_with("Message too large"), "{err}");
    }

    /// A runtime directory others can reach: the socket is staged in a private
    /// directory, then linked into place already `0600`
    #[tokio::test]
    async fn test_server_bind_in_shared_dir_uses_private_staging() {
        use std::os::unix::fs::PermissionsExt;

        let base = tempfile::tempdir().expect("create temp dir");
        let shared = base.path().join("run");
        std::fs::create_dir(&shared).expect("create dir");
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755))
            .expect("set permissions");
        assert!(!dir_is_private(&shared, current_uid()));

        let loc = socket_location_from(Some(shared.as_os_str()), Path::new("/unused"), 1);
        let server = IpcServer::bind_at(&loc).await.expect("bind in shared dir");
        let mode = std::fs::symlink_metadata(&loc.path)
            .expect("stat file")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
        let leftovers: Vec<_> = std::fs::read_dir(&shared)
            .expect("read dir")
            .map(|e| e.expect("dir entry").file_name())
            .filter(|name| name != SOCKET_FILE_NAME)
            .collect();
        assert!(
            leftovers.is_empty(),
            "staging directory left behind: {leftovers:?}"
        );

        roundtrip(&server, &loc).await;
        drop(server);
        assert!(!loc.path.exists(), "own socket removed on drop");
    }

    /// Binding never replaces a file that appears at the socket path
    #[tokio::test]
    async fn test_bind_private_never_replaces_existing_path() {
        use std::os::unix::fs::PermissionsExt;

        let base = tempfile::tempdir().expect("create temp dir");
        std::fs::set_permissions(base.path(), std::fs::Permissions::from_mode(0o755))
            .expect("set permissions");
        let path = base.path().join(SOCKET_FILE_NAME);
        std::fs::write(&path, b"someone else's").expect("write file");

        let err = bind_private(&path, current_uid()).expect_err("the path is taken");
        assert!(
            format!("{err:#}").contains("(is another daemon starting?): File exists"),
            "{err:#}"
        );
        assert_eq!(std::fs::read(&path).expect("read file"), b"someone else's");
    }

    #[test]
    fn test_dir_is_private() {
        use std::os::unix::fs::PermissionsExt;

        let base = tempfile::tempdir().expect("create temp dir");
        let dir = base.path().join("d");
        std::fs::create_dir(&dir).expect("create dir");
        for (mode, private) in [(0o700, true), (0o500, true), (0o710, false), (0o701, false)] {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode))
                .expect("set permissions");
            assert_eq!(dir_is_private(&dir, current_uid()), private, "{mode:o}");
        }
        assert!(!dir_is_private(&dir, current_uid().wrapping_add(1)));
        let link = base.path().join("link");
        std::os::unix::fs::symlink(&dir, &link).expect("create symlink");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .expect("set permissions");
        assert!(
            !dir_is_private(&link, current_uid()),
            "symlinks are not trusted"
        );
    }

    /// Leave a socket file at `path` with nobody listening on it.
    ///
    /// The socket is bound but never `listen()`ed. Binding a listener and dropping it
    /// is not reliably "nobody listening": a child process forked by another test
    /// thread holds a copy of every descriptor until it `exec`s, and meanwhile the
    /// listener stays alive and accepts connections.
    fn make_stale_socket(path: &Path) {
        use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};

        let fd = rustix::net::socket_with(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::CLOEXEC,
            None,
        )
        .expect("socket");
        rustix::net::bind(&fd, &SocketAddrUnix::new(path).expect("socket address")).expect("bind");
    }

    #[tokio::test]
    async fn test_cleanup_stale_socket_non_socket_file() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let socket_path = dir.path().join("pwsw.sock");

        // Create a regular file at the socket path
        std::fs::write(&socket_path, b"not a socket").expect("write file");

        // Should not remove non-socket files
        assert_eq!(
            cleanup_stale_socket_at(&socket_path)
                .await
                .expect("`cleanup_stale_socket_at` succeeds"),
            StaleSocketCheck::NotASocket
        );
        assert!(
            socket_path.exists(),
            "Non-socket file should not be removed"
        );
    }

    #[tokio::test]
    async fn test_cleanup_stale_socket_active_socket() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let socket_path = dir.path().join("pwsw.sock");

        // Bind and keep the listener alive to simulate active daemon
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).expect("bind socket");

        // Active socket should not be removed
        assert_eq!(
            cleanup_stale_socket_at(&socket_path)
                .await
                .expect("`cleanup_stale_socket_at` succeeds"),
            StaleSocketCheck::Live
        );
        assert!(socket_path.exists(), "Active socket should not be removed");

        drop(listener);
    }

    #[tokio::test]
    async fn test_cleanup_stale_socket_removes_on_econnrefused() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let socket_path = dir.path().join("pwsw.sock");

        // The file exists but nobody listens (ECONNREFUSED)
        make_stale_socket(&socket_path);
        assert!(socket_path.exists());

        assert_eq!(
            cleanup_stale_socket_at(&socket_path)
                .await
                .expect("`cleanup_stale_socket_at` succeeds"),
            StaleSocketCheck::Removed
        );
        assert!(!socket_path.exists(), "stale socket should be removed");
        assert_eq!(
            cleanup_stale_socket_at(&socket_path)
                .await
                .expect("`cleanup_stale_socket_at` succeeds"),
            StaleSocketCheck::Missing
        );
    }

    #[tokio::test]
    async fn test_cleanup_stale_socket_keeps_on_other_errors() {
        use std::os::unix::fs::PermissionsExt;

        // Root bypasses the permission check, so connect would be refused instead
        if current_uid() == 0 {
            return;
        }
        let dir = tempfile::tempdir().expect("create temp dir");
        let socket_path = dir.path().join("pwsw.sock");
        make_stale_socket(&socket_path);
        // No write permission on the socket: connect fails with EACCES, not ECONNREFUSED
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o000))
            .expect("set permissions");

        assert_eq!(
            cleanup_stale_socket_at(&socket_path)
                .await
                .expect("`cleanup_stale_socket_at` succeeds"),
            StaleSocketCheck::ProbeFailed(std::io::ErrorKind::PermissionDenied)
        );
        assert!(
            socket_path.exists(),
            "only ECONNREFUSED proves a socket is stale"
        );
    }

    #[test]
    fn test_request_test_rule_roundtrip() {
        let request = Request::TestRule {
            pattern: "firefox".to_string(),
        };
        let json = serde_json::to_string(&request).expect("serialize");
        let deserialized: Request = serde_json::from_str(&json).expect("deserialize");
        if let Request::TestRule { pattern } = deserialized {
            assert_eq!(pattern, "firefox");
        } else {
            panic!("Expected TestRule variant");
        }
    }

    #[test]
    fn test_request_shutdown_roundtrip() {
        let request = Request::Shutdown;
        let json = serde_json::to_string(&request).expect("serialize");
        let deserialized: Request = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(deserialized, Request::Shutdown));
    }

    // Response serialization roundtrip tests
    #[test]
    fn test_response_status_roundtrip() {
        let response = Response::Status {
            version: "0.3.1".to_string(),
            protocol: PROTOCOL_VERSION,
            uptime_secs: 3600,
            current_sink: Some("test_sink".to_string()),
            current_sink_fallback: true,
            active_window: Some("firefox".to_string()),
            tracked_windows: 2,
            paused: true,
            current_source: Some("Headset Mic".to_string()),
            current_source_fallback: true,
            sources_configured: true,
            backend: Some("wlr".to_string()),
            monitor_info: true,
        };
        let json = serde_json::to_string(&response).expect("serialize");
        let deserialized: Response = serde_json::from_str(&json).expect("deserialize");
        if let Response::Status {
            version,
            protocol,
            uptime_secs,
            current_sink,
            current_sink_fallback,
            active_window,
            tracked_windows,
            paused,
            current_source,
            current_source_fallback,
            sources_configured,
            backend,
            monitor_info,
        } = deserialized
        {
            assert_eq!(backend.as_deref(), Some("wlr"));
            assert!(monitor_info);
            assert_eq!(current_source.as_deref(), Some("Headset Mic"));
            assert!(current_source_fallback && sources_configured);
            assert!(current_sink_fallback);
            assert!(paused);
            assert_eq!(version, "0.3.1");
            assert_eq!(protocol, PROTOCOL_VERSION);
            assert_eq!(uptime_secs, 3600);
            assert_eq!(current_sink.as_deref(), Some("test_sink"));
            assert_eq!(active_window, Some("firefox".to_string()));
            assert_eq!(tracked_windows, 2);
        } else {
            panic!("Expected Status variant");
        }
    }

    /// A `Status` from a daemon that predates protocol versioning still parses
    #[test]
    fn test_response_status_without_protocol_is_protocol_zero() {
        let json = r#"{"type":"Status","version":"v0.5.0","uptime_secs":1,"current_sink":"s","active_window":null,"tracked_windows":0}"#;
        let response: Response = serde_json::from_str(json).expect("deserialize");
        assert!(matches!(response, Response::Status {
            protocol: 0,
            current_sink_fallback: false,
            paused: false,
            current_source: None,
            current_source_fallback: false,
            sources_configured: false,
            backend: None,
            monitor_info: false,
            ..
        }));
    }

    /// A window from a daemon that predates placement reporting parses with no outputs
    #[test]
    fn test_window_info_without_placement_parses() {
        let window: WindowInfo =
            serde_json::from_str(r#"{"id":3,"app_id":"mpv","title":"video"}"#).expect("parse");
        assert_eq!(window.outputs, Vec::<String>::new());
        assert!(!window.minimized);
    }

    #[test]
    fn test_set_source_request_roundtrip() {
        let json = serde_json::to_string(&Request::SetSource {
            source: "Headset Mic".to_string(),
        })
        .expect("serialize");
        assert_eq!(json, r#"{"type":"SetSource","source":"Headset Mic"}"#);
        let back: Request = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(back, Request::SetSource { source } if source == "Headset Mic"));
    }

    #[test]
    fn test_pause_requests_and_state_roundtrip() {
        for request in [Request::Pause, Request::Resume, Request::TogglePause] {
            let json = serde_json::to_string(&request).expect("serialize");
            let back: Request = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(format!("{back:?}"), format!("{request:?}"), "{json}");
        }
        let json = serde_json::to_string(&Response::PauseState {
            paused: true,
            changed: false,
        })
        .expect("serialize");
        assert_eq!(
            json,
            r#"{"type":"PauseState","paused":true,"changed":false}"#
        );
        let back: Response = serde_json::from_str(&json).expect("deserialize");
        assert!(matches!(back, Response::PauseState {
            paused: true,
            changed: false
        }));
    }

    #[test]
    fn test_response_ok_roundtrip() {
        let response = Response::Ok {
            message: "Success".to_string(),
        };
        let json = serde_json::to_string(&response).expect("serialize");
        let deserialized: Response = serde_json::from_str(&json).expect("deserialize");
        if let Response::Ok { message } = deserialized {
            assert_eq!(message, "Success");
        } else {
            panic!("Expected Ok variant");
        }
    }

    /// `Subscribe` and its `StatusUpdate` messages survive the wire format
    #[test]
    fn test_subscribe_and_status_update_roundtrip() {
        let json = serde_json::to_string(&Request::Subscribe).expect("serialize");
        assert_eq!(json, r#"{"type":"Subscribe"}"#);
        assert!(matches!(
            serde_json::from_str::<Request>(&json),
            Ok(Request::Subscribe)
        ));

        let update = StatusUpdate {
            version: "v0.7.0".to_string(),
            protocol: PROTOCOL_VERSION,
            daemon_manager: crate::daemon_manager::DaemonManager::Direct,
            paused: true,
            sink: Some(EndpointStatus {
                desc: "Headphones".to_string(),
                node: "alsa_output.usb".to_string(),
                fallback: true,
                window: Some(RuleWindow {
                    app_id: "discord".to_string(),
                    rule: "Discord".to_string(),
                    rule_index: 2,
                }),
            }),
            source: None,
            sources_configured: false,
        };
        let json = serde_json::to_string(&Response::StatusUpdate(update.clone())).expect("ser");
        assert!(json.starts_with(r#"{"type":"StatusUpdate","#), "{json}");
        assert!(
            !json.contains("title"),
            "no window title on the wire: {json}"
        );
        match serde_json::from_str::<Response>(&json) {
            Ok(Response::StatusUpdate(parsed)) => assert_eq!(parsed, update),
            other => panic!("expected a status update, got {other:?}"),
        }
    }

    #[test]
    fn test_response_error_roundtrip() {
        let response = Response::Error {
            message: "An error occurred".to_string(),
        };
        let json = serde_json::to_string(&response).expect("serialize");
        let deserialized: Response = serde_json::from_str(&json).expect("deserialize");
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
            outputs: vec!["DP-1".to_string(), "HDMI-A-1".to_string()],
            minimized: true,
            matched_on: Some("firefox pattern".to_string()),
            tracked: Some(TrackedInfo {
                sink_name: "hdmi_sink".to_string(),
                sink_desc: "HDMI Output".to_string(),
                fallbacks: vec![FallbackSink {
                    name: "speakers".to_string(),
                    desc: "Speakers".to_string(),
                }],
                sources: vec![FallbackSink {
                    name: "mic".to_string(),
                    desc: "Mic".to_string(),
                }],
                fallback_in_use: Some(FallbackSink {
                    name: "speakers".to_string(),
                    desc: "Speakers".to_string(),
                }),
            }),
        };
        let response = Response::Windows {
            windows: vec![window_info],
        };
        let json = serde_json::to_string(&response).expect("serialize");
        let deserialized: Response = serde_json::from_str(&json).expect("deserialize");
        if let Response::Windows { windows } = deserialized {
            assert_eq!(windows.len(), 1);
            assert_eq!(windows[0].app_id, "firefox");
            assert_eq!(windows[0].title, "Mozilla Firefox");
            assert_eq!(windows[0].outputs, ["DP-1", "HDMI-A-1"]);
            assert!(windows[0].minimized);
            let tracked = windows[0].tracked.as_ref().expect("tracked");
            assert_eq!(tracked.fallbacks[0].desc, "Speakers");
            assert_eq!(tracked.sources[0].desc, "Mic");
            assert_eq!(
                tracked.fallback_in_use.as_ref().map(|f| f.desc.as_str()),
                Some("Speakers")
            );
        } else {
            panic!("Expected Windows variant");
        }
    }

    /// Tracked info from a daemon without fallback support still parses
    #[test]
    fn test_tracked_info_without_fallbacks_parses() {
        let tracked: TrackedInfo =
            serde_json::from_str(r#"{"sink_name":"a","sink_desc":"A"}"#).expect("deserialize");
        assert_eq!(tracked.fallbacks, []);
        assert_eq!(tracked.fallback_in_use, None);
        let json = serde_json::to_string(&tracked).expect("serialize");
        assert!(!json.contains("fallbacks"), "{json}");
        assert!(!json.contains("fallback_in_use"), "{json}");
    }
}
