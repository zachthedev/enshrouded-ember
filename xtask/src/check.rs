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
//! Every row that walks the tree hands its tool the tracked files it reads,
//! proves from the tool's own report that each one was checked, and fails when
//! none was.
//!
//! Every command goes through a [`Runner`], so a test drives the table with a
//! fake and spawns nothing. The one filesystem side effect outside the runner
//! is the canary workflow, written to a temporary directory of its own.

use std::io::{self, Write};
use std::path::Path;

use owo_colors::{OwoColorize, Stream};

use crate::runner::{Captured, Exit, Runner};
use crate::{pins, proof, shellcheck, tree};

/// Every TypeScript project config this repository keeps. The tree rules
/// refuse any other `tsconfig.json` or `jsconfig.json`, and read each of
/// these along its `extends` chain for an option that sends an import into
/// repository code.
const PROJECT_CONFIGS: &[&str] = &["tsconfig.json"];

/// Where a row's program comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program {
    /// A tool mise installs, run by the path `mise which` resolves, so the
    /// binary the lockfile records is the binary that runs.
    Mise(&'static str),
    /// A program the toolchain or the package manager puts on `PATH`.
    Path(&'static str),
}

/// What a row proves it read, beyond its exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proof {
    /// The row walks no tree, and its exit code is its result.
    Exit,
    /// rustfmt names every file it formats, and every tracked Rust file must
    /// be among them.
    Rustfmt,
    /// taplo names the files it found, which must be the tracked TOML files
    /// handed to it.
    Taplo,
    /// cargo-machete names every directory it analyzed, which must be the
    /// tracked crates handed to it.
    Machete,
    /// Prettier's own `getFileInfo` picks the tracked files it formats, and the
    /// row hands it exactly those.
    Prettier,
    /// actionlint names every workflow it linted, which must be the tracked
    /// workflows handed to it.
    Actionlint,
    /// zizmor names every input it completed, which must cover the tracked
    /// workflows. A second pass with no config and no ignores holds every job
    /// passing `secrets: inherit` to the shared workflows.
    Zizmor,
    /// tsc names every file it read, which must be the tracked TypeScript
    /// files `tsconfig.json` includes.
    Typecheck,
    /// `bun test` says how many test files it ran, which must be the tracked
    /// ones.
    BunTest,
    /// `cargo test --doc` counts every documented example, and the row fails on
    /// a filtered run, on every example ignored, and on none unless
    /// [`NO_DOC_EXAMPLES`] declares none.
    Doctests,
}

/// Whether this repository declares it holds no documented example. It holds
/// several, so the doctests row fails on a run that counts none.
const NO_DOC_EXAMPLES: bool = false;

/// One row of the gate.
pub struct Step {
    /// The name the result row carries.
    pub name: &'static str,
    /// What the row checks, as `check --rows` prints it.
    pub covers: &'static str,
    /// Where the program comes from.
    pub program: Program,
    /// The arguments after the program.
    pub args: &'static [&'static str],
    /// What to run when the program is absent.
    pub install: &'static str,
    /// Variables set for the child, on top of the scrubbed environment.
    pub env: &'static [(&'static str, &'static str)],
    /// What the row proves it read.
    pub proof: Proof,
}

/// What to run when a tool mise owns is absent. One command covers every one
/// of them, because `mise.toml` names them all and mise reads it.
pub const MISE_INSTALL: &str = "mise install --locked";

/// The rustup command that installs the toolchain a cargo row needs.
const RUSTUP: &str = "rustup toolchain install";

/// What to run when the Bun packages a row starts are absent.
const BUN_INSTALL: &str = "bun install";

/// Prettier's command-line name, which `bunx --bun --no-install` runs once the
/// row finds it in the checkout's `node_modules/.bin`.
const PRETTIER: &str = "prettier";

/// The flag every Bun the gate starts itself carries. Bun loads an env file
/// beside it into every `bun <file>`, `bun -e` and `bun test`, an untracked one
/// included. bunx takes the flag and never passes it on, so a tool it starts
/// loads the file.
const NO_ENV_FILE: &str = "--no-env-file";

/// Where the install puts `tool`'s command in the checkout, as bunx finds it
/// first: `node_modules/.bin/<tool>`, an `.exe` on Windows.
fn installed_bin(tool: &str) -> String {
    let extension = if cfg!(windows) { ".exe" } else { "" };
    format!("node_modules/.bin/{tool}{extension}")
}

/// The command that runs `script` under Bun, reading the paths it is handed
/// on standard input.
fn bun_script(script: &str) -> Vec<String> {
    ["bun", NO_ENV_FILE, "-e", script]
        .map(str::to_string)
        .to_vec()
}

/// tsc's command-line name, which `bunx --bun --no-install` runs the same way.
const TSC: &str = "tsc";

/// The rows that build test artifacts share a target directory of their own
/// on Windows, where a test build cannot replace the running `xtask.exe` under
/// `target/debug`.
const TEST_ENV: &[(&str, &str)] = if cfg!(windows) {
    &[("CARGO_TARGET_DIR", "target/check")]
} else {
    &[]
};

/// The level at which taplo prints its found-files line and zizmor its
/// completed inputs, set so a contributor's own `RUST_LOG` cannot hide them.
const REPORTING: &[(&str, &str)] = &[("RUST_LOG", "info")];

/// Every row, in the order they run. The rows that read files come before
/// the rows that build and run repository code.
pub const STEPS: &[Step] = &[
    Step {
        name: "fmt",
        covers: "Rust formatting under rustfmt.toml alone, every tracked Rust file proven formatted",
        program: Program::Path("cargo"),
        // --config-path stops rustfmt's search for a nearer config, and
        // --verbose names each file it formats.
        args: &[
            "fmt",
            "--check",
            "--",
            "--config-path",
            "rustfmt.toml",
            "--verbose",
        ],
        install: "rustup component add rustfmt",
        env: &[],
        proof: Proof::Rustfmt,
    },
    Step {
        name: "taplo",
        covers: "TOML formatting over every tracked TOML file, under .taplo.toml, each proven checked",
        program: Program::Mise("taplo"),
        args: &["fmt", "--check", "--config", ".taplo.toml"],
        install: MISE_INSTALL,
        env: REPORTING,
        proof: Proof::Taplo,
    },
    Step {
        name: "deny",
        covers: "Licenses, bans and sources, per deny.toml alone",
        program: Program::Mise("cargo-deny"),
        // --config goes ahead of `check`: it is a flag of cargo-deny itself.
        args: &[
            "--locked",
            "--config",
            "deny.toml",
            "check",
            "licenses",
            "bans",
            "sources",
        ],
        install: MISE_INSTALL,
        env: &[],
        proof: Proof::Exit,
    },
    Step {
        name: "machete",
        covers: "Dependencies a crate declares and never uses, over every tracked crate, each proven analyzed",
        program: Program::Mise("cargo-machete"),
        // Each tracked crate's directory is handed over by name, so no ignore
        // file decides what the walk reaches.
        args: &["--no-ignore", "--skip-target-dir"],
        install: MISE_INSTALL,
        env: &[],
        proof: Proof::Machete,
    },
    Step {
        name: "prettier",
        covers: "Markup, JavaScript and TypeScript formatting over every tracked file Prettier formats, under .prettierrc alone",
        program: Program::Path("bunx"),
        // --ignore-path names .prettierignore alone, so .gitignore never
        // narrows the list, --config stops the search for another config, and
        // --no-editorconfig keeps any .editorconfig from setting an option.
        args: &[
            "--bun",
            "--no-install",
            PRETTIER,
            "--check",
            "--config",
            ".prettierrc",
            "--ignore-path",
            ".prettierignore",
            "--no-editorconfig",
        ],
        install: BUN_INSTALL,
        env: &[],
        proof: Proof::Prettier,
    },
    Step {
        name: "actionlint",
        covers: "Workflow syntax, runner labels, expressions, and run: blocks through ShellCheck, every tracked workflow proven linted",
        program: Program::Mise("actionlint"),
        // -verbose names each file it finished.
        args: &["-verbose", "-pyflakes="],
        install: MISE_INSTALL,
        env: &[],
        proof: Proof::Actionlint,
    },
    Step {
        name: "zizmor",
        covers: "Workflow pinning, credentials, permissions and injection, every tracked workflow proven audited, and every secrets: inherit held to the shared workflows",
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
        env: REPORTING,
        proof: Proof::Zizmor,
    },
    Step {
        name: "typecheck",
        covers: "The types in tools/, which Bun strips rather than checks, under tsconfig.json alone, every file proven read",
        program: Program::Path("bunx"),
        // --project names the one config, so tsc never searches past the
        // checkout, and --listFiles names every file it read.
        args: &[
            "--bun",
            "--no-install",
            TSC,
            "--noEmit",
            "--project",
            "tsconfig.json",
            "--listFiles",
        ],
        install: BUN_INSTALL,
        env: &[],
        proof: Proof::Typecheck,
    },
    Step {
        name: "tools",
        covers: "The tests in tools/, every tracked test file handed by path and proven run",
        program: Program::Path("bun"),
        // Each tracked test file goes by its ./ path, since a bare argument is
        // a substring filter over every path. CI makes a committed test.only
        // fail rather than skip the file's other tests.
        args: &[NO_ENV_FILE, "test"],
        install: "install Bun from https://bun.sh",
        env: &[("CI", "true")],
        proof: Proof::BunTest,
    },
    Step {
        name: "clippy",
        covers: "Lints on every target, warnings denied, with clippy.toml read from the root alone",
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
        proof: Proof::Exit,
    },
    Step {
        name: "tests",
        covers: "The test suites, under cargo-nextest, failing when no test ran",
        program: Program::Mise("cargo-nextest"),
        // --no-tests=fail fails a run that skipped every test, and no user
        // config reaches the run.
        args: &[
            "nextest",
            "run",
            "--workspace",
            "--locked",
            "--no-tests=fail",
            "--user-config-file",
            "none",
        ],
        install: MISE_INSTALL,
        env: TEST_ENV,
        proof: Proof::Exit,
    },
    Step {
        name: "doctests",
        covers: "Every documented example, which nextest runs none of, each proven run",
        program: Program::Path("cargo"),
        args: &["test", "--workspace", "--doc", "--locked"],
        install: RUSTUP,
        env: TEST_ENV,
        proof: Proof::Doctests,
    },
    Step {
        name: "doc",
        covers: "rustdoc over every crate, warnings denied",
        program: Program::Path("cargo"),
        args: &["doc", "--workspace", "--no-deps", "--locked"],
        install: RUSTUP,
        env: &[("RUSTDOCFLAGS", "-D warnings")],
        proof: Proof::Exit,
    },
];

/// The name the row for the pin rules carries.
pub const PINS_STEP: &str = "pins";

/// A workflow with one unquoted expansion, which `ShellCheck` reports as
/// `SC2086`.
const CANARY: &str = "on: push\njobs:\n  canary:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo $GITHUB_REF\n";

/// The same workflow with a directive waiving `SC2086`, which the `ShellCheck`
/// stand-in refuses.
const DIRECTIVE_CANARY: &str = "on: push\njobs:\n  canary:\n    runs-on: ubuntu-latest\n    steps:\n      - run: |\n          # shellcheck disable=SC2086\n          echo $GITHUB_REF\n";

/// Whether the canary run reported the finding it was written to trigger.
fn canary_passed(heard: &str) -> bool {
    heard.contains("SC2086")
}

/// Whether the directive canary came back with the stand-in's refusal.
fn directive_refused(heard: &str) -> bool {
    heard.contains(shellcheck::REFUSAL)
}

/// How a row ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The row ran and exited zero, with the note its row carries.
    Passed(String),
    /// The row ran and exited non-zero, or could not prove what it read.
    Failed,
    /// The row could not start, with the sentence saying why.
    Unrun(String),
}

/// One row of the summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The step name.
    pub step: &'static str,
    /// How the step ended.
    pub outcome: Outcome,
}

impl Row {
    /// Whether the step passed.
    #[must_use]
    pub fn passed(&self) -> bool {
        matches!(self.outcome, Outcome::Passed(_))
    }
}

/// The command a row runs: its argument vector, its environment, and the note
/// its row carries when it passes.
struct Prepared {
    command: Vec<String>,
    env: Vec<(String, String)>,
    note: String,
}

/// The gate's rows and the runner that executes them.
pub struct Gate<'a> {
    steps: &'a [Step],
    runner: &'a dyn Runner,
}

