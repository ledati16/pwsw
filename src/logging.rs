use std::fs::{self, File};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Mutex;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

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
            eprintln!("Failed to rotate log file: {e}");
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
