//! Log file tailer for displaying daemon logs in TUI
//!
//! Reads and tails the daemon log file located at `~/.local/share/pwsw/daemon.log`.
//!
//! - Only complete lines are returned; a line still being written is kept until
//!   its newline arrives.
//! - Bytes are decoded lossily, so invalid `UTF-8` can never stall the tail.
//! - Change notifications are filtered to modifications, creations, renames and
//!   removals of the log file (the tailer's own opens and reads are ignored).
//!
//! ## Log Rotation Handling
//!
//! The daemon's `RotatingFileAppender` renames `daemon.log` to `daemon.log.old`
//! when it exceeds 1MB and starts a new `daemon.log`. The tailer remembers the
//! identity (device and inode) of the file it reads; when `daemon.log` becomes a
//! different file it finishes the old one through `daemon.log.old` (if that is
//! still the same file) and then reads the new one from the start.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use color_eyre::eyre::{Context, ContextCompat, Result};
use notify::{Event, EventKind, RecursiveMode, Watcher};

/// How much of the end of the log is read to find the initial lines.
const INITIAL_READ_BYTES: u64 = 256 * 1024;

/// Longest line kept; a longer one is emitted in pieces of this size.
const MAX_LINE_BYTES: usize = 64 * 1024;

/// Most bytes read per call (the rest follows on the next call).
const MAX_READ_BYTES: u64 = 2 * 1024 * 1024;

/// Identity of a file on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileId {
    dev: u64,
    ino: u64,
}

impl FileId {
    fn of(meta: &std::fs::Metadata) -> Self {
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
        }
    }
}

/// Log tailer that reads daemon log file
pub struct LogTailer {
    log_path: PathBuf,
    /// Bytes of the current file consumed so far
    position: u64,
    /// The file `position` refers to
    file_id: Option<FileId>,
    /// Bytes of an unfinished last line
    partial: Vec<u8>,
    _watcher: notify::RecommendedWatcher,
    event_rx: mpsc::Receiver<notify::Result<Event>>,
}

impl LogTailer {
    /// Create a new log tailer with file watching
    ///
    /// # Errors
    /// Returns an error if the log file path cannot be determined or watcher creation fails
    pub fn new() -> Result<Self> {
        Self::with_path(crate::daemon::get_log_file_path()?)
    }

    /// Create a tailer for the log file at `log_path` (its directory is created
    /// if missing and watched).
    ///
    /// # Errors
    /// Returns an error if the directory cannot be created or watched.
    pub fn with_path(log_path: PathBuf) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher =
            notify::recommended_watcher(tx).context("Failed to create file watcher")?;

        // Watch the parent directory (file may not exist yet)
        let watch_dir = log_path
            .parent()
            .context("Log file has no parent directory")?;
        std::fs::create_dir_all(watch_dir)
            .with_context(|| format!("Failed to create log directory: {}", watch_dir.display()))?;
        watcher
            .watch(watch_dir, RecursiveMode::NonRecursive)
            .with_context(|| format!("Failed to watch log directory: {}", watch_dir.display()))?;

