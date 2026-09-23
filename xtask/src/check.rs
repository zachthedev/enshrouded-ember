//! The single gate.
//!
//! `CONTRIBUTING.md`, continuous integration and the pre-push hook all call
//! `cargo xtask check` and nothing else, which is what keeps them from drifting
//! apart. The rows run in order and the run stops at the first failure, naming
//! the row. `cargo xtask check --rows` prints the same table.
//!
//! A tool that is not installed is reported by name with the command that
//! installs it, and the gate stops there. It is never skipped quietly, because
//! a gate that reports a pass for a row it did not run is worse than no gate.
//!
//! Every child runs with this crate's own build metadata removed from its
//! environment. Cargo sets `CARGO_PKG_NAME` and its siblings for a binary it
//! launches, and `cargo-machete` reads that one to decide whether its first
//! argument is its own subcommand name, so a child that inherits it scans
//! nothing and exits zero.

use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, Result};

use crate::ui::{Mark, Row, Ui};

/// Where a row's program comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Program {
    /// A tool mise installs, run by the path `mise which` resolves, so the
    /// binary the lockfile records is the binary that runs.
    Mise(&'static str),
    /// A program the toolchain or the package manager puts on `PATH`.
    Path(&'static str),
}

/// One row of the gate.
pub(crate) struct Step {
    /// The name the result row carries.
    pub(crate) name: &'static str,
    /// What the row checks, as `check --rows` prints it.
    pub(crate) covers: &'static str,
    pub(crate) program: Program,
    pub(crate) args: &'static [&'static str],
    /// What to run when the program is absent.
    pub(crate) install: &'static str,
    /// Variables set for the child, on top of the scrubbed environment.
    pub(crate) env: &'static [(&'static str, &'static str)],
}

/// What to run when a tool mise owns is absent. One command covers every one
/// of them, because `mise.toml` names them all and mise reads it.
pub(crate) const MISE_INSTALL: &str = "mise install --locked";

/// The rows that build test artifacts share a target directory of their own
/// on Windows, where a test build cannot replace the running `xtask.exe` under
/// `target/debug`.
const TEST_ENV: &[(&str, &str)] = if cfg!(windows) {
    &[("CARGO_TARGET_DIR", "target/check")]
} else {
    &[]
};

/// The rustup command that installs the toolchain a cargo row needs.
const RUSTUP: &str = "rustup toolchain install";

/// The install command for a row that runs out of `node_modules`.
const BUN_INSTALL: &str = "bun install";

/// Every row, in the order they run.
pub(crate) const STEPS: &[Step] = &[
    Step {
        name: "fmt",
        covers: "Rust formatting",
        program: Program::Path("cargo"),
        args: &["fmt", "--check"],
        install: "rustup component add rustfmt",
        env: &[],
    },
    Step {
        name: "taplo",
        covers: "TOML formatting, over the files .taplo.toml names",
        program: Program::Mise("taplo"),
        args: &["fmt", "--check", "--config", ".taplo.toml"],
        install: MISE_INSTALL,
        env: &[],
    },
    Step {
        name: "clippy",
        covers: "Lints on every target, warnings denied",
        program: Program::Path("cargo"),
        args: &[
            "clippy",
            "--workspace",
            "--all-targets",
            "--locked",
            "--",
            "-D",
            "warnings",
        ],
        install: "rustup component add clippy",
        env: &[],
    },
    Step {
        name: "tests",
        covers: "The test suites, under cargo-nextest",
        program: Program::Mise("cargo-nextest"),
        args: &["nextest", "run", "--workspace", "--locked"],
        install: MISE_INSTALL,
        env: TEST_ENV,
    },
    Step {
        name: "doctests",
        covers: "Every documented example, which nextest runs none of",
        program: Program::Path("cargo"),
        args: &["test", "--workspace", "--doc", "--locked"],
        install: RUSTUP,
        env: TEST_ENV,
    },
    Step {
        name: "doc",
        covers: "rustdoc over every crate, warnings denied",
        program: Program::Path("cargo"),
        args: &["doc", "--workspace", "--no-deps", "--locked"],
        install: RUSTUP,
        env: &[("RUSTDOCFLAGS", "-D warnings")],
    },
    Step {
        name: "deny",
        covers: "Licenses, bans and sources, per deny.toml",
        program: Program::Mise("cargo-deny"),
        args: &["--locked", "check", "licenses", "bans", "sources"],
        install: MISE_INSTALL,
        env: &[],
    },
    Step {
        name: "machete",
        covers: "Dependencies a crate declares and never uses",
        program: Program::Mise("cargo-machete"),
        // The one path is the workspace root. With no argument at all, the binary
        // indexes an argument that is not there and panics.
        args: &["."],
        install: MISE_INSTALL,
        env: &[],
    },
    Step {
        name: "prettier",
        covers: "Markup, JavaScript and TypeScript formatting",
        program: Program::Path("bunx"),
        args: &["--no-install", "--bun", "prettier", "--check", "."],
        install: BUN_INSTALL,
        env: &[],
    },
    Step {
        name: "typecheck",
        covers: "The types in tools/, which Bun strips rather than checks",
        program: Program::Path("bunx"),
        args: &["--no-install", "--bun", "tsc", "--noEmit"],
        install: BUN_INSTALL,
        env: &[],
    },
    Step {
        name: "tools",
        covers: "The tests in tools/",
        program: Program::Path("bun"),
        args: &["test", "tools"],
        install: "install Bun from https://bun.sh",
        env: &[],
    },
    Step {
        name: "actionlint",
        covers: "Workflow syntax, runner labels, expressions, and run: blocks through ShellCheck",
        program: Program::Mise("actionlint"),
        args: &["-pyflakes="],
        install: MISE_INSTALL,
        env: &[],
    },
    Step {
        name: "zizmor",
        covers: "Workflow pinning, credentials, permissions and injection",
        program: Program::Mise("zizmor"),
        // .github whole, with ignore handling off. A directory input honors
        // .gitignore files, .git/info/exclude and the global excludes, so a
        // committed ignore line could hide a workflow, and --collect=all turns
        // all of them off. The input stays .github, which collects
        // dependabot.yml and never reaches node_modules, target or a worktree.
        args: &[
            "--no-progress",
            "--strict-collection",
            "--collect=all",
            "--config",
            ".github/zizmor.yml",
            ".github",
        ],
        install: MISE_INSTALL,
        env: &[],
    },
];

