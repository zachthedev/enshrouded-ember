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
struct Step {
    /// The name the result row carries.
    name: &'static str,
    /// The executable that must be on `PATH` for the step to run.
    requires: &'static str,
    /// How to install it when it is missing.
    install: &'static str,
}

/// Every step, in the order they run.
const STEPS: [Step; 9] = [
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
    let root = workspace_root();
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
const PREFERRED_TEST_RUNNER: &str = "cargo-nextest";

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

/// The directory holding the workspace manifest.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::{
        CRATE_ENV, DOCTEST_ARGS, MACHETE_DIRS, PREFERRED_TEST_RUNNER, STEPS, on_path, prettier_args,
    };
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

    /// A crate's manifest under the workspace, with the names its
    /// cargo-machete ignore list carries.
    fn ignore_lists() -> Vec<(std::path::PathBuf, Vec<String>)> {
        let root = super::workspace_root();
        let mut manifests: Vec<std::path::PathBuf> = vec![root.join("xtask").join("Cargo.toml")];
        if let Ok(entries) = std::fs::read_dir(root.join("crates")) {
            manifests.extend(
                entries
                    .filter_map(std::result::Result::ok)
                    .map(|entry| entry.path().join("Cargo.toml"))
                    .filter(|path| path.is_file()),
            );
        }
        manifests
            .into_iter()
            .filter_map(|manifest| {
                let text = std::fs::read_to_string(&manifest).ok()?;
                let value: toml::Value = toml::from_str(&text).ok()?;
                let ignored = value
                    .get("package")?
                    .get("metadata")?
                    .get("cargo-machete")?
                    .get("ignored")?
                    .as_array()?
                    .iter()
                    .filter_map(|name| name.as_str().map(str::to_string))
                    .collect();
                Some((manifest, ignored))
            })
            .collect()
    }

    /// Every `.rs` file under a crate's `src`.
    fn sources(crate_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let mut pending = vec![crate_dir.join("src")];
        while let Some(dir) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.filter_map(std::result::Result::ok) {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    out.push(path);
                }
            }
        }
        out
    }

    /// Every name a member manifest takes from the workspace table.
    ///
    /// A member opts in with `<name>.workspace = true`, in its own dependency
    /// tables and in any `[target.'cfg(..)'.dependencies]` table.
    fn names_taken_from_the_workspace(manifest: &toml::Value) -> Vec<String> {
        const TABLES: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
        let mut out: Vec<String> = Vec::new();
        let mut collect = |table: Option<&toml::Value>| {
            let Some(table) = table.and_then(toml::Value::as_table) else {
                return;
            };
            for (name, entry) in table {
                let takes_it = entry
                    .get("workspace")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false);
                if takes_it {
                    out.push(name.clone());
                }
            }
        };
        for table in TABLES {
            collect(manifest.get(table));
        }
        if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
            for spec in targets.values() {
                for table in TABLES {
                    collect(spec.get(table));
                }
            }
        }
        out
    }

    /// The workspace manifest and every member manifest it lists.
    fn workspace_and_members() -> (toml::Value, Vec<(std::path::PathBuf, toml::Value)>) {
        let root = super::workspace_root();
        let text = std::fs::read_to_string(root.join("Cargo.toml"))
            .expect("the workspace manifest is readable");
        let workspace: toml::Value = toml::from_str(&text).expect("the workspace manifest parses");
        let members: Vec<String> = workspace
            .get("workspace")
            .and_then(|w| w.get("members"))
            .and_then(toml::Value::as_array)
            .expect("the workspace lists members")
            .iter()
            .filter_map(|m| m.as_str().map(str::to_string))
            .collect();
        let loaded = members
            .into_iter()
            .map(|member| {
                let path = root.join(&member).join("Cargo.toml");
                let text = std::fs::read_to_string(&path)
                    .unwrap_or_else(|err| panic!("reading {}: {err}", path.display()));
                let value = toml::from_str(&text)
                    .unwrap_or_else(|err| panic!("parsing {}: {err}", path.display()));
                (path, value)
            })
            .collect();
        (workspace, loaded)
    }

    /// Every entry in `[workspace.dependencies]` is taken by some member.
    ///
    /// `cargo machete` walks each crate's own tables against that crate's
    /// sources, so an entry here that no member names is invisible to it. Such
    /// an entry pins a version and carries a name that nothing can reach, which
    /// is a trap for whoever reads the table next and assumes it is in use.
    ///
    /// The five `ember-*` path entries are checked like any other. They are
    /// ordinary entries that happen to point inside the workspace, and a crate
    /// dropped from every member's dependencies would leave one orphaned in
    /// exactly the same way.
    #[test]
    fn every_workspace_dependency_is_taken_by_a_member() {
        let (workspace, members) = workspace_and_members();
        let declared: Vec<String> = workspace
            .get("workspace")
            .and_then(|w| w.get("dependencies"))
            .and_then(toml::Value::as_table)
            .expect("the workspace declares dependencies")
            .keys()
            .cloned()
            .collect();
        assert!(!declared.is_empty(), "the workspace table is not empty");

        let mut taken: Vec<String> = Vec::new();
        for (_, manifest) in &members {
            taken.extend(names_taken_from_the_workspace(manifest));
        }
        let orphaned: Vec<&String> = declared
            .iter()
            .filter(|name| !taken.contains(name))
            .collect();
        assert!(
            orphaned.is_empty(),
            "[workspace.dependencies] entries no member takes, so nothing can reach them: {}",
            orphaned
                .iter()
                .map(|name| name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    /// Every member inherits the workspace lint table.
    ///
    /// `[workspace.lints]` is opt in per crate. A member with no `[lints]`
    /// block compiles with none of `missing_docs`, `clippy::pedantic`,
    /// `undocumented_unsafe_blocks` or `unsafe_op_in_unsafe_fn`, and the gate
    /// stays green while saying nothing about it. The crates here write inline
    /// detours, so a missing block is a crate writing unsafe code with the
    /// unsafe lints switched off.
    #[test]
    fn every_member_inherits_the_workspace_lints() {
        let (_, members) = workspace_and_members();
        assert!(!members.is_empty(), "the workspace lists members");
        let missing: Vec<String> = members
            .iter()
            .filter(|(_, manifest)| {
                !manifest
                    .get("lints")
                    .and_then(|lints| lints.get("workspace"))
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false)
            })
            .map(|(path, _)| path.display().to_string())
            .collect();
        assert!(
            missing.is_empty(),
            "these members do not carry `[lints] workspace = true`, so the workspace lints are off for them: {}",
            missing.join(", ")
        );
    }

    /// Every key in `[workspace.package]` is taken by some member.
    ///
    /// The same blind spot in a table that costs less when it drifts: an unused
    /// key here pins no version and reaches no registry, it just describes a
    /// crate nothing inherits.
    #[test]
    fn every_workspace_package_key_is_taken_by_a_member() {
        let (workspace, members) = workspace_and_members();
        let declared: Vec<String> = workspace
            .get("workspace")
            .and_then(|w| w.get("package"))
            .and_then(toml::Value::as_table)
            .expect("the workspace declares package keys")
            .keys()
            .cloned()
            .collect();

        let mut taken: Vec<String> = Vec::new();
        for (_, manifest) in &members {
            let Some(package) = manifest.get("package").and_then(toml::Value::as_table) else {
                continue;
            };
            for (key, entry) in package {
                let inherited = entry
                    .get("workspace")
                    .and_then(toml::Value::as_bool)
                    .unwrap_or(false);
                if inherited {
                    taken.push(key.clone());
                }
            }
        }
        let orphaned: Vec<&String> = declared.iter().filter(|key| !taken.contains(key)).collect();
        assert!(
            orphaned.is_empty(),
            "[workspace.package] keys no member inherits: {}",
            orphaned
                .iter()
                .map(|key| key.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    /// An ignored dependency is one no module references. The moment code
    /// starts using it, its name has to leave the ignore list, or machete
    /// would stop watching a dependency that could later fall out of use
    /// again. This is what makes the lists self-pruning.
    #[test]
    fn every_ignored_dependency_is_still_unreferenced() {
        let lists = ignore_lists();
        assert!(
            !lists.is_empty(),
            "the seven skeleton manifests carry ignore lists"
        );
        let mut stale: Vec<String> = Vec::new();
        for (manifest, ignored) in lists {
            let crate_dir = manifest.parent().expect("a manifest has a directory");
            let files = sources(crate_dir);
            for name in ignored {
                let ident = name.replace('-', "_");
                let needles = [
                    format!("use {ident}"),
                    format!("{ident}::"),
                    format!("extern crate {ident}"),
                ];
                for file in &files {
                    let text = std::fs::read_to_string(file).unwrap_or_default();
                    if needles.iter().any(|needle| text.contains(needle.as_str())) {
                        stale.push(format!(
                            "{} ignores {name}, but {} uses it",
                            manifest.display(),
                            file.display()
                        ));
                        break;
                    }
                }
            }
        }
        assert!(
            stale.is_empty(),
            "prune these names from their ignore lists: {}",
            stale.join("; ")
        );
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

    /// Every string literal in a JavaScript source, paired with the bracket
    /// depth it sits at, in source order.
    ///
    /// Characters inside a literal open no bracket and comments are skipped, so
    /// a URL, an apostrophe in a sentence and a commented-out list all read
    /// correctly.
    fn js_string_literals(source: &str) -> Vec<(usize, String)> {
        let chars: Vec<char> = source.chars().collect();
        let mut found: Vec<(usize, String)> = Vec::new();
        let mut depth: usize = 0;
        let mut index = 0;
        while index < chars.len() {
            match chars[index] {
                '/' if chars.get(index + 1) == Some(&'/') => {
                    while index < chars.len() && chars[index] != '\n' {
                        index += 1;
                    }
                }
                '/' if chars.get(index + 1) == Some(&'*') => {
                    index += 2;
                    while index < chars.len()
                        && !(chars[index] == '*' && chars.get(index + 1) == Some(&'/'))
                    {
                        index += 1;
                    }
                    index += 2;
                }
                '[' => {
                    depth += 1;
                    index += 1;
                }
                ']' => {
                    depth = depth.saturating_sub(1);
                    index += 1;
                }
                quote @ ('\'' | '"' | '`') => {
                    index += 1;
                    let mut text = String::new();
                    while index < chars.len() && chars[index] != quote {
                        if chars[index] == '\\' {
                            index += 1;
                            if index >= chars.len() {
                                break;
                            }
                        }
                        text.push(chars[index]);
                        index += 1;
                    }
                    index += 1;
                    found.push((depth, text));
                }
                _ => index += 1,
            }
        }
        found
    }

    /// The scope list `commitlint.config.js` enforces.
    ///
    /// The `scope-enum` rule is `[level, applicability, [scopes]]`, so the
    /// scopes are the run of literals two brackets deeper than the rule's own
    /// name. An empty result means the file declares no such rule.
    fn commitlint_scopes(source: &str) -> Vec<String> {
        let literals = js_string_literals(source);
        let Some(rule) = literals.iter().position(|(_, text)| text == "scope-enum") else {
            return Vec::new();
        };
        let wanted = literals[rule].0 + 2;
        literals[rule + 1..]
            .iter()
            .skip_while(|(depth, _)| *depth != wanted)
            .take_while(|(depth, _)| *depth == wanted)
            .map(|(_, text)| text.clone())
            .collect()
    }

    /// One `##` section of a markdown document, its heading excluded.
    ///
    /// A heading is a stabler anchor than the sentence under it, so a section
    /// can be reworded without moving what reads it.
    fn section<'a>(markdown: &'a str, heading: &str) -> Option<&'a str> {
        let opening = format!("\n## {heading}\n");
        let start = markdown.find(&opening)? + opening.len();
        let rest = &markdown[start..];
        Some(rest.find("\n## ").map_or(rest, |end| &rest[..end]))
    }

    /// Every inline code span in a markdown fragment.
    fn code_spans(text: &str) -> Vec<String> {
        text.split('`')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect()
    }

    /// Every paragraph of a markdown fragment that is a bare list of inline
    /// code spans, as the spans it holds.
    ///
    /// Anchoring on the paragraph's shape rather than on the sentence above it
    /// means rewording the section around it leaves the check working.
    fn code_span_lists(markdown: &str) -> Vec<Vec<String>> {
        markdown
            .split("\n\n")
            .filter_map(|paragraph| {
                let spans = code_spans(paragraph);
                if spans.is_empty() {
                    return None;
                }
                let mut rest = paragraph.to_string();
                for span in &spans {
                    rest = rest.replacen(&format!("`{span}`"), "", 1);
                }
                rest.chars()
                    .all(|c| c == ',' || c == '.' || c.is_whitespace())
                    .then_some(spans)
            })
            .collect()
    }

    /// The first cell of every body row of the first markdown table in a
    /// fragment, with its inline code markers removed.
    fn table_first_column(markdown: &str) -> Vec<String> {
        markdown
            .lines()
            .filter(|line| line.starts_with('|'))
            .skip(2)
            .filter_map(|line| {
                let cell = line.split('|').nth(1)?.trim();
                Some(cell.trim_matches('`').to_string())
            })
            .collect()
    }

    /// The package an install command names, for a command that is a
    /// `cargo install`.
    ///
    /// The flag and the package can come in either order, so the package is the
    /// first word past `cargo install` that is not a flag.
    fn cargo_install_package(install: &str) -> Option<&str> {
        let mut words = install.split_whitespace();
        if words.next()? != "cargo" || words.next()? != "install" {
            return None;
        }
        words.find(|word| !word.starts_with('-'))
    }

    /// A file at the workspace root, read whole.
    fn read(relative: &str) -> String {
        let path = super::workspace_root().join(relative);
        std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
    }

    /// The pinned tools, as `(name, version)`.
    fn pinned_tools() -> Vec<(String, String)> {
        read(".github/cargo-tools")
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| {
                let (name, version) = line
                    .split_once('@')
                    .unwrap_or_else(|| panic!("{line:?} is not name@version"));
                assert!(!version.is_empty(), "{line:?} has no version");
                (name.to_string(), version.to_string())
            })
            .collect()
    }

    /// The commit scope vocabulary lives in three places: the constant
    /// `cargo xtask scopes` prints, the rule the commit hook enforces, and the
    /// sentence `CONTRIBUTING.md` restates. A contributor who reads one and a
    /// hook that enforces another disagree silently.
    #[test]
    fn every_copy_of_the_scope_list_agrees() {
        let declared: Vec<String> = crate::SCOPES
            .iter()
            .map(|scope| (*scope).to_string())
            .collect();
        assert!(!declared.is_empty(), "xtask/src/main.rs declares no scopes");

        let commitlint = commitlint_scopes(&read("commitlint.config.js"));
        let contributing = read("CONTRIBUTING.md");
        let commits = section(&contributing, "Commit messages")
            .expect("CONTRIBUTING.md has a Commit messages section");
        let documented = code_span_lists(commits);
        assert_eq!(
            documented.len(),
            1,
            "the Commit messages section holds {} paragraphs that are a bare list of code spans, so which one restates the scopes is ambiguous",
            documented.len()
        );

        let mut disagree: Vec<String> = Vec::new();
        if commitlint != declared {
            disagree.push(format!(
                "commitlint.config.js scope-enum has {commitlint:?}"
            ));
        }
        if documented[0] != declared {
            disagree.push(format!("CONTRIBUTING.md restates {:?}", documented[0]));
        }
        assert!(
            disagree.is_empty(),
            "xtask/src/main.rs SCOPES has {declared:?}, and {}",
            disagree.join("; ")
        );
    }

    /// The README's crate table is the map a reader opens first. A member it
    /// omits is a crate nobody looking at the table knows exists.
    ///
    /// The order is asserted too, because the table and `[workspace.members]`
    /// are both in dependency order and are meant to be read side by side.
    #[test]
    fn the_crate_table_lists_every_workspace_member_in_order() {
        let workspace: toml::Value =
            toml::from_str(&read("Cargo.toml")).expect("the workspace manifest parses");
        let members: Vec<String> = workspace
            .get("workspace")
            .and_then(|w| w.get("members"))
            .and_then(toml::Value::as_array)
            .expect("the workspace lists members")
            .iter()
            .filter_map(|member| member.as_str())
            .map(|member| member.rsplit('/').next().unwrap_or(member).to_string())
            .collect();

        let readme = read("README.md");
        let crates = section(&readme, "Crates").expect("README.md has a Crates section");
        let tabled = table_first_column(crates);

        assert_eq!(
            tabled, members,
            "the README crate table and [workspace.members] disagree"
        );
    }

    /// Every tool the gate installs with `cargo install` carries a version, in
    /// one file. A second copy of a version is a copy that drifts.
    ///
    /// The check runs both ways. A step whose tool is unpinned installs
    /// whatever the registry serves today, and a pinned entry no step installs
    /// is a version continuous integration fetches for nothing.
    #[test]
    fn the_pinned_tool_file_and_the_gate_name_the_same_tools() {
        let pinned = pinned_tools();
        assert!(!pinned.is_empty(), "the pinned tool file names nothing");

        // The tests step prefers cargo-nextest at run time rather than
        // declaring it, so its name reaches the pinned file from here.
        assert_eq!(
            pinned
                .iter()
                .filter(|(name, _)| name == PREFERRED_TEST_RUNNER)
                .count(),
            1,
            "{PREFERRED_TEST_RUNNER} is not pinned once in .github/cargo-tools"
        );
        let mut installed: Vec<String> = vec![PREFERRED_TEST_RUNNER.to_string()];
        for step in &STEPS {
            let Some(package) = cargo_install_package(step.install) else {
                continue;
            };
            let found = pinned.iter().filter(|(name, _)| name == package).count();
            assert_eq!(
                found, 1,
                "{package} appears {found} times in .github/cargo-tools"
            );
            installed.push(package.to_string());
        }

        let unused: Vec<&String> = pinned
            .iter()
            .map(|(name, _)| name)
            .filter(|name| !installed.contains(name))
            .collect();
        assert!(
            unused.is_empty(),
            "no gate step installs {unused:?} from .github/cargo-tools"
        );
    }

    /// Ember's manifests are formatted by `taplo`, which reads `.taplo.toml`.
    /// A gate step whose configuration is absent formats whatever the tool
    /// defaults to.
    #[test]
    fn the_toml_formatter_has_a_configuration_file() {
        let config: toml::Value = toml::from_str(&read(".taplo.toml")).expect(".taplo.toml parses");

        assert!(
            config.get("include").is_some(),
            ".taplo.toml names no files to format"
        );
    }

    /// The parser reads the rule's nested array and leaves every other literal
    /// in the file alone, including one inside a comment and one holding a
    /// bracket.
    #[test]
    fn commitlint_scopes_reads_the_nested_rule_array() {
        let cases = [
            (
                "\
export default {
  extends: ['@commitlint/config-conventional'],
  ignores: [(message) => message.includes('Signed-off-by: dependabot[bot]')],
  rules: {
    'scope-enum': [2, 'always', ['one', 'two']],
    'body-max-line-length': [2, 'always', 72],
  },
};
",
                vec!["one", "two"],
            ),
            (
                "\
export default {
  rules: {
    'scope-enum': [
      2,
      'always',
      // A list at https://example.invalid that isn't the rule's own.
      /* 'commented' */
      ['one', 'two', 'three'],
    ],
  },
};
",
                vec!["one", "two", "three"],
            ),
            ("export default { rules: {} };", vec![]),
        ];

        for (source, expected) in cases {
            assert_eq!(commitlint_scopes(source), expected, "{source}");
        }
    }

    /// A paragraph of prose holding code spans is not a list, a bullet is not a
    /// bare paragraph, and a table yields its first column alone.
    #[test]
    fn the_markdown_readers_take_the_shapes_they_name() {
        let markdown = "\
# Title

## First

Run `cargo xtask scopes` for the live list.

`one`, `two`, `three`.

- `four`, `five`

## Second

| Crate   | Holds        |
| ------- | ------------ |
| `alpha` | The first    |
| `beta`  | The second   |

## Third

three
";
        let first = section(markdown, "First").expect("a First section");
        assert_eq!(code_span_lists(first), [["one", "two", "three"]]);

        let second = section(markdown, "Second").expect("a Second section");
        assert_eq!(table_first_column(second), ["alpha", "beta"]);

        assert_eq!(section(markdown, "Fourth"), None);
    }

    /// The flag and the package name come in either order, and a step that
    /// installs through rustup or bun names no package at all.
    #[test]
    fn cargo_install_package_reads_either_argument_order() {
        let cases = [
            ("cargo install --locked cargo-deny", Some("cargo-deny")),
            ("cargo install cargo-deny --locked", Some("cargo-deny")),
            ("cargo install --locked taplo-cli", Some("taplo-cli")),
            ("rustup component add rustfmt", None),
            ("rustup toolchain install", None),
            ("install Bun from https://bun.sh", None),
            ("cargo install", None),
            ("cargo install --locked", None),
            ("", None),
        ];

        for (install, expected) in cases {
            assert_eq!(cargo_install_package(install), expected, "{install:?}");
        }
    }

    #[test]
    fn path_lookup_finds_cargo_and_refuses_a_name_that_is_not_there() {
        assert!(on_path("cargo"), "cargo runs this test, so it is on PATH");
        assert!(!on_path("ember-tool-that-does-not-exist"));
    }
}
