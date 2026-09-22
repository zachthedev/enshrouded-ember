# Contributing

## Setup

Install the hooks before the first commit:

```sh
bun install
```

`lefthook.yml` holds them: `commit-msg` runs commitlint, and `pre-push` runs
the gate. [lefthook](https://lefthook.dev) installs them into `.git/hooks` when
`bun install` runs the `prepare` script. Each hook resolves its tool through
`bunx --no-install`, so a tool that is not installed fails the commit or the
push rather than letting it through. If `git config core.hooksPath` prints a
path, unset it first: git ignores `.git/hooks` while that setting names another
directory.

[docs/dev.md#prerequisites](docs/dev.md#prerequisites) lists what to install
and the file that pins each version.

## The gate

One command, and the only one:

```sh
cargo xtask check
```

It runs its rows in order and stops at the first failure. The rows, and what
each one covers, are printed by the same table the gate runs:

```sh
cargo xtask check --rows
```

The opening row holds `mise.toml` and `mise.lock` to their rules before any
tool runs, and `cargo xtask pins` runs that row on its own. A lockfile entry
the rules reject installs whatever its url serves, so a rule that ran later
would report a finding about a binary that had already executed.

`cargo xtask` runs `--locked`, and so does every cargo row, so a manifest edit
with no relock is refused before the gate starts rather than rewriting
`Cargo.lock`. `taplo` reads `.taplo.toml` for the files it covers.

`typecheck` and `tools` cover `tools/`, the repository's own TypeScript. Bun
strips types rather than checking them, so without `typecheck` the gate would
run TypeScript whose types nothing reads.

`actionlint` checks workflow syntax, runner labels and every expression,
including whether a `needs.<job>.outputs.<name>` names an output that job
declares. `build-watch.yml` hands every decision between its jobs through those
outputs, and a misspelled one reads as an empty string rather than an error.

Its shellcheck pass is on, and `mise.toml` holds the version every host
installs. actionlint shells out to an analyzer it finds on `PATH` and says
nothing at all when it does not, and no flag changes that, so the gate hands it
the path mise resolved and first runs it over a canary workflow with one
unquoted expansion. The row is refused unless that run reports `SC2086`. Both
matrix legs then read the shell in a `run:` block the same way. pyflakes stays
off, because no Windows package manager ships it and leaving it on would have
the quiet leg report a pass for an analysis it never ran.

`zizmor` audits the same files for supply chain and credential problems: an
action not pinned to a commit, a checkout that leaves a credential behind, a
workflow with no `permissions` block, and expression injection through untrusted
context. `--strict-collection` makes a file it cannot parse fail the step rather
than drop out of the audit. It runs online when `gh auth token` answers, so the
audits that read the GitHub API run, and `--offline` otherwise; the row's note
says which. `--config` names `.github/zizmor.yml`, which holds the hash-pin
policy and the Dependabot cooldown threshold, so the environment cannot swap it
for another. The inline markers in the workflows answer the findings this
repository accepts.

`doctests` runs beside `tests`, because `cargo nextest` runs none of them and a
doctest that stops compiling would otherwise pass the gate in silence. `doc`
builds every crate's documentation with warnings denied, so a broken link is a
failure.

A missing tool stops the gate and names itself, because a check that did not
run is not a check that passed.

The pre-push hook and continuous integration call the same command, so neither
can run a different gate. Continuous integration adds what one machine cannot
check:

- It runs the gate on Windows and on Linux, because the pre-push hook runs it
  on whichever host a contributor uses. Code behind `cfg(windows)` builds only
  on the first host, and code behind its inverse only on the second, so both
  have to hold. The Linux leg compiles `zstd-sys`, which is C, and the ubuntu
  runner image ships a C toolchain for it.
- `commits` runs commitlint over every commit in a pull request, because the
  `commit-msg` hook checks one commit on one machine and a rebase or a
  `--no-verify` reaches the branch unchecked.

## Commit messages

[Conventional Commits](https://www.conventionalcommits.org), enforced by the
`commit-msg` hook and by the `commits` job. `.github/commit-scopes.json` holds
the scope list, one sentence per scope saying what it covers.
`cargo xtask scopes` prints it, and `commitlint.config.js` enforces it.

Scopes are the workspace crate names past the `ember-` prefix, plus
cross-cutting names no crate owns. A new top-level crate earns a scope. Omit the
scope rather than invent one.

The header and every body line stop at 72 columns; commitlint refuses longer.
The subject is imperative and lowercase with no trailing period.

A body says what was wrong, what the change does now, and what a reader needs
that the diff cannot show, such as what was deliberately not done. Past tense
belongs here and nowhere else: a code comment describes the code as it is, and
the commit message carries the history.

## Where code goes

Ask what the code describes:

- Keen's engine, true of any game built on Holistic: `ember-holistic`
- The Enshrouded server specifically: `ember-enshrouded`
- The operating system: `ember-platform`
- A mod's own idea: not this repository, but the mod's own, such as
  [enshrouded-mods](https://github.com/zachthedev/enshrouded-mods)

`cargo xtask crates` prints every crate and what it holds, read from each
crate's own manifest.

## Tests

- A test that needs a real build is `#[ignore]` and reads the build's path from
  the environment, skipping cleanly when the variable is unset. Every other test
  stands on a synthetic image built in memory, so the suite runs on a machine
  with no server at all.
  [docs/dev.md#tests-that-need-a-real-thing](docs/dev.md#tests-that-need-a-real-thing)
  names the variables and says how to run them.
- A test never relies on catching a panic from a hook body. Cargo ignores
  `panic = "abort"` for the test profile, so a hook that unwinds under test
  aborts the shipped library.
- A case states its expectation as a literal, never as a value computed from
  the subject under test. An expectation derived from the subject can only
  restate it.

## Code

- Code behind `cfg(windows)` builds on Windows alone and code behind its
  inverse on Linux alone. The resolution path carries no `cfg(windows)`, and
  every Linux backend is a stub that returns `Unsupported`, so both hold on a
  host with no Windows API.
- A module that needs unsafe code takes an `allow(unsafe_code)` with a reason
  on its `mod` line, so the list of those attributes is the list of modules
  that hold any.
- Every line a command prints goes through `xtask/src/ui.rs`, so the color
  policy and the column math live in one place.
- A comment explains a constraint the reader can verify today. What was wrong
  before, and why an earlier approach failed, goes in the commit message.

## Dependencies

`bunfig.toml` sets the install cooldown for Bun and travels with the clone, so
a container run with no user-level configuration sees the same gate. Cargo has
no cooldown file of its own: every version bump comes through Renovate, and the
preset `.github/renovate.json` extends holds the cooldown. `renovate.json` adds
what is true of this repository alone, and says why beside each entry.

The gate's `deny` row runs `cargo deny check licenses bans sources`.
Advisories are not a row, because an advisory published overnight would turn
a change red that touched nothing. Two legs read the lockfile for them, and
they fail in opposite directions: Dependabot alerts read GitHub's database on
every push, which lacks part of RustSec, and `.github/workflows/audit.yml`
runs `cargo deny check advisories` against RustSec weekly, reading the
`[advisories]` table in `deny.toml`. A red audit run is a report, never a
check, and no ruleset requires it.

An advisory is fixed by the tool that sees the crate. A crate `Cargo.toml`
names gets Renovate's security pull request, which skips the schedule and the
cooldown. A transitive crate gets Dependabot's, a lockfile-only bump inside the
parent's range; `.github/dependabot.yml` opens that kind of pull request and no
other. When no fixed release satisfies the requirement, bump the direct
dependency that pulls it in.

## Releases

None.

## What never happens

- No path inside a Steam library is ever written to, launched, or injected into.
  A dedicated server for development is fetched separately into `.cache`.
  `.claude/settings.json` carries two `deny` entries that refuse an agent an
  edit under a Steam library, because a rule read is a rule that can be
  forgotten and a deny cannot.
- No schema dump, string table, protocol registry or other recovered game data
  is committed. The extractors are committed; their output is not. The one
  published exception is the signature table, which holds function addresses and
  the patterns that recover them.
- No hardcoded address is committed without the pattern that recovers it. A
  table row is a lock on a scan result, not a substitute for the scan.
  Addresses resolve in tiers at run time: a cross-reference from a format
  string, a wildcard byte pattern, then the recorded offset. Every tier that
  runs has to agree with the recorded offset, because a recorded offset the scan
  contradicts means the build moved.
