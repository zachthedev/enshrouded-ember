//! The single gate.
//!
//! `CONTRIBUTING.md`, continuous integration and the pre-push hook all call
//! `cargo xtask check` and nothing else, which is what keeps them from drifting
//! apart. The steps run in order and the run stops at the first failure, naming
//! the step.
//!
//! A tool that is not installed is reported by name with the command that
//! installs it, and the gate stops there. It is never skipped quietly, because
//! a gate that reports a pass for a step it did not run is worse than no gate.
//!
//! Every child runs with this crate's own build metadata removed from its
//! environment. Cargo sets `CARGO_PKG_NAME` and its siblings for a binary it
//! launches, and `cargo xtask check` is launched that way. `cargo-machete` reads
//! `CARGO_PKG_NAME` to decide whether its first argument is its own subcommand
//! name, so a child that inherits it reads `machete` as a directory to scan,
//! prints an error about a directory that is not there, and exits zero. The gate
//! would report a pass for a step that examined nothing.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, bail};

use crate::ui::{Mark, Row, Ui};

/// One step of the gate.
pub(crate) struct Step {
    /// The name the result row carries.
    pub(crate) name: &'static str,
    /// The executable the step runs.
    pub(crate) requires: &'static str,
    /// Whether mise installs `requires`, under the name `requires` spells.
    ///
    /// The gate resolves such a program through `mise which` and runs the path
    /// it gives back, so the binary it checked is the binary it ran. A program
    /// the toolchain or the package manager provides is not one of these and
    /// runs by name off `PATH`.
    pub(crate) mise: bool,
    /// How to install it when it is missing.
    pub(crate) install: &'static str,
    /// The `node_modules/.bin` entry `bun install` has to have written.
    ///
    /// A step naming one is checked for it before it runs, which is what turns
    /// a missing install into `bun install` rather than a fetch of whatever
    /// npm serves as latest. A step that follows one needs no entry of its
    /// own, because the gate stops at the first failure.
    package: Option<&'static str>,
    /// A second program the step's tool shells out to.
    ///
    /// A step naming one is refused unless that program is installed at the
    /// release its pin file holds, which is what keeps an analysis the tool
    /// performs from depending on what a host happens to carry.
    pub(crate) analyzer: Option<Analyzer>,
}

/// A program a gate tool shells out to.
///
/// actionlint runs shellcheck when it finds one on `PATH` and says nothing at
/// all when it does not, so a host without it reports a pass for an analysis
/// nobody ran. The version comes from [`PINS`] on every run, under this
/// program's own name, so one file holds it.
pub(crate) struct Analyzer {
    /// The executable, resolved through mise and read from [`PINS`].
    pub(crate) program: &'static str,
}

/// Every step, in the order they run.
pub(crate) const STEPS: [Step; 13] = [
    Step {
        name: "fmt",
        requires: "cargo-fmt",
        mise: false,
        install: "rustup component add rustfmt",
        package: None,
        analyzer: None,
    },
    Step {
        name: "taplo",
        requires: "taplo",
        mise: true,
        install: MISE_INSTALL,
        package: None,
        analyzer: None,
    },
    Step {
        name: "clippy",
        requires: "cargo-clippy",
        mise: false,
        install: "rustup component add clippy",
        package: None,
        analyzer: None,
    },
    Step {
        name: "tests",
        requires: "cargo",
        mise: false,
        install: "rustup toolchain install",
        package: None,
        analyzer: None,
    },
    Step {
        name: "doctests",
        requires: "cargo",
        mise: false,
        install: "rustup toolchain install",
        package: None,
        analyzer: None,
    },
    Step {
        name: "deny",
        requires: "cargo-deny",
        mise: true,
        install: MISE_INSTALL,
        package: None,
        analyzer: None,
    },
    Step {
        name: "machete",
        requires: "cargo-machete",
        mise: true,
        install: MISE_INSTALL,
        package: None,
        analyzer: None,
    },
    Step {
        name: "audit",
        requires: "cargo-audit",
        mise: true,
        install: MISE_INSTALL,
        package: None,
        analyzer: None,
    },
    Step {
        name: "prettier",
        requires: "bunx",
        mise: false,
        install: "install Bun from https://bun.sh",
        package: Some("prettier"),
        analyzer: None,
    },
    Step {
        name: "typecheck",
        requires: "bunx",
        mise: false,
        install: "install Bun from https://bun.sh",
        package: Some("tsc"),
        analyzer: None,
    },
    Step {
        name: "tools",
        requires: "bun",
        mise: false,
        install: "install Bun from https://bun.sh",
        package: None,
        analyzer: None,
    },
    Step {
        name: "actionlint",
        requires: "actionlint",
        mise: true,
        install: MISE_INSTALL,
        package: None,
        analyzer: Some(Analyzer {
            program: "shellcheck",
        }),
    },
    Step {
        name: "zizmor",
        requires: "zizmor",
        mise: true,
        install: MISE_INSTALL,
        package: None,
        analyzer: None,
    },
];