/// The name the row for the pin rules carries.
pub(crate) const PINS_STEP: &str = "pins";

/// Every way the pin files fall short, with an unreadable file reported as a
/// problem of its own.
pub(crate) fn pin_problems(root: &Path) -> Vec<String> {
    let read = |path: &str| {
        std::fs::read_to_string(root.join(path)).map_err(|err| format!("reading {path}: {err}"))
    };
    let (main, semver) = (crate::pins::MAIN, crate::pins::SEMVER);
    match (
        read(main.pins),
        read(main.lock),
        read(semver.pins),
        read(semver.lock),
    ) {
        (Ok(pins), Ok(lock), Ok(semver_pins), Ok(semver_lock)) => {
            let mut found = crate::pins::problems(&pins, &lock);
            found.extend(crate::pins::semver_problems(
                &pins,
                &semver_pins,
                &semver_lock,
            ));
            found.extend(stray_problems(root));
            found
        }
        (first, second, third, fourth) => [first, second, third, fourth]
            .into_iter()
            .filter_map(Result::err)
            .chain(stray_problems(root))
            .collect(),
    }
}

/// Every other mise configuration or lockfile under `root`, or the reason the
/// tree could not be listed.
fn stray_problems(root: &Path) -> Vec<String> {
    match crate::pins::config_paths(root) {
        Ok(paths) => crate::pins::stray_config_problems(&paths),
        Err(problem) => vec![problem],
    }
}

/// Print the row table: the pin rules, then every row and what it covers.
pub fn rows(ui: &Ui) {
    let mut table = vec![
        Row::new(Mark::Note, PINS_STEP, "")
            .note("both mise pin files and their lockfiles against pins.rs, before any tool runs"),
    ];
    table.extend(
        STEPS
            .iter()
            .map(|step| Row::new(Mark::Note, step.name, "").note(step.covers)),
    );
    ui.rows(&table);
}

