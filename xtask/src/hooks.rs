//! Git hook installation.
//!
//! `bun install` points `core.hooksPath` at `.githooks` through its `prepare`
//! script. A contributor who works on the Rust side and has no Bun still needs
//! the hooks, so the same one-line configuration is reachable from cargo.

use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::ui::{Mark, Ui};

/// The directory holding the repository's hooks.
const HOOKS_DIR: &str = ".githooks";

/// Point `core.hooksPath` at the repository's hook directory.
///
/// # Errors
///
/// Returns an error when git is not reachable or refuses the configuration.
pub fn install(ui: &Ui) -> Result<()> {
    let root = crate::workspace_root();
    let hooks = root.join(HOOKS_DIR);
    if !hooks.is_dir() {
        bail!("no hook directory at {}", hooks.display());
    }
    let output = Command::new("git")
        .args(["config", "core.hooksPath", HOOKS_DIR])
        .current_dir(&root)
        .output()
        .context("running git config")?;
    if !output.status.success() {
        bail!(
            "git refused to set core.hooksPath: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    ui.section("hooks install");
    ui.row(Mark::Ok, "core.hooksPath", HOOKS_DIR);
    ui.line("the commit-msg hook runs commitlint through bunx");
    Ok(())
}
