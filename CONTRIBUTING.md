# Contributing

## Toolchain

`rust-toolchain.toml` pins the Rust release, and rustup installs it on the first
cargo command. A C toolchain is needed for MinHook, the hook engine.
[Bun](https://bun.sh) runs the repository's own tooling, at the release
`.bun-version` pins. Continuous integration reads the same file.

The gate calls tools that rustup does not install. `.github/cargo-tools` pins
the crates.io packages among them, and `.github/go-tools` pins the Go programs,
which need [Go](https://go.dev) to install. The versions live in those files and
nowhere else, so this installs what continuous integration installs:

```powershell
cargo install --locked @(Get-Content .github/cargo-tools | Where-Object { $_ -notmatch '^\s*#' -and $_.Trim() })
Get-Content .github/go-tools | Where-Object { $_ -notmatch '^\s*#' -and $_.Trim() } | ForEach-Object { go install $_ }
```

On a shell without PowerShell:

```sh
cargo install --locked $(grep -v '^#' .github/cargo-tools | grep .)
grep -v '^#' .github/go-tools | grep . | xargs -n 1 go install
```

## The gate

One command, and the only one:

```sh
cargo xtask check
```

It runs, in order and stopping at the first failure:

| Step         | What it checks                                           |
| ------------ | -------------------------------------------------------- |
| `fmt`        | Rust formatting                                          |
| `taplo`      | TOML formatting                                          |
| `clippy`     | Lints on every target, with warnings denied              |
| `tests`      | The workspace's tests, through `cargo nextest`           |
| `doctests`   | Every documented example                                 |
| `deny`       | Advisories, licenses, bans and sources                   |
| `machete`    | Dependencies a crate declares and never uses             |
| `audit`      | The lockfile against the RustSec advisory database       |
| `prettier`   | Markup, JavaScript and TypeScript formatting             |
| `typecheck`  | The types in `tools/`                                    |
| `tools`      | The tests in `tools/`                                    |
| `actionlint` | Workflow syntax, runner labels and expressions           |
| `zizmor`     | Workflow pinning, credentials, permissions and injection |

`taplo` reads `.taplo.toml` for the files it covers.

`typecheck` and `tools` cover `tools/`, the repository's own TypeScript. Bun
strips types rather than checking them, so without `typecheck` the gate would
run TypeScript whose types nothing reads.

`actionlint` checks workflow syntax, runner labels and every expression,
including whether a `needs.<job>.outputs.<name>` names an output that job
declares. `build-watch.yml` hands every decision between its jobs through those
outputs, and a misspelled one reads as an empty string rather than an error. Its
external analyzers, shellcheck and pyflakes, are off: actionlint runs them when
it finds them on `PATH` and says nothing when it does not, and `ubuntu-latest`
carries shellcheck while `windows-latest` does not, so leaving them on would
have the matrix legs check different things and the quiet leg report a pass for
an analysis it never ran.

`zizmor` audits the same files for supply chain and credential problems: an
action not pinned to a commit, a checkout that leaves a credential behind, a
workflow with no `permissions` block, and expression injection through untrusted
context. `--strict-collection` makes a file it cannot parse fail the step rather
than drop out of the audit. `--offline` keeps it from needing a GitHub token, so
a runner and a laptop get the same findings. `--config` names
`.github/zizmor.yml`, which holds the Dependabot cooldown threshold, so the
environment cannot swap it for another. The inline markers in `build-watch.yml`
answer the findings this repository accepts, and a test allowlists each one.

`doctests` runs whether or not `cargo-nextest` is installed, because
`cargo nextest` runs none of them and a doctest that stops compiling would
otherwise pass the gate in silence.

`tests` falls back to `cargo test --workspace` when `cargo-nextest` is absent,
and the summary says which runner ran. Any other missing tool stops the gate and
names itself, because a check that did not run is not a check that passed.

The pre-push hook and continuous integration call the same command, so neither
can run a different gate.

Continuous integration runs the same gate on Windows and on Linux, because the
pre-push hook runs it on whichever host a contributor uses. Code behind
`cfg(windows)` builds only on the first host, and code behind its inverse only
on the second. The resolution path carries no `cfg(windows)`. Every Linux
backend is a stub that returns `Unsupported`. Both have to hold on a host with
no Windows API.

The Linux leg compiles `zstd-sys`, which is C. The ubuntu runner image ships a C
toolchain, so the leg installs nothing for it.

## Hooks

`.githooks` holds the hooks: `commit-msg` runs commitlint, and `pre-push` runs
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
`commit-msg` hook. `.github/commit-scopes.json` holds the scope list.
`cargo xtask scopes` prints it, and `commitlint.config.js` enforces it:

`loader`, `sdk`, `holistic`, `kfc`, `enshrouded`, `sigs`, `platform`, `testkit`,
`xtask`, `deps`, `ci`, `release`.

Scopes are the workspace crate names past the `ember-` prefix, plus
cross-cutting names no crate owns. A new top-level crate earns a scope. Omit the
scope rather than invent one.

## Where code goes

Ask what the code describes:

- Keen's engine, true of any game built on Holistic: `ember-holistic`
- The Enshrouded server specifically: `ember-enshrouded`
- The operating system: `ember-platform`
- A mod's own idea: not this repository, but the mod's own, such as
  [enshrouded-mods](https://github.com/zachthedev/enshrouded-mods)

## Recovered addresses

Never commit a hardcoded address without the pattern that recovers it. A table
row is a lock on a scan result, not a substitute for the scan.

Addresses resolve in tiers at run time: a cross-reference from a format string,
a wildcard byte pattern, then the recorded offset. Every tier that runs has to
agree with the recorded offset, because a recorded offset the scan contradicts
means the build moved.

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