        Ok(Self {
            log_path,
            position: 0,
            file_id: None,
            partial: Vec::new(),
            _watcher: watcher,
            event_rx: rx,
        })
    }

    /// Read the last `max_lines` complete lines and start tailing after them.
    ///
    /// Only the end of the file is read. Blocking: call off the async runtime.
    ///
    /// # Errors
    /// Returns an error if the file exists but cannot be read
    pub fn read_initial(&mut self, max_lines: usize) -> Result<Vec<String>> {
        let Some(mut file) = self.open()? else {
            // Log file doesn't exist yet (daemon not started or no logs)
            return Ok(Vec::new());
        };
        let meta = file.metadata()?;
        let start = meta.len().saturating_sub(INITIAL_READ_BYTES);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;

        self.file_id = Some(FileId::of(&meta));
        self.position = start + bytes.len() as u64;
        self.partial.clear();

        // Started mid-file: the first line is incomplete
        let mut chunk = bytes.as_slice();
        if start > 0 {
            chunk = chunk
                .iter()
                .position(|&b| b == b'\n')
                .map_or(&[][..], |nl| &chunk[nl + 1..]);
        }
        let mut lines = self.split_lines(chunk);
        let excess = lines.len().saturating_sub(max_lines);
        lines.drain(..excess);
        Ok(lines)
    }

    /// Open the log file; `None` if it does not exist.
    fn open(&self) -> Result<Option<File>> {
        match File::open(&self.log_path) {
            Ok(file) => Ok(Some(file)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e)
                .with_context(|| format!("Failed to open log file: {}", self.log_path.display())),
        }
    }

    /// Get the backup log file path (daemon.log.old)
    ///
    /// This matches the path used by `RotatingFileAppender` in logging.rs.
    fn backup_path(&self) -> PathBuf {
        let filename = self.log_path.file_name().map_or_else(
            || "daemon.log".to_string(),
            |f| f.to_string_lossy().to_string(),
        );
        self.log_path.with_file_name(format!("{filename}.old"))
    }

    /// Append `bytes` to the unfinished line and return the lines completed.
    fn split_lines(&mut self, bytes: &[u8]) -> Vec<String> {
        self.partial.extend_from_slice(bytes);
        let mut lines = Vec::new();
        let mut rest = self.partial.as_slice();
        while let Some(nl) = rest.iter().position(|&b| b == b'\n') {
            lines.push(decode_line(&rest[..nl]));
            rest = &rest[nl + 1..];
        }
        while rest.len() > MAX_LINE_BYTES {
            lines.push(decode_line(&rest[..MAX_LINE_BYTES]));
            rest = &rest[MAX_LINE_BYTES..];
        }
        self.partial = rest.to_vec();
        lines
    }

    /// Read from `file` at the current position and return the completed lines.
    fn read_from(&mut self, file: &mut File) -> Result<Vec<String>> {
        file.seek(SeekFrom::Start(self.position))?;
        let mut bytes = Vec::new();
        file.take(MAX_READ_BYTES).read_to_end(&mut bytes)?;
        self.position += bytes.len() as u64;
        Ok(self.split_lines(&bytes))
    }

    /// Check for new log lines since last read
    ///
    /// Handles log rotation by finishing the rotated-out file (`daemon.log.old`)
    /// before continuing with the new file, and truncation by starting over.
    /// Blocking (small reads of a local file).
    ///
    /// # Errors
    /// Returns an error if the file cannot be read
    pub fn read_new_lines(&mut self) -> Result<Vec<String>> {
        let Some(mut file) = self.open()? else {
            return Ok(Vec::new());
        };
        let meta = file.metadata()?;
        let id = FileId::of(&meta);
        let mut new_lines = Vec::new();

        match self.file_id {
            Some(old) if old != id => {
                // Rotated: what we were reading is now the backup (if it still is)
                if let Ok(mut old_file) = File::open(self.backup_path())
                    && old_file.metadata().is_ok_and(|m| FileId::of(&m) == old)
                {
                    new_lines.extend(self.read_from(&mut old_file)?);
                }
                self.finish_partial(&mut new_lines);
                self.position = 0;
            }
            // Truncated in place
            _ if meta.len() < self.position => {
                self.partial.clear();
                self.position = 0;
            }
            _ => {}
        }
        self.file_id = Some(id);

        new_lines.extend(self.read_from(&mut file)?);
        Ok(new_lines)
    }

    /// The unfinished line of a file that will not grow any more is complete.
    fn finish_partial(&mut self, lines: &mut Vec<String>) {
        if !self.partial.is_empty() {
            lines.push(decode_line(&self.partial));
            self.partial.clear();
        }
    }

    /// Check if log file has been modified (non-blocking)
    ///
    /// Drains all pending file watch events and returns true if the log file was
    /// written, created, renamed or removed.
    #[must_use]
    pub fn has_file_changed(&self) -> bool {
        let mut changed = false;
        // Drain all pending events (non-blocking)
        while let Ok(event_result) = self.event_rx.try_recv() {
            if let Ok(event) = event_result
                && is_log_change(&event, &self.log_path)
            {
                changed = true;
            }
        }
        changed
    }
}