/// The file pinning a version for every tool mise installs, read on every run.
///
/// A version is in this file and nowhere else. mise installs from it and the
/// gate reads it back to refuse a binary reporting anything else, so neither
/// carries a copy that can drift.
pub(crate) use crate::pins::PINS;

/// The name the row for the pin rules carries.
pub(crate) const PINS_STEP: &str = "pins";

/// Every way the pin files fall short, with an unreadable file reported as a
/// problem of its own.
pub(crate) fn pin_problems(root: &Path) -> Vec<String> {
    let read = |path: &str| {
        std::fs::read_to_string(root.join(path)).map_err(|err| format!("reading {path}: {err}"))
    };
    match (
        read(crate::pins::PINS),
        read(crate::pins::LOCK),
        read(crate::pins::WORKFLOW),
    ) {
        (Ok(pins), Ok(lock), Ok(workflow)) => crate::pins::problems(&pins, &lock, &workflow),
        (first, second, third) => [first, second, third]
            .into_iter()
            .filter_map(Result::err)
            .collect(),
    }
}

/// What to run when a tool mise owns is absent. One command covers every one of
/// them, because [`PINS`] names them all and mise reads it.
pub(crate) const MISE_INSTALL: &str = "mise install";

/// The path mise installs for `tool`, or `None` when mise resolves none.
///
/// Every pinned tool runs by this path rather than by name, so the binary the
/// gate checked and the binary it ran are the same file by construction rather
/// than by two lookups agreeing. `mise which` exits non-zero for a tool it does
/// not install, and prints one path on the first line when it does.
fn mise_which(root: &Path, tool: &str) -> Option<PathBuf> {
    let output = Command::new("mise")
        .args(["which", tool])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let printed = String::from_utf8_lossy(&output.stdout).into_owned();
    let line = printed.lines().next()?.trim();
    (!line.is_empty()).then(|| PathBuf::from(line))
}

