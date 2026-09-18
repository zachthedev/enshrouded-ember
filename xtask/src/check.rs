//! The single gate.
//!
//! `CONTRIBUTING.md`, continuous integration and the pre-push hook all call
//! `cargo xtask check` and nothing else, which is what keeps the three from
//! drifting apart. The steps run in order and the run stops at the first
//! failure, naming the step.
//!
//! A tool that is not installed is reported by name with the command that
//! installs it, and the gate stops there. It is never skipped quietly, because
//! a gate that reports a pass for a step it did not run is worse than no gate.
//!
//! Every child runs with this crate's own build metadata removed from its
//! environment. Cargo sets `CARGO_PKG_NAME` and sixteen siblings for a binary it
//! launches, and `cargo xtask check` is launched that way. `cargo-machete` reads
//! `CARGO_PKG_NAME` to decide whether its first argument is its own subcommand
//! name, so a child that inherits it reads `machete` as a directory to scan,
//! prints an error about a directory that is not there, and exits zero. The gate
//! would report a pass for a step that examined nothing.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result};

use crate::ui::{Mark, Row, Ui};

/// One step of the gate.
pub(crate) struct Step {
    /// The name the result row carries.
    name: &'static str,
    /// The executable that must be on `PATH` for the step to run.
    requires: &'static str,
    /// How to install it when it is missing.
    pub(crate) install: &'static str,
}

/// Every step, in the order they run.
pub(crate) const STEPS: [Step; 9] = [
    Step {
        name: "fmt",
        requires: "cargo-fmt",
        install: "rustup component add rustfmt",
    },
    Step {
        name: "taplo",
        requires: "taplo",
        install: "cargo install --locked taplo-cli",
    },
    Step {
        name: "clippy",
        requires: "cargo-clippy",
        install: "rustup component add clippy",
    },
    Step {
        name: "tests",
        requires: "cargo",
        install: "rustup toolchain install",
    },
    Step {
        name: "doctests",
        requires: "cargo",
        install: "rustup toolchain install",
    },
    Step {
        name: "deny",
        requires: "cargo-deny",
        install: "cargo install --locked cargo-deny",
    },
    Step {
        name: "machete",
        requires: "cargo-machete",
        install: "cargo install --locked cargo-machete",
    },
    Step {
        name: "audit",
        requires: "cargo-audit",
        install: "cargo install --locked cargo-audit",
    },
    Step {
        name: "prettier",
        requires: "bunx",
        install: "install Bun from https://bun.sh",
    },
];

/// Where `bun install` puts the prettier launcher, per host.
///
/// The gate runs prettier with `--no-install`, so the package has to be in
/// `node_modules` already. Checking for the launcher is what lets a missing
/// install be reported as `bun install` rather than as a fetch from npm.
#[cfg(windows)]
const PRETTIER_LAUNCHER: &str = "node_modules/.bin/prettier.exe";
#[cfg(not(windows))]
const PRETTIER_LAUNCHER: &str = "node_modules/.bin/prettier";

/// Run the gate.
///
/// # Errors
///
/// Returns an error only when a step cannot be started for a reason other than
/// the tool being absent. A step that runs and fails is reported, not raised.
pub fn run(ui: &Ui) -> Result<bool> {
    let root = crate::workspace_root();
    ui.section("check");
    let mut rows: Vec<Row> = Vec::new();
    for step in &STEPS {
        if !ui.quiet() {
            ui.line(&format!("running {}", step.name));
        }
        if !on_path(step.requires) {
            rows.push(Row::new(Mark::Fail, step.name, "did not run").note(format!(
                "{} is not installed: {}",
                step.requires, step.install
            )));
            return Ok(report(ui, &rows, Some(step.name), None));
        }
        if step.name == "prettier" && !root.join(PRETTIER_LAUNCHER).is_file() {
            rows.push(
                Row::new(Mark::Fail, step.name, "did not run")
                    .note("prettier is not in node_modules: bun install"),
            );
            return Ok(report(ui, &rows, Some(step.name), None));
        }
        let (outcome, output) = invoke(step.name, &root)?;
        rows.push(outcome.row(step.name));
        if outcome.failed() {
            return Ok(report(ui, &rows, Some(step.name), output.as_ref()));
        }
    }
    Ok(report(ui, &rows, None, None))
}

