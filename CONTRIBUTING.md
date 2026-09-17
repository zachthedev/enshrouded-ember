# Contributing

## Toolchain

`rust-toolchain.toml` pins Rust 1.98.1. Rustup installs it on the first cargo
command. A C toolchain is needed for the MinHook engine.

## The gate

Every one of these must pass before a commit is pushed:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
```

## Commit messages

Conventional Commits, enforced by a `commit-msg` hook running commitlint.
Install the hook once per clone:

```sh
bun install
```

Scopes are the workspace crate names past the `ember-` prefix, plus three
cross-cutting names. Run `cargo xtask scopes` for the live list.

## Scopes

`loader`, `sdk`, `holistic`, `enshrouded`, `sigs`, `platform`, `testkit`,
`xtask`, `deps`, `ci`, `release`.

A new top-level crate earns a scope. Omit the scope rather than invent one.

## Where code goes

Ask what the code describes:

- Keen's engine, true of any game built on Holistic: `ember-holistic`
- The Enshrouded server specifically: `ember-enshrouded`
- The operating system: `ember-platform`
- A mod's own idea: not this repository

## Recovered addresses

Never commit a hardcoded address without the pattern that recovers it. A table
row is a lock on a scan result, not a substitute for the scan.
