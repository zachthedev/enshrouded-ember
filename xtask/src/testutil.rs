//! Filesystem sandboxing for the test suite.
//!
//! Every test that touches disk works inside its own directory under the
//! process temp directory and removes it on drop, so no case depends on
//! another's leftovers and no case writes into the repository.
//!
//! The directory itself is `tempfile`'s, which handles the name collision and
//! the cleanup. What sits on top is the two helpers the suite needs that
//! `tempfile` does not offer: writing a file with its parents, and pointing a
//! directory link at a target.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

/// A directory that exists for one test and is removed when it drops.
pub struct TestDir {
    inner: TempDir,
}

impl TestDir {
    /// Create an empty directory named after `label`.
    ///
    /// # Panics
    ///
    /// Panics when the directory cannot be created, because a test with no
    /// sandbox has nowhere safe to write.
    pub fn new(label: &str) -> Self {
        let inner = TempDir::with_prefix(format!("ember-xtask-{label}-"))
            .unwrap_or_else(|err| panic!("creating a sandbox for {label}: {err}"));
        Self { inner }
    }

    /// The sandbox root.
    pub fn path(&self) -> &Path {
        self.inner.path()
    }

    /// Write a file inside the sandbox, creating parent directories.
    ///
    /// # Panics
    ///
    /// Panics when the write fails, which means the sandbox is unusable.
    pub fn write(&self, relative: &str, contents: &[u8]) -> PathBuf {
        let target = self.path().join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|err| panic!("create {}: {err}", parent.display()));
        }
        std::fs::write(&target, contents)
            .unwrap_or_else(|err| panic!("write {}: {err}", target.display()));
        target
    }

    /// Try to point `link` at the directory `target`.
    ///
    /// Windows refuses this without developer mode or elevation, so the result
    /// says whether the link exists rather than failing the caller.
    pub fn link_dir(target: &Path, link: &Path) -> bool {
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(target, link);
        #[cfg(not(windows))]
        let made = std::os::unix::fs::symlink(target, link);
        made.is_ok() && link.exists()
    }
}