/// What one step did.
enum Outcome {
    /// The step ran and passed, with a note for the row.
    Passed(String),
    /// The step ran and failed, with the exit code.
    Failed(String),
}

impl Outcome {
    /// Whether the gate stops here.
    fn failed(&self) -> bool {
        matches!(self, Outcome::Failed(_))
    }

    /// The result row for this step.
    fn row(&self, name: &'static str) -> Row {
        match self {
            Outcome::Passed(note) => Row::new(Mark::Ok, name, "ok").note(note.clone()),
            Outcome::Failed(note) => Row::new(Mark::Fail, name, "failed").note(note.clone()),
        }
    }
}

/// Environment variables that describe this crate rather than the workspace
/// under check.
///
/// `CARGO_MAKEFLAGS` is deliberately absent, so a nested cargo still shares the
/// job slots this process was given.
const CRATE_ENV: &[&str] = &[
    "CARGO_BIN_NAME",
    "CARGO_CRATE_NAME",
    "CARGO_MANIFEST_DIR",
    "CARGO_MANIFEST_PATH",
    "CARGO_PKG_AUTHORS",
    "CARGO_PKG_DESCRIPTION",
    "CARGO_PKG_HOMEPAGE",
    "CARGO_PKG_LICENSE",
    "CARGO_PKG_NAME",
    "CARGO_PKG_REPOSITORY",
    "CARGO_PKG_RUST_VERSION",
    "CARGO_PKG_VERSION",
    "CARGO_PKG_VERSION_MAJOR",
    "CARGO_PKG_VERSION_MINOR",
    "CARGO_PKG_VERSION_PATCH",
    "CARGO_PKG_VERSION_PRE",
    "CARGO_PRIMARY_PACKAGE",
];

/// The directories `cargo machete` walks.
///
/// Naming them keeps the walk off anything vendored beside the workspace, whose
/// manifests belong to whoever owns that tree.
const MACHETE_DIRS: [&str; 2] = ["crates", "xtask"];

/// The doctest command.
///
/// `cargo nextest` runs no doctests at all, so the tests step leaves every
/// documented example unbuilt and a doctest that stops compiling would pass the
/// gate in silence.
const DOCTEST_ARGS: [&str; 3] = ["test", "--workspace", "--doc"];

/// The test runner the tests step prefers.
///
/// The step declares `cargo` as its requirement, because it falls back to
/// `cargo test` when this runner is absent. The choice is made at run time from
/// `PATH`, so this is the only place the name appears.
pub(crate) const PREFERRED_TEST_RUNNER: &str = "cargo-nextest";

/// A target directory of the test step's own.
///
/// `cargo xtask check` runs out of `target/debug/xtask.exe`, and a test build
/// replaces that file. Windows refuses to replace a running executable, so the
/// test step builds somewhere else.
const TEST_TARGET_DIR: &str = "target/check";

/// Run one step and judge it.
fn invoke(name: &str, root: &Path) -> Result<(Outcome, Option<Output>)> {
    match name {
        "fmt" => run_one(root, "cargo", &["fmt", "--check"], String::new(), None),
        // The files and the exclusions are in .taplo.toml, so the same set is
        // formatted whether the gate or an editor runs the tool.
        "taplo" => run_one(root, "taplo", &["fmt", "--check"], String::new(), None),
        "clippy" => run_one(
            root,
            "cargo",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
            String::new(),
            None,
        ),
        "tests" => {
            let target = Some(root.join(TEST_TARGET_DIR));
            if on_path(PREFERRED_TEST_RUNNER) {
                run_one(
                    root,
                    "cargo",
                    &["nextest", "run", "--workspace"],
                    "cargo nextest".to_string(),
                    target,
                )
            } else {
                run_one(
                    root,
                    "cargo",
                    &["test", "--workspace"],
                    "cargo test, because cargo-nextest is absent".to_string(),
                    target,
                )
            }
        }
        // The step runs unconditionally, which costs a second run of the
        // doctests on the rare host where the tests step fell back to
        // `cargo test`. What the gate covers then does not depend on which test
        // runner is installed. The build goes where the test build goes, so the
        // doctests reuse what the step above just compiled.
        "doctests" => run_one(
            root,
            "cargo",
            &DOCTEST_ARGS,
            String::new(),
            Some(root.join(TEST_TARGET_DIR)),
        ),
        "deny" => run_one(root, "cargo", &["deny", "check"], String::new(), None),
        "machete" => {
            let mut args = vec!["machete"];
            args.extend_from_slice(&MACHETE_DIRS);
            run_one(root, "cargo", &args, MACHETE_DIRS.join(", "), None)
        }
        "audit" => run_one(root, "cargo", &["audit"], String::new(), None),
        "prettier" => {
            let args = prettier_args(root);
            let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
            run_one(
                root,
                "bunx",
                &borrowed,
                "markup, JavaScript and TypeScript".to_string(),
                None,
            )
        }
        other => unreachable!("no step named {other}"),
    }
}