/// Where `bun install` writes a package's launcher, per host.
fn launcher(root: &Path, package: &str) -> PathBuf {
    let name = if cfg!(windows) {
        format!("{package}.exe")
    } else {
        package.to_string()
    };
    root.join("node_modules").join(".bin").join(name)
}

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

    // The pin rules run before any tool. A lockfile entry carrying a url and no
    // checksum installs whatever that url serves, so a rule that ran later would
    // report a finding about a binary that had already executed.
    let problems = pin_problems(&root);
    if problems.is_empty() {
        rows.push(Row::new(Mark::Ok, PINS_STEP, "ok"));
    } else {
        for problem in &problems {
            ui.line(problem);
        }
        rows.push(
            Row::new(Mark::Fail, PINS_STEP, "did not pass").note(format!(
                "rewrite the lockfile with: {}",
                crate::pins::RELOCK
            )),
        );
        return Ok(report(ui, &rows, Some(PINS_STEP), None));
    }

    for step in &STEPS {
        if !ui.quiet() {
            ui.line(&format!("running {}", step.name));
        }
        let program = if step.mise {
            let Some(path) = mise_which(&root, step.requires) else {
                rows.push(Row::new(Mark::Fail, step.name, "did not run").note(format!(
                    "mise resolves no {}: {}",
                    step.requires, step.install
                )));
                return Ok(report(ui, &rows, Some(step.name), None));
            };
            path.display().to_string()
        } else {
            if !on_path(step.requires) {
                rows.push(Row::new(Mark::Fail, step.name, "did not run").note(format!(
                    "{} is not installed: {}",
                    step.requires, step.install
                )));
                return Ok(report(ui, &rows, Some(step.name), None));
            }
            step.requires.to_string()
        };
        if let Some(package) = step.package
            && !launcher(&root, package).is_file()
        {
            rows.push(
                Row::new(Mark::Fail, step.name, "did not run")
                    .note(format!("{package} is not in node_modules: bun install")),
            );
            return Ok(report(ui, &rows, Some(step.name), None));
        }
        let mut resolved: Option<PathBuf> = None;
        if let Some(analyzer) = &step.analyzer {
            match resolve_analyzer(&root, analyzer) {
                Ok(path) => resolved = Some(path),
                Err(problem) => {
                    rows.push(Row::new(Mark::Fail, step.name, "did not run").note(problem));
                    return Ok(report(ui, &rows, Some(step.name), None));
                }
            }
        }
        let (outcome, output) = invoke(step.name, &program, &root, resolved.as_deref())?;
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
///
/// `analyzer` is the resolved path to the step's analyzer, for a step that
/// names one.
fn invoke(
    name: &str,
    program: &str,
    root: &Path,
    analyzer: Option<&Path>,
) -> Result<(Outcome, Option<Output>)> {
    match name {
        "fmt" => run_one(root, "cargo", &["fmt", "--check"], String::new(), None),
        // The files and the exclusions are in .taplo.toml, so the same set is
        // formatted whether the gate or an editor runs the tool.
        "taplo" => run_one(root, program, &["fmt", "--check"], String::new(), None),
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
            // The runner is a pinned tool and the step's own program is not, so
            // this resolves it separately. The binary takes its own subcommand
            // name first, which cargo's dispatch would otherwise supply.
            if let Some(runner) = mise_which(root, PREFERRED_TEST_RUNNER) {
                run_one(
                    root,
                    &runner.display().to_string(),
                    &["nextest", "run", "--workspace"],
                    PREFERRED_TEST_RUNNER.to_string(),
                    target,
                )
            } else {
                run_one(
                    root,
                    program,
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
        "deny" => run_one(root, program, &["check"], String::new(), None),
        "machete" => run_one(root, program, &MACHETE_DIRS, MACHETE_DIRS.join(", "), None),
        "audit" => run_one(root, program, &["audit"], String::new(), None),
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
        // Bun strips types rather than checking them, so the tools step would
        // run TypeScript that does not typecheck and never say so.
        "typecheck" => run_one(root, "bunx", &TYPECHECK_ARGS, "tools/".to_string(), None),
        "tools" => run_one(root, "bun", &TOOLS_TEST_ARGS, "bun test".to_string(), None),
        "actionlint" => {
            let Some(path) = analyzer else {
                bail!("the actionlint step reached its command with no resolved analyzer");
            };
            let args = actionlint_args(path);
            let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
            run_one(
                root,
                program,
                &borrowed,
                ".github/workflows".to_string(),
                None,
            )
        }
        "zizmor" => run_one(
            root,
            program,
            &ZIZMOR_ARGS,
            ".github/workflows, dependabot.yml".to_string(),
            None,
        ),
        other => unreachable!("no step named {other}"),
    }
}

/// The actionlint command.
///
/// No path argument: actionlint resolves the enclosing git repository and
/// reads its `.github/workflows`. It takes files rather than directories, so
/// naming the directory would be a read error rather than a narrowing.
///
/// The empty flag that turns pyflakes off.
///
/// actionlint runs an analyzer it finds on `PATH` and says nothing at all when
/// it does not. No Windows package manager ships pyflakes, so left on it is
/// the analysis one leg runs and the other skips in silence.
const PYFLAKES_OFF: &str = "-pyflakes=";

/// The actionlint command, given the resolved path to shellcheck.
///
/// The path is passed rather than the bare name, so the binary the gate
/// checked the release of is the binary actionlint runs. actionlint exits zero
/// and prints nothing when its analyzer will not start, whether the name
/// resolves to nothing or to something that is not shellcheck, and a name
/// resolved twice is two chances to resolve differently.
fn actionlint_args(analyzer: &Path) -> Vec<String> {
    vec![
        format!("-shellcheck={}", analyzer.display()),
        PYFLAKES_OFF.to_string(),
    ]
}

/// The zizmor command.
///
/// The paths are named so the audit covers what the row says it covers and
/// nothing a walk of the tree happens to reach.
///
/// `--strict-collection` makes a file zizmor cannot parse a failure. Without it
/// zizmor logs a warning, drops the file, and reports no findings for a
/// workflow it never read, which a byte order mark at the top of the file is
/// enough to cause.
///
/// `--offline` keeps it from needing a GitHub token, so a runner and a laptop
/// report the same findings. `--config` names the committed configuration, so
/// `ZIZMOR_CONFIG` in the environment cannot swap it for another.
const ZIZMOR_ARGS: [&str; 7] = [
    "--no-progress",
    "--offline",
    "--strict-collection",
    "--config",
    ".github/zizmor.yml",
    ".github/workflows",
    ".github/dependabot.yml",
];

/// The typecheck command.
///
/// `--no-install` makes an absent package a failure rather than a fetch from
/// npm, the same way the prettier step does. `tsconfig.json` names the files.
const TYPECHECK_ARGS: [&str; 4] = ["--no-install", "--bun", "tsc", "--noEmit"];

/// The command that runs the repository's own TypeScript tests.
///
/// Scoped to `tools`, which is the only directory holding any. A bare
/// `bun test` would walk whatever else a contributor left in the tree.
const TOOLS_TEST_ARGS: [&str; 2] = ["test", "tools"];

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

/// The path `analyzer` resolves to, once it reports the pinned release.
///
/// The step is refused rather than run, because actionlint exits zero and
/// prints nothing when the analyzer will not start. A gate that let that
/// through would report a pass for shell nobody read.
///
/// The resolved path is handed back rather than discarded, so the command the
/// step runs names the binary this checked rather than the name again.
///
/// # Errors
///
/// Returns the sentence the result row carries.
fn resolve_analyzer(root: &Path, analyzer: &Analyzer) -> Result<PathBuf, String> {
    let text =
        std::fs::read_to_string(root.join(PINS)).map_err(|err| format!("reading {PINS}: {err}"))?;
    let pinned = pinned_version(&text, analyzer.program)
        .ok_or_else(|| format!("{PINS} pins no version for {}", analyzer.program))?;
    let path = mise_which(root, analyzer.program).ok_or_else(|| {
        format!(
            "mise resolves no {}, and actionlint skips the analysis in silence: {MISE_INSTALL}",
            analyzer.program
        )
    })?;
    let output = Command::new(&path)
        .arg("--version")
        .output()
        .map_err(|err| format!("running {} --version: {err}", path.display()))?;
    let mut said = String::from_utf8_lossy(&output.stdout).into_owned();
    said.push_str(&String::from_utf8_lossy(&output.stderr));
    match release_problem(analyzer.program, &pinned, reported_release(&said)) {
        Some(problem) => Err(problem),
        None => Ok(path),
    }
}

/// The version [`PINS`] holds for the tool spelled `name`, or `None` when its
/// `[tools]` table has no such entry.
///
/// The value is the version itself, or a table carrying it under `version`,
/// which is the shape an entry with backend options takes. The key is matched
/// whole, so a backend coordinate is never read as the tool at the end of it.
/// Text that is not TOML reads as no version, which the gate reports the same
/// way as a missing entry.
pub(crate) fn pinned_version(text: &str, name: &str) -> Option<String> {
    let document: toml::Value = toml::from_str(text).ok()?;
    match document.get("tools")?.get(name)? {
        toml::Value::String(version) => Some(version.clone()),
        entry => entry.get("version")?.as_str().map(str::to_string),
    }
}

/// The first exact release in `text`: three runs of digits separated by dots.
///
/// A tool names itself and its license around the release it reports, so the
/// shape is what finds it rather than the line it sits on. The shape is the
/// one `is_exact_release` in the policy suite holds a pin to, and the one the
/// mods repository reads, so a release is the same thing everywhere.
fn reported_release(text: &str) -> Option<&str> {
    text.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .filter(|token| !token.is_empty())
        .find(|token| {
            let parts: Vec<&str> = token.split('.').collect();
            parts.len() == 3
                && parts
                    .iter()
                    .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        })
}

/// Why the release `program` reports is not `pinned`, or `None` when it is.
fn release_problem(program: &str, pinned: &str, reported: Option<&str>) -> Option<String> {
    match reported {
        Some(release) if release == pinned => None,
        Some(release) => Some(format!(
            "{program} reports {release} and the pin holds {pinned}, so the two \
             hosts would read a script differently"
        )),
        None => Some(format!(
            "{program} --version named no release, so nothing here can hold it \
             to {pinned}"
        )),
    }
}

/// Whether an executable is reachable through `PATH`.
///
/// A missing tool has to be told apart from a tool that ran and failed, and an
/// operating-system error from spawning is not specific enough. `which` applies
/// `PATHEXT` on Windows and the executable bit elsewhere.
fn on_path(program: &str) -> bool {
    which::which(program).is_ok()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        Analyzer, CRATE_ENV, DOCTEST_ARGS, MACHETE_DIRS, MISE_INSTALL, PINS, PYFLAKES_OFF, STEPS,
        TOOLS_TEST_ARGS, TYPECHECK_ARGS, ZIZMOR_ARGS, actionlint_args, launcher, on_path,
        pinned_version, prettier_args, release_problem, reported_release, resolve_analyzer,
    };
    use crate::testutil::TestDir;

    #[test]
    fn the_gate_runs_its_steps_in_the_documented_order() {
        let names: Vec<&str> = STEPS.iter().map(|step| step.name).collect();
        assert_eq!(
            names,
            vec![
                "fmt",
                "taplo",
                "clippy",
                "tests",
                "doctests",
                "deny",
                "machete",
                "audit",
                "prettier",
                "typecheck",
                "tools",
                "actionlint",
                "zizmor"
            ]
        );
    }

    /// actionlint shells out to shellcheck and pyflakes when it finds them on
    /// `PATH` and skips them in silence when it does not, so an analyzer left
    /// on has to be one every host carries at a pinned release.
    ///
    /// shellcheck is that, and it is named by the path the gate resolved and
    /// version-checked rather than by its bare name, so the binary actionlint
    /// runs is the binary this checked. pyflakes is not: no Windows package
    /// manager ships it, so it stays off and the empty flag holds it off.
    #[test]
    fn actionlint_names_the_resolved_analyzer_and_leaves_pyflakes_off() {
        let resolved = Path::new("/opt/pinned/shellcheck");
        let args = actionlint_args(resolved);
        let shellcheck: Vec<&String> = args
            .iter()
            .filter(|arg| arg.starts_with("-shellcheck="))
            .collect();
        assert_eq!(shellcheck.len(), 1, "got {args:?}");
        assert_eq!(
            shellcheck[0],
            &format!("-shellcheck={}", resolved.display()),
            "actionlint resolves the name again rather than taking the path the gate checked"
        );
        assert!(
            !args.contains(&"-shellcheck=".to_string()),
            "shellcheck is turned off, so no host reads the shell in a workflow step: {args:?}"
        );
        assert!(
            args.contains(&PYFLAKES_OFF.to_string()),
            "pyflakes is left to whatever the host has: {args:?}"
        );
        assert_eq!(PYFLAKES_OFF, "-pyflakes=", "the flag names a pyflakes");
    }

    /// The step that runs actionlint names the analyzer actionlint shells out
    /// to, so the gate can refuse the step rather than let actionlint exit
    /// zero over shell nobody read.
    ///
    /// The pin file is read here too. A pin naming no release leaves the gate
    /// with nothing to hold the installed binary to.
    #[test]
    fn the_actionlint_step_names_the_analyzer_it_shells_out_to() {
        let step = STEPS
            .iter()
            .find(|step| step.name == "actionlint")
            .expect("the gate has an actionlint step");
        let analyzer = step
            .analyzer
            .as_ref()
            .expect("the actionlint step names no analyzer, so a host without one passes");
        assert_eq!(analyzer.program, "shellcheck");

        let text = std::fs::read_to_string(crate::workspace_root().join(PINS))
            .unwrap_or_else(|err| panic!("{PINS}: {err}"));
        let pinned = pinned_version(&text, analyzer.program)
            .unwrap_or_else(|| panic!("{PINS} pins no version for {}", analyzer.program));
        assert!(
            pinned.split('.').count() >= 2
                && pinned
                    .split('.')
                    .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())),
            "{PINS} pins {pinned:?} for {}, which is not a dotted release",
            analyzer.program
        );
    }

    /// No step but the one running actionlint names an analyzer, so nothing
    /// else pays for a tool it does not shell out to.
    #[test]
    fn only_the_step_that_shells_out_names_an_analyzer() {
        let naming: Vec<&str> = STEPS
            .iter()
            .filter(|step| step.analyzer.is_some())
            .map(|step| step.name)
            .collect();
        assert_eq!(naming, ["actionlint"]);
    }

    /// The pin file gives a version per tool, under the key that tool's binary
    /// is named, whether the entry is the bare version or a table carrying
    /// backend options. A tool no entry names has no version, which is a
    /// refusal rather than a default.
    #[test]
    fn pinned_version_reads_each_entry_shape() {
        let document = concat!(
            "[tools]\n",
            "shellcheck = \"0.11.0\"\n",
            "\"github:nextest-rs/nextest\" = { version = \"0.9.145\", version_prefix = \"x-\" }\n",
            "\n[settings]\nlocked = true\n",
        );
        let cases: &[(&str, Option<&str>)] = &[
            ("shellcheck", Some("0.11.0")),
            ("github:nextest-rs/nextest", Some("0.9.145")),
            // A coordinate is one key, so the name at the end of it is not an
            // entry of its own.
            ("nextest", None),
            // A table in another section is not a tool.
            ("locked", None),
            ("", None),
        ];
        for (name, expected) in cases {
            assert_eq!(
                pinned_version(document, name).as_deref(),
                *expected,
                "{name:?}"
            );
        }
        assert_eq!(
            pinned_version("this is not toml = = =", "shellcheck"),
            None,
            "an unparseable pin file reads as no version rather than panicking"
        );
        assert_eq!(
            pinned_version("[settings]\nlocked = true\n", "shellcheck"),
            None,
            "a pin file with no tools table names no version"
        );
    }

    /// A tool prints its name, its license and a website around the release it
    /// reports, so the reader takes the shape rather than the line.
    #[test]
    fn reported_release_finds_the_release_among_the_prose() {
        let shellcheck = "ShellCheck - shell script analysis tool\nversion: 0.11.0\n\
                          license: GNU General Public License, version 3\n\
                          website: https://www.shellcheck.net\n";
        let cases: &[(&str, Option<&str>)] = &[
            (shellcheck, Some("0.11.0")),
            ("1.7.12\n", Some("1.7.12")),
            ("tool 3 build 2.1.0\n", Some("2.1.0")),
            ("tool 3 build 2.1\n", None),
            ("1.4.2.1\n", None),
            ("no release here\n", None),
            ("version 3\n", None),
            ("", None),
        ];
        for (text, expected) in cases {
            assert_eq!(reported_release(text), *expected, "{text:?}");
        }
    }

    /// The whole contract of the comparison: the releases agreeing passes, and
    /// each way it can fail names the release the pin holds.
    ///
    /// A binary that reports a release other than the pinned one is the case
    /// the gate exists for, because the two hosts then read the same script
    /// under different rules while both report a pass.
    #[test]
    fn release_problem_refuses_anything_but_the_pinned_release() {
        assert_eq!(
            release_problem("shellcheck", "0.11.0", Some("0.11.0")),
            None
        );

        for reported in [Some("0.10.0"), Some("0.11.1"), Some("1.11.0"), None] {
            let problem = release_problem("shellcheck", "0.11.0", reported)
                .unwrap_or_else(|| panic!("{reported:?} was accepted"));
            assert!(problem.contains("shellcheck"), "{problem}");
            assert!(problem.contains("0.11.0"), "{problem}");
        }
    }

    /// An analyzer whose pin file is missing, or holds nothing but comments,
    /// refuses the step rather than letting actionlint skip the analysis.
    #[test]
    fn an_unreadable_pin_refuses_the_step() {
        let dir = TestDir::new("analyzer-pin");
        let analyzer = Analyzer {
            program: "shellcheck",
        };
        let missing =
            resolve_analyzer(dir.path(), &analyzer).expect_err("no pin file is a problem");
        assert!(missing.contains(PINS), "{missing}");

        dir.write(PINS, b"[tools]\ntaplo = \"0.10.0\"\n");
        let empty = resolve_analyzer(dir.path(), &analyzer).expect_err("a pin naming no version");
        assert!(empty.contains("pins no version for shellcheck"), "{empty}");
    }

    /// An analyzer mise resolves nowhere refuses the step and says what to
    /// run, because actionlint would otherwise exit zero over shell nobody
    /// read.
    #[test]
    fn an_absent_analyzer_refuses_the_step_and_says_what_to_run() {
        const ABSENT: &str = "ember-analyzer-that-does-not-exist";
        let dir = TestDir::new("analyzer-absent");
        dir.write(PINS, format!("[tools]\n{ABSENT} = \"0.11.0\"\n").as_bytes());
        let analyzer = Analyzer { program: ABSENT };
        let problem = resolve_analyzer(dir.path(), &analyzer).expect_err("an absent analyzer");
        assert!(problem.contains(ABSENT), "{problem}");
        assert!(problem.contains(MISE_INSTALL), "{problem}");
    }

    /// A bare `.` audits whatever a walk of the tree reaches, and the mods
    /// repository vendors this one, so the same habit there audits Ember's
    /// workflows from the wrong gate. The paths are named here to match.
    #[test]
    fn zizmor_names_its_paths_rather_than_walking_the_tree() {
        assert!(
            ZIZMOR_ARGS.iter().any(|arg| arg.starts_with(".github/")),
            "zizmor names no path under .github, so it walks the whole tree: {ZIZMOR_ARGS:?}"
        );
        assert!(
            !ZIZMOR_ARGS.contains(&"."),
            "zizmor walks the whole tree: {ZIZMOR_ARGS:?}"
        );
        assert!(
            ZIZMOR_ARGS.contains(&"--offline"),
            "zizmor reaches for GitHub, so a laptop and a runner can disagree: {ZIZMOR_ARGS:?}"
        );
    }

    /// zizmor drops a workflow it cannot parse, logs a warning, and reports no
    /// findings, so a byte order mark at the top of a workflow hides every
    /// finding in it. `--strict-collection` turns that into a failure.
    ///
    /// zizmor also reads its configuration from `ZIZMOR_CONFIG`. Naming the
    /// committed file keeps an environment variable from changing what the
    /// gate reports.
    #[test]
    fn zizmor_fails_on_an_unread_file_and_reads_only_the_committed_config() {
        assert!(
            ZIZMOR_ARGS.contains(&"--strict-collection"),
            "zizmor skips a file it cannot parse and still passes: {ZIZMOR_ARGS:?}"
        );
        let config = ZIZMOR_ARGS
            .iter()
            .position(|argument| *argument == "--config")
            .and_then(|at| ZIZMOR_ARGS.get(at + 1));
        assert_eq!(
            config,
            Some(&".github/zizmor.yml"),
            "zizmor takes its configuration from wherever the environment says: {ZIZMOR_ARGS:?}"
        );
    }

    /// Bun strips types rather than checking them, so the TypeScript under
    /// `tools` would run without its types ever being read.
    #[test]
    fn the_gate_typechecks_the_typescript_it_runs() {
        assert!(TYPECHECK_ARGS.contains(&"tsc"));
        assert!(TYPECHECK_ARGS.contains(&"--noEmit"));
        assert!(
            TYPECHECK_ARGS.contains(&"--no-install"),
            "the gate never fetches from npm, got {TYPECHECK_ARGS:?}"
        );
    }

    /// Prettier checks how the TypeScript is laid out and nothing else, so a
    /// gate without this step covers no behavior in `tools` at all.
    #[test]
    fn the_gate_runs_the_typescript_tests() {
        assert_eq!(TOOLS_TEST_ARGS, ["test", "tools"]);
    }

    /// A step running out of `node_modules` is checked for its launcher first,
    /// so an absent install is reported as `bun install`.
    ///
    /// The set is derived from the steps rather than written out here. A
    /// hardcoded pair passes untouched when a later step is added without a
    /// package, which is the one case this is for.
    #[test]
    fn every_step_running_from_node_modules_names_its_package() {
        let through_bunx: Vec<&&str> = STEPS
            .iter()
            .filter(|step| step.requires == "bunx")
            .map(|step| &step.name)
            .collect();
        assert!(
            !through_bunx.is_empty(),
            "no step runs through bunx, so this case checked nothing"
        );
        for step in STEPS.iter().filter(|step| step.requires == "bunx") {
            assert!(
                step.package.is_some(),
                "{} runs `bunx --no-install` and names no package, so a missing \
                 install reports as a failed command rather than `bun install`",
                step.name
            );
        }
    }

    #[test]
    fn a_launcher_resolves_under_node_modules() {
        let dir = TestDir::new("launcher");
        let path = launcher(dir.path(), "tsc");
        assert!(path.starts_with(dir.path()));
        assert!(path.to_string_lossy().contains("node_modules"));
        assert!(
            path.file_stem().and_then(|stem| stem.to_str()) == Some("tsc"),
            "the launcher is named for the package, got {}",
            path.display()
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

    /// A bare `cargo machete` walks whatever sits beside the workspace, so the
    /// step names its directories, and they are the ones the workspace members
    /// live under. A member added under a new directory is a crate machete
    /// never reads until the list names it.
    #[test]
    fn machete_walks_the_directories_the_members_live_under() {
        let manifest: toml::Value = toml::from_str(
            &std::fs::read_to_string(crate::workspace_root().join("Cargo.toml"))
                .expect("the workspace manifest is readable"),
        )
        .expect("the workspace manifest parses");
        let mut members: Vec<&str> = manifest["workspace"]["members"]
            .as_array()
            .expect("the workspace lists members")
            .iter()
            .filter_map(toml::Value::as_str)
            .filter_map(|member| member.split('/').next())
            .collect();
        members.sort_unstable();
        members.dedup();

        let mut walked: Vec<&str> = MACHETE_DIRS.to_vec();
        walked.sort_unstable();
        assert_eq!(
            walked, members,
            "cargo machete walks {walked:?} and the members live under {members:?}"
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

    /// The extensions in a `{a,b}` brace list, sorted.
    fn brace_list(pattern: &str) -> Vec<String> {
        let open = pattern
            .find('{')
            .unwrap_or_else(|| panic!("{pattern} holds no brace list"));
        let close = pattern
            .rfind('}')
            .unwrap_or_else(|| panic!("{pattern} holds no brace list"));
        let mut list: Vec<String> = pattern[open + 1..close]
            .split(',')
            .map(str::to_string)
            .collect();
        list.sort();
        list
    }

    /// `.editorconfig` names the extensions prettier owns here, in the section
    /// its comment introduces, and prettier reads that file. An extension it
    /// owns that the gate's glob leaves out is a file whose formatting nothing
    /// enforces, and one the glob adds is formatted to a width no editor
    /// agrees with.
    #[test]
    fn prettier_checks_the_extensions_editorconfig_gives_it() {
        let editorconfig = std::fs::read_to_string(crate::workspace_root().join(".editorconfig"))
            .expect(".editorconfig is readable");
        let section = editorconfig
            .lines()
            .skip_while(|line| !(line.starts_with('#') && line.contains("prettier owns")))
            .find(|line| line.starts_with('['))
            .expect(".editorconfig introduces the section prettier owns");
        let owned = brace_list(section);

        let dir = TestDir::new("prettier-extensions");
        let args = prettier_args(dir.path());
        let glob = args.last().expect("the step names a glob");
        assert!(
            glob.starts_with("**/*.{"),
            "{glob} is not a brace list of extensions"
        );
        assert_eq!(
            brace_list(glob),
            owned,
            "the prettier glob and the .editorconfig section it owns disagree"
        );
    }

    #[test]
    fn path_lookup_finds_cargo_and_refuses_a_name_that_is_not_there() {
        assert!(on_path("cargo"), "cargo runs this test, so it is on PATH");
        assert!(!on_path("ember-tool-that-does-not-exist"));
    }
}
