//! Repository automation, run as `cargo xtask <command>`.
//!
//! Three groups live here. `check` is the single gate that continuous
//! integration, the pre-push hook and `CONTRIBUTING.md` all call. `server`
//! fetches a dedicated server build, seeds it, launches it with the loader
//! injected, tails it and stops it. `schema` recovers the reflection schema
//! from a build and diffs two recoveries.
//!
//! Nothing recovered from a Keen binary is committed. Every extraction lands
//! under the gitignored `.cache` directory and is regenerated from a build the
//! caller fetched.

mod check;
mod hooks;
mod image;
mod kfc;
mod proc;
mod root;
mod schema;
mod server;
mod steam;
#[cfg(test)]
mod testutil;
mod ui;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

/// Commit scopes: the workspace crate names past the `ember-` prefix, plus the
/// three cross-cutting names no crate will ever own.
const SCOPES: &[&str] = &[
    "loader",
    "sdk",
    "holistic",
    "kfc",
    "enshrouded",
    "sigs",
    "platform",
    "testkit",
    "xtask",
    "deps",
    "ci",
    "release",
];

#[derive(Parser)]
#[command(name = "xtask", about = "Repository automation for Ember")]
struct Cli {
    #[command(flatten)]
    global: GlobalArgs,
    #[command(subcommand)]
    command: Command,
}

/// Flags every command shares.
#[derive(Args, Clone)]
struct GlobalArgs {
    /// Directory holding `.cache`. Overrides `EMBER_DEV_ROOT` and the
    /// workspace root.
    #[arg(long, global = true, value_name = "PATH")]
    root: Option<PathBuf>,
    /// Print only failures, and emit no color.
    #[arg(long, global = true)]
    quiet: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Print the commit scopes this repository accepts.
    Scopes,
    /// Run every gate step in order and stop at the first failure.
    Check,
    /// Manage the repository's git hooks.
    #[command(subcommand)]
    Hooks(HooksCommand),
    /// Fetch, seed, launch, tail and stop a dedicated server build.
    #[command(subcommand)]
    Server(ServerCommand),
    /// Recover the reflection schema from a build, and diff two recoveries.
    #[command(subcommand)]
    Schema(SchemaCommand),
}

#[derive(Subcommand)]
enum HooksCommand {
    /// Point `core.hooksPath` at `.githooks`.
    Install,
}

#[derive(Subcommand)]
enum ServerCommand {
    /// Pull app 2278520 from Steam into `<root>/.cache/server/<buildid>`.
    Fetch {
        /// Buildid to fetch. Defaults to the branch head, whose buildid is
        /// read from the app manifest once the pull finishes.
        #[arg(long, value_name = "BUILDID")]
        build: Option<String>,
    },
    /// Copy a fixture save and config into a build's run directory.
    Seed {
        /// Directory holding the fixture: a `savegame` directory, or the save
        /// files themselves, plus an optional `enshrouded_server.json`.
        #[arg(long, value_name = "PATH")]
        fixture: PathBuf,
        #[arg(long, value_name = "BUILDID")]
        build: Option<String>,
        /// Replace an existing seeded save.
        #[arg(long)]
        force: bool,
    },
    /// Launch a build, optionally injecting a library into it.
    Run {
        #[arg(long, value_name = "BUILDID")]
        build: Option<String>,
        /// Library to inject once the process is up.
        #[arg(long, value_name = "DLL")]
        inject: Option<PathBuf>,
        /// Wait for the server to exit instead of returning once it is up.
        #[arg(long)]
        wait: bool,
    },
    /// Print the running or last server's log.
    Logs {
        #[arg(long, value_name = "BUILDID")]
        build: Option<String>,
        /// Keep printing as the server writes.
        #[arg(long, short)]
        follow: bool,
        /// Lines to print before following. Defaults to 40.
        #[arg(long, short = 'n', value_name = "COUNT")]
        lines: Option<usize>,
    },
    /// Ask the server to shut down, and wait for the process to exit.
    Stop {
        #[arg(long, value_name = "BUILDID")]
        build: Option<String>,
        /// Seconds to wait for a clean exit. Defaults to 60.
        #[arg(long, value_name = "SECONDS")]
        timeout: Option<u64>,
        /// Terminate the process when the clean stop times out.
        #[arg(long)]
        force: bool,
        /// Raise the shutdown request for one process and exit.
        ///
        /// Joining another process's console means leaving this one, so the
        /// command that an operator types spawns itself with this flag and
        /// keeps its own terminal.
        #[arg(long, hide = true, value_name = "PID")]
        send_break: Option<u32>,
    },
}

#[derive(Subcommand)]
enum SchemaCommand {
    /// Write the seven dumps to `<root>/.cache/schema/<buildid>`.
    Extract {
        #[arg(long, value_name = "BUILDID")]
        build: Option<String>,
        /// Server executable to read instead of a fetched build's.
        #[arg(long, value_name = "EXE")]
        server: Option<PathBuf>,
        /// Client executable, which is the only source of `cli.schema.txt`.
        #[arg(long, value_name = "EXE")]
        client: Option<PathBuf>,
        /// Overwrite an existing extraction.
        #[arg(long)]
        force: bool,
    },
    /// List the extractions under `<root>/.cache/schema`.
    List,
    /// Print what changed between two extractions, watchlist hits first.
    Diff {
        /// Buildid of the older extraction, or a unique prefix of it.
        old: String,
        /// Buildid of the newer extraction, or a unique prefix of it.
        new: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let ui = ui::Ui::new(cli.global.quiet);
    match run(&cli, &ui) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(err) => {
            ui.error(&format!("{err:#}"));
            ExitCode::FAILURE
        }
    }
}

/// Dispatch one command. `Ok(false)` means the command ran and reported a
/// failure of its own, so no error text is added on top.
fn run(cli: &Cli, ui: &ui::Ui) -> anyhow::Result<bool> {
    // The shutdown helper needs no root: it joins one process's console and
    // exits. Resolving a root here would read `EMBER_DEV_ROOT` with inputs the
    // parent never had.
    if let Command::Server(ServerCommand::Stop {
        send_break: Some(pid),
        ..
    }) = &cli.command
    {
        return proc::send_break(*pid).map(|()| true);
    }
    match &cli.command {
        Command::Scopes => {
            for scope in SCOPES {
                println!("{scope}");
            }
            Ok(true)
        }
        Command::Check => check::run(ui),
        Command::Hooks(HooksCommand::Install) => hooks::install(ui).map(|()| true),
        Command::Server(command) => {
            let root = root::DevRoot::resolve(cli.global.root.as_deref())?;
            server::run(command, &root, ui)
        }
        Command::Schema(command) => {
            let root = root::DevRoot::resolve(cli.global.root.as_deref())?;
            schema::run(command, &root, ui)
        }
    }
}