impl<'a> Gate<'a> {
    /// A gate over `steps`, executed by `runner`.
    #[must_use]
    pub fn new(steps: &'a [Step], runner: &'a dyn Runner) -> Self {
        Self { steps, runner }
    }

    /// Print the row table: the pin rules, then every row and what it covers.
    ///
    /// # Errors
    ///
    /// Returns the error `out` raised.
    pub fn rows(&self, out: &mut dyn Write) -> io::Result<()> {
        let width = self
            .steps
            .iter()
            .map(|step| step.name.len())
            .max()
            .unwrap_or(0)
            .max(PINS_STEP.len());
        writeln!(
            out,
            "  {PINS_STEP:width$}  both mise pin files and their lockfiles against pins.rs, and every config each tool reads against tree.rs, before any tool runs"
        )?;
        for step in self.steps {
            writeln!(out, "  {:width$}  {}", step.name, step.covers)?;
        }
        Ok(())
    }

    /// Run the pin rules alone and print each problem.
    ///
    /// Returns whether any problem was found.
    ///
    /// # Errors
    ///
    /// Returns the error `out` raised.
    pub fn pins(&self, out: &mut dyn Write) -> io::Result<bool> {
        let problems = self.pin_problems();
        for problem in &problems {
            writeln!(out, "  {problem}")?;
        }
        Ok(!problems.is_empty())
    }

    /// Every way the pin files and the tree fall short, with an unreadable file
    /// reported as a problem of its own.
    fn pin_problems(&self) -> Vec<String> {
        let read = |path: &str| {
            self.runner
                .read_file(path)
                .ok_or_else(|| format!("{path} cannot be read"))
        };
        let (main, semver) = (pins::MAIN, pins::SEMVER);
        let mut found = match (
            read(main.pins),
            read(main.lock),
            read(semver.pins),
            read(semver.lock),
        ) {
            (Ok(pin_text), Ok(lock), Ok(semver_pins), Ok(semver_lock)) => {
                let mut found = pins::problems(&pin_text, &lock);
                found.extend(pins::semver_problems(&pin_text, &semver_pins, &semver_lock));
                found
            }
            (first, second, third, fourth) => [first, second, third, fourth]
                .into_iter()
                .filter_map(Result::err)
                .collect(),
        };
        found.extend(self.stray_problems());
        found.extend(self.tree_problems());
        found
    }

    /// Every other mise configuration or lockfile in the tree, or the reason
    /// the tree could not be listed.
    fn stray_problems(&self) -> Vec<String> {
        match self.runner.config_paths() {
            Ok(paths) => pins::stray_config_problems(&paths),
            Err(problem) => vec![problem],
        }
    }

    /// Every config a tool reads in place of the one the gate names, and every
    /// other tree rule's finding, or the reason the tree could not be listed.
    fn tree_problems(&self) -> Vec<String> {
        let listing = match self.runner.listing() {
            Ok(listing) => listing,
            Err(problem) => return vec![problem],
        };
        let root_names: Vec<String> = match self.runner.config_paths() {
            Ok(entries) => entries
                .into_iter()
                .filter(|entry| !entry.path.contains('/'))
                .map(|entry| entry.path)
                .collect(),
            Err(problem) => return vec![problem],
        };
        tree::findings(&listing, &root_names, PROJECT_CONFIGS, &|path| {
            self.runner.read_file(path)
        })
    }

    /// Run every row in order, stopping at the first that does not pass.
    ///
    /// # Errors
    ///
    /// Returns the error `out` raised.
    pub fn run(&self, out: &mut dyn Write) -> io::Result<Vec<Row>> {
        writeln!(out, "check")?;
        let mut rows = Vec::new();

        // The pin and tree rules run before any tool. A lockfile entry carrying
        // a url and no checksum installs whatever that url serves, and Bun runs
        // a preload bunfig.toml names before any script, so a rule that ran
        // later would report a finding about code that already executed.
        let problems = self.pin_problems();
        if problems.is_empty() {
            rows.push(Row {
                step: PINS_STEP,
                outcome: Outcome::Passed(String::new()),
            });
        } else {
            for problem in &problems {
                writeln!(out, "  {problem}")?;
            }
            // A stray file or link is removed, never relocked: `mise lock` would
            // read the configuration the finding refuses. The relock is the
            // remedy only when a problem is about a lockfile; an unreadable or
            // malformed pin file needs an edit, not a relock.
            let strays = self.stray_problems();
            let tree = self.tree_problems();
            let relocks: Vec<&str> = pins::PAIRS
                .iter()
                .filter(|pair| problems.iter().any(|problem| problem.contains(pair.lock)))
                .map(|pair| pair.relock)
                .collect();
            let remedy = if !strays.is_empty() {
                "remove each file and link named above".to_string()
            } else if !tree.is_empty() {
                "resolve each finding above, which names what to change".to_string()
            } else if relocks.is_empty() {
                format!("fix {}", pins::PINS)
            } else {
                format!("rewrite the lockfile with: {}", relocks.join(", then "))
            };
            rows.push(Row {
                step: PINS_STEP,
                outcome: Outcome::Unrun(remedy),
            });
            self.summarize(out, &rows)?;
            return Ok(rows);
        }

        let tracked = match self.runner.listing() {
            Ok(listing) => listing.tracked,
            Err(problem) => {
                writeln!(out, "  {problem}")?;
                Vec::new()
            }
        };
        for step in self.steps {
            // The build and test rows run repository code, and a later row reads
            // a config that code could write, so the tree rules run again before
            // each row rather than once.
            let changed = self.tree_problems();
            if !changed.is_empty() {
                for problem in changed {
                    writeln!(out, "  {}", tree::printable(problem))?;
                }
                rows.push(Row {
                    step: step.name,
                    outcome: Outcome::Unrun(
                        "the tree changed while the gate ran: resolve each finding above"
                            .to_string(),
                    ),
                });
                break;
            }
            let outcome = match self.prepare(step) {
                Ok(prepared) if step.proof == Proof::Exit => self.invoke(out, &prepared)?,
                Ok(prepared) => match self.prove(out, step, &prepared, &tracked)? {
                    Ok(note) => Outcome::Passed(note),
                    Err(sentence) => {
                        writeln!(out, "  {}", tree::printable(sentence))?;
                        Outcome::Failed
                    }
                },
                Err(problem) => Outcome::Unrun(problem),
            };
            let stop = !matches!(outcome, Outcome::Passed(_));
            rows.push(Row {
                step: step.name,
                outcome,
            });
            if stop {
                break;
            }
        }
        self.summarize(out, &rows)?;
        Ok(rows)
    }

    /// The command a row runs, or the sentence its row carries when it cannot.
    fn prepare(&self, step: &Step) -> Result<Prepared, String> {
        let program = match step.program {
            Program::Mise(tool) => self
                .runner
                .resolve(tool)
                .map_err(|reason| format!("{reason}: {}", step.install))?
                .display()
                .to_string(),
            Program::Path(name) => name.to_string(),
        };
        // bunx runs a copy from a parent directory or PATH when the checkout
        // holds none, so a row it starts needs the checkout's own first.
        if step.program == Program::Path("bunx")
            && let Some(tool) = step.args.iter().find(|arg| !arg.starts_with("--"))
        {
            let bin = installed_bin(tool);
            if !self.runner.exists(&bin) {
                return Err(format!("{bin} is not in the checkout: {}", step.install));
            }
        }
        let mut command = vec![program];
        command.extend(step.args.iter().map(|arg| (*arg).to_string()));
        let mut env: Vec<(String, String)> = step
            .env
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        let mut note = String::new();
        match step.name {
            "clippy" => {
                // clippy reads clippy.toml from the directory this names and
                // nowhere else, so no crate's or parent's copy reaches it.
                env.push((
                    "CLIPPY_CONF_DIR".to_string(),
                    self.runner.root().display().to_string(),
                ));
            }
            "actionlint" => {
                // actionlint exits zero and prints nothing when its analyzer is
                // absent or will not execute, and no flag changes that. The
                // analyzer is this binary's ShellCheck stand-in over the
                // ShellCheck mise resolved. Two canaries prove both before the
                // real run is trusted: a finding ShellCheck must report, and a
                // directive the stand-in must refuse.
                let analyzer = self.runner.resolve("shellcheck").map_err(|reason| {
                    format!(
                        "{reason}, and actionlint would skip the analysis in silence: {MISE_INSTALL}"
                    )
                })?;
                let stand_in = std::env::current_exe().map_err(|err| {
                    format!("the running xtask cannot be found to stand in for ShellCheck: {err}")
                })?;
                command.push(format!(
                    "-shellcheck={}",
                    shellcheck::flag_value(&stand_in, &analyzer)?
                ));
                if !canary_passed(&self.canary(&command, CANARY)?) {
                    return Err(
                        "actionlint ran without ShellCheck: the canary workflow came back with no SC2086"
                            .to_string(),
                    );
                }
                if !directive_refused(&self.canary(&command, DIRECTIVE_CANARY)?) {
                    return Err(
                        "actionlint ran without the ShellCheck stand-in: the canary carrying a directive came back unrefused"
                            .to_string(),
                    );
                }
            }
            "zizmor" => {
                // Offline in CI, where the gate holds no token: the online
                // audits run in the shared workflows job, the one job that
                // names the token. Locally, online when the host has a GitHub
                // login, so those audits run before a push; offline otherwise.
                let in_ci = self
                    .runner
                    .env_var("CI")
                    .is_some_and(|value| !value.is_empty());
                let token = if in_ci { None } else { self.gh_token() };
                if let Some(token) = token {
                    env.push(("GH_TOKEN".to_string(), token));
                    note.push_str("online");
                } else {
                    command.push("--offline".to_string());
                    note.push_str("offline");
                }
            }
            _ => {}
        }
        Ok(Prepared { command, env, note })
    }

    /// Run the actionlint command over a canary workflow holding `text` and
    /// return what it printed. An exit code alone says nothing, because a
    /// finding is the expected result.
    fn canary(&self, command: &[String], text: &str) -> Result<String, String> {
        let dir =
            tempfile::tempdir().map_err(|err| format!("creating the canary directory: {err}"))?;
        let workflow = dir.path().join("canary.yml");
        std::fs::write(&workflow, text)
            .map_err(|err| format!("writing the canary workflow: {err}"))?;
        let path = workflow.display().to_string();
        let mut argv: Vec<&str> = command.iter().map(String::as_str).collect();
        argv.push(&path);
        self.runner
            .capture_any(&argv)
            .ok_or_else(|| format!("{} could not start for the canary run", command[0]))
    }

    /// The token `gh` holds for github.com, or `None` when nobody is logged in.
    ///
    /// The first line alone: `capture` merges the streams, and the token is
    /// the one line standard output carries.
    fn gh_token(&self) -> Option<String> {
        let printed = self.runner.capture(&["gh", "auth", "token"])?;
        let token = printed.lines().next()?.trim().to_string();
        (!token.is_empty()).then_some(token)
    }

    /// Run one prepared command and judge it by its exit code.
    fn invoke(&self, out: &mut dyn Write, prepared: &Prepared) -> io::Result<Outcome> {
        let line = tree::printable(prepared.command.join(" "));
        writeln!(
            out,
            "\n{}",
            line.if_supports_color(Stream::Stdout, OwoColorize::dimmed)
        )?;
        let argv: Vec<&str> = prepared.command.iter().map(String::as_str).collect();
        let pairs: Vec<(&str, &str)> = prepared
            .env
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        Ok(match self.runner.run(&argv, &pairs) {
            Ok(Exit::Ok) => Outcome::Passed(prepared.note.clone()),
            Ok(Exit::Err) => Outcome::Failed,
            Err(err) => Outcome::Unrun(format!("{} could not start: {err}", prepared.command[0])),
        })
    }

    /// Run `command` and return what it printed, printing the command line and,
    /// when it exits non-zero, everything it printed.
    fn capture(
        &self,
        out: &mut dyn Write,
        command: &[String],
        env: &[(String, String)],
        input: Option<&str>,
    ) -> io::Result<Result<Captured, String>> {
        let line = tree::printable(command.join(" "));
        writeln!(
            out,
            "\n{}",
            line.if_supports_color(Stream::Stdout, OwoColorize::dimmed)
        )?;
        let argv: Vec<&str> = command.iter().map(String::as_str).collect();
        let pairs: Vec<(&str, &str)> = env
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        Ok(match self.runner.output(&argv, &pairs, input) {
            Ok(captured) => {
                if captured.exit == Exit::Err {
                    for line in captured.stdout.lines().chain(captured.stderr.lines()) {
                        if !noise(line) {
                            writeln!(out, "{}", tree::printable(line.to_string()))?;
                        }
                    }
                }
                Ok(captured)
            }
            Err(err) => Err(format!("{} could not start: {err}", command[0])),
        })
    }