/// The `bunx` argument vector for the prettier step.
///
/// `--no-install` makes an absent package a failure rather than a fetch of
/// whatever npm serves as latest, past the lockfile. The launcher check that
/// precedes the step is what turns that failure into an install instruction.
fn prettier_args(root: &Path) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--no-install".to_string(),
        "--bun".to_string(),
        "prettier".to_string(),
        "--check".to_string(),
        "--no-error-on-unmatched-pattern".to_string(),
        "--ignore-path".to_string(),
        ".gitignore".to_string(),
    ];
    if root.join(".prettierignore").is_file() {
        args.push("--ignore-path".to_string());
        args.push(".prettierignore".to_string());
    }
    // A PostToolUse hook formats .js on every edit, so the glob covers every
    // extension prettier owns here rather than the markup alone. .ts is in the
    // list before the first one lands.
    args.push("**/*.{md,yml,yaml,json,js,mjs,cjs,ts}".to_string());
    args
}

/// Run one command and capture what it said.
fn run_one(
    root: &Path,
    program: &str,
    args: &[&str],
    note: String,
    target_dir: Option<PathBuf>,
) -> Result<(Outcome, Option<Output>)> {
    let mut command = Command::new(program);
    command.args(args).current_dir(root);
    for name in CRATE_ENV {
        command.env_remove(name);
    }
    if let Some(dir) = target_dir {
        command.env("CARGO_TARGET_DIR", dir);
    }
    let output = command
        .output()
        .with_context(|| format!("running {program} {}", args.join(" ")))?;
    if output.status.success() {
        Ok((Outcome::Passed(note), None))
    } else {
        let code = output
            .status
            .code()
            .map_or_else(|| "no exit code".to_string(), |code| format!("exit {code}"));
        Ok((Outcome::Failed(code), Some(output)))
    }
}

/// Print the rows, the failing step's output, and the summary.
fn report(ui: &Ui, rows: &[Row], failed: Option<&str>, output: Option<&Output>) -> bool {
    if let Some(output) = output {
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        for line in text.lines() {
            ui.always(line);
        }
    }
    ui.rows(rows);
    ui.divider(48);
    if let Some(step) = failed {
        ui.summary(&format!("the gate failed at {step}"), true);
        return false;
    }
    ui.summary(&format!("{} steps passed", rows.len()), false);
    true
}