/// Decode one line (without its newline), replacing invalid `UTF-8`.
fn decode_line(bytes: &[u8]) -> String {
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

/// Whether `event` means the log file's content may have changed. Access
/// events (opening, reading, closing, including the tailer's own) do not.
fn is_log_change(event: &Event, log_path: &Path) -> bool {
    let relevant_kind = matches!(
        event.kind,
        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
    );
    relevant_kind
        && event
            .paths
            .iter()
            .any(|p| p.file_name() == log_path.file_name())
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;
    use std::io::Write;
    use std::time::{Duration, Instant};

    use notify::event::{AccessKind, AccessMode, CreateKind, DataChange, ModifyKind, RenameMode};
    use pretty_assertions::assert_eq;
    use tempfile::TempDir;

    use super::*;

    fn tailer_in(dir: &TempDir) -> (LogTailer, PathBuf) {
        let path = dir.path().join("daemon.log");
        (LogTailer::with_path(path.clone()).expect("tailer"), path)
    }

    fn append(path: &Path, bytes: &[u8]) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("open log");
        file.write_all(bytes).expect("write log");
    }

    #[test]
    fn test_log_tailer_reads_initial_logs() {
        let dir = TempDir::new().expect("tempdir");
        let (mut tailer, path) = tailer_in(&dir);
        append(
            &path,
            b"2025-12-17T10:00:00 INFO line 1\n2025-12-17T10:00:01 WARN line 2\n",
        );

        let lines = tailer.read_initial(100).expect("read");
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("INFO"));
        assert!(lines[1].contains("WARN"));

        // Only the last N are returned
        let (mut tailer, _) = tailer_in(&dir);
        assert_eq!(tailer.read_initial(1).expect("read"), vec![
            "2025-12-17T10:00:01 WARN line 2"
        ]);
    }

    #[test]
    fn test_log_tailer_initial_read_of_large_file_reads_only_the_end() {
        let dir = TempDir::new().expect("tempdir");
        let (mut tailer, path) = tailer_in(&dir);
        let mut content = String::new();
        for i in 0..20_000 {
            let _ = writeln!(content, "line {i:05} padding padding padding");
        }
        append(&path, content.as_bytes());

        let lines = tailer.read_initial(3).expect("read");
        assert_eq!(lines, vec![
            "line 19997 padding padding padding",
            "line 19998 padding padding padding",
            "line 19999 padding padding padding"
        ]);
        append(&path, b"next\n");
        assert_eq!(tailer.read_new_lines().expect("read"), vec!["next"]);
    }

    #[test]
    fn test_log_tailer_reads_new_lines() {
        let dir = TempDir::new().expect("tempdir");
        let (mut tailer, path) = tailer_in(&dir);
        append(&path, b"line 1\n");
        assert_eq!(tailer.read_initial(100).expect("read").len(), 1);

        append(&path, b"line 2\nline 3\n");
        assert_eq!(tailer.read_new_lines().expect("read"), vec![
            "line 2", "line 3"
        ]);
        assert_eq!(tailer.read_new_lines().expect("read"), Vec::<String>::new());
    }

    #[test]
    fn test_log_tailer_keeps_partial_line_until_complete() {
        let dir = TempDir::new().expect("tempdir");
        let (mut tailer, path) = tailer_in(&dir);
        append(&path, b"first\nsec");
        assert_eq!(tailer.read_initial(100).expect("read"), vec!["first"]);

        append(&path, b"ond half");
        assert_eq!(tailer.read_new_lines().expect("read"), Vec::<String>::new());
        append(&path, b"\nthird\n");
        assert_eq!(tailer.read_new_lines().expect("read"), vec![
            "second half",
            "third"
        ]);
    }

    #[test]
    fn test_log_tailer_survives_invalid_utf8() {
        let dir = TempDir::new().expect("tempdir");
        let (mut tailer, path) = tailer_in(&dir);
        assert_eq!(
            tailer.read_initial(100).expect("read"),
            Vec::<String>::new()
        );

        append(&path, b"bad \xff\xfe bytes\n");
        let lines = tailer.read_new_lines().expect("read");
        assert_eq!(lines, vec!["bad \u{fffd}\u{fffd} bytes"]);

        // The position moved on: later lines still arrive
        append(&path, b"after\n");
        assert_eq!(tailer.read_new_lines().expect("read"), vec!["after"]);
    }

    #[test]
    fn test_log_tailer_handles_rotation_by_inode() {
        let dir = TempDir::new().expect("tempdir");
        let (mut tailer, path) = tailer_in(&dir);
        let backup = dir.path().join("daemon.log.old");

        append(&path, b"line 1\nline 2\n");
        assert_eq!(tailer.read_initial(100).expect("read").len(), 2);
        append(&path, b"line 3\nline 4\n");

        // Rotate: the new file is *larger* than what was read of the old one, so
        // a size comparison would not notice
        std::fs::rename(&path, &backup).expect("rotate");
        append(&path, b"line 5 is long enough to exceed the old position\n");

        assert_eq!(tailer.read_new_lines().expect("read"), vec![
            "line 3",
            "line 4",
            "line 5 is long enough to exceed the old position"
        ]);
        append(&path, b"line 6\n");
        assert_eq!(tailer.read_new_lines().expect("read"), vec!["line 6"]);
    }

    #[test]
    fn test_log_tailer_handles_truncation() {
        let dir = TempDir::new().expect("tempdir");
        let (mut tailer, path) = tailer_in(&dir);
        append(&path, b"a long first line\n");
        tailer.read_initial(10).expect("read");

        std::fs::write(&path, b"x\n").expect("truncate");
        assert_eq!(tailer.read_new_lines().expect("read"), vec!["x"]);
    }

    #[test]
    fn test_is_log_change_ignores_access_events() {
        let path = Path::new("/logs/daemon.log");
        let event = |kind| Event::new(kind).add_path(PathBuf::from("/logs/daemon.log"));

        assert!(!is_log_change(
            &event(EventKind::Access(AccessKind::Open(AccessMode::Any))),
            path
        ));
        assert!(!is_log_change(
            &event(EventKind::Access(AccessKind::Close(AccessMode::Read))),
            path
        ));
        assert!(is_log_change(
            &event(EventKind::Modify(ModifyKind::Data(DataChange::Any))),
            path
        ));
        assert!(is_log_change(
            &event(EventKind::Modify(ModifyKind::Name(RenameMode::From))),
            path
        ));
        assert!(is_log_change(
            &event(EventKind::Create(CreateKind::File)),
            path
        ));
        let other = Event::new(EventKind::Modify(ModifyKind::Data(DataChange::Any)))
            .add_path(PathBuf::from("/logs/other.log"));
        assert!(!is_log_change(&other, path));
    }

    #[test]
    fn test_reading_the_log_does_not_count_as_a_change() {
        let dir = TempDir::new().expect("tempdir");
        let (mut tailer, path) = tailer_in(&dir);
        append(&path, b"line\n");

        // The write is noticed...
        let deadline = Instant::now() + Duration::from_secs(5);
        while !tailer.has_file_changed() {
            assert!(Instant::now() < deadline, "write never reported");
            std::thread::sleep(Duration::from_millis(20));
        }
        // One append can produce several modify events that arrive spread out under
        // load; let them all drain (300 ms of quiet) so only the read is measured.
        let mut quiet_since = Instant::now();
        while quiet_since.elapsed() < Duration::from_millis(300) {
            assert!(Instant::now() < deadline, "write events never settled");
            if tailer.has_file_changed() {
                quiet_since = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // ...but the tailer's own open and read of the file are not
        assert_eq!(tailer.read_new_lines().expect("read"), vec!["line"]);
        std::thread::sleep(Duration::from_millis(300));
        assert!(!tailer.has_file_changed(), "own read reported as a change");
    }

    #[test]
    fn test_backup_path_generation() {
        let dir = TempDir::new().expect("tempdir");
        let (tailer, path) = tailer_in(&dir);
        assert_eq!(tailer.backup_path(), path.with_file_name("daemon.log.old"));
    }
}
