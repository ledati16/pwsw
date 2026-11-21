//! Daemon logging: a size-rotated log file plus a reloadable level filter

use std::fs::{self, File};
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use color_eyre::eyre::{Context, Result};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry, fmt, reload};

/// Daemon log file name inside the log directory
const DAEMON_LOG_FILE: &str = "daemon.log";
/// Rotate the daemon log beyond this size
const DAEMON_LOG_MAX_BYTES: u64 = 1_000_000;

/// Handle to the daemon's logging setup
///
/// Keep it alive for the whole run: dropping it flushes and stops the log writer
/// thread, so log lines written after that are lost.
pub struct DaemonLogging {
    _guard: WorkerGuard,
    filter: reload::Handle<EnvFilter, Registry>,
    /// `RUST_LOG` was set: it wins over the config's `log_level`, also on reload
    env_override: bool,
}

/// The filter for a config `log_level`: only this crate logs, at that level
fn level_filter(level: &str) -> EnvFilter {
    EnvFilter::new(format!("pwsw={level}"))
}

/// Install the global `tracing` subscriber for the daemon
///
/// Always writes to `dir/daemon.log` (rotated at 1 MB, one backup) for the TUI; with
/// `to_stderr` (foreground / systemd) also logs to stderr. `RUST_LOG`, when set,
/// overrides `level`.
///
/// # Errors
/// Returns an error if the log directory cannot be created or a global subscriber is
/// already installed.
pub fn init_daemon_logging(dir: &Path, to_stderr: bool, level: &str) -> Result<DaemonLogging> {
    fs::create_dir_all(dir)
        .with_context(|| format!("Failed to create log directory: {}", dir.display()))?;

    let (filter, env_override) = match EnvFilter::try_from_default_env() {
        Ok(filter) => (filter, true),
        Err(_) => (level_filter(level), false),
    };
    let (filter_layer, filter_handle) = reload::Layer::new(filter);

    let appender = RotatingFileAppender::new(dir, DAEMON_LOG_FILE, DAEMON_LOG_MAX_BYTES);
    let (writer, guard) = tracing_appender::non_blocking(appender);

    tracing_subscriber::registry()
        .with(filter_layer)
        .with(to_stderr.then(|| {
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(crate::style::color_enabled(crate::style::Stream::Stderr))
        }))
        .with(fmt::layer().with_writer(writer).with_ansi(false))
        .try_init()
        .context("Failed to install the daemon log subscriber")?;

    Ok(DaemonLogging {
        _guard: guard,
        filter: filter_handle,
        env_override,
    })
}

impl DaemonLogging {
    /// A callback that re-applies a config `log_level` (no-op while `RUST_LOG` is set)
    pub fn level_setter(&self) -> impl Fn(&str) + Send + 'static {
        let handle = self.filter.clone();
        let env_override = self.env_override;
        move |level: &str| {
            if env_override {
                tracing::debug!("RUST_LOG is set; ignoring log_level = {level}");
                return;
            }
            if let Err(e) = handle.reload(level_filter(level)) {
                tracing::warn!("Could not apply log_level = {level}: {e}");
            }
        }
    }
}

/// A file appender that rotates logs based on size.
///
/// Keeps exactly two files:
/// - `current`: The active log file.
/// - `backup`: The previous log file (rotated when `current` exceeds limit).
///
/// Features:
/// - **Size Limit:** Rotates when file exceeds `max_size_bytes`.
/// - **Self-Healing:** Automatically re-creates the file if deleted externally.
/// - **Thread-Safe:** Uses an internal Mutex to coordinate writes.
/// - **Secure:** Sets 0o600 permissions on created files (Unix).
pub struct RotatingFileAppender {
    path: PathBuf,
    backup_path: PathBuf,
    max_size_bytes: u64,
    file: Mutex<Option<File>>,
}

impl RotatingFileAppender {
    /// Create a new rotating file appender.
    ///
    /// # Arguments
    /// * `dir` - Directory to store logs in.
    /// * `filename` - Base filename (e.g., "daemon.log").
    /// * `max_size_bytes` - Maximum size before rotation (e.g., `1_000_000` for 1MB).
    pub fn new(dir: impl Into<PathBuf>, filename: &str, max_size_bytes: u64) -> Self {
        let dir = dir.into();
        let path = dir.join(filename);
        let backup_path = dir.join(format!("{filename}.old"));

        Self {
            path,
            backup_path,
            max_size_bytes,
            file: Mutex::new(None),
        }
    }

    /// Helper to open a file with secure permissions
    fn open_secure(path: &std::path::Path, append: bool) -> io::Result<File> {
        let mut options = fs::OpenOptions::new();
        options.create(true).write(true);

        if append {
            options.append(true);
        } else {
            options.truncate(true);
        }

        #[cfg(unix)]
        {
            options.mode(0o600);
        }

        options.open(path)
    }

    /// Open the file if not open, or re-open if deleted.
    /// Returns the file handle and current size.
    fn get_file<'a>(&self, guard: &'a mut Option<File>) -> io::Result<&'a mut File> {
        // Check if file exists on disk to handle external deletion
        if !self.path.exists() {
            *guard = None; // Force re-open
        }

        if guard.is_none() {
            // Ensure directory exists
            if let Some(parent) = self.path.parent() {
                fs::create_dir_all(parent)?;
            }

            let file = Self::open_secure(&self.path, true)?;
            *guard = Some(file);
        }

        Ok(guard
            .as_mut()
            .expect("guard was set to Some(file) above when it was None"))
    }

    /// Rotate the log file: current -> backup, create new current.
    fn rotate(&self, guard: &mut Option<File>) -> io::Result<()> {
        // Close current file
        *guard = None;

        // Rename current -> backup (overwrites existing backup)
        if self.path.exists() {
            fs::rename(&self.path, &self.backup_path)?;
        }

        // Create new empty file with secure permissions
        let file = Self::open_secure(&self.path, false)?;
        *guard = Some(file);

        Ok(())
    }
}

impl Write for RotatingFileAppender {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut guard = self
            .file
            .lock()
            .map_err(|e| io::Error::other(format!("Log mutex poisoned: {e}")))?;

        // 1. Ensure file is open and check size
        // Note: get_file returns mutable reference, but for metadata we only need immutable access.
        // We block to get the size, then rotate if needed.
        let current_size = match self.get_file(&mut guard) {
            Ok(f) => f.metadata()?.len(),
            Err(_) => 0, // Will attempt to re-open/create below
        };

        // 2. Rotate if needed
        if current_size >= self.max_size_bytes
            && let Err(e) = self.rotate(&mut guard)
        {
            // If rotation fails, try to continue with current file but log to stderr
            crate::errln!("Failed to rotate log file: {e}");
        }

        // 3. Write to file (re-acquiring in case rotation happened)
        let file = self.get_file(&mut guard)?;
        file.write_all(buf)?;

        // Return buf length to satisfy Write trait contract (we wrote everything)
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut guard = self
            .file
            .lock()
            .map_err(|e| io::Error::other(format!("Log mutex poisoned: {e}")))?;

        if let Some(file) = guard.as_mut() {
            file.flush()?;
        }
        Ok(())
    }
}
