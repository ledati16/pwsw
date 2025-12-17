#![allow(dead_code)]

#[cfg(test)]
use std::ffi::OsString;

#[cfg(test)]
/// RAII helper: set `XDG_CONFIG_HOME` to a tempdir for the lifetime of this guard.
pub(crate) struct XdgTemp {
    prev: Option<OsString>,
    dir: tempfile::TempDir,
}

#[cfg(test)]
impl XdgTemp {
    /// Create and activate a temporary `XDG_CONFIG_HOME`.
    ///
    /// # Panics
    ///
    /// Panics if a temporary directory cannot be created.
    #[must_use]
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("failed to create tempdir for XDG_CONFIG_HOME");
        let prev = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
        Self { prev, dir }
    }

    /// Path to the temporary `XDG_CONFIG_HOME` directory.
    #[must_use]
    pub fn path(&self) -> &std::path::Path {
        self.dir.path()
    }
}

#[cfg(test)]
impl Default for XdgTemp {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl Drop for XdgTemp {
    fn drop(&mut self) {
        if let Some(ref val) = self.prev {
            std::env::set_var("XDG_CONFIG_HOME", val);
        } else {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
        // TempDir will be removed when dropped
    }
}

#[cfg(not(test))]
/// Placeholder for non-test builds to keep API stable.
pub struct XdgTemp;

#[cfg(not(test))]
impl XdgTemp {
    pub fn new() -> Self {
        Self
    }
    pub fn path(&self) -> &std::path::Path {
        std::path::Path::new("")
    }
}
