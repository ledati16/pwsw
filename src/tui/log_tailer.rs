//! Log file tailer for displaying daemon logs in TUI
//!
//! Reads and tails the daemon log file located at `~/.local/share/pwsw/daemon.log`

use anyhow::{Context, Result};
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;

/// Maximum number of log lines to keep in memory
const MAX_LOG_LINES: usize = 1000;

/// Log tailer that reads daemon log file
pub(crate) struct LogTailer {
    log_path: PathBuf,
    lines: Vec<String>,
    last_position: u64,
}

impl LogTailer {
    /// Create a new log tailer
    ///
    /// # Errors
    /// Returns an error if the log file path cannot be determined
    pub fn new() -> Result<Self> {
        let log_path = crate::daemon::get_log_file_path()?;
        Ok(Self {
            log_path,
            lines: Vec::new(),
            last_position: 0,
        })
    }

    /// Read initial log contents (last N lines)
    ///
    /// # Errors
    /// Returns an error if the file cannot be read
    pub fn read_initial(&mut self, max_lines: usize) -> Result<()> {
        if !self.log_path.exists() {
            // Log file doesn't exist yet (daemon not started or no logs)
            return Ok(());
        }

        let file = File::open(&self.log_path)
            .with_context(|| format!("Failed to open log file: {}", self.log_path.display()))?;

        let reader = BufReader::new(file);
        let all_lines: Vec<String> = reader.lines().collect::<std::io::Result<_>>()?;

        // Keep only last N lines
        let start = all_lines.len().saturating_sub(max_lines);
        self.lines = all_lines.into_iter().skip(start).collect();

        // Update position to end of file
        let file = File::open(&self.log_path)?;
        self.last_position = file.metadata()?.len();

        Ok(())
    }

    /// Check for new log lines since last read
    ///
    /// # Errors
    /// Returns an error if the file cannot be read
    pub fn read_new_lines(&mut self) -> Result<Vec<String>> {
        if !self.log_path.exists() {
            return Ok(Vec::new());
        }

        let mut file = File::open(&self.log_path)
            .with_context(|| format!("Failed to open log file: {}", self.log_path.display()))?;

        let current_size = file.metadata()?.len();

        // Handle log rotation (file got smaller)
        if current_size < self.last_position {
            self.last_position = 0;
            self.lines.clear();
        }

        // Seek to last read position
        file.seek(SeekFrom::Start(self.last_position))?;

        let reader = BufReader::new(file);
        let new_lines: Vec<String> = reader.lines().collect::<std::io::Result<_>>()?;

        // Update position
        self.last_position = current_size;

        // Add new lines to buffer
        self.lines.extend(new_lines.iter().cloned());

        // Trim if we exceed max lines
        if self.lines.len() > MAX_LOG_LINES {
            let excess = self.lines.len() - MAX_LOG_LINES;
            self.lines.drain(0..excess);
        }

        Ok(new_lines)
    }

    /// Get all buffered log lines
    #[must_use]
    pub fn get_lines(&self) -> &[String] {
        &self.lines
    }

    /// Get the log file path
    #[must_use]
    pub fn log_path(&self) -> &PathBuf {
        &self.log_path
    }

    /// Check if the log file exists
    #[must_use]
    pub fn log_exists(&self) -> bool {
        self.log_path.exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_log_tailer_reads_initial_logs() {
        use tempfile::NamedTempFile;

        // Create temp log file
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "2025-12-17T10:00:00 INFO test log line 1").unwrap();
        writeln!(temp_file, "2025-12-17T10:00:01 WARN test log line 2").unwrap();
        temp_file.flush().unwrap();

        // Create tailer with this specific path
        let log_path = temp_file.path().to_path_buf();
        let mut tailer = LogTailer {
            log_path,
            lines: Vec::new(),
            last_position: 0,
        };

        // Read initial logs
        tailer.read_initial(100).unwrap();

        // Verify logs were read
        let lines = tailer.get_lines();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("INFO"));
        assert!(lines[1].contains("WARN"));
    }

    #[test]
    fn test_log_tailer_reads_new_lines() {
        use tempfile::NamedTempFile;

        // Create temp file with initial content
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "line 1").unwrap();
        temp_file.flush().unwrap();

        let log_path = temp_file.path().to_path_buf();
        let mut tailer = LogTailer {
            log_path: log_path.clone(),
            lines: Vec::new(),
            last_position: 0,
        };

        // Read initial content
        tailer.read_initial(100).unwrap();
        assert_eq!(tailer.get_lines().len(), 1);

        // Append new lines
        writeln!(temp_file, "line 2").unwrap();
        writeln!(temp_file, "line 3").unwrap();
        temp_file.flush().unwrap();

        // Read new lines
        let new_lines = tailer.read_new_lines().unwrap();
        assert_eq!(new_lines.len(), 2);
        assert_eq!(tailer.get_lines().len(), 3);
    }
}
