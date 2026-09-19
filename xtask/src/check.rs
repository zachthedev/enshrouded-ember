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

use anyhow::{Context, Result};

use crate::ui::{Mark, Row, Ui};

/// One step of the gate.
pub(crate) struct Step {
    /// The name the result row carries.
    pub(crate) name: &'static str,
    /// The executable that must be on `PATH` for the step to run.
    requires: &'static str,
    /// How to install it when it is missing.
    pub(crate) install: &'static str,
    /// The `node_modules/.bin` entry `bun install` has to have written.
    ///
    /// A step naming one is checked for it before it runs, which is what turns
    /// a missing install into `bun install` rather than a fetch of whatever
    /// npm serves as latest. A step that follows one needs no entry of its
    /// own, because the gate stops at the first failure.
    package: Option<&'static str>,
}

/// Every step, in the order they run.
pub(crate) const STEPS: [Step; 13] = [
    Step {
        name: "fmt",
        requires: "cargo-fmt",
        install: "rustup component add rustfmt",
        package: None,
    },
    Step {
        name: "taplo",
        requires: "taplo",
        install: "cargo install --locked taplo-cli",
        package: None,
    },
    Step {
        name: "clippy",
        requires: "cargo-clippy",
        install: "rustup component add clippy",
        package: None,
    },
    Step {
        name: "tests",
        requires: "cargo",
        install: "rustup toolchain install",
        package: None,
    },
    Step {
        name: "doctests",
        requires: "cargo",
        install: "rustup toolchain install",
        package: None,
    },
    Step {
        name: "deny",
        requires: "cargo-deny",
        install: "cargo install --locked cargo-deny",
        package: None,
    },
    Step {
        name: "machete",
        requires: "cargo-machete",
        install: "cargo install --locked cargo-machete",
        package: None,
    },
    Step {
        name: "audit",
        requires: "cargo-audit",
        install: "cargo install --locked cargo-audit",
        package: None,
    },
    Step {
        name: "prettier",
        requires: "bunx",
        install: "install Bun from https://bun.sh",
        package: Some("prettier"),
    },
    Step {
        name: "typecheck",
        requires: "bunx",
        install: "install Bun from https://bun.sh",
        package: Some("tsc"),
    },
    Step {
        name: "tools",
        requires: "bun",
        install: "install Bun from https://bun.sh",
        package: None,
    },
    Step {
        name: "actionlint",
        requires: "actionlint",
        // The version is here and in .github/go-tools, and a test asserts the
        // two agree. actionlint is on neither crates.io nor
        // taiki-e/install-action, so it cannot sit in .github/cargo-tools
        // beside the rest.
        install: "go install github.com/rhysd/actionlint/cmd/actionlint@v1.7.12",
        package: None,
    },
    Step {
        name: "zizmor",
        requires: "zizmor",
        install: "cargo install --locked zizmor",
        package: None,
    },
];

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
        if let Some(package) = step.package
            && !launcher(&root, package).is_file()
        {
            rows.push(
                Row::new(Mark::Fail, step.name, "did not run")
                    .note(format!("{package} is not in node_modules: bun install")),
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
        // Bun strips types rather than checking them, so the tools step would
        // run TypeScript that does not typecheck and never say so.
        "typecheck" => run_one(root, "bunx", &TYPECHECK_ARGS, "tools/".to_string(), None),
        "tools" => run_one(root, "bun", &TOOLS_TEST_ARGS, "bun test".to_string(), None),
        "actionlint" => run_one(
            root,
            "actionlint",
            &ACTIONLINT_ARGS,
            ".github/workflows".to_string(),
            None,
        ),
        "zizmor" => run_one(
            root,
            "zizmor",
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
/// The empty flags turn off the external analyzers. actionlint runs shellcheck
/// and pyflakes when it finds them on `PATH` and says nothing at all when it
/// does not, and `ubuntu-latest` carries shellcheck while `windows-latest` does
/// not. Left on, the matrix legs check different things and the quiet leg
/// reports a pass for an analysis it never ran.
const ACTIONLINT_ARGS: [&str; 2] = ["-shellcheck=", "-pyflakes="];

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
    use super::{
        ACTIONLINT_ARGS, CRATE_ENV, DOCTEST_ARGS, MACHETE_DIRS, STEPS, TOOLS_TEST_ARGS,
        TYPECHECK_ARGS, ZIZMOR_ARGS, launcher, on_path, prettier_args,
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

    /// actionlint runs shellcheck and pyflakes when it finds them on `PATH` and
    /// skips them in silence when it does not. `ubuntu-latest` carries
    /// shellcheck and `windows-latest` does not, so without the flags the matrix
    /// legs check different things and the quiet leg reports a pass for an
    /// analysis it never ran.
    #[test]
    fn actionlint_takes_no_analysis_that_depends_on_what_the_host_has() {
        for flag in ["-shellcheck=", "-pyflakes="] {
            assert!(
                ACTIONLINT_ARGS.contains(&flag),
                "{flag} is absent, so that pass is left to whatever the host has: \
                 {ACTIONLINT_ARGS:?}"
            );
        }
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
