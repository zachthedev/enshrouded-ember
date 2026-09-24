# Contributing

## Setup

Install the hooks before the first commit:

```sh
bun install
```

`lefthook.yml` holds them: `commit-msg` runs commitlint, and `pre-push` runs
the gate. [lefthook](https://lefthook.dev) installs them into `.git/hooks` when
`bun install` runs the `prepare` script. Each hook runs its tool under Bun by
its path in `node_modules`, so a tool that is not installed fails the commit or
the push rather than letting it through. The commitlint hook first unsets every
spelling of the Bun variables that add flags or run a module, and starts Bun
with `--no-env-file`, so nothing in your environment or an env file reaches
it. The hooks themselves need `node_modules`:
in a fresh clone before `bun install`, or once `node_modules` is gone, no hook
runs and every commit and push goes through unchecked. Continuous integration's
`commits` job and gate are the control that holds either way. If
`git config core.hooksPath` prints a path, unset it first: git ignores
`.git/hooks` while that setting names another directory.

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

The opening row holds `mise.toml`, `mise.semver.toml` and their lockfiles to
their rules before any tool runs, and `cargo xtask pins` runs that row on its own. A lockfile entry
the rules reject installs whatever its url serves, so a rule that ran later
would report a finding about a binary that had already executed. It also
refuses any link, and any other mise configuration or lockfile, at the root or
under `.config`, `.mise` or `mise`. mise would merge such a file and its
lockfile over the pair the rules read, and follow a link to whatever it names.
`mise.toml` holds `[tools]`, `[tool_config]` and `[settings]` alone, and each
tool entry holds its version and tag prefix alone. mise runs hooks, tasks and
postinstall commands from that file, and no rule reads them.

The same row runs the tree rules in `xtask/src/tree.rs`. rustfmt,
cargo-deny, taplo, zizmor, Prettier and commitlint each run with their one
config named, and none of them reads another under that flag. clippy searches
upward from the root, where the committed `clippy.toml` stops it, and a root
`.clippy.toml`, which would win beside it, is refused. A program that
finds its config by name with no flag naming one has every other name refused:
a second lefthook config, an actionlint config, a nested `.cargo/config` or
toolchain file, and a root `.config` directory, which commitlint's cosmiconfig
reads even under `--config`. `tree.rs` lists every name.
Such a file is refused on disk, tracked or not, so a local run agrees with
continuous integration. A personal file, such as an env file, an `.npmrc` or a
`lefthook-local` config, is refused only when tracked, and `.gitignore` lists
it. The rules run again before every later row, since the build and test rows
run repository code.

What a config holds is for a reviewer to judge, and CODEOWNERS sends every
change to one to a code owner. The rules refuse only a key that runs or
redirects code from a file that reads as data:

- `bunfig.toml` holds `[install] minimumReleaseAge`, and nothing else.
- `.prettierrc` is JSON and names no `plugins`, at its top level or in an
  override.
- No `package.json` carries a `cosmiconfig` key, which commitlint's cosmiconfig
  reads even under `--config`.
- `rust-toolchain.toml` names a channel and its components, and nothing else.
- No TypeScript project config sets `paths`, `baseUrl` or `noCheck`, and
  one the gate does not name is refused.

Every JavaScript tool a row or a hook starts runs under Bun by its path in
`node_modules`. A checkout missing the package stops there, where `bunx` would
run a copy found on `PATH` or in its cache. The opening row also refuses a
`package.json` carrying `patchedDependencies`, since a patch changes an
installed package away from the release `bun.lock` pins. No child gets
`BUN_OPTIONS`, which Bun reads into every process as flags, a preload or a test
filter among them, or `BUN_INSPECT_PRELOAD`, `BUN_INSPECT` and
`BUN_INSPECT_CONNECT_TO`, which run a module or open Bun's inspector.

`cargo xtask` runs `--locked`, and so does every cargo row, so a manifest edit
with no relock is refused before the gate starts rather than rewriting
`Cargo.lock`. Every row that walks the tree hands its tool the tracked files it
reads, fails when none was handed, and prints the count. Where the tool names
what it read, as rustfmt, taplo, tsc, actionlint and zizmor do, and as `bun
test`'s junit report does, the row reads that back and fails on a file it
skipped. Prettier is handed exactly the files its own file info keeps.
cargo-machete names each crate directory it visited, and the row fails when it
says it could not read one. A tool whose output a row
reads runs with `NO_COLOR` set, and the row strips any color code before it
reads.

`typecheck` and `tools` cover `tools/`, the repository's own TypeScript. Bun
strips types rather than checking them, so without `typecheck` the gate would
run TypeScript whose types nothing reads. tsc must list every tracked
TypeScript file, so one outside `tools/` fails the row rather than going
unchecked. `bun test` is handed every tracked test file by path, wherever it
sits, and runs with `CI` set, so a committed `test.only` fails.

`actionlint` checks workflow syntax, runner labels and every expression,
including whether a `needs.<job>.outputs.<name>` names an output that job
declares. `build-watch.yml` hands every decision between its jobs through those
outputs, and a misspelled one reads as an empty string rather than an error.

actionlint shells out to an analyzer it finds on `PATH` and says nothing at all
when it does not, and no flag changes that. ShellCheck runs through a stand-in.
`-shellcheck` names this xtask under a hidden subcommand, which reads each
`run:` script as actionlint decoded it. The stand-in refuses any line holding a
ShellCheck directive, and otherwise runs the ShellCheck mise resolved over the
same bytes. actionlint looks the whole value up as one path before it splits
it, so the opening row refuses a root entry named `'`. A directive drops a finding from the report, and YAML escapes and
folding hide one from any reading of the workflow file. The row first runs two
canary workflows. One must come back with `SC2086`, and the other, which carries
a directive, must come back refused. actionlint hands only a bash or sh script
to ShellCheck, so the row also reads every step's `shell:` through Bun's YAML
parser and refuses any shell but bash, sh and pwsh. pyflakes stays off, because
no Windows package manager ships it and leaving it on would have the quiet leg
report a pass for an analysis it never ran.

`zizmor` audits the same files for supply chain and credential problems: an
action not pinned to a commit, a checkout that leaves a credential behind, a
workflow with no `permissions` block, and expression injection through untrusted
context. It is given `.github` with `--collect=all`, which turns off every
ignore file, so a committed ignore line cannot hide a workflow.
`--strict-collection` makes a file it cannot parse fail the step rather than
drop out of the audit. Locally it runs online when `gh auth token` answers, so
the audits that read the GitHub API run before a push, and `--offline`
otherwise. The row's note says which. In CI it always runs offline and holds no
token. Those audits catch an impostor commit, an advisory against a pinned
action and a version comment naming the wrong tag, and they run in CI's shared
`workflows` job on every pull request. `--config` names `.github/zizmor.yml`, which holds the hash-pin
policy, the Dependabot cooldown threshold and every waiver, so the environment
cannot swap it for another. The opening row refuses an inline `zizmor: ignore`
comment under `.github`, so nothing waives an audit outside that file. A second
pass with no config and no ignores fails unless every job passing `secrets:
inherit` calls a workflow under `zachthedev/.github/.github/workflows/`. That
hold is what lets `zizmor.yml` waive the audit by file.

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

A pull request's title takes the type of its most user-facing commit, and `!`
when any commit breaks something users see. A squash lands the title alone,
and release-plz cannot recover a break the title dropped.

A revert is `revert(<scope>): <what is undone, in fresh words>`, with a
`Refs: <sha>` footer naming each reverted commit. git's own
`Revert "<header>"` subject carries no type, so neither the changelog nor the
release decision sees it, and repeating the reverted header overflows 72
columns.

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

- A lint waiver is an `expect` naming the one lint it waives, with a reason
  holding a letter or a digit, so a waiver whose lint stops firing fails the
  build. clippy refuses an `allow` wherever its cfg holds, and
  `cargo xtask pins` refuses one on every platform. It refuses a waiver naming
  a lint group, and `rustfmt::skip` in any form, `rustfmt_skip` and a raw
  identifier included. It also
  refuses any attribute naming `allow_attributes` or
  `allow_attributes_without_reason`, and a crate whose `[lints]` holds anything
  but `workspace = true`. The scan reads each tracked `.rs` file as Rust
  tokens, so it refuses an attribute built from a macro argument, a module file
  set by `#[path]` even under `cfg_attr`, `include!`, and a `use` that
  imports `include` under another name.
- A crate declares only the dependencies its code uses. `cargo xtask pins`
  refuses a cargo-machete ignore list in any `Cargo.toml`.
- A TypeScript file keeps tsc's checking on. `cargo xtask pins` refuses
  `@ts-nocheck`, `@ts-ignore`, an `@ts-expect-error` with no reason, and a
  tracked declaration file, which tsc never checks under `skipLibCheck`.
- Code behind `cfg(windows)` builds on Windows alone and code behind its
  inverse on Linux alone. The resolution path carries no `cfg(windows)`, and
  every Linux backend is a stub that returns `Unsupported`, so both hold on a
  host with no Windows API.
- A module that needs unsafe code takes an `expect(unsafe_code)` with a
  reason on its `mod` line, so the list of those attributes is the list of
  modules that hold any. A module whose unsafe code builds on Windows alone
  takes it as `cfg_attr(windows, expect(...))`, since an expect that goes
  unfulfilled on the Linux leg fails there.
- Every line a command other than the gate prints goes through
  `xtask/src/ui.rs`, so the color policy and the column math live in one place.
  The gate prints to the writer it is handed, so its tests read what it printed.
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

[release-plz](https://release-plz.dev) releases the workspace, configured in
`release-plz.toml`, and `.github/workflows/cd.yml` runs it:

1. Every push to main runs `release-update`, which computes each changed
   crate's next version and changelog entry, then `release-pr`, which applies
   that change and opens or updates one release pull request under the
   zachthedev-releaser app.
2. Merging that pull request, as a squash, is the release. The next run waits
   for the `release` environment's reviewer, then publishes each crate the pull
   request names to crates.io, tags it `<crate>-v<version>`, and drafts a
   GitHub release for ember-loader.
3. The publish job waits for the same reviewer a second time, then flips the
   draft public. Two approvals per release is the cost of creating every
   release as a draft.

release-plz's semver check runs in `release-update` and nowhere else. It
builds rustdoc for each changed library and its last release, which runs every
dependency's build script and proc macro, so that job holds no credential.
Its log carries the verdict, and the release pull request carries none.
`mise.toml` pins release-plz, and `mise.semver.toml` pins cargo-semver-checks,
which mise loads only where `MISE_ENV=semver` and so never beside the
releaser's key.

release-plz owns every version in the manifests and every crate's
`CHANGELOG.md`, beside the crate's `Cargo.toml`. Nobody edits either by hand;
to change what a release says, edit the release pull request before merging
it. A red release pull request is never merged with `--admin`, because the
bypass also skips the required checks.

A release needs a user-facing change in a crate's packaged files: a `feat`,
`fix`, `perf` or `revert` subject, or a breaking change of any type. Every
other type is hidden from the changelog and releases nothing. The workspace
shares one version, so the published crates are one `version_group` and move
together: every release publishes all of them. The commit type sets the
changelog section and the bump size, and below 1.0.0 a `feat` bumps the patch
and a breaking change the minor. The workspace starts at 0.1.0 because nothing
depends on it yet, and `0.x` promises no compatibility.

Every version heading in a changelog links GitHub's compare view from the
previous tag, which lists every change in the release, hidden types included.
`git log --oneline <crate>-v<old>..<crate>-v<new>` lists the same, and
ember-loader's GitHub release carries GitHub's generated notes after its
changelog.

crates.io takes each crate through trusted publishing: the release job's OIDC
token is exchanged for a short-lived publish token, so no registry token is
stored anywhere. crates.io accepts that only for a crate that already exists.
A release pull request naming a crate crates.io has never seen would fail
after the approval, with the crates before it in publish order already out.
So a new crate's first version is published by hand, from the release pull
request's branch and in dependency order, before that pull request merges.
The release job skips a version already on crates.io, so a version published
by hand gets no tag and no GitHub release.

A failed release is recovered by cutting the next version, never by moving a
tag. Only the releaser app can create a tag.

## What never happens

- Nobody hand-edits a version in a `Cargo.toml` or a crate's `CHANGELOG.md`.
  release-plz writes both from the commits, as [Releases](#releases) says, and
  a hand edit is overwritten or shifts the next version it computes.
- No commit message leaves the convention [Commit messages](#commit-messages)
  sets. release-plz computes each version and changelog entry from the
  messages, so a message outside it becomes a wrong entry in a release.
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