    /// Run a row that proves what it read, returning its note or the sentence
    /// saying why it failed.
    fn prove(
        &self,
        out: &mut dyn Write,
        step: &Step,
        prepared: &Prepared,
        tracked: &[String],
    ) -> io::Result<Result<String, String>> {
        let root = self.runner.root();
        let existing: Vec<String> = tracked
            .iter()
            .filter(|path| self.runner.exists(path))
            .cloned()
            .collect();
        match step.proof {
            Proof::Exit => Ok(Ok(prepared.note.clone())),
            Proof::Rustfmt => self.prove_rustfmt(out, prepared, &existing, &root),
            Proof::Taplo => self.prove_batches(out, prepared, &existing, &root, &Batched::TAPLO),
            Proof::Machete => self.prove_machete(out, prepared, &existing, &root),
            Proof::Prettier => self.prove_prettier(out, prepared, &existing),
            Proof::Actionlint => self.prove_actionlint(out, prepared, &existing, &root),
            Proof::Zizmor => self.prove_zizmor(out, prepared, &existing, &root),
            Proof::Doctests => self.prove_doctests(out, prepared),
            Proof::Typecheck => self.prove_typecheck(out, prepared, &existing, &root),
            Proof::BunTest => self.prove_bun_test(out, prepared, &existing, &root),
        }
    }

    /// The fmt row: every tracked Rust file must be one rustfmt formatted.
    fn prove_rustfmt(
        &self,
        out: &mut dyn Write,
        prepared: &Prepared,
        tracked: &[String],
        root: &Path,
    ) -> io::Result<Result<String, String>> {
        let captured = match self.capture(out, &prepared.command, &prepared.env, None)? {
            Ok(captured) => captured,
            Err(sentence) => return Ok(Err(sentence)),
        };
        if captured.exit == Exit::Err {
            return Ok(Err(
                "rustfmt exited non-zero, and its output is above".to_string()
            ));
        }
        let handed: Vec<String> = tracked
            .iter()
            .filter(|path| tree::has_extension(&tree::fold(path), "rs"))
            .cloned()
            .collect();
        let formatted =
            proof::rustfmt_formatted(&format!("{}\n{}", captured.stdout, captured.stderr));
        // rustfmt finds its files through each crate's module tree, an untracked
        // module included, so the row holds every tracked file to be among them.
        Ok(
            proof::covered("rustfmt", proof::FILES, &handed, &formatted, root)
                .map(|()| proof::count(handed.len(), "file", "files")),
        )
    }

    /// A row that hands its tool the tracked files it reads in batches, after
    /// `--`, and proves each one reported.
    fn prove_batches(
        &self,
        out: &mut dyn Write,
        prepared: &Prepared,
        tracked: &[String],
        root: &Path,
        batched: &Batched,
    ) -> io::Result<Result<String, String>> {
        let handed: Vec<String> = tracked
            .iter()
            .filter(|path| (batched.reads)(&tree::fold(path)))
            .cloned()
            .collect();
        if handed.is_empty() {
            return Ok(Err(format!(
                "no tracked file is one {} reads, so the row checks nothing",
                batched.tool
            )));
        }
        for batch in proof::batches(&handed) {
            let mut command = prepared.command.clone();
            command.push("--".to_string());
            command.extend(batch.iter().cloned());
            let captured = match self.capture(out, &command, &prepared.env, None)? {
                Ok(captured) => captured,
                Err(sentence) => return Ok(Err(sentence)),
            };
            if captured.exit == Exit::Err {
                return Ok(Err(format!(
                    "{} exited non-zero, and its output is above",
                    batched.tool
                )));
            }
            let printed = format!("{}\n{}", captured.stdout, captured.stderr);
            let Some(reported) = (batched.reported)(&printed) else {
                return Ok(Err(format!(
                    "{} printed no line naming what it checked, so what it read is unknown",
                    batched.tool
                )));
            };
            if let Err(sentence) = proof::prove(batched.tool, proof::FILES, &batch, &reported, root)
            {
                return Ok(Err(sentence));
            }
        }
        Ok(Ok(proof::count(handed.len(), "file", "files")))
    }

    /// The actionlint row: every shell a tracked workflow's steps run under is
    /// one actionlint hands to `ShellCheck`, or pwsh, and then every tracked
    /// workflow is linted and proven.
    fn prove_actionlint(
        &self,
        out: &mut dyn Write,
        prepared: &Prepared,
        tracked: &[String],
        root: &Path,
    ) -> io::Result<Result<String, String>> {
        let workflows: Vec<String> = tracked
            .iter()
            .filter(|path| (Batched::ACTIONLINT.reads)(&tree::fold(path)))
            .cloned()
            .collect();
        if !workflows.is_empty() {
            let ask = bun_script(shellcheck::WORKFLOW_SHELLS);
            let captured = match self.capture(out, &ask, &[], Some(&workflows.join("\0")))? {
                Ok(captured) => captured,
                Err(sentence) => return Ok(Err(format!("{sentence}: {BUN_INSTALL}"))),
            };
            if captured.exit == Exit::Err {
                return Ok(Err(format!(
                    "bun could not read the shells the workflows run: {BUN_INSTALL}"
                )));
            }
            let (read, refused) = match shellcheck::shell_findings(&captured.stdout) {
                Ok(found) => found,
                Err(sentence) => return Ok(Err(sentence)),
            };
            if let Err(sentence) = proof::prove("bun", proof::FILES, &workflows, &read, root) {
                return Ok(Err(sentence));
            }
            if !refused.is_empty() {
                return Ok(Err(refused.join("; ")));
            }
        }
        self.prove_batches(out, prepared, tracked, root, &Batched::ACTIONLINT)
    }

    /// The doctests row: the examples `cargo test --doc` counts must show one
    /// ran, or that none exists where none is declared.
    fn prove_doctests(
        &self,
        out: &mut dyn Write,
        prepared: &Prepared,
    ) -> io::Result<Result<String, String>> {
        let captured = match self.capture(out, &prepared.command, &prepared.env, None)? {
            Ok(captured) => captured,
            Err(sentence) => return Ok(Err(sentence)),
        };
        if captured.exit == Exit::Err {
            return Ok(Err(
                "cargo test --doc exited non-zero, and its output is above".to_string(),
            ));
        }
        Ok(
            proof::doctest_examples(&format!("{}\n{}", captured.stdout, captured.stderr))
                .and_then(|examples| proof::doctests_proven(examples, NO_DOC_EXAMPLES)),
        )
    }

    /// The machete row: every tracked crate's directory is handed over, and
    /// each must be one cargo-machete names as analyzed.
    fn prove_machete(
        &self,
        out: &mut dyn Write,
        prepared: &Prepared,
        tracked: &[String],
        root: &Path,
    ) -> io::Result<Result<String, String>> {
        let handed: Vec<String> = tracked
            .iter()
            .filter_map(|path| {
                let (dir, name) = path.rsplit_once('/')?;
                (tree::fold(name) == "cargo.toml").then(|| dir.to_string())
            })
            .collect();
        if handed.is_empty() {
            return Ok(Err(
                "no tracked crate sits below the root, so the machete row checks nothing"
                    .to_string(),
            ));
        }
        let mut command = prepared.command.clone();
        command.extend(handed.iter().cloned());
        let captured = match self.capture(out, &command, &prepared.env, None)? {
            Ok(captured) => captured,
            Err(sentence) => return Ok(Err(sentence)),
        };
        if captured.exit == Exit::Err {
            return Ok(Err(
                "cargo-machete exited non-zero, and its output is above".to_string(),
            ));
        }
        // cargo-machete exits zero and names a directory as clean when it cannot
        // read the crate there, and says so on one line of its own.
        let printed = format!("{}\n{}", captured.stdout, captured.stderr);
        if let Some(line) = printed
            .lines()
            .find(|line| line.contains("error when handling"))
        {
            return Ok(Err(format!(
                "cargo-machete could not read a crate it was handed: {}",
                line.trim()
            )));
        }
        let analyzed =
            proof::machete_analyzed(&format!("{}\n{}", captured.stdout, captured.stderr));
        Ok(
            proof::prove("cargo-machete", proof::CRATES, &handed, &analyzed, root)
                .map(|()| proof::count(handed.len(), "crate", "crates")),
        )
    }