/// Run the gate.
///
/// # Errors
///
/// Returns an error only when a row cannot be started for a reason other than
/// the tool being absent. A row that runs and fails is reported, not raised.
pub fn run(ui: &Ui) -> Result<bool> {
    let root = crate::workspace_root();
    ui.section("check");
    let mut rows: Vec<Row> = Vec::new();

    // The pin rules run before any tool. A lockfile entry carrying a url and no
    // checksum installs whatever that url serves, so a rule that ran later would
    // report a finding about a binary that already executed.
    let problems = pin_problems(&root);
    if !problems.is_empty() {
        for problem in &problems {
            ui.line(problem);
        }
        // A stray file or link is removed, never relocked: `mise lock` would
        // read the configuration the finding refuses. The relock is the remedy
        // only when a problem is about a lockfile; an unreadable or malformed
        // pin file needs an edit, not a relock.
        let strays = stray_problems(&root);
        let relocks: Vec<&str> = crate::pins::PAIRS
            .iter()
            .filter(|pair| problems.iter().any(|problem| problem.contains(pair.lock)))
            .map(|pair| pair.relock)
            .collect();
        let remedy = if !strays.is_empty() {
            "remove each file and link named above".to_string()
        } else if relocks.is_empty() {
            format!("fix {}", crate::pins::PINS)
        } else {
            format!("rewrite the lockfile with: {}", relocks.join(", then "))
        };
        rows.push(Row::new(Mark::Fail, PINS_STEP, "did not pass").note(remedy));
        return Ok(report(ui, &rows, Some(PINS_STEP), None));
    }
    rows.push(Row::new(Mark::Ok, PINS_STEP, "ok"));

    for step in STEPS {
        if !ui.quiet() {
            ui.line(&format!("running {}", step.name));
        }
        let (outcome, output) = match prepare(&root, step) {
            Ok(mut command) => invoke(&mut command)?,
            Err(problem) => (Outcome::Unrun(problem), None),
        };
        rows.push(outcome.row(step.name));
        if outcome.stops() {
            return Ok(report(ui, &rows, Some(step.name), output.as_ref()));
        }
    }
    Ok(report(ui, &rows, None, None))
}

/// The command a row runs, or the sentence its row carries when it cannot.
fn prepare(root: &Path, step: &Step) -> Result<Command, String> {
    let program = match step.program {
        Program::Mise(tool) => crate::spawn::mise_which(root, tool)
            .map_err(|reason| format!("{reason}: {}", step.install))?,
        Program::Path(name) => {
            crate::spawn::resolve(name).map_err(|reason| format!("{reason}: {}", step.install))?
        }
    };
    let mut command = crate::spawn::command(&program)?;
    command.args(step.args).current_dir(root);
    for name in CRATE_ENV {
        command.env_remove(name);
    }
    command.envs(step.env.iter().copied());
    match step.name {
        "actionlint" => {
            // actionlint exits zero and prints nothing when its analyzer is
            // absent or will not execute, and no flag changes that. The path
            // mise resolved goes on the command line, and a workflow with one
            // known finding has to come back with that finding before the real
            // run is trusted.
            let analyzer = crate::spawn::mise_which(root, "shellcheck").map_err(|reason| {
                format!(
                    "{reason}, and actionlint would skip the analysis in silence: {MISE_INSTALL}"
                )
            })?;
            let flag = format!("-shellcheck={}", analyzer.display());
            let heard = canary(command.get_program(), step.args, &flag)?;
            if !canary_passed(&heard) {
                return Err("actionlint ran without ShellCheck: the canary workflow came back with no SC2086".to_string());
            }
            command.arg(flag);
        }
        "zizmor" => {
            // Offline in CI, where the gate holds no token: the online audits
            // run in the shared workflows job, the one job that names the
            // token. Locally, online when the host has a GitHub login, so those
            // audits run before a push; offline otherwise.
            let in_ci = std::env::var_os("CI").is_some_and(|value| !value.is_empty());
            match if in_ci { None } else { gh_token() } {
                Some(token) => {
                    command.env("GH_TOKEN", token);
                }
                None => {
                    command.arg("--offline");
                }
            }
        }
        _ => {}
    }
    Ok(command)
}

/// Run `program` over a workflow whose one `run:` block `ShellCheck` flags, and
/// return what it printed.
fn canary(program: &std::ffi::OsStr, args: &[&str], analyzer_flag: &str) -> Result<String, String> {
    let dir = tempfile::tempdir().map_err(|err| format!("creating the canary directory: {err}"))?;
    let workflow = dir.path().join("canary.yml");
    std::fs::write(&workflow, CANARY)
        .map_err(|err| format!("writing the canary workflow: {err}"))?;
    let output = crate::spawn::command(Path::new(program))?
        .args(args)
        .arg(analyzer_flag)
        .arg(&workflow)
        .output()
        .map_err(|err| format!("running the actionlint canary: {err}"))?;
    let mut heard = String::from_utf8_lossy(&output.stdout).into_owned();
    heard.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(heard)
}

/// A workflow with one unquoted expansion, which `ShellCheck` reports as `SC2086`.
const CANARY: &str = "on: push\njobs:\n  canary:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo $GITHUB_REF\n";

/// Whether the canary run reported the finding it was written to trigger.
fn canary_passed(heard: &str) -> bool {
    heard.contains("SC2086")
}