/// Whether an executable is reachable through `PATH`.
///
/// Windows spells an executable with an extension from `PATHEXT`, so the lookup
/// tries each one. A missing tool has to be told apart from a tool that ran and
/// failed, and an operating-system error from spawning is not specific enough.
fn on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let extensions: Vec<String> = std::env::var("PATHEXT").map_or_else(
        |_| vec![String::new()],
        |value| {
            std::iter::once(String::new())
                .chain(value.split(';').map(str::to_lowercase))
                .collect()
        },
    );
    std::env::split_paths(&path).any(|dir| {
        extensions.iter().any(|extension| {
            let mut name = program.to_string();
            name.push_str(extension);
            dir.join(name).is_file()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::{CRATE_ENV, DOCTEST_ARGS, MACHETE_DIRS, STEPS, on_path, prettier_args};
    use crate::testutil::TestDir;

    #[test]
    fn the_gate_runs_the_nine_steps_in_the_documented_order() {
        let names: Vec<&str> = STEPS.iter().map(|step| step.name).collect();
        assert_eq!(
            names,
            vec![
                "fmt", "taplo", "clippy", "tests", "doctests", "deny", "machete", "audit",
                "prettier"
            ]
        );
    }

    /// `cargo nextest run` runs no doctests, so a gate whose only test step is
    /// nextest lets a doctest that stops compiling through in silence. The step
    /// is unconditional, so what the gate covers does not depend on which test
    /// runner the host has.
    #[test]
    fn the_gate_runs_doctests_in_their_own_step() {
        assert!(
            STEPS.iter().any(|step| step.name == "doctests"),
            "the gate has no doctests step"
        );
        assert!(
            DOCTEST_ARGS.contains(&"--doc"),
            "the doctests step runs the whole suite again, got {DOCTEST_ARGS:?}"
        );
    }

    #[test]
    fn every_step_names_the_tool_it_needs_and_how_to_install_it() {
        for step in &STEPS {
            assert!(!step.requires.is_empty(), "{} names no tool", step.name);
            assert!(!step.install.is_empty(), "{} names no install", step.name);
        }
    }

    /// `cargo-machete` reads `CARGO_PKG_NAME` and treats its own subcommand
    /// name as a directory when it is set, then exits zero. A gate that
    /// inherits the variable reports a pass for a step that examined nothing.
    #[test]
    fn the_scrubbed_set_covers_the_variable_cargo_machete_reads() {
        assert!(CRATE_ENV.contains(&"CARGO_PKG_NAME"));
    }

    /// Every scrubbed name describes this crate rather than the workspace under
    /// check, so nothing here removes a variable a tool needs.
    #[test]
    fn every_scrubbed_name_is_cargo_build_metadata() {
        for name in CRATE_ENV {
            assert!(name.starts_with("CARGO_"), "{name} is not cargo metadata");
        }
        assert!(
            !CRATE_ENV.contains(&"CARGO_MAKEFLAGS"),
            "a nested cargo needs the job slots this process was given"
        );
    }

    /// A repeated name would hide a missing one in a hand-read of the list.
    #[test]
    fn the_scrubbed_set_has_no_repeats() {
        let mut seen = CRATE_ENV.to_vec();
        let count = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), count, "the scrubbed set repeats a name");
    }

    /// A bare `cargo machete` walks whatever sits beside the workspace.
    #[test]
    fn machete_names_the_directories_it_walks() {
        assert!(MACHETE_DIRS.contains(&"crates"));
        assert!(MACHETE_DIRS.contains(&"xtask"));
    }

    /// The gate never fetches from npm. An absent prettier is a failure that
    /// names `bun install`, not a download of whatever `latest` is today.
    #[test]
    fn prettier_runs_with_no_install_and_the_lockfile_ignore_files() {
        let dir = TestDir::new("prettier-args");
        let plain = prettier_args(dir.path());
        assert_eq!(plain.first().map(String::as_str), Some("--no-install"));
        assert!(plain.contains(&"--check".to_string()));
        assert_eq!(plain.iter().filter(|a| *a == "--ignore-path").count(), 1);

        dir.write(".prettierignore", b".cache/");
        let with_ignore = prettier_args(dir.path());
        assert_eq!(
            with_ignore.iter().filter(|a| *a == "--ignore-path").count(),
            2
        );
        assert!(with_ignore.contains(&".prettierignore".to_string()));
        assert_eq!(
            with_ignore.last().map(String::as_str),
            Some("**/*.{md,yml,yaml,json,js,mjs,cjs,ts}")
        );
    }

    /// A `PostToolUse` hook formats JavaScript on every edit. An extension
    /// prettier owns and the gate does not check is a file whose formatting
    /// nothing enforces.
    #[test]
    fn prettier_checks_every_extension_it_owns_here() {
        let dir = TestDir::new("prettier-extensions");
        let args = prettier_args(dir.path());
        let glob = args.last().expect("the step names a glob");
        let list = glob
            .strip_prefix("**/*.{")
            .and_then(|rest| rest.strip_suffix('}'))
            .unwrap_or_else(|| panic!("{glob} is not a brace list of extensions"));
        let covered: Vec<&str> = list.split(',').collect();

        for extension in ["md", "yml", "yaml", "json", "js", "mjs", "cjs", "ts"] {
            assert!(
                covered.contains(&extension),
                "the prettier glob skips .{extension}, got {glob}"
            );
        }
    }

    #[test]
    fn path_lookup_finds_cargo_and_refuses_a_name_that_is_not_there() {
        assert!(on_path("cargo"), "cargo runs this test, so it is on PATH");
        assert!(!on_path("ember-tool-that-does-not-exist"));
    }
}