    /// The prettier row: Prettier's `getFileInfo` picks the tracked files it
    /// formats, and the row hands it exactly those, in batches.
    fn prove_prettier(
        &self,
        out: &mut dyn Write,
        prepared: &Prepared,
        tracked: &[String],
    ) -> io::Result<Result<String, String>> {
        let ask = bun_script(proof::PRETTIER_FILES);
        let listed = tracked.join("\0");
        let captured = match self.capture(out, &ask, &[], Some(&listed))? {
            Ok(captured) => captured,
            Err(sentence) => return Ok(Err(format!("{sentence}: {BUN_INSTALL}"))),
        };
        if captured.exit == Exit::Err {
            return Ok(Err(format!(
                "bun could not ask Prettier which files it formats: {BUN_INSTALL}"
            )));
        }
        let checked: Vec<String> = captured
            .stdout
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_string)
            .collect();
        if checked.is_empty() {
            return Ok(Err(
                "no tracked file is one Prettier formats, so the row checks nothing".to_string(),
            ));
        }
        for batch in proof::batches(&checked) {
            let mut command = prepared.command.clone();
            command.push("--".to_string());
            command.extend(batch);
            let captured = match self.capture(out, &command, &prepared.env, None)? {
                Ok(captured) => captured,
                Err(sentence) => return Ok(Err(sentence)),
            };
            if captured.exit == Exit::Err {
                return Ok(Err(
                    "Prettier exited non-zero, and its output is above".to_string()
                ));
            }
        }
        Ok(Ok(proof::count(checked.len(), "file", "files")))
    }

    /// The zizmor row: every tracked workflow must be an input zizmor completed,
    /// and a second pass with nothing waived holds every job passing
    /// `secrets: inherit` to the shared workflows.
    fn prove_zizmor(
        &self,
        out: &mut dyn Write,
        prepared: &Prepared,
        tracked: &[String],
        root: &Path,
    ) -> io::Result<Result<String, String>> {
        let captured = match self.capture(out, &prepared.command, &prepared.env, None)? {
            Ok(captured) => captured,
            Err(sentence) => return Ok(Err(sentence)),
        };
        if captured.exit == Exit::Err {
            return Ok(Err("zizmor reported findings".to_string()));
        }
        let workflows: Vec<String> = tracked
            .iter()
            .filter(|path| workflow(&tree::fold(path)))
            .cloned()
            .collect();
        let completed: Vec<String> =
            proof::zizmor_completed(&format!("{}\n{}", captured.stdout, captured.stderr))
                .into_iter()
                .filter(|path| workflow(&proof::comparable(path, root)))
                .collect();
        if let Err(sentence) = proof::prove("zizmor", proof::FILES, &workflows, &completed, root) {
            return Ok(Err(sentence));
        }
        // No config and no ignores, so neither the file-level waiver in
        // zizmor.yml nor an inline comment hides a job from this pass. Its exit
        // code reports the audits it ran, and the row reads its JSON alone.
        let held = vec![
            prepared.command[0].clone(),
            "--offline".to_string(),
            "--no-config".to_string(),
            "--no-ignores".to_string(),
            "--strict-collection".to_string(),
            "--format".to_string(),
            "json".to_string(),
            "--collect=all".to_string(),
            ".github".to_string(),
        ];
        writeln!(
            out,
            "\n{}",
            held.join(" ")
                .if_supports_color(Stream::Stdout, OwoColorize::dimmed)
        )?;
        let argv: Vec<&str> = held.iter().map(String::as_str).collect();
        let report = match self.runner.output(&argv, &[], None) {
            Ok(captured) => captured.stdout,
            Err(err) => return Ok(Err(format!("{} could not start: {err}", held[0]))),
        };
        match proof::inherit_callees(&report) {
            Ok(callees) if callees.is_empty() => {}
            Ok(callees) => return Ok(Err(callees.join("; "))),
            Err(sentence) => return Ok(Err(sentence)),
        }
        let count = proof::count(workflows.len(), "workflow", "workflows");
        Ok(Ok(format!("{count}, {}", prepared.note)))
    }

    /// The typecheck row: tsc must read every tracked file `tsconfig.json`
    /// includes, and nothing else of the repository's.
    fn prove_typecheck(
        &self,
        out: &mut dyn Write,
        prepared: &Prepared,
        tracked: &[String],
        root: &Path,
    ) -> io::Result<Result<String, String>> {
        writeln!(
            out,
            "\n{}",
            prepared
                .command
                .join(" ")
                .if_supports_color(Stream::Stdout, OwoColorize::dimmed)
        )?;
        let argv: Vec<&str> = prepared.command.iter().map(String::as_str).collect();
        let pairs: Vec<(&str, &str)> = prepared
            .env
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let captured = match self.runner.output(&argv, &pairs, None) {
            Ok(captured) => captured,
            Err(err) => {
                return Ok(Err(format!(
                    "{} could not start: {err}: install Bun from https://bun.sh",
                    prepared.command[0]
                )));
            }
        };
        if captured.exit == Exit::Err {
            // tsc lists every file it read even when it fails, which would bury
            // its errors, so the listed paths are left out.
            for line in captured.stdout.lines().chain(captured.stderr.lines()) {
                if !noise(line) && tsc_listed(line).is_empty() {
                    writeln!(out, "{}", tree::printable(line.to_string()))?;
                }
            }
            return Ok(Err(
                "tsc exited non-zero, and its errors are above".to_string()
            ));
        }
        let handed: Vec<String> = tracked
            .iter()
            .filter(|path| typescript(&tree::fold(path)))
            .cloned()
            .collect();
        let read: Vec<String> = tsc_listed(&captured.stdout)
            .into_iter()
            .filter(|path| {
                let relative = proof::comparable(path, root);
                !(relative.starts_with('/') || relative.as_bytes().get(1) == Some(&b':'))
            })
            .collect();
        Ok(proof::prove("tsc", proof::FILES, &handed, &read, root)
            .map(|()| proof::count(handed.len(), "file", "files")))
    }

    /// The tools row: every tracked test file is handed to `bun test` by its
    /// `./` path, and the junit report it writes must name each one and
    /// nothing else.
    fn prove_bun_test(
        &self,
        out: &mut dyn Write,
        prepared: &Prepared,
        tracked: &[String],
        root: &Path,
    ) -> io::Result<Result<String, String>> {
        let handed: Vec<String> = tracked
            .iter()
            .filter(|path| bun_test(&tree::fold(path)))
            .cloned()
            .collect();
        if handed.is_empty() {
            return Ok(Err(
                "no tracked file is a test file bun test runs, so the row checks nothing"
                    .to_string(),
            ));
        }
        let dir = match tempfile::tempdir() {
            Ok(dir) => dir,
            Err(err) => return Ok(Err(format!("creating the junit report directory: {err}"))),
        };
        let report = dir.path().join("junit.xml");
        let mut command = prepared.command.clone();
        command.push("--reporter=junit".to_string());
        command.push(format!("--reporter-outfile={}", report.display()));
        command.extend(handed.iter().map(|path| format!("./{path}")));
        let captured = match self.capture(out, &command, &prepared.env, None)? {
            Ok(captured) => captured,
            Err(sentence) => return Ok(Err(sentence)),
        };
        if captured.exit == Exit::Err {
            return Ok(Err(
                "bun test exited non-zero, and its output is above".to_string()
            ));
        }
        let Some(written) = self.runner.read_file(&report.display().to_string()) else {
            return Ok(Err(
                "bun test wrote no junit report, so which files it ran is unknown".to_string(),
            ));
        };
        let ran = junit_files(&written);
        let idle = junit_idle(&written);
        Ok(
            proof::prove("bun test", proof::FILES, &handed, &ran, root).and_then(|()| {
                if idle.is_empty() {
                    Ok(proof::count(handed.len(), "file", "files"))
                } else {
                    let named: Vec<String> = idle.iter().map(|file| format!("{file:?}")).collect();
                    Err(format!(
                        "bun test ran no test in {}: each one there was skipped, a todo or held back by its condition",
                        named.join(", ")
                    ))
                }
            }),
        )
    }

    /// Print the summary table.
    #[expect(
        clippy::unused_self,
        reason = "the summary belongs to the gate that produced the rows"
    )]
    fn summarize(&self, out: &mut dyn Write, rows: &[Row]) -> io::Result<()> {
        let width = rows.iter().map(|row| row.step.len()).max().unwrap_or(0);
        let ok = "\u{2713}"
            .if_supports_color(Stream::Stdout, OwoColorize::green)
            .to_string();
        let bad = "\u{2717}"
            .if_supports_color(Stream::Stdout, OwoColorize::red)
            .to_string();
        writeln!(out)?;
        for row in rows {
            let (mark, text) = match &row.outcome {
                Outcome::Passed(note) => (&ok, note.clone()),
                Outcome::Failed => (&bad, "failed".to_string()),
                Outcome::Unrun(why) => (&bad, format!("did not run: {why}")),
            };
            writeln!(out, "  {mark} {:width$}  {text}", row.step)?;
        }
        let rule = "\u{2500}".repeat(width + 12);
        writeln!(
            out,
            "  {}",
            rule.if_supports_color(Stream::Stdout, OwoColorize::dimmed)
        )?;
        match rows.iter().find(|row| !row.passed()) {
            Some(row) => {
                let text = format!("the gate failed at {}", row.step);
                writeln!(
                    out,
                    "  {}",
                    text.if_supports_color(Stream::Stdout, OwoColorize::red)
                )
            }
            None => writeln!(out, "  {} steps passed", rows.len()),
        }
    }
}

/// The extensions tsc reads as TypeScript.
const TYPESCRIPT: &[&str] = &["ts", "tsx", "mts", "cts"];

/// The extensions a Bun test file carries.
const TEST_SCRIPTS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

/// Whether the folded path `folded` is TypeScript. The typecheck row holds tsc
/// to read every one wherever it sits, so a file `tsconfig.json` leaves out
/// fails the row rather than going unchecked.
fn typescript(folded: &str) -> bool {
    TYPESCRIPT
        .iter()
        .any(|extension| tree::has_extension(folded, extension))
}

/// Whether the folded path `folded` is a test file by Bun's naming. The tools
/// row hands `bun test` every one by path wherever it sits, so a test outside
/// `tools/` runs too.
fn bun_test(folded: &str) -> bool {
    folded.rsplit_once('.').is_some_and(|(stem, extension)| {
        TEST_SCRIPTS.contains(&extension)
            && [".test", "_test", ".spec", "_spec"]
                .iter()
                .any(|suffix| stem.ends_with(suffix))
    })
}

/// Every absolute path tsc's `--listFiles` output names, outside
/// `node_modules`.
fn tsc_listed(printed: &str) -> Vec<String> {
    printed
        .lines()
        .map(str::trim)
        .filter(|line| {
            let bytes = line.as_bytes();
            line.starts_with('/') || (bytes.len() > 2 && bytes[1] == b':' && bytes[2] == b'/')
        })
        .filter(|line| !line.to_ascii_lowercase().contains("/node_modules/"))
        .map(str::to_string)
        .collect()
}

/// Each file a `bun test` junit report names, with the start tag of the first
/// suite naming it, in the order the report names them. That suite is the
/// file's outermost, and its counts take in every suite nested in it.
fn junit_suites(report: &str) -> Vec<(String, &str)> {
    let mut suites: Vec<(String, &str)> = Vec::new();
    for (at, _) in report.match_indices("<testsuite ") {
        let tag = &report[at..report[at..].find('>').map_or(report.len(), |end| at + end)];
        if let Some(file) = xml_attribute(tag, "file")
            && !suites.iter().any(|(seen, _)| *seen == file)
        {
            suites.push((file, tag));
        }
    }
    suites
}

/// Every distinct file a `bun test` junit report names on a test suite, in
/// the order the report names them.
fn junit_files(report: &str) -> Vec<String> {
    junit_suites(report)
        .into_iter()
        .map(|(file, _)| file)
        .collect()
}

/// Every file a `bun test` junit report names where no test ran. Bun counts a
/// skipped test, a todo and one whose condition kept it from running as
/// skipped, and a suite carrying no count ran nothing either.
fn junit_idle(report: &str) -> Vec<String> {
    let count = |tag: &str, name: &str| {
        xml_attribute(tag, name)
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0)
    };
    junit_suites(report)
        .into_iter()
        .filter(|(_, tag)| count(tag, "tests") <= count(tag, "skipped"))
        .map(|(file, _)| file)
        .collect()
}

/// The value of the attribute `name` in the XML start tag `tag`, its five
/// predefined entities decoded.
fn xml_attribute(tag: &str, name: &str) -> Option<String> {
    let opening = format!(" {name}=\"");
    let start = tag.find(&opening)? + opening.len();
    let end = tag[start..].find('"')? + start;
    Some(
        tag[start..end]
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&"),
    )
}

/// A row that hands its tool files in batches: the tool, which tracked files it
/// reads, and how its output names the ones it finished.
struct Batched {
    tool: &'static str,
    reads: fn(&str) -> bool,
    reported: fn(&str) -> Option<Vec<String>>,
}

impl Batched {
    /// taplo reads every tracked TOML file and names what it found.
    const TAPLO: Self = Self {
        tool: "taplo",
        reads: |folded| tree::has_extension(folded, "toml"),
        reported: proof::taplo_found,
    };

    /// actionlint reads every tracked workflow and names each it linted.
    const ACTIONLINT: Self = Self {
        tool: "actionlint",
        reads: workflow,
        reported: |printed| Some(proof::actionlint_linted(printed)),
    };
}

/// Whether the folded path `folded` is a workflow GitHub reads.
fn workflow(folded: &str) -> bool {
    folded
        .strip_prefix(".github/workflows/")
        .is_some_and(|name| {
            !name.contains('/')
                && (tree::has_extension(name, "yml") || tree::has_extension(name, "yaml"))
        })
}