/// The token `gh` holds for github.com, or `None` when nobody is logged in.
fn gh_token() -> Option<String> {
    let gh = crate::spawn::resolve("gh").ok()?;
    let output = crate::spawn::command(&gh)
        .ok()?
        .args(["auth", "token"])
        .output()
        .ok()?;
    let token = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (output.status.success() && !token.is_empty()).then_some(token)
}

/// What one row did.
enum Outcome {
    /// The row ran and passed, with the note its row carries.
    Passed(String),
    /// The row ran and failed, with the exit code.
    Failed(String),
    /// The row could not start, with the sentence saying why.
    Unrun(String),
}

impl Outcome {
    /// Whether the gate stops here.
    fn stops(&self) -> bool {
        !matches!(self, Outcome::Passed(_))
    }

    /// The result row for this step.
    fn row(&self, name: &'static str) -> Row {
        match self {
            Outcome::Passed(note) => Row::new(Mark::Ok, name, "ok").note(note.clone()),
            Outcome::Failed(note) => Row::new(Mark::Fail, name, "failed").note(note.clone()),
            Outcome::Unrun(note) => Row::new(Mark::Fail, name, "did not run").note(note.clone()),
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

/// Run one prepared command and judge it.
fn invoke(command: &mut Command) -> Result<(Outcome, Option<Output>)> {
    let output = command
        .output()
        .with_context(|| format!("running {}", command.get_program().display()))?;
    if output.status.success() {
        let online = command
            .get_envs()
            .any(|(name, value)| name.to_str() == Some("GH_TOKEN") && value.is_some());
        let offline = command
            .get_args()
            .any(|arg| arg.to_str() == Some("--offline"));
        let note = if online {
            "online".to_string()
        } else if offline {
            "offline".to_string()
        } else {
            String::new()
        };
        Ok((Outcome::Passed(note), None))
    } else {
        let code = output
            .status
            .code()
            .map_or_else(|| "no exit code".to_string(), |code| format!("exit {code}"));
        Ok((Outcome::Failed(code), Some(output)))
    }
}

/// Print the rows, the failing row's output, and the summary.
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

#[cfg(test)]
mod tests {
    use super::{CRATE_ENV, Program, STEPS, canary_passed};

    /// The order is the contract `--rows` prints and CONTRIBUTING.md names, so
    /// a row added out of place or twice is caught here.
    #[test]
    fn the_gate_runs_its_rows_in_the_documented_order() {
        let names: Vec<&str> = STEPS.iter().map(|step| step.name).collect();
        assert_eq!(
            names,
            [
                "fmt",
                "taplo",
                "clippy",
                "tests",
                "doctests",
                "doc",
                "deny",
                "machete",
                "prettier",
                "typecheck",
                "tools",
                "actionlint",
                "zizmor",
            ]
        );
        for step in STEPS {
            assert!(
                !step.covers.is_empty(),
                "{} says nothing about what it covers",
                step.name
            );
            assert!(
                !step.install.is_empty(),
                "{} names no install command",
                step.name
            );
        }
    }

    /// A cargo command that resolves dependencies runs `--locked`, so an edit to
    /// a manifest with no relock stops the gate rather than rewriting Cargo.lock.
    #[test]
    fn every_cargo_row_that_resolves_dependencies_is_locked() {
        for step in STEPS {
            let resolves = match step.program {
                Program::Path("cargo") => step.args[0] != "fmt",
                Program::Mise("cargo-nextest" | "cargo-deny") => true,
                _ => false,
            };
            if resolves {
                assert!(
                    step.args.contains(&"--locked"),
                    "{} runs without --locked",
                    step.name
                );
            }
        }
    }

    /// The canary is judged on the finding it was written to produce, so an
    /// analyzer that prints anything else, or nothing, fails it.
    #[test]
    fn the_canary_passes_only_on_its_own_finding() {
        assert!(canary_passed(
            "canary.yml:6:9: shellcheck reported issue in this script: SC2086:info:1:6: Double quote"
        ));
        assert!(!canary_passed(""));
        assert!(!canary_passed(
            "canary.yml:6:9: shellcheck reported issue in this script: SC2046:warning"
        ));
    }

    /// The scrubbed set carries the variable cargo-machete reads, has no
    /// repeats, and names nothing but cargo's build metadata.
    #[test]
    fn the_scrubbed_set_is_cargo_build_metadata_with_no_repeats() {
        assert!(CRATE_ENV.contains(&"CARGO_PKG_NAME"));
        let mut sorted = CRATE_ENV.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), CRATE_ENV.len());
        for name in CRATE_ENV {
            assert!(name.starts_with("CARGO_"), "{name} is not cargo metadata");
            assert_ne!(
                *name, "CARGO_MAKEFLAGS",
                "the job slots have to reach a nested cargo"
            );
        }
    }
}
