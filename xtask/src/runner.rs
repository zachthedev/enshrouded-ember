//! Process execution behind one trait.
//!
//! Every command this crate runs goes through a `Runner`. A test builds its own
//! and spawns nothing, so no test formats the tree, installs a git hook or
//! reaches the network.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Environment variables that describe this crate rather than the workspace.
///
/// Cargo sets these for a binary it launches, and a child that inherits them
/// reads them as facts about itself. `cargo-machete` reads `CARGO_PKG_NAME` to
/// decide whether its first argument is its own subcommand name, so a child
/// that inherits it parses `machete` as a directory to scan.
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

/// Environment variables no child gets, whatever this process holds.
///
/// `ShellCheck` reads extra flags from `SHELLCHECK_OPTS` whatever actionlint's
/// `--norc` says, and one can exclude any finding. Bun reads command-line flags
/// from `BUN_OPTIONS` into every process it starts, a preload or a test name
/// filter among them. rustdoc takes flags from `RUSTDOCFLAGS`,
/// `CARGO_BUILD_RUSTDOCFLAGS` and `CARGO_ENCODED_RUSTDOCFLAGS`, where a test
/// filter drops every documented example from the doctests row. Windows
/// matches a variable name in any case, and so does the removal there.
const WITHHELD_ENV: &[&str] = &[
    "SHELLCHECK_OPTS",
    "BUN_OPTIONS",
    "RUSTDOCFLAGS",
    "CARGO_BUILD_RUSTDOCFLAGS",
    "CARGO_ENCODED_RUSTDOCFLAGS",
];

/// `text` without ANSI CSI and OSC sequences. A tool can color what it prints,
/// or wrap a path in a link, on a runner that asks for color whatever
/// `NO_COLOR` says, and a row parses the plain text.
#[must_use]
pub fn plain(text: &str) -> String {
    let characters: Vec<char> = text.chars().collect();
    let mut kept = String::with_capacity(text.len());
    let mut at = 0;
    while at < characters.len() {
        if let Some(end) = sequence_end(&characters, at) {
            at = end;
        } else {
            kept.push(characters[at]);
            at += 1;
        }
    }
    kept
}

/// One past the last character of the CSI or OSC sequence opening at `at`, or
/// `None` when none opens there.
///
/// A CSI sequence is an escape, `[`, parameter bytes and one final byte, and
/// one left open runs to the end of the text. An OSC sequence is an escape, `]`
/// and a string ended by a bell or by an escape and `\`, and one left open is
/// no sequence.
fn sequence_end(characters: &[char], at: usize) -> Option<usize> {
    if characters.get(at) != Some(&'\u{1b}') {
        return None;
    }
    let body = at + 2;
    match characters.get(at + 1) {
        Some('[') => Some(
            characters[body..]
                .iter()
                .position(|next| ('\u{40}'..='\u{7e}').contains(next))
                .map_or(characters.len(), |end| body + end + 1),
        ),
        Some(']') => {
            let stop = body
                + characters[body..]
                    .iter()
                    .position(|next| matches!(next, '\u{7}' | '\u{1b}'))?;
            if characters[stop] == '\u{7}' {
                Some(stop + 1)
            } else {
                (characters.get(stop + 1) == Some(&'\\')).then_some(stop + 2)
            }
        }
        _ => None,
    }
}

/// What a command printed, and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    /// How the command ended.
    pub exit: Exit,
    /// What it wrote to standard output.
    pub stdout: String,
    /// What it wrote to standard error.
    pub stderr: String,
}

/// How one command ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// The command exited zero.
    Ok,
    /// The command exited non-zero, or a signal killed it.
    Err,
}

/// Runs commands on behalf of an xtask subcommand.
pub trait Runner {
    /// Run `command` silently and return what it printed, or `None` when the
    /// program cannot be started or exits non-zero.
    ///
    /// `command[0]` is the program. Standard output and standard error come
    /// back as one string, because a tool is free to print its release to
    /// either. A caller that only wants to know whether the tool answers reads
    /// this as `Some`, so one call covers both questions.
    fn capture(&self, command: &[&str]) -> Option<String>;

    /// Run `command` silently and return what it printed whatever its exit
    /// code, or `None` when the program cannot be started.
    ///
    /// This is for a command whose finding is its expected result, so a
    /// non-zero exit carries the answer rather than the failure.
    fn capture_any(&self, command: &[&str]) -> Option<String>;

    /// Run `command` with `env` set on top of the scrubbed environment, letting
    /// it write straight to this process's terminal.
    ///
    /// `command[0]` is the program.
    ///
    /// # Errors
    ///
    /// Returns the operating system error when the program cannot be started.
    fn run(&self, command: &[&str], env: &[(&str, &str)]) -> io::Result<Exit>;

