//! Repository automation, run as `cargo xtask <command>`.
//!
//! Commands land here as the signature tooling arrives: resolving every table
//! row against every cached server build, diffing two builds to propose fresh
//! patterns, and packaging a release.

use clap::{Parser, Subcommand};

/// Commit scopes: the workspace crate names past the `ember-` prefix, plus the
/// three cross-cutting names no crate will ever own.
const SCOPES: &[&str] = &[
    "loader",
    "sdk",
    "holistic",
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
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print the commit scopes this repository accepts.
    Scopes,
}

fn main() {
    match Cli::parse().command {
        Command::Scopes => {
            for scope in SCOPES {
                println!("{scope}");
            }
        }
    }
}
