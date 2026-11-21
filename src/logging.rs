//! Daemon logging: a size-rotated log file plus a reloadable level filter
//!
//! Lines are written synchronously: each event is on disk (in the page cache) before
//! the logging call returns, so nothing queued is lost when the process aborts on a
//! panic (release builds use `panic = "abort"`). The daemon logs a handful of lines
//! per window switch, so the cost is negligible.

use std::any::Any;
use std::fs::{self, File};
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::panic::Location;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry, fmt, reload};

/// Daemon log file name inside the log directory
const DAEMON_LOG_FILE: &str = "daemon.log";
/// Rotate the daemon log beyond this size
const DAEMON_LOG_MAX_BYTES: u64 = 1_000_000;

/// Handle to the daemon's logging setup (for re-applying the config's `log_level`)
pub struct DaemonLogging {
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

    // Synchronous: `Arc<W>` is a `MakeWriter` because `&RotatingFileAppender: Write`
    let writer = Arc::new(RotatingFileAppender::new(
        dir,
        DAEMON_LOG_FILE,
        DAEMON_LOG_MAX_BYTES,
    ));

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

/// Log panics through `tracing` (message and location), then run the previous hook
///
/// The background daemon's stderr is the log file as it was opened at spawn time; once
/// the log rotates, stderr points at `daemon.log.old` (and later at an unlinked file),
/// so the default hook's message would be lost. Logging it as an event writes it to the
/// current log file before the process aborts.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("{}", panic_message(info.payload(), info.location()));
        previous(info);
    }));
}

/// "panicked at `file:line:col`: message" for a panic payload
fn panic_message(payload: &(dyn Any + Send), location: Option<&Location<'_>>) -> String {
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("(non-string panic payload)");
    let thread = std::thread::current();
    let thread = thread.name().unwrap_or("<unnamed>");
    match location {
        Some(location) => format!("thread '{thread}' panicked at {location}: {message}"),
        None => format!("thread '{thread}' panicked: {message}"),
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
        (&*self).write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&*self).flush()
    }
}

/// Writes go through the internal mutex, so a shared reference is enough (this is
/// what makes `Arc<RotatingFileAppender>` a `tracing` writer)
impl Write for &RotatingFileAppender {
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

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn read(path: &Path) -> String {
        fs::read_to_string(path).expect("read log")
    }

    /// Past the size limit the next write rotates: the old lines move to `.old` and
    /// the new line lands in a fresh `daemon.log`
    #[test]
    fn test_write_after_rotation_lands_in_new_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut appender = RotatingFileAppender::new(dir.path(), "daemon.log", 16);
        appender
            .write_all(b"first line, long enough\n")
            .expect("write");
        appender.write_all(b"second\n").expect("write");
        assert_eq!(read(&dir.path().join("daemon.log")), "second\n");
        assert_eq!(
            read(&dir.path().join("daemon.log.old")),
            "first line, long enough\n"
        );
    }

    /// A `tracing` event is in the file as soon as the logging call returns (no
    /// background writer that an abort would cut off)
    #[test]
    fn test_events_are_written_synchronously() {
        let dir = tempfile::tempdir().expect("tempdir");
        let writer = Arc::new(RotatingFileAppender::new(dir.path(), "daemon.log", 1 << 20));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(Arc::clone(&writer))
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            tracing::error!("written before returning");
            let text = read(&dir.path().join("daemon.log"));
            assert!(text.contains("written before returning"), "{text:?}");
        });
    }

    #[test]
    fn test_panic_message_names_message_and_location() {
        let location = Location::caller();
        let thread = std::thread::current();
        let thread = thread.name().unwrap_or("<unnamed>").to_string();

        let payload: Box<dyn Any + Send> = Box::new("static message");
        assert_eq!(
            panic_message(payload.as_ref(), Some(location)),
            format!("thread '{thread}' panicked at {location}: static message")
        );
        let payload: Box<dyn Any + Send> = Box::new(format!("formatted {}", 42));
        assert_eq!(
            panic_message(payload.as_ref(), None),
            format!("thread '{thread}' panicked: formatted 42")
        );
        let payload: Box<dyn Any + Send> = Box::new(7_u32);
        assert!(
            panic_message(payload.as_ref(), Some(location))
                .ends_with(": (non-string panic payload)")
        );
    }
}