    /// Run `command` with `env` set on top of the scrubbed environment and
    /// `input` on its standard input, and return what it printed.
    ///
    /// This is for a row that reads what its tool reports, so the row can
    /// prove which files the tool finished.
    ///
    /// # Errors
    ///
    /// Returns the operating system error when the program cannot be started.
    fn output(
        &self,
        command: &[&str],
        env: &[(&str, &str)],
        input: Option<&str>,
    ) -> io::Result<Captured>;

    /// Read the file at `relative`, resolved from the current directory, which
    /// is the repository root when the gate runs, or `None` when it cannot be
    /// read.
    fn read_file(&self, relative: &str) -> Option<String>;

    /// The path mise installs for `tool`, or the sentence saying why mise
    /// resolves none.
    ///
    /// Every pinned tool runs by this path rather than by name. The binary the
    /// gate probed, the binary a step runs and the binary handed to a tool that
    /// would otherwise look one up are then the same file by construction,
    /// rather than three lookups agreeing.
    ///
    /// # Errors
    ///
    /// Returns the sentence saying why mise resolves no `tool`.
    fn resolve(&self, tool: &str) -> Result<PathBuf, String>;

    /// The value this process holds for the environment variable `name`, or
    /// `None` when it is unset or not unicode.
    ///
    /// A test runner overrides this, so a gate case never reads the
    /// environment its own test binary runs in.
    fn env_var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    /// Every entry at the repository root and every file under the
    /// directories mise reads configuration from, which the stray
    /// configuration rule reads.
    ///
    /// # Errors
    ///
    /// Returns the sentence a result row carries when the tree cannot be
    /// listed.
    fn config_paths(&self) -> Result<Vec<crate::pins::TreeEntry>, String> {
        crate::pins::config_paths(Path::new("."))
    }

    /// The tracked files, and the untracked ones on disk, which the tree rules
    /// and every row that walks the tree read.
    ///
    /// # Errors
    ///
    /// Returns the sentence a result row carries when git cannot list the tree.
    fn listing(&self) -> Result<crate::tree::Listing, String> {
        crate::tree::listing(Path::new("."))
    }

    /// Whether `relative` names an entry on disk, a file or a directory.
    fn exists(&self, relative: &str) -> bool {
        Path::new(relative).exists()
    }

    /// The repository root, as a row compares a path a tool printed against
    /// it.
    fn root(&self) -> PathBuf {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }
}

/// The `Runner` that spawns real child processes.
pub struct Processes;

impl Runner for Processes {
    fn capture(&self, command: &[&str]) -> Option<String> {
        let (program, args) = command.split_first()?;
        let mut child = command_for(program).ok()?;
        let output = child.args(args).stdin(Stdio::null()).output().ok()?;
        if !output.status.success() {
            return None;
        }
        let mut printed = String::from_utf8_lossy(&output.stdout).into_owned();
        printed.push_str(&String::from_utf8_lossy(&output.stderr));
        Some(printed)
    }

    fn capture_any(&self, command: &[&str]) -> Option<String> {
        let (program, args) = command.split_first()?;
        let mut child = command_for(program).ok()?;
        let output = child.args(args).stdin(Stdio::null()).output().ok()?;
        let mut printed = String::from_utf8_lossy(&output.stdout).into_owned();
        printed.push_str(&String::from_utf8_lossy(&output.stderr));
        Some(printed)
    }

