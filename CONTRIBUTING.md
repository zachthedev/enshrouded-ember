# Contributing

## Toolchain

`rust-toolchain.toml` pins Rust 1.98.1, and rustup installs it on the first
cargo command. A C toolchain is needed for the MinHook engine.
[Bun](https://bun.sh) runs the repository's own tooling.

The gate calls four cargo subcommands that rustup does not install.
`.github/cargo-tools` pins their versions and is the only place those numbers
live, so this installs what continuous integration installs:

```powershell
cargo install --locked @(Get-Content .github/cargo-tools | Where-Object { $_ -notmatch '^\s*#' -and $_.Trim() })
```

On a shell without PowerShell:

```sh
cargo install --locked $(grep -v '^#' .github/cargo-tools | grep .)
```

## The gate

One command, and the only one:

```sh
cargo xtask check
```

It runs, in order and stopping at the first failure:

| Step       | Command                                                              |
| ---------- | -------------------------------------------------------------------- |
| `fmt`      | `cargo fmt --check`                                                  |
| `clippy`   | `cargo clippy --workspace --all-targets -- -D warnings`              |
| `tests`    | `cargo nextest run --workspace`                                      |
| `deny`     | `cargo deny check`                                                   |
| `machete`  | `cargo machete`                                                      |
| `audit`    | `cargo audit`                                                        |
| `prettier` | `bunx --no-install --bun prettier --check` over markdown, YAML, JSON |

`tests` falls back to `cargo test --workspace` when `cargo-nextest` is absent,
and the summary says which runner ran. Any other missing tool stops the gate and
names itself, because a check that did not run is not a check that passed.

The pre-push hook and continuous integration call the same command, so the three
cannot drift apart.

Continuous integration also runs a Linux leg: `cargo check --workspace` plus the
tests for `ember-sigs` and `ember-platform`. The resolution path carries no
`cfg(windows)` and every Linux backend is a stub that returns `Unsupported`, so
both have to keep compiling on a host with no Windows API.

## Hooks

Two hooks live in `.githooks`: `commit-msg` runs commitlint, and `pre-push` runs
the gate. Install them once per clone:

```sh
bun install
```

Without Bun:

```sh
cargo xtask hooks install
```

Either one points `core.hooksPath` at `.githooks`.

## Commit messages

[Conventional Commits](https://www.conventionalcommits.org), enforced by the
`commit-msg` hook. Run `cargo xtask scopes` for the live scope list, which is
the same list `commitlint.config.js` enforces:

`loader`, `sdk`, `holistic`, `enshrouded`, `sigs`, `platform`, `testkit`,
`xtask`, `deps`, `ci`, `release`.

Scopes are the workspace crate names past the `ember-` prefix, plus three
cross-cutting names. A new top-level crate earns a scope. Omit the scope rather
than invent one.

## Where code goes

Ask what the code describes:

- Keen's engine, true of any game built on Holistic: `ember-holistic`
- The Enshrouded server specifically: `ember-enshrouded`
- The operating system: `ember-platform`
- A mod's own idea: not this repository

## Recovered addresses

Never commit a hardcoded address without the pattern that recovers it. A table
row is a lock on a scan result, not a substitute for the scan.

Addresses resolve in three tiers at run time: a cross-reference from a format
string, a wildcard byte pattern, then the recorded offset. Every tier that runs
has to agree with the recorded offset, because a recorded offset the scan
contradicts means the build moved.

## The first run

[docs/dev.md](docs/dev.md) has it end to end. In short: fetch a dedicated server
into `.cache`, extract the schema from it, then run the gate. The extract step
is required, because nothing recovered from a Keen binary is committed to this
repository.

## What never happens

- No path inside a Steam library is ever written to, launched, or injected into.
  A dedicated server for development is fetched separately into `.cache`.
- No schema dump, string table, protocol registry or other recovered game data
  is committed. The extractors are committed; their output is not. The one
  published exception is the signature table, which holds function addresses and
  the patterns that recover them.