/// Whether `line`, from a proving row's tool, is progress the row reads rather
/// than a finding a person reads.
fn noise(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("verbose: ")
        || trimmed.starts_with("Formatting ")
        || trimmed.starts_with("Spent ")
        || trimmed.starts_with("Using rustfmt config file")
        || line.contains(" INFO ")
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fmt::Write as _;
    use std::io;
    use std::path::PathBuf;

    use super::{
        Gate, MISE_INSTALL, Outcome, PRETTIER, Program, Row, STEPS, TSC, canary_passed,
        directive_refused, junit_files, junit_idle,
    };
    use crate::runner::{Captured, Exit, Runner};
    use crate::{pins, proof, shellcheck, tree};

    /// The root the fake answers with, which every path a fake tool prints
    /// starts with.
    const ROOT: &str = "/fake/root";

    /// The tracked files the fake lists: one of each kind a walking row reads.
    const TRACKED: &[&str] = &[
        "Cargo.toml",
        "crates/a/Cargo.toml",
        "crates/a/src/lib.rs",
        "xtask/Cargo.toml",
        "xtask/src/main.rs",
        ".github/workflows/ci.yml",
        ".github/workflows/deps.yml",
        ".github/dependabot.yml",
        "README.md",
        "docs/dev.md",
        "LICENSE",
        "tsconfig.json",
        "tools/a.ts",
        "tools/a.test.ts",
    ];

    /// A runner that spawns nothing: every mise tool resolves under one fake
    /// prefix, every tool reports what it was handed, and the canary answers
    /// with its finding.
    #[expect(
        clippy::struct_excessive_bools,
        reason = "each bool is one independent answer the fake gives"
    )]
    struct FakeRunner {
        /// Tools `resolve` answers `None` for.
        unresolvable: Vec<&'static str>,
        /// Programs whose command exits non-zero, by basename.
        failing: Vec<&'static str>,
        /// Paths a fake tool leaves out of its report, by the tool's basename.
        unreported: Vec<(&'static str, &'static str)>,
        /// Test files whose every test the junit report counts as skipped.
        skipping: Vec<&'static str>,
        /// The tracked files the listing answers with.
        tracked: Vec<&'static str>,
        /// The untracked files the listing answers with.
        untracked: Vec<&'static str>,
        /// The workflow the held zizmor pass names as a secrets-inherit callee.
        callee: &'static str,
        /// Whether `gh auth token` answers.
        logged_in: bool,
        /// Whether `CI` reads as set.
        in_ci: bool,
        /// The paths the tree listing answers with beside the two pairs.
        extra_paths: Vec<&'static str>,
        /// Whether the pin files can be read.
        pins_readable: bool,
        /// Whether the directive canary comes back analyzed rather than refused.
        unrefused: bool,
        /// The shell every workflow step reports.
        shell: &'static str,
        /// Tools whose command the install left out of `node_modules/.bin`.
        uninstalled: Vec<&'static str>,
        /// Crate directories cargo-machete cannot read, which it names on a line
        /// of its own and still reports as clean.
        unreadable: Vec<&'static str>,
        /// Untracked paths the listing gains once a program has run, by that
        /// program's basename.
        plants: Vec<(&'static str, &'static str)>,
        /// Where the shell report says each workflow's one shell is set.
        step_at: &'static str,
        /// The documented examples `cargo test --doc` reports passed, ignored
        /// and filtered out.
        examples: (usize, usize, usize),
        /// Every command passed to `run` or `output`, in order.
        ran: RefCell<Vec<Vec<String>>>,
        /// The environment each of those commands set, in the same order.
        envs: RefCell<Vec<Vec<(String, String)>>>,
    }

    impl FakeRunner {
        fn all_installed() -> Self {
            Self {
                unresolvable: Vec::new(),
                failing: Vec::new(),
                unreported: Vec::new(),
                skipping: Vec::new(),
                tracked: TRACKED.to_vec(),
                untracked: Vec::new(),
                callee: "zachthedev/.github/.github/workflows/deps.yml@c53d09e393028ceddee0d761f2a7963394289a72",
                logged_in: true,
                in_ci: false,
                extra_paths: Vec::new(),
                pins_readable: true,
                unrefused: false,
                shell: "bash",
                uninstalled: Vec::new(),
                unreadable: Vec::new(),
                plants: Vec::new(),
                step_at: "jobs.a.steps[0].shell",
                examples: (1, 0, 0),
                ran: RefCell::new(Vec::new()),
                envs: RefCell::new(Vec::new()),
            }
        }

        fn in_ci(mut self) -> Self {
            self.in_ci = true;
            self
        }

        fn with_path(mut self, path: &'static str) -> Self {
            self.extra_paths.push(path);
            self
        }

        fn unresolvable(mut self, tool: &'static str) -> Self {
            self.unresolvable.push(tool);
            self
        }

        fn failing(mut self, program: &'static str) -> Self {
            self.failing.push(program);
            self
        }

        fn uninstalled(mut self, tool: &'static str) -> Self {
            self.uninstalled.push(tool);
            self
        }

        fn unreported(mut self, tool: &'static str, path: &'static str) -> Self {
            self.unreported.push((tool, path));
            self
        }

        fn skipping(mut self, path: &'static str) -> Self {
            self.skipping.push(path);
            self
        }

        fn tracking(mut self, tracked: &[&'static str]) -> Self {
            self.tracked = tracked.to_vec();
            self
        }

        fn untracked(mut self, path: &'static str) -> Self {
            self.untracked.push(path);
            self
        }

        fn calling(mut self, callee: &'static str) -> Self {
            self.callee = callee;
            self
        }

        fn logged_out(mut self) -> Self {
            self.logged_in = false;
            self
        }

        fn unpinned(mut self) -> Self {
            self.pins_readable = false;
            self
        }

        fn unrefused(mut self) -> Self {
            self.unrefused = true;
            self
        }

        fn shell(mut self, shell: &'static str) -> Self {
            self.shell = shell;
            self
        }

        fn unreadable(mut self, dir: &'static str) -> Self {
            self.unreadable.push(dir);
            self
        }

        fn planting(mut self, program: &'static str, path: &'static str) -> Self {
            self.plants.push((program, path));
            self
        }

        fn step_at(mut self, at: &'static str) -> Self {
            self.step_at = at;
            self
        }

        fn examples(mut self, passed: usize, ignored: usize, filtered: usize) -> Self {
            self.examples = (passed, ignored, filtered);
            self
        }

        /// The first command that runs the JavaScript entry `entry` under Bun.
        fn running(&self, entry: &str) -> Option<Vec<String>> {
            self.ran()
                .into_iter()
                .find(|command| command.iter().any(|arg| arg == entry))
        }

        fn ran(&self) -> Vec<Vec<String>> {
            self.ran.borrow().clone()
        }

        /// The first command whose program's basename is `program`.
        fn command(&self, program: &str) -> Option<Vec<String>> {
            self.ran()
                .into_iter()
                .find(|command| basename(&command[0]) == program)
        }

        fn record(&self, command: &[&str], env: &[(&str, &str)]) {
            self.ran
                .borrow_mut()
                .push(command.iter().map(|arg| (*arg).to_string()).collect());
            self.envs.borrow_mut().push(
                env.iter()
                    .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                    .collect(),
            );
        }

        /// `paths` less the ones `tool` leaves out.
        fn reported<'a>(&self, tool: &str, paths: &'a [&'a str]) -> Vec<&'a str> {
            paths
                .iter()
                .copied()
                .filter(|path| !self.unreported.contains(&(tool, *path)))
                .collect()
        }
    }

    impl FakeRunner {
        /// What `cargo test --doc` prints for the examples the case sets, one
        /// crate's header on standard error and its result line on standard
        /// output.
        fn doctests(&self) -> (String, String) {
            let (passed, ignored, filtered) = self.examples;
            (
                format!(
                    "test result: ok. {passed} passed; 0 failed; {ignored} ignored; 0 measured; {filtered} filtered out; finished in 0.00s\n"
                ),
                "   Doc-tests a\n".to_string(),
            )
        }

        /// What `program` prints to standard output and standard error for
        /// `command`: each tool's report of what it read, less what the case
        /// has it leave out.
        fn answer(&self, program: &str, command: &[&str], input: Option<&str>) -> (String, String) {
            let handed: Vec<&str> = command
                .iter()
                .position(|arg| *arg == "--")
                .map(|at| command[at + 1..].to_vec())
                .unwrap_or_default();
            match program {
                "cargo" if command.get(1) == Some(&"test") => self.doctests(),
                "cargo" => {
                    let rust: Vec<&str> = self
                        .tracked
                        .iter()
                        .copied()
                        .filter(|path| tree::has_extension(path, "rs"))
                        .collect();
                    let formatted = self.reported(program, &rust);
                    (
                        lines(&formatted, |path| format!("Formatting {ROOT}/{path}")),
                        String::new(),
                    )
                }
                "taplo" => {
                    let files: Vec<String> = self
                        .reported(program, &handed)
                        .iter()
                        .map(|path| format!("\"{ROOT}/{path}\""))
                        .collect();
                    let line = format!(
                        " INFO taplo: found files total={} excluded=0 files=[{}] cwd=\"{ROOT}\"\n",
                        files.len(),
                        files.join(", ")
                    );
                    (String::new(), line)
                }
                "cargo-machete" => {
                    let dirs: Vec<&str> = command[1..]
                        .iter()
                        .copied()
                        .filter(|arg| !arg.starts_with("--"))
                        .collect();
                    let analyzed = self.reported(program, &dirs);
                    let shape = |dir: &str| {
                        format!(
                            "cargo-machete didn't find any unused dependencies in {dir}. Good job!"
                        )
                    };
                    let failed = |dir: &str| {
                        format!("error when handling {dir}: the manifest does not parse")
                    };
                    (lines(&analyzed, shape), lines(&self.unreadable, failed))
                }
                "bun" if command.get(2) == Some(&"test") => (String::new(), String::new()),
                "bunx" if command.contains(&TSC) => {
                    let mut printed = self.tsc();
                    if self.failing.contains(&"tsc") {
                        printed.push_str(TSC_ERROR);
                    }
                    (printed, String::new())
                }
                "bun" if script(command) == Some(shellcheck::WORKFLOW_SHELLS) => {
                    (self.shells(input), String::new())
                }
                "bun" if script(command) == Some(proof::PRETTIER_FILES) => {
                    let kept: Vec<&str> = input
                        .unwrap_or("")
                        .split('\0')
                        .filter(|path| {
                            ["md", "yml", "json", "ts"]
                                .iter()
                                .any(|extension| tree::has_extension(path, extension))
                        })
                        .collect();
                    (kept.join("\0"), String::new())
                }
                "actionlint" => {
                    let linted = self.reported(program, &handed);
                    let shape =
                        |path: &str| format!("verbose: Found total 0 errors in 1 ms for {path}");
                    (String::new(), lines(&linted, shape))
                }
                "zizmor" if command.contains(&"--no-config") => (
                    format!(
                        r#"[{{"ident":"secrets-inherit","locations":[{{"symbolic":{{"kind":"Primary","key":{{"Local":{{"verbatim_path":".github/workflows/deps.yml"}}}}}},"concrete":{{"feature":"{}"}}}}]}}]"#,
                        self.callee
                    ),
                    String::new(),
                ),
                "zizmor" => {
                    let inputs: Vec<&str> = self
                        .tracked
                        .iter()
                        .copied()
                        .filter(|path| path.starts_with(".github/"))
                        .collect();
                    let completed = self.reported(program, &inputs);
                    let shape = |path: &str| format!(" INFO audit: zizmor: completed {path}");
                    (String::new(), lines(&completed, shape))
                }
                _ => (String::new(), String::new()),
            }
        }
    }

    impl FakeRunner {
        /// The workflow shell report for the NUL-separated paths in `input`:
        /// one step per workflow, under the fake's shell, less what the case
        /// has the reader leave out.
        fn shells(&self, input: Option<&str>) -> String {
            let paths: Vec<&str> = input
                .unwrap_or("")
                .split('\0')
                .filter(|path| !path.is_empty())
                .collect();
            let entries: Vec<serde_json::Value> = self
                .reported("bun", &paths)
                .iter()
                .map(|path| {
                    serde_json::json!({
                        "path": path,
                        "shells": [{ "at": self.step_at, "shell": self.shell }],
                    })
                })
                .collect();
            serde_json::Value::Array(entries).to_string()
        }

        /// The junit report `bun test` writes: one suite per file it was
        /// handed by path, less what the case has it leave out, named with the
        /// platform separator Bun uses on Windows.
        fn junit(&self) -> Option<String> {
            let ran = self.ran();
            let test = ran.iter().rev().find(|command| {
                basename(&command[0]) == "bun" && command.get(2).map(String::as_str) == Some("test")
            })?;
            let handed: Vec<&str> = test
                .iter()
                .filter_map(|arg| arg.strip_prefix("./"))
                .collect();
            let suites: Vec<String> = self
                .reported("bun", &handed)
                .iter()
                .map(|path| {
                    let name = path.replace('/', "\\");
                    let skipped = u8::from(self.skipping.contains(path));
                    format!(
                        "<testsuite name=\"{name}\" file=\"{name}\" tests=\"1\" skipped=\"{skipped}\"></testsuite>"
                    )
                })
                .collect();
            Some(format!(
                "<testsuites name=\"bun test\">{}</testsuites>",
                suites.concat()
            ))
        }

        /// What tsc's `--listFiles` prints: a library file under
        /// `node_modules`, then each tracked TypeScript file the case leaves in.
        fn tsc(&self) -> String {
            let typescript: Vec<&str> = self
                .tracked
                .iter()
                .copied()
                .filter(|path| path.starts_with("tools/") && tree::has_extension(path, "ts"))
                .collect();
            let read = self.reported("tsc", &typescript);
            let mut listed = format!("{ROOT}/node_modules/typescript/lib/lib.es5.d.ts\n");
            listed.push_str(&lines(&read, |path| format!("{ROOT}/{path}")));
            listed
        }
    }

    /// The error a failing tsc prints after the files it listed.
    const TSC_ERROR: &str =
        "tools/a.ts(1,7): error TS2322: Type 'string' is not assignable to type 'number'.\n";

    /// One line per path, each shaped by `shape`.
    fn lines(paths: &[&str], shape: impl Fn(&str) -> String) -> String {
        paths.iter().fold(String::new(), |mut text, path| {
            text.push_str(&shape(path));
            text.push('\n');
            text
        })
    }

    /// The file name a command runs, without its directory.
    fn basename(program: &str) -> &str {
        program.rsplit(['/', '\\']).next().unwrap_or(program)
    }

    /// The script a `bun -e` command evaluates.
    fn script<'a>(command: &[&'a str]) -> Option<&'a str> {
        let at = command.iter().position(|arg| *arg == "-e")?;
        command.get(at + 1).copied()
    }

    /// The pin file and lockfile for `pair` the fake reads, built from
    /// `pins::TOOLS` so the pin rules pass and the cases exercise the rows
    /// rather than the pin round.
    fn sound_pair(pair: pins::Pair) -> (String, String) {
        let digest = "a".repeat(64);
        let mut pinned = String::from("[tools]\n");
        let mut lock = String::new();
        for tool in pins::TOOLS.iter().filter(|tool| tool.pair == pair) {
            writeln!(pinned, "\"{}\" = \"1.2.3\"", tool.key).expect("write to a String");
            writeln!(
                lock,
                "[[tools.\"{}\"]]\nversion = \"1.2.3\"\nbackend = \"{}\"",
                tool.key,
                tool.coordinate()
            )
            .expect("write to a String");
            for asset in tool.assets {
                let platform = asset.platform;
                writeln!(
                    lock,
                    "[tools.\"{}\".\"platforms.{platform}\"]\nchecksum = \"sha256:{digest}\"\nurl = \"{}\"\nurl_api = \"{}1\"",
                    tool.key,
                    tool.url(platform, "1.2.3").expect("an asset names its own platform"),
                    tool.api_prefix()
                )
                .expect("write to a String");
                if let Some(provenance) = tool.provenance {
                    writeln!(lock, "provenance = \"{provenance}\"").expect("write to a String");
                }
            }
        }
        if pair == pins::MAIN {
            let platforms: Vec<String> = pins::TOOLS[0]
                .assets
                .iter()
                .map(|asset| format!("\"{}\"", asset.platform))
                .collect();
            write!(
                pinned,
                "\n[tool_config]\nlocked = true\n\n[settings]\nlocked = true\nlockfile = true\nlocked_verify_provenance = true\nprovenance_api_failures_fatal = true\ngithub_attestations = true\nlockfile_platforms = [{}]\nurl_replacements = {{ '{}' = \"{}\" }}\n\n[settings.aqua]\ngithub_attestations = true\n",
                platforms.join(", "),
                pins::URL_API_PATTERN,
                pins::URL_API_REFUSED
            )
            .expect("write to a String");
        }
        (pinned, lock)
    }

    impl Runner for FakeRunner {
        fn capture(&self, command: &[&str]) -> Option<String> {
            (command == ["gh", "auth", "token"] && self.logged_in).then(|| "ghp_fake\n".to_string())
        }

        fn capture_any(&self, command: &[&str]) -> Option<String> {
            if !basename(command[0]).starts_with("actionlint") {
                return None;
            }
            let workflow = std::fs::read_to_string(command.last()?).ok()?;
            Some(
                if workflow.contains("# shellcheck disable") && !self.unrefused {
                    format!(
                        "canary.yml:7:11: shellcheck reported issue in this script: SC0:error:1:1: {}, because it drops findings from actionlint's report",
                        shellcheck::REFUSAL
                    )
                } else {
                    "canary.yml:6:9: shellcheck reported issue in this script: SC2086:info:1:6"
                        .to_string()
                },
            )
        }

        fn run(&self, command: &[&str], env: &[(&str, &str)]) -> io::Result<Exit> {
            self.record(command, env);
            Ok(if self.failing.contains(&basename(command[0])) {
                Exit::Err
            } else {
                Exit::Ok
            })
        }

        fn output(
            &self,
            command: &[&str],
            env: &[(&str, &str)],
            input: Option<&str>,
        ) -> io::Result<Captured> {
            self.record(command, env);
            let program = basename(command[0]);
            let (stdout, stderr) = self.answer(program, command, input);
            let tsc = program == "bunx" && command.contains(&TSC);
            Ok(Captured {
                exit: if self.failing.contains(&program) || (tsc && self.failing.contains(&"tsc")) {
                    Exit::Err
                } else {
                    Exit::Ok
                },
                stdout,
                stderr,
            })
        }

        fn read_file(&self, relative: &str) -> Option<String> {
            if relative.ends_with("junit.xml") {
                return self.junit();
            }
            if !self.pins_readable {
                return None;
            }
            let pinned = pins::PAIRS.iter().find_map(|pair| {
                let (pinned, lock) = sound_pair(*pair);
                if relative == pair.pins {
                    Some(pinned)
                } else if relative == pair.lock {
                    Some(lock)
                } else {
                    None
                }
            });
            pinned
                .or_else(|| {
                    tree::sound_files()
                        .into_iter()
                        .find(|(path, _)| path == relative)
                        .map(|(_, text)| text)
                })
                .or_else(|| {
                    // A crate manifest below the root inherits the workspace's
                    // lints, as the tree rules hold every one to.
                    (relative.ends_with("/Cargo.toml") && self.tracked.contains(&relative)).then(
                        || "[package]\nname = \"a\"\n\n[lints]\nworkspace = true\n".to_string(),
                    )
                })
        }

        fn resolve(&self, tool: &str) -> Result<PathBuf, String> {
            if self.unresolvable.contains(&tool) {
                Err(format!("mise resolves no {tool}"))
            } else {
                Ok(PathBuf::from(format!("/fake/bin/{tool}")))
            }
        }

        fn env_var(&self, name: &str) -> Option<String> {
            (name == "CI" && self.in_ci).then(|| "true".to_string())
        }

        fn config_paths(&self) -> Result<Vec<pins::TreeEntry>, String> {
            let pairs = pins::PAIRS.iter().flat_map(|pair| [pair.pins, pair.lock]);
            Ok(pairs
                .chain(self.extra_paths.iter().copied())
                .map(|path| pins::TreeEntry {
                    path: path.to_string(),
                    link: false,
                })
                .collect())
        }

        fn listing(&self) -> Result<tree::Listing, String> {
            let ran = self.ran();
            let planted = self
                .plants
                .iter()
                .filter(|(program, _)| ran.iter().any(|command| basename(&command[0]) == *program))
                .map(|(_, path)| (*path).to_string());
            Ok(tree::Listing {
                tracked: self
                    .tracked
                    .iter()
                    .map(|path| (*path).to_string())
                    .collect(),
                untracked: self
                    .untracked
                    .iter()
                    .map(|path| (*path).to_string())
                    .chain(planted)
                    .collect(),
            })
        }

        fn exists(&self, relative: &str) -> bool {
            self.tracked.contains(&relative)
                || relative
                    .strip_prefix("node_modules/.bin/")
                    .is_some_and(|bin| {
                        !self
                            .uninstalled
                            .iter()
                            .any(|tool| bin.trim_end_matches(".exe") == *tool)
                    })
        }

        fn root(&self) -> PathBuf {
            PathBuf::from(ROOT)
        }
    }

    fn gate(runner: &FakeRunner) -> (Vec<Row>, String) {
        let mut out = Vec::new();
        let rows = Gate::new(STEPS, runner)
            .run(&mut out)
            .expect("write to a Vec");
        (rows, String::from_utf8(out).expect("utf-8"))
    }

    /// The row named `step` from a run.
    fn row<'a>(rows: &'a [Row], step: &str) -> &'a Row {
        rows.iter()
            .find(|row| row.step == step)
            .unwrap_or_else(|| panic!("{step} did not run: {rows:?}"))
    }

    /// The order is the contract `--rows` prints, so a row added out of place
    /// or twice is caught here, and every walking row's note is the count of
    /// what its tool reported checking.
    #[test]
    fn the_gate_runs_its_rows_in_the_documented_order() {
        let names: Vec<&str> = STEPS.iter().map(|step| step.name).collect();
        assert_eq!(
            names,
            [
                "fmt",
                "taplo",
                "deny",
                "machete",
                "prettier",
                "actionlint",
                "zizmor",
                "typecheck",
                "tools",
                "clippy",
                "tests",
                "doctests",
                "doc"
            ]
        );
        let runner = FakeRunner::all_installed();
        let (rows, text) = gate(&runner);
        let ran: Vec<&str> = rows.iter().map(|row| row.step).collect();
        assert_eq!(ran[0], "pins");
        assert_eq!(&ran[1..], names.as_slice());
        assert!(rows.iter().all(Row::passed), "{rows:?}\n{text}");
        for (step, note) in [
            ("fmt", "2 files"),
            ("taplo", "3 files"),
            ("machete", "2 crates"),
            ("prettier", "8 files"),
            ("typecheck", "2 files"),
            ("tools", "1 file"),
            ("actionlint", "2 files"),
            ("zizmor", "2 workflows, online"),
            ("doctests", "1 example"),
        ] {
            assert_eq!(
                row(&rows, step).outcome,
                Outcome::Passed(note.to_string()),
                "{step}"
            );
        }
        for step in STEPS {
            assert!(
                !step.covers.is_empty(),
                "{} says nothing about what it covers",
                step.name
            );
        }
    }

    /// This repository declares no absence of examples, so the doctests row
    /// fails on a run that ran none: every example ignored, or none at all.
    #[test]
    fn the_doctests_row_fails_when_no_example_ran() {
        for (what, runner, sentence) in [
            (
                "every example ignored",
                FakeRunner::all_installed().examples(0, 2, 0),
                "cargo test --doc ignored every example it counted, 2 of them, so none ran",
            ),
            (
                "no example at all",
                FakeRunner::all_installed().examples(0, 0, 0),
                "cargo test --doc ran no documented example. Write one, or declare none beside the step table",
            ),
        ] {
            let (rows, text) = gate(&runner);
            let last = rows.last().expect("one row");
            assert_eq!(last.step, "doctests", "{what}");
            assert_eq!(last.outcome, Outcome::Failed, "{what}");
            assert!(text.contains(sentence), "{what}: {text}");
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

    /// Every mise tool runs by the path the runner resolved, never by name.
    #[test]
    fn every_mise_row_runs_the_path_mise_resolved() {
        let runner = FakeRunner::all_installed();
        gate(&runner);
        for step in STEPS {
            if let Program::Mise(tool) = step.program {
                let command = runner
                    .command(tool)
                    .unwrap_or_else(|| panic!("{tool} never ran"));
                assert_eq!(command[0], format!("/fake/bin/{tool}"), "{}", step.name);
            }
        }
        for command in runner.ran() {
            for tool in [
                "taplo",
                "cargo-nextest",
                "cargo-deny",
                "cargo-machete",
                "actionlint",
                "zizmor",
            ] {
                assert_ne!(command[0], tool, "{tool} ran by name");
            }
        }
    }

    /// Each tool runs with its one config named, so no nearer copy reaches it:
    /// rustfmt with its config path, clippy under the root as its config
    /// directory, cargo-deny with its config ahead of `check`, taplo and zizmor
    /// with theirs, and Prettier with its config, its ignore file and no
    /// editorconfig.
    #[test]
    fn every_tool_runs_with_its_one_config_named() {
        let runner = FakeRunner::all_installed();
        gate(&runner);
        let joined = |program: &str| runner.command(program).expect(program).join(" ");
        let starts = |program: &str, wanted: &str| {
            let ran = joined(program);
            assert!(
                ran.starts_with(wanted),
                "{program} ran as {ran:?}, and it must start {wanted:?}"
            );
        };
        let carries = |program: &str, wanted: &str| {
            let ran = joined(program);
            assert!(
                ran.contains(wanted),
                "{program} ran as {ran:?}, and it must carry {wanted:?}"
            );
        };
        starts(
            "cargo",
            "cargo fmt --check -- --config-path rustfmt.toml --verbose",
        );
        starts(
            "cargo-deny",
            "/fake/bin/cargo-deny --locked --config deny.toml check ",
        );
        carries("taplo", " --config .taplo.toml -- ");
        carries("zizmor", " --config .github/zizmor.yml ");
        carries("cargo-nextest", " --no-tests=fail --user-config-file none");
        let prettier = runner.running(PRETTIER).expect("prettier ran").join(" ");
        let wanted = format!(
            "bunx --bun --no-install {PRETTIER} --check --config .prettierrc --ignore-path .prettierignore --no-editorconfig -- "
        );
        assert!(
            prettier.starts_with(&wanted),
            "prettier ran as {prettier:?}, and it must start {wanted:?}"
        );
        let clippy = runner
            .ran()
            .iter()
            .position(|command| command.get(1).map(String::as_str) == Some("clippy"))
            .expect("clippy ran");
        let env = runner.envs.borrow()[clippy].clone();
        assert!(
            env.contains(&("CLIPPY_CONF_DIR".to_string(), ROOT.to_string())),
            "clippy ran with {env:?}, and it must set CLIPPY_CONF_DIR to {ROOT}"
        );
    }

    /// Every Bun the gate starts itself skips env files, so no untracked env
    /// file reaches a script it evaluates or the tests it runs.
    #[test]
    fn every_bun_the_gate_starts_skips_env_files() {
        let runner = FakeRunner::all_installed();
        gate(&runner);
        let buns: Vec<Vec<String>> = runner
            .ran()
            .into_iter()
            .filter(|command| basename(&command[0]) == "bun")
            .collect();
        assert!(buns.len() >= 3, "the gate started {} Buns", buns.len());
        for command in buns {
            assert_eq!(
                command.get(1).map(String::as_str),
                Some("--no-env-file"),
                "Bun ran as {command:?}"
            );
        }
    }

    /// cargo-machete is handed each tracked crate's directory, and never the
    /// root, so the walk reaches nothing a crate's own directory does not hold.
    #[test]
    fn machete_is_handed_each_tracked_crate() {
        let runner = FakeRunner::all_installed();
        gate(&runner);
        let machete = runner.command("cargo-machete").expect("machete ran");
        assert_eq!(
            machete[1..],
            ["--no-ignore", "--skip-target-dir", "crates/a", "xtask"]
        );
    }

    /// Prettier is handed exactly the tracked files its own file info keeps,
    /// read by a script that asks with the ignore file and no config.
    #[test]
    fn prettier_is_handed_what_its_file_info_keeps() {
        let runner = FakeRunner::all_installed();
        gate(&runner);
        let ask = runner.command("bun").expect("bun asked Prettier");
        assert_eq!(ask[1..3], ["--no-env-file", "-e"]);
        assert!(ask[3].contains("resolveConfig: false"), "{}", ask[3]);
        assert!(
            ask[3].contains("ignorePath: '.prettierignore'"),
            "{}",
            ask[3]
        );
        assert!(
            ask[3].contains("import('./node_modules/prettier/index.mjs')"),
            "the file-info script imports Prettier from outside the checkout: {}",
            ask[3]
        );
        let bunx = runner.running(PRETTIER).expect("prettier ran");
        let handed = &bunx[bunx.iter().position(|arg| arg == "--").expect("--") + 1..];
        assert_eq!(
            handed,
            [
                ".github/workflows/ci.yml",
                ".github/workflows/deps.yml",
                ".github/dependabot.yml",
                "README.md",
                "docs/dev.md",
                "tsconfig.json",
                "tools/a.ts",
                "tools/a.test.ts"
            ]
        );
    }

    /// A walking row fails, naming the file, when its tool never reports one
    /// it was handed, and the gate stops there.
    #[test]
    fn a_walking_row_fails_when_its_tool_skips_a_file() {
        for (tool, path, step, sentence) in [
            (
                "cargo",
                "xtask/src/main.rs",
                "fmt",
                "rustfmt never reported 1 file of the 2 files it had to read: \"xtask/src/main.rs\"",
            ),
            (
                "taplo",
                "crates/a/Cargo.toml",
                "taplo",
                "taplo never reported 1 file of the 3 files it had to read: \"crates/a/Cargo.toml\"",
            ),
            (
                "cargo-machete",
                "xtask",
                "machete",
                "cargo-machete never reported 1 crate of the 2 crates it had to read: \"xtask\"",
            ),
            (
                "actionlint",
                ".github/workflows/deps.yml",
                "actionlint",
                "actionlint never reported 1 file of the 2 files it had to read: \".github/workflows/deps.yml\"",
            ),
        ] {
            let runner = FakeRunner::all_installed().unreported(tool, path);
            let (rows, text) = gate(&runner);
            let last = rows.last().expect("one row");
            assert_eq!(last.step, step, "{path}: {sentence}");
            assert_eq!(last.outcome, Outcome::Failed, "{path}: {sentence}");
            assert!(text.contains(sentence), "{path}: {text}");
        }
    }

    /// tsc must read every tracked TypeScript file tsconfig.json includes,
    /// and bun test must run every tracked test file, or the row
    /// fails naming the gap.
    #[test]
    fn a_typescript_row_fails_when_its_tool_skips_a_file() {
        for (tool, path, step, sentence) in [
            (
                "tsc",
                "tools/a.ts",
                "typecheck",
                "tsc never reported 1 file of the 2 files it had to read: \"tools/a.ts\"",
            ),
            (
                "bun",
                "tools/a.test.ts",
                "tools",
                "bun test never reported 1 file of the 1 file it had to read: \"tools/a.test.ts\"",
            ),
        ] {
            let runner = FakeRunner::all_installed().unreported(tool, path);
            let (rows, text) = gate(&runner);
            let last = rows.last().expect("one row");
            assert_eq!(last.step, step, "{path}: {sentence}");
            assert_eq!(last.outcome, Outcome::Failed, "{path}: {sentence}");
            assert!(text.contains(sentence), "{path}: {text}");
        }
    }

    /// A test file bun test reports with every test skipped fails the tools
    /// row, naming the file, though bun test exits zero over it.
    #[test]
    fn a_test_file_whose_every_test_was_skipped_fails_the_tools_row() {
        let runner = FakeRunner::all_installed().skipping("tools/a.test.ts");
        let (rows, text) = gate(&runner);
        let last = rows.last().expect("one row");
        assert_eq!(last.step, "tools");
        assert_eq!(last.outcome, Outcome::Failed);
        let sentence = "bun test ran no test in \"tools\\\\a.test.ts\": each one there was skipped, a todo or held back by its condition";
        assert!(text.contains(sentence), "{text}");
    }

    /// A failing tsc prints its errors without the files it listed, and the
    /// row names the failure.
    #[test]
    fn a_failing_typecheck_prints_its_errors_alone() {
        let runner = FakeRunner::all_installed().failing("tsc");
        let (rows, text) = gate(&runner);
        let last = rows.last().expect("one row");
        assert_eq!(last.step, "typecheck", "{text}");
        assert_eq!(last.outcome, Outcome::Failed, "{text}");
        assert!(
            text.contains("tools/a.ts(1,7): error TS2322"),
            "tsc's error was not printed: {text}"
        );
        assert!(
            text.contains("tsc exited non-zero, and its errors are above"),
            "the row did not say why it failed: {text}"
        );
        assert!(
            !text.contains(&format!("{ROOT}/tools/a.ts")),
            "a file tsc listed was printed: {text}"
        );
    }

    /// A tracked TypeScript file outside `tools/` fails the typecheck row,
    /// since tsconfig.json never reads it.
    #[test]
    fn a_typescript_file_outside_tools_fails_its_row() {
        let sentence = "tsc never reported 1 file of the 3 files it had to read: \"scripts/x.ts\"";
        let mut tracked = TRACKED.to_vec();
        tracked.push("scripts/x.ts");
        let runner = FakeRunner::all_installed().tracking(&tracked);
        let (rows, text) = gate(&runner);
        let last = rows.last().expect("one row");
        assert_eq!(last.outcome, Outcome::Failed, "{sentence}");
        assert_eq!(last.step, "typecheck", "{sentence}");
        assert!(text.contains(sentence), "{sentence}: {text}");
    }

    /// bun test is handed every tracked test file by its `./` path, wherever
    /// it sits, with CI set so a committed `test.only` fails, and writes the
    /// junit report the row reads.
    #[test]
    fn bun_test_is_handed_each_tracked_test_file() {
        let mut tracked = TRACKED.to_vec();
        tracked.push("scripts/x.spec.js");
        let runner = FakeRunner::all_installed().tracking(&tracked);
        let (rows, _) = gate(&runner);
        assert_eq!(
            row(&rows, "tools").outcome,
            Outcome::Passed("2 files".to_string())
        );
        let ran = runner.ran();
        let at = ran
            .iter()
            .position(|command| {
                basename(&command[0]) == "bun" && command.get(2).map(String::as_str) == Some("test")
            })
            .expect("bun test ran");
        let test = &ran[at];
        assert!(test.contains(&"--reporter=junit".to_string()), "{test:?}");
        assert!(
            test.iter()
                .any(|arg| arg.starts_with("--reporter-outfile=")),
            "{test:?}"
        );
        let handed: Vec<&str> = test
            .iter()
            .map(String::as_str)
            .filter(|arg| arg.starts_with("./"))
            .collect();
        assert_eq!(handed, ["./tools/a.test.ts", "./scripts/x.spec.js"]);
        assert!(
            runner.envs.borrow()[at].contains(&("CI".to_string(), "true".to_string())),
            "bun test runs without CI set"
        );
    }

    /// A junit report names each file once, however many suites sit in it, with
    /// its entities decoded.
    #[test]
    fn a_junit_report_names_each_file_once() {
        let report = r#"<testsuites name="bun test"><testsuite name="tools\a.test.ts" file="tools\a.test.ts"><testsuite name="inner" file="tools\a.test.ts"></testsuite></testsuite><testsuite name="b" file="tools\b &amp; c.test.ts"></testsuite></testsuites>"#;
        assert_eq!(
            junit_files(report),
            ["tools\\a.test.ts", "tools\\b & c.test.ts"],
            "a report names each file once, its entities decoded"
        );
    }

    /// A file ran nothing when its outermost suite counts no more tests than
    /// skipped ones, nested suites taken in, or carries no count at all. The
    /// reports are the shapes bun test 1.4.2 writes.
    #[test]
    fn a_file_whose_every_test_was_skipped_ran_nothing() {
        for (what, report, idle) in [
            (
                "a skipped test",
                r#"<testsuite name="s.test.ts" file="s.test.ts" tests="1" assertions="0" failures="0" skipped="1">"#,
                vec!["s.test.ts"],
            ),
            (
                "one test runs beside skipped ones",
                r#"<testsuite name="m.test.ts" file="m.test.ts" tests="3" assertions="1" failures="0" skipped="2"><testsuite name="group" file="m.test.ts" line="4" tests="1" assertions="0" failures="0" skipped="1">"#,
                vec![],
            ),
            (
                "every test sits in a skipped group",
                r#"<testsuite name="n.test.ts" file="n.test.ts" tests="2" assertions="0" failures="0" skipped="2"><testsuite name="outer" file="n.test.ts" line="2" tests="2" assertions="0" failures="0" skipped="2">"#,
                vec!["n.test.ts"],
            ),
            (
                "a suite with no count",
                r#"<testsuite name="c.test.ts" file="c.test.ts">"#,
                vec!["c.test.ts"],
            ),
            (
                "a count that is not a number",
                r#"<testsuite name="x.test.ts" file="x.test.ts" tests="many" skipped="0">"#,
                vec!["x.test.ts"],
            ),
            (
                "every test ran",
                r#"<testsuite name="r.test.ts" file="r.test.ts" tests="2" skipped="0">"#,
                vec![],
            ),
        ] {
            assert_eq!(junit_idle(report), idle, "{what}");
        }
    }

    /// The root `tsconfig.json` this repository keeps passes the tree rules.
    #[test]
    fn the_kept_project_config_passes_the_tree_rules() {
        let runner = FakeRunner::all_installed();
        let (_, text) = gate(&runner);
        assert!(
            !text.contains("\"tsconfig.json\" is a TypeScript project config"),
            "the kept tsconfig.json was refused: {text}"
        );
    }

    /// zizmor must report every tracked workflow completed, dependabot.yml
    /// aside, or the row fails naming the one it skipped.
    #[test]
    fn zizmor_fails_when_it_skips_a_workflow() {
        let runner = FakeRunner::all_installed().unreported("zizmor", ".github/workflows/ci.yml");
        let (rows, text) = gate(&runner);
        let last = rows.last().expect("one row");
        assert_eq!(last.step, "zizmor");
        assert_eq!(last.outcome, Outcome::Failed);
        assert!(
            text.contains("zizmor never reported 1 file of the 2 files it had to read: \".github/workflows/ci.yml\""),
            "{text}"
        );
    }

    /// A walking row with nothing to read fails rather than passing over
    /// nothing.
    #[test]
    fn a_walking_row_with_nothing_to_read_fails() {
        let runner = FakeRunner::all_installed().tracking(&["crates/a/src/lib.rs", "README.md"]);
        let (rows, text) = gate(&runner);
        let last = rows.last().expect("one row");
        assert_eq!(last.step, "taplo");
        assert_eq!(last.outcome, Outcome::Failed);
        assert!(
            text.contains("no tracked file is one taplo reads, so the row checks nothing"),
            "{text}"
        );
    }

    /// The held zizmor pass runs with no config and no ignores, and a job
    /// handing its secrets to a workflow outside the shared ones fails the row.
    #[test]
    fn a_secrets_inherit_callee_outside_the_shared_workflows_fails() {
        let runner = FakeRunner::all_installed();
        gate(&runner);
        let held = runner
            .ran()
            .into_iter()
            .find(|command| command.contains(&"--no-config".to_string()))
            .expect("the held pass ran");
        assert_eq!(
            held[1..],
            [
                "--offline",
                "--no-config",
                "--no-ignores",
                "--strict-collection",
                "--format",
                "json",
                "--collect=all",
                ".github"
            ]
        );
        let runner =
            FakeRunner::all_installed().calling("someone/else/.github/workflows/x.yml@main");
        let (rows, text) = gate(&runner);
        let last = rows.last().expect("one row");
        assert_eq!(last.step, "zizmor");
        assert_eq!(last.outcome, Outcome::Failed);
        assert!(
            text.contains(
                "passes secrets: inherit to \"someone/else/.github/workflows/x.yml@main\""
            ),
            "{text}"
        );
    }

    /// A tool mise resolves nowhere stops the gate at its row, names the install
    /// command, and runs nothing after it.
    #[test]
    fn an_unresolvable_tool_stops_the_gate_and_names_the_install() {
        let runner = FakeRunner::all_installed().unresolvable("cargo-deny");
        let (rows, text) = gate(&runner);
        let last = rows.last().expect("one row");
        assert_eq!(last.step, "deny");
        assert!(
            matches!(&last.outcome, Outcome::Unrun(why) if why.contains(MISE_INSTALL)),
            "{last:?}"
        );
        assert!(
            runner.command("cargo-machete").is_none(),
            "a row after deny ran"
        );
        assert!(text.contains("the gate failed at deny"), "{text}");
    }

    /// A row that exits non-zero stops the gate there.
    #[test]
    fn a_failing_row_stops_the_gate() {
        let runner = FakeRunner::all_installed().failing("cargo-nextest");
        let (rows, _) = gate(&runner);
        let last = rows.last().expect("one row");
        assert_eq!(last.step, "tests");
        assert_eq!(last.outcome, Outcome::Failed);
    }

    /// actionlint takes this binary's stand-in over the `ShellCheck` mise
    /// resolved, both paths single-quoted with forward slashes, and the row
    /// refuses to run when mise resolves none, because actionlint would skip
    /// the analysis in silence.
    #[test]
    fn actionlint_takes_shellcheck_by_resolved_path_or_refuses() {
        let runner = FakeRunner::all_installed();
        gate(&runner);
        let actionlint = runner.command("actionlint").expect("actionlint ran");
        let stand_in = std::env::current_exe()
            .expect("the test binary")
            .display()
            .to_string()
            .replace('\\', "/");
        let flag = format!(
            "-shellcheck='{stand_in}' {} '/fake/bin/shellcheck'",
            shellcheck::SUBCOMMAND
        );
        assert!(
            actionlint.contains(&flag),
            "{actionlint:?} carries no {flag}"
        );
        assert!(
            actionlint.contains(&"-pyflakes=".to_string()),
            "{actionlint:?}"
        );

        let runner = FakeRunner::all_installed().unresolvable("shellcheck");
        let (rows, _) = gate(&runner);
        let last = rows.last().expect("one row");
        assert_eq!(last.step, "actionlint");
        assert!(
            matches!(&last.outcome, Outcome::Unrun(why) if why.contains("shellcheck")),
            "{last:?}"
        );
    }

    /// The canary is judged on the finding it was written to produce, so an
    /// analyzer that prints anything else, or nothing, fails it.
    #[test]
    fn the_canary_passes_only_on_its_own_finding() {
        assert!(canary_passed(
            "canary.yml:6:9: shellcheck reported issue in this script: SC2086:info:1:6"
        ));
        assert!(!canary_passed(""));
        assert!(!canary_passed(
            "canary.yml:6:9: shellcheck reported issue in this script: SC2046:warning"
        ));
        assert!(directive_refused(&format!(
            "canary.yml:7:11: shellcheck reported issue in this script: SC0:error:1:1: {}",
            shellcheck::REFUSAL
        )));
        assert!(!directive_refused(
            "canary.yml:6:9: shellcheck reported issue in this script: SC2086:info:1:6"
        ));
        assert!(!directive_refused(""));
    }

    /// cargo-machete names a directory as clean when it cannot read the crate
    /// there, so the row fails on the line saying it could not.
    #[test]
    fn a_crate_machete_cannot_read_fails_the_row() {
        let runner = FakeRunner::all_installed().unreadable("crates/a");
        let (rows, text) = gate(&runner);
        let sentence = "cargo-machete could not read a crate it was handed: error when handling crates/a: the manifest does not parse";
        let last = rows.last().expect("one row");
        assert_eq!(last.outcome, Outcome::Failed, "{sentence}");
        assert_eq!(last.step, "machete", "{sentence}");
        assert!(text.contains(sentence), "{sentence}: {text}");
    }

    /// A config a build or test row writes stops the gate at the next row, since
    /// the tree rules run again before each one.
    #[test]
    fn a_config_written_mid_run_stops_the_next_row() {
        let runner =
            FakeRunner::all_installed().planting("cargo-nextest", ".github/actionlint.yaml");
        let (rows, text) = gate(&runner);
        let last = rows.last().expect("one row");
        assert_eq!(
            last.outcome,
            Outcome::Unrun(
                "the tree changed while the gate ran: resolve each finding above".to_string()
            )
        );
        assert_eq!(last.step, "doctests");
        assert!(text.contains("\".github/actionlint.yaml\""), "{text}");
    }

    /// A failure sentence carrying text a workflow wrote is printed with its
    /// control characters escaped, so it cannot move or erase terminal lines.
    #[test]
    fn a_failure_sentence_prints_no_control_character() {
        let runner = FakeRunner::all_installed()
            .shell("python")
            .step_at("jobs.a\u{1b}[2K.steps[0].shell");
        let (rows, text) = gate(&runner);
        assert_eq!(rows.last().expect("one row").outcome, Outcome::Failed);
        assert!(!text.contains('\u{1b}'), "{text:?}");
        assert!(text.contains("jobs.a\\u{1b}[2K.steps[0].shell"), "{text:?}");
    }

    /// A JavaScript tool bunx starts must be in the checkout's own
    /// `node_modules/.bin`, or its row refuses to run: bunx would run a copy
    /// from a parent directory or PATH.
    #[test]
    fn a_js_tool_missing_from_the_checkout_stops_its_row() {
        for (tool, step) in [("prettier", "prettier"), ("tsc", "typecheck")] {
            let runner = FakeRunner::all_installed().uninstalled(tool);
            let (rows, _) = gate(&runner);
            let last = rows.last().expect("one row");
            assert_eq!(last.step, step, "{tool}");
            assert_eq!(
                last.outcome,
                Outcome::Unrun(format!(
                    "{} is not in the checkout: bun install",
                    super::installed_bin(tool)
                )),
                "{tool}"
            );
        }
    }

    /// The row refuses to run when the directive canary comes back analyzed,
    /// because actionlint then runs `ShellCheck` without the stand-in.
    #[test]
    fn the_directive_canary_must_come_back_refused() {
        let runner = FakeRunner::all_installed().unrefused();
        let (rows, _) = gate(&runner);
        let last = rows.last().expect("one row");
        assert_eq!(
            last.outcome,
            Outcome::Unrun(
                "actionlint ran without the ShellCheck stand-in: the canary carrying a directive came back unrefused"
                    .to_string()
            )
        );
        assert_eq!(last.step, "actionlint");
    }

    /// A step shell outside bash, sh and pwsh fails the actionlint row before
    /// actionlint runs, and so does a workflow the shell read skipped.
    #[test]
    fn a_step_shell_outside_the_list_fails_the_actionlint_row() {
        for (runner, sentence) in [
            (
                FakeRunner::all_installed().shell("python"),
                "\".github/workflows/ci.yml\" runs jobs.a.steps[0].shell under the shell \"python\", and actionlint hands only a bash or sh script to ShellCheck. Use bash, sh, pwsh",
            ),
            (
                FakeRunner::all_installed().unreported("bun", ".github/workflows/deps.yml"),
                "bun never reported 1 file of the 2 files it had to read: \".github/workflows/deps.yml\"",
            ),
        ] {
            let (rows, text) = gate(&runner);
            let last = rows.last().expect("one row");
            assert_eq!(last.step, "actionlint", "{sentence}");
            assert_eq!(last.outcome, Outcome::Failed, "{sentence}");
            assert!(text.contains(sentence), "{sentence}: {text}");
            assert!(runner.command("actionlint").is_none(), "{sentence}");
        }
    }

    /// zizmor runs online with a token and offline without one, and the row
    /// says which.
    #[test]
    fn zizmor_runs_online_with_a_login_and_offline_without() {
        let runner = FakeRunner::all_installed();
        let (rows, _) = gate(&runner);
        assert_eq!(
            row(&rows, "zizmor").outcome,
            Outcome::Passed("2 workflows, online".to_string())
        );
        let audit = runner.command("zizmor").expect("zizmor ran");
        assert!(!audit.contains(&"--offline".to_string()), "{audit:?}");

        let runner = FakeRunner::all_installed().logged_out();
        let (rows, _) = gate(&runner);
        assert_eq!(
            row(&rows, "zizmor").outcome,
            Outcome::Passed("2 workflows, offline".to_string())
        );
        let audit = runner.command("zizmor").expect("zizmor ran");
        assert!(audit.contains(&"--offline".to_string()), "{audit:?}");
    }

    /// In CI zizmor runs offline and is handed no token, even where `gh`
    /// answers, because the online audits run in the shared workflows job.
    #[test]
    fn zizmor_runs_offline_in_ci_whatever_gh_answers() {
        let runner = FakeRunner::all_installed().in_ci();
        let (rows, _) = gate(&runner);
        assert_eq!(
            row(&rows, "zizmor").outcome,
            Outcome::Passed("2 workflows, offline".to_string())
        );
        let at = runner
            .ran()
            .iter()
            .position(|command| basename(&command[0]) == "zizmor")
            .expect("zizmor ran");
        assert!(runner.ran()[at].contains(&"--offline".to_string()));
        let env = runner.envs.borrow()[at].clone();
        assert!(env.iter().all(|(name, _)| name != "GH_TOKEN"), "{env:?}");
    }

    /// A pin file nothing can read stops the gate at its first row, before any
    /// tool runs.
    #[test]
    fn an_unreadable_pin_stops_the_gate() {
        let runner = FakeRunner::all_installed().unpinned();
        let (rows, text) = gate(&runner);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].step, "pins");
        assert!(!rows[0].passed());
        assert!(runner.ran().is_empty(), "a tool ran before the pin rules");
        assert!(
            text.contains(&format!("{} cannot be read", pins::PINS)),
            "{text}"
        );
    }

    /// A committed mise configuration beside the two pairs stops the gate at
    /// the pin rules, before any tool runs, because mise would merge it and its
    /// lockfile over the files those rules read.
    #[test]
    fn a_stray_mise_configuration_stops_the_gate() {
        // A stray lockfile's name holds `mise.lock`, and its remedy is still
        // removal: a relock would read the configuration the rule refuses.
        for stray in ["mise.local.toml", ".mise.lock"] {
            let runner = FakeRunner::all_installed().with_path(stray);
            let (rows, text) = gate(&runner);
            assert_eq!(rows.len(), 1, "{stray}");
            assert_eq!(rows[0].step, "pins");
            assert_eq!(
                rows[0].outcome,
                Outcome::Unrun("remove each file and link named above".to_string()),
                "{stray}"
            );
            assert!(runner.ran().is_empty(), "a tool ran before the pin rules");
            assert!(
                text.contains(&format!("{stray:?} is mise configuration beside mise.toml")),
                "{text}"
            );
        }
    }

    /// A config a tool would read in place of the named one stops the gate at
    /// the opening row, before any tool runs, on disk and untracked alike.
    #[test]
    fn a_tree_finding_stops_the_gate_before_any_tool() {
        let runner = FakeRunner::all_installed().untracked("crates/a/.cargo/config.toml");
        let (rows, text) = gate(&runner);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].step, "pins");
        assert_eq!(
            rows[0].outcome,
            Outcome::Unrun("resolve each finding above, which names what to change".to_string())
        );
        assert!(runner.ran().is_empty(), "a tool ran before the tree rules");
        assert!(
            text.contains("\"crates/a/.cargo/config.toml\" is a cargo config"),
            "{text}"
        );
    }

    /// `--rows` prints the pin rules and every row, one line each, and runs
    /// nothing.
    #[test]
    fn the_row_table_prints_one_line_per_row() {
        let runner = FakeRunner::all_installed();
        let mut out = Vec::new();
        Gate::new(STEPS, &runner)
            .rows(&mut out)
            .expect("write to a Vec");
        let text = String::from_utf8(out).expect("utf-8");
        assert_eq!(text.lines().count(), STEPS.len() + 1);
        assert!(text.lines().next().expect("a line").contains("pins"));
        assert!(runner.ran().is_empty(), "--rows ran a tool");
    }
}