    fn run(&self, command: &[&str], env: &[(&str, &str)]) -> io::Result<Exit> {
        let Some((program, args)) = command.split_first() else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty command"));
        };
        let mut child = command_for(program)?;
        child.envs(env.iter().copied());
        let status = child.args(args).status()?;
        Ok(if status.success() {
            Exit::Ok
        } else {
            Exit::Err
        })
    }

    fn output(
        &self,
        command: &[&str],
        env: &[(&str, &str)],
        input: Option<&str>,
    ) -> io::Result<Captured> {
        let Some((program, args)) = command.split_first() else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty command"));
        };
        let mut child = command_for(program)?;
        // A row parses what this child prints, so it prints no color, and cargo
        // prints its headers whatever quiet setting a user config holds.
        child
            .env("NO_COLOR", "1")
            .env("CARGO_TERM_COLOR", "never")
            .env("CARGO_TERM_QUIET", "false");
        child.envs(env.iter().copied()).args(args);
        child.stdout(Stdio::piped()).stderr(Stdio::piped());
        child.stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        let mut running = child.spawn()?;
        // The input is written while the output is read, so neither side waits
        // on a full pipe.
        let writer = input.zip(running.stdin.take()).map(|(input, mut stdin)| {
            let input = input.to_string();
            std::thread::spawn(move || stdin.write_all(input.as_bytes()))
        });
        let output = running.wait_with_output()?;
        let written = match writer.map(std::thread::JoinHandle::join) {
            None | Some(Ok(Ok(()))) => Ok(()),
            Some(Ok(Err(err))) => Err(err.to_string()),
            Some(Err(_)) => Err("the thread writing it panicked".to_string()),
        };
        let mut stderr = plain(&String::from_utf8_lossy(&output.stderr));
        if let Err(reason) = &written {
            stderr.push_str("\nthe input was not read whole: ");
            stderr.push_str(reason);
            stderr.push('\n');
        }
        Ok(Captured {
            exit: if output.status.success() && written.is_ok() {
                Exit::Ok
            } else {
                Exit::Err
            },
            stdout: plain(&String::from_utf8_lossy(&output.stdout)),
            stderr,
        })
    }

    fn read_file(&self, relative: &str) -> Option<String> {
        fs::read_to_string(relative).ok()
    }

    fn resolve(&self, tool: &str) -> Result<PathBuf, String> {
        let root = std::env::current_dir()
            .map_err(|err| format!("reading the working directory: {err}"))?;
        crate::spawn::mise_which(&root, tool)
    }
}

/// A command running `program`, with this crate's build metadata removed from
/// its environment and `PATH` narrowed as `spawn` narrows it. An absolute path
/// runs as given, and a bare name runs from the absolute path `PATH` holds for
/// it.
fn command_for(program: &str) -> io::Result<Command> {
    let path = if Path::new(program).is_absolute() {
        PathBuf::from(program)
    } else {
        crate::spawn::resolve(program)
            .map_err(|problem| io::Error::new(io::ErrorKind::NotFound, problem))?
    };
    let mut command = crate::spawn::command(&path).map_err(io::Error::other)?;
    scrub(&mut command);
    Ok(command)
}

/// The prefix every cargo-nextest option read from the environment carries.
/// `NEXTEST_NO_TESTS=pass` passes a run that skipped every test,
/// `NEXTEST_RETRIES` passes a test that fails before it passes, and
/// `NEXTEST_USER_CONFIG_FILE` names a config the tests row never reads.
const WITHHELD_PREFIX: &str = "NEXTEST_";

/// Drop this crate's own build metadata, every variable in [`WITHHELD_ENV`],
/// every inherited one [`WITHHELD_PREFIX`] opens, and every inherited
/// `CARGO_TARGET_<triple>_RUSTDOCFLAGS`, which hands rustdoc a test filter for
/// one target, from a child's environment.
fn scrub(command: &mut Command) {
    scrub_inherited(command, std::env::vars_os().map(|(name, _)| name));
}

/// [`scrub`] over the variable names this process holds, `inherited`.
fn scrub_inherited(command: &mut Command, inherited: impl Iterator<Item = OsString>) {
    for name in CRATE_ENV.iter().chain(WITHHELD_ENV) {
        command.env_remove(name);
    }
    for name in inherited {
        let upper = name.to_string_lossy().to_ascii_uppercase();
        let target_doc_flags =
            upper.starts_with("CARGO_TARGET_") && upper.ends_with("_RUSTDOCFLAGS");
        if upper.starts_with(WITHHELD_PREFIX) || target_doc_flags {
            command.env_remove(&name);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::{OsStr, OsString};
    use std::process::Command;

    use super::{CRATE_ENV, scrub, scrub_inherited};

    /// `cargo-machete` reads `CARGO_PKG_NAME`, and inheriting it makes the tool
    /// scan a directory named after its own subcommand.
    #[test]
    fn the_scrubbed_set_covers_the_variable_cargo_machete_reads() {
        assert!(CRATE_ENV.contains(&"CARGO_PKG_NAME"));
    }

    /// Every scrubbed name describes this crate, never the workspace under
    /// check, so nothing here removes a variable a tool needs.
    #[test]
    fn every_scrubbed_name_is_cargo_build_metadata() {
        for name in CRATE_ENV {
            assert!(name.starts_with("CARGO_"), "{name} is not cargo metadata");
        }
    }

    /// A repeated name would hide a missing one in a hand-read of the list.
    #[test]
    fn the_scrubbed_set_has_no_repeats() {
        let mut seen = CRATE_ENV.to_vec();
        let count = seen.len();
        seen.sort_unstable();
        seen.dedup();

        assert_eq!(seen.len(), count, "two entries share a name");
    }

    /// A child loses `SHELLCHECK_OPTS` whatever this process holds, since
    /// `ShellCheck` reads extra flags from it and one can exclude any finding.
    #[test]
    fn a_child_never_gets_shellcheck_opts() {
        let mut command = Command::new("child");
        scrub(&mut command);
        let removed: Vec<&OsStr> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(name, _)| name)
            .collect();
        for name in [
            "SHELLCHECK_OPTS",
            "BUN_OPTIONS",
            "RUSTDOCFLAGS",
            "CARGO_BUILD_RUSTDOCFLAGS",
            "CARGO_ENCODED_RUSTDOCFLAGS",
            "CARGO_PKG_NAME",
        ] {
            assert!(
                removed.contains(&OsStr::new(name)),
                "a child keeps {name}: the scrub removes only {removed:?}"
            );
        }
    }

    /// A child loses every inherited `CARGO_TARGET_<triple>_RUSTDOCFLAGS`, in
    /// any case, and keeps the other target keys and the target directory.
    #[test]
    fn a_child_never_gets_target_doc_flags() {
        let mut command = Command::new("child");
        let inherited = [
            "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTDOCFLAGS",
            "cargo_target_x86_64_unknown_linux_gnu_rustdocflags",
            "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER",
            "CARGO_TARGET_DIR",
        ];
        scrub_inherited(&mut command, inherited.into_iter().map(OsString::from));
        let removed: Vec<&OsStr> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(name, _)| name)
            .collect();
        for name in &inherited[..2] {
            assert!(
                removed.contains(&OsStr::new(name)),
                "a child keeps {name}: the scrub removes only {removed:?}"
            );
        }
        for name in &inherited[2..] {
            assert!(
                !removed.contains(&OsStr::new(name)),
                "a child loses {name}, which carries no rustdoc flag"
            );
        }
    }

    /// A child loses every inherited variable that opens with `NEXTEST_`, in
    /// any case, and keeps the rest, a name that merely starts alike included.
    #[test]
    fn a_child_never_gets_a_nextest_option() {
        let mut command = Command::new("child");
        let inherited = [
            "NEXTEST_NO_TESTS",
            "nextest_retries",
            "NEXTEST_USER_CONFIG_FILE",
            "NEXTESTER",
            "PATH",
        ];
        scrub_inherited(&mut command, inherited.into_iter().map(OsString::from));
        let removed: Vec<&OsStr> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(name, _)| name)
            .collect();
        for name in [
            "NEXTEST_NO_TESTS",
            "nextest_retries",
            "NEXTEST_USER_CONFIG_FILE",
        ] {
            assert!(
                removed.contains(&OsStr::new(name)),
                "a child keeps {name}: the scrub removes only {removed:?}"
            );
        }
        for name in ["NEXTESTER", "PATH"] {
            assert!(
                !removed.contains(&OsStr::new(name)),
                "a child loses {name}, which no nextest option reads"
            );
        }
    }

    /// What a child printed reaches a row with its color sequences gone and
    /// every other character kept.
    #[test]
    fn colored_output_reads_as_plain_text() {
        for (printed, wanted) in [
            ("\u{1b}[32mFormatting\u{1b}[0m a.rs", "Formatting a.rs"),
            ("\u{1b}[1;31merror\u{1b}[39;49m: x", "error: x"),
            ("Ran 2 tests across 2 files.", "Ran 2 tests across 2 files."),
            ("a \u{1b} lone escape", "a \u{1b} lone escape"),
            ("", ""),
        ] {
            assert_eq!(super::plain(printed), wanted, "{printed:?}");
        }
    }

    /// An OSC sequence goes whole, ended by a bell or by an escape and `\`,
    /// and one left open stays as printed.
    #[test]
    fn a_link_or_title_reads_as_plain_text() {
        let esc = '\x1b';
        let bel = '\x07';
        for (what, printed, wanted) in [
            (
                "a link ended by an escape and a backslash",
                format!("{esc}]8;;https://example.com/a.rs{esc}\\a.rs{esc}]8;;{esc}\\ ok"),
                "a.rs ok".to_string(),
            ),
            (
                "a title ended by a bell",
                format!("{esc}]0;gate{bel}Formatting a.rs"),
                "Formatting a.rs".to_string(),
            ),
            (
                "a link around colored text",
                format!("{esc}]8;;x{bel}{esc}[31ma.rs{esc}[0m{esc}]8;;{bel}"),
                "a.rs".to_string(),
            ),
            (
                "one left open",
                format!("a {esc}]0;title"),
                format!("a {esc}]0;title"),
            ),
            (
                "one broken by an escape that ends nothing",
                format!("{esc}]0;t{esc}x b"),
                format!("{esc}]0;t{esc}x b"),
            ),
        ] {
            assert_eq!(super::plain(&printed), wanted, "{what}");
        }
    }
}
