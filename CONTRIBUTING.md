# Contributing

## Setup

### Prerequisites

- Rust, at the release `rust-toolchain.toml` pins. rustup installs it on the
  first cargo command.
- A C toolchain, for MinHook, the hook engine. On Windows that is Visual Studio
  Build Tools.
- [Bun](https://bun.sh), for the repository's own tooling, at the release
  `packageManager` in `package.json` pins. Continuous integration reads the
  same field.
- [mise](https://mise.jdx.dev), for every gate tool that rustup, cargo and bun
  do not provide. `mise.toml` pins a version per tool and `mise.lock` records
  a checksum per platform, so an install takes the recorded artifact or fails.
  `mise.semver.toml` and `mise.semver.lock` are the same pair for
  cargo-semver-checks alone, which only the release workflow loads.

Install mise, then:

```sh
mise install
```

Nothing from that lands on `PATH`. The gate asks `mise which` for each binary
and runs the path it gives back, so the binary it checked is the binary it ran.
Turning on `mise activate` in a shell puts the same binaries on `PATH` under
their own names, which is what makes `cargo nextest run` and its siblings work
at a prompt. The split is deliberate: a check runs the binary it resolved, and a
person gets the convenience. [Dependencies](#dependencies) says how each
download is held to its pin.

### First run

From a fresh clone to a green gate:

```sh
git clone https://github.com/zachthedev/enshrouded-ember.git
cd enshrouded-ember
mise install                  # the gate's tools, at the releases mise.lock records
bun install --frozen-lockfile # the hooks and the markup formatter
cargo xtask server fetch      # a dedicated server, into .cache
cargo xtask schema extract    # the reflection schema, out of that server
cargo xtask check             # the gate
```

The fetch pulls a dedicated server from SteamCMD into `.cache`, which is
gitignored. Anonymous login works for app 2278520, so no credentials are
involved. The server runs to gigabytes. Fetch it once.

The extraction reads the fetched server's executable and writes every dump
`OUTPUTS` in `xtask/src/schema/mod.rs` names, plus `build.json`, into
`.cache/schema/<buildid>/`. The build id names the directory. It is not what the
loader matches at run time: Ember identifies a build by the CodeView fingerprint
in the image itself, because the build id is not readable from the running
process.

**The extraction is required.** Nothing recovered from a Keen binary is
committed to this repository: no schema dump, no string table, no protocol
registry, no game data. The extractors are committed and every contributor runs
them against a server they fetched themselves. Anything the extractor produces
is derived data, lives under `.cache`, and is regenerated rather than shared.
The loop is fetch, extract, check.

### Hooks

Install the packages and the hooks before the first commit:

```sh
bun install --frozen-lockfile
```

In a linked worktree, run `bun install --frozen-lockfile --ignore-scripts`
instead. The hooks sit in the `.git/hooks` every worktree shares and name the
installing checkout's `node_modules`, so a worktree's install skips the
`prepare` script that rewrites them.

`lefthook.yml` holds them: `commit-msg` runs commitlint, and `pre-push` runs
the gate. [lefthook](https://lefthook.dev) installs them into `.git/hooks` when
`bun install` runs the `prepare` script. Each hook starts its tool through
`bunx --bun --no-install`, which runs the copy in `node_modules`, or one from a
parent directory or `PATH` when the install is missing. If
`git config core.hooksPath` prints a path, unset it first: git ignores
`.git/hooks` while that setting names another directory.

The hooks run in your own environment, and [Safety](#safety) says which of your
settings reach them and why they are no control.

## Safety

Read a pull request's diff before running anything on its branch. The branch
supplies the install, the hooks and the gate, so `bun install`, a commit and
`cargo xtask check` each run code the branch chose, build scripts, procedural
macros and the scripts under `tools/` included.

Some of your own environment reaches the tools:

- `BUN_OPTIONS` reaches every Bun you start directly: `bun install`, the
  lefthook install its `prepare` script starts through Bun, and
  `bun run archive` or `bun run watch-builds` run by hand. A `--preload` in it
  runs first in each. The gate withholds it from every process it starts.
  `BUN_INSPECT`, `BUN_INSPECT_CONNECT_TO` and `BUN_INSPECT_PRELOAD` open an
  inspector or run a module in any Bun that sees them, the gate's own
  included. Leave all four unset.
- A personal env file reaches the JavaScript tools. bunx never passes
  `--no-env-file` to the tool it starts, so Prettier and tsc in the gate and
  commitlint in the hooks load an env file at the checkout's root. Every Bun
  the gate starts itself carries the flag.
- The gate withholds from every child the variables that change a result:
  `BUN_OPTIONS`, `SHELLCHECK_OPTS`, the rustdoc flag variables and every
  `NEXTEST_` variable. [The gate](#the-gate) says what each would change.

Hooks are not a control. They run in your shell's environment and clear no
variable. In a fresh clone before `bun install`, or once `node_modules` is
gone, no hook runs and every commit and push goes through unchecked.
Continuous integration's `commits` job and gate decide a merge either way.

Nothing here is ever pointed at a Steam library, as
[What never happens](#what-never-happens) says.

## Running it

The fetched dedicated server is what Ember runs against, never an installed
copy of the game:

```sh
cargo xtask server seed --fixture <path>   # lay a fixture world into a run directory
cargo xtask server run --inject <dll>      # start it, with the loader injected
cargo xtask server logs --follow           # tail it
cargo xtask server stop                    # ask it to shut down, and wait
```

A build lands in a directory named for its Steam build id, which is the key
Steam, SteamCMD and the depot manifest all speak:

```text
.cache/
  steamcmd/            SteamCMD itself
  server/<buildid>/    The fetched server
  schema/<buildid>/    What the extractor reads out of it
  loca/<buildid>/      A client's localization tables
```

Every byte under `.cache` is regenerated by a fetch or an extract, which is why
none of it is committed.

**Never point any of this at a Steam library.** A path under `steamapps/common`
is refused. Your installed copy of the game is not a server, Steam overwrites
its own files, and nothing here ever writes to, launches, or injects into one.

A command that acts on one build takes `--build <buildid>` and otherwise picks
the only one there is. `--root <path>` moves `.cache` somewhere else, which is
how a mod repository keeps its fetched server under its own tree.
`EMBER_DEV_ROOT` sets the same directory for every command, and `--root`
overrides it. `cargo xtask --help` lists every command, and each one takes
`--help` for its own flags.

### The server and schema commands

```sh
cargo xtask server fetch          # pull a dedicated server from SteamCMD
cargo xtask server seed           # lay a fixture world into a run directory
cargo xtask server run            # start it, optionally injecting the loader
cargo xtask server logs           # tail it
cargo xtask server stop           # ask it to shut down, and wait
cargo xtask schema extract        # read the schema out of a fetched build
cargo xtask schema list           # list the extractions under .cache/schema
cargo xtask schema diff old new   # compare two extractions by buildid
cargo xtask loca extract          # write a client build's localization tables
```

### Generated files

| File                       | Regenerated by                                                |
| -------------------------- | ------------------------------------------------------------- |
| `Cargo.lock`               | any cargo build after a manifest edit; the gate runs locked   |
| `bun.lock`                 | `bun install` after a `package.json` edit                     |
| `mise.lock`                | `mise lock` after a `mise.toml` edit                          |
| `mise.semver.lock`         | `MISE_ENV=semver mise lock` after a `mise.semver.toml` edit   |
| `data/steam-builds.jsonl`  | the build watcher workflows, one appended row per change      |
| `data/build-digests.jsonl` | `bun run archive emit` or `archive record`, one row per build |

Nobody hand-edits a row of either record. `mise.lock` keeps the hand-computed
taplo hashes across a relock at the same version, as
[Dependencies](#dependencies) says.

### The localization tables

```sh
cargo xtask loca extract --client "<steam library>\steamapps\common\Enshrouded\enshrouded.exe"
```

This writes one tab-separated table per language into `.cache/loca/<buildid>/`,
with the tag id, the two argument counts and the text. A mod that shows a line
names a `LocaTagId`, so a developer reads the text here to find the id and
commits the id alone.

Only a client ships a localization table. The dedicated server carries none, so
this step needs an installed client rather than a fetched server. The build id
naming the directory is the client's, read from its own Steam app manifest, and
it is a different series from the dedicated server's. Reading the client's
bytes is the one thing done with an installed copy of the game.

The same rule applies as to every other extraction: Keen's written text is
derived data, stays under `.cache`, and is never committed.

### The signature diff

Keen ships no symbols, so every address Ember hooks has to be found again in
each new build. `cargo xtask sig` drives Ghidra, BinExport and BinDiff over a
pair of builds and prints where a known address landed in the newer one.

```sh
cargo xtask sig analyze --build <id>   # import, analyze and seed a build
cargo xtask sig export --build <id>    # write the BinExport file
cargo xtask sig diff <old> <new>       # diff a pair of exports
cargo xtask sig read <old> <new> --address 0x1401aba90
```

Neither tool is pinned here and no runner carries one, so each stage reads its
installation out of the environment. `EMBER_GHIDRA_HOME` names a Ghidra
installation and `EMBER_BINDIFF_HOME` names a BinDiff one, and a stage that
finds neither names the variable it wanted. BinExport's Ghidra extension is
built against one Ghidra release and Ghidra refuses any other, so those versions
have to agree, and BinExport's protobuf runtime has to win on Ghidra's
classpath.

`analyze` seeds functions before it saves, and that is what makes the rest work
on an engine entry. A program record's entry function is referenced only from
the record in `.rdata`, nothing calls it, and auto-analysis never reaches it, so
the address is absent from the export and the differ can never match it. The
entries come from the same walk `schema extract` prints, and `--address` adds
one no record names.

`old` names the build whose addresses are known and `new` the build whose
addresses are wanted, so a proposal for the new build is the second address in
each row `read` prints. An address with no proposal prints as a miss and fails
the command, because an address the export never carried looks exactly like one
the differ could not place.

**Every proposal is a candidate.** The differ matches call graphs, and a row
earns a place in a signature table only once the binary itself agrees: the same
call graph shape around it, and a string anchor where one exists. Similarity
and confidence describe the match and the algorithm behind it, and neither is
that confirmation.

The cost is why this runs on a developer's machine rather than on a runner.
Analysis of the server image takes a quarter of an hour, the export takes
minutes, and a project, its export and a pair's result database take hundreds of
megabytes. Every stage takes `--timeout <seconds>` and stops the tool at it.
Everything lands under `.cache` and is regenerated.

### The build watcher and the archive

`tools/` holds the Bun scripts behind the build watcher and the archive.
`build-watch.yml` watches the dedicated server on the schedule its `cron` line
sets, then has the archive client ask the bucket whether the build the public
branch names is archived, and archives it when it is not.

`watch-builds.ts` reads the output of SteamCMD `app_info_print` and appends a
row to `data/steam-builds.jsonl` when a branch build id or a depot manifest gid
moves. Valve serves that output directly, so the record depends on no scraper.
The project owns its build history from the first run, because none of the
keyless routes to Steam carries any history at all.

`client-build-watch.yml` runs the same script against the Enshrouded client on
a schedule of its own and appends to the same record. It is a workflow rather
than another job because GitHub titles a failed run after the workflow and
names nothing inside it, so a client read that fails says which application it
was in the title. Nothing downstream reads the client rows: no depot of the
client's is archived, and `loca extract` takes the client build id from an
installed client rather than from the record.

Both watchers push that one file, and a rebase replays an append onto a last
line the other just wrote, which conflicts. The two jobs therefore share one
concurrency group, named for the record rather than for either workflow, and a
group name is repository-scoped so one name serializes jobs across workflows.
The two schedules also sit a stated distance apart, which keeps them from
contending for the group at all, because a run held for a group can be
canceled by the next one arriving. Cases hold both.

`archive.ts` moves a build in and out of the R2 archive. Steam serves a depot
manifest only while it is current, so a build that is not archived inside the
polling window needs an authenticated pull by hand to recover. The archive is
the primary copy, not a convenience.

```sh
bun run archive status --manifest <gid>
bun run archive verify --dir <path> --manifest <gid>
bun run archive pull --manifest <gid> --out <path>
bun run archive push --dir <path> --manifest <gid>
bun run archive previous --manifest <gid>
```

`data/build-digests.jsonl` is what each recorded build's files hash to, and
every pull checks what arrived against it before the bytes take their final
name. R2 has no object versioning and no Object Lock, so an overwrite is final
and that record is the only thing that would notice one. `push` hashes an object
already at a key before it calls the build archived, and refuses to write over
one that holds anything else.

A row pins bytes and is not a receipt for an upload, so it says nothing about
whether the bucket holds a build. `archive status` asks the bucket with the read
token. A build is archived when it has a row and every file the row names is in
the bucket at the recorded size. A missing row or a missing object sends the
archive job back to work, so a failure anywhere in the archive path is tried
again on the next run rather than lost. An object at a size the row does not
state fails the check instead, because `push` refuses to write over it and a
person has to look. The job that asks draws its read token from an environment
that carries no reviewer, so a scheduled run never waits on an approval, and the
archive job asks for one only when there is work.

`status` then sweeps every other recorded build and fails the run when one of
them is not in the archive. The archive job fetches the head of the branch and
can repair that build alone, so the rest are the ones whose loss is absolute.
The sweep runs after the answer is handed on, so a historical build that has
gone missing never holds back the one build that can still be archived.

The archive job fetches a build with SteamCMD, the same tool and the same pinned
container the watcher already uses for app info, narrowed to the Keen files the
archive keeps with `sDepotDownloadFileFilter`. That costs megabytes over the
wire rather than the multi-gigabyte depot. A Linux SteamCMD selects depots by
client platform and this depot declares `oslist windows`, so the fetch forces
the platform; without that it writes nothing and reports
`Missing configuration`.

`app_update` takes no manifest parameter and always fetches the head of the
branch, so `archive emit` reads the manifest gid out of the app manifest the
fetch leaves beside the payload, and refuses a row that would key those bytes
under a different gid. A build that already has a row keeps it: `emit` checks
the fetched bytes against that row and hands on the row unchanged.

Both fetching routes leave that evidence, SteamCMD in `steamapps/` and
DepotDownloader in `.DepotDownloader/`, so a directory carrying neither is
refused rather than recorded. `--unattested` records it anyway, on the word of
whoever typed the gid, and prints that in the output. No workflow passes it,
which a case enforces.

`revision` and `branch` come from the first 512 bytes of
`enshrouded_server.kfc`, where the container's version line names the content
revision and the Subversion branch the build was cut from. A supported-build row
carries that revision so a person reading it knows which build it describes, and
no Steam field carries it. The loader matches on the image fingerprint instead,
as the `ember-sigs` crate documents. `--revision` and `--branch` still win, for
a container this reader cannot parse.

**A historical manifest needs DepotDownloader, run by hand under a real Steam
account.** `app_update` cannot ask for one and an anonymous session is refused
one, so a build missed while its manifest was current is recovered that way
rather than by continuous integration. That is how the backfilled builds
arrived, and `archive record` reads DepotDownloader's provenance too. A build
recovered by hand reaches the bucket by hand too: `archive record` for its row,
then `archive push` with the write token in the environment.

`archive.json` at the repository root names the account and the bucket those
commands reach. A fork points the pipeline at its own bucket by editing that
file and nothing else. It is committed rather than kept in a secret, so a change
to the destination shows up in a diff and a reviewer sees it, and it is the only
place either value is written down: no workflow restates it, and a case refuses
one that starts to. Nothing reads the destination from the environment, which a
case proves by running a process with every plausible override name set and
checking where a request would still have gone.

Every command that reaches R2 reads its credential from the environment and
names the variables that are missing when it is absent: `status` and `pull` the
read token, `push` the write token. A pull without one creates nothing.

`schema-diff` is the job that answers the question a new build raises. It pulls
the new build and the build it follows out of the archive, recovers a schema
from each, and posts `cargo xtask schema diff` to the build issue with watchlist
hits first. `archive previous` names the build it follows. Builds are ordered by
the revision in each one's own container header rather than by the order rows
were recorded, because the record is append-only and a backfill appends in
whatever order the builds were recovered. The new build's row is not committed
while this job runs, so the row is handed over as a string and the record
supplies the candidates.

That job holds the read token and nothing else, and its only write is a comment
on the issue the watcher opened. It is gated on the archive job, which runs once
per build, so a failure in it is not retried by the next scheduled run. Rerun it
from the Actions tab instead.

### Building a mod against this checkout

To build a mod in another checkout against this one, put a `[patch.crates-io]`
block in a `.cargo/config.toml` in a directory **above** both. Cargo merges
config from every parent directory, so nothing in either repository has to
change.

## Where code goes

Ask what the code describes:

- Keen's engine, true of any game built on Holistic: `ember-holistic`
- The Enshrouded server specifically: `ember-enshrouded`
- The operating system: `ember-platform`
- A mod's own idea: not this repository, but the mod's own, such as
  [enshrouded-mods](https://github.com/zachthedev/enshrouded-mods)

`cargo xtask crates` prints every crate and what it holds, read from each
crate's own manifest.

## Code

- A lint waiver is an `expect` naming the one lint it waives, with a reason
  holding a letter or a digit, so a waiver whose lint stops firing fails the
  build. clippy refuses an `allow` wherever its cfg holds, and
  `cargo xtask pins` refuses one on every platform. It refuses a waiver naming
  a lint group, and `rustfmt::skip` in any form, `rustfmt_skip` and a raw
  identifier included. It also refuses any attribute naming `allow_attributes`
  or `allow_attributes_without_reason`, and a crate whose `[lints]` holds
  anything but `workspace = true`. The scan reads each tracked `.rs` file as
  Rust tokens, so it refuses an attribute built from a macro argument, a module
  file set by `#[path]` even under `cfg_attr`, `include!`, and a `use` that
  imports `include` under another name.
- A crate declares only the dependencies its code uses. `cargo xtask pins`
  refuses a cargo-machete ignore list in any `Cargo.toml`.
- A TypeScript file keeps tsc's checking on. `cargo xtask pins` refuses
  `@ts-nocheck`, `@ts-ignore` and an `@ts-expect-error` with no reason.
- Code behind `cfg(windows)` builds on Windows alone and code behind its
  inverse elsewhere. The resolution path carries no `cfg(windows)`, and every
  other backend is a stub that returns `Unsupported`, so both hold on a host
  with no Windows API.
- A module that needs unsafe code takes an `expect(unsafe_code)` with a
  reason on its `mod` line, so the list of those attributes is the list of
  modules that hold any. A module whose unsafe code builds on Windows alone
  takes it as `cfg_attr(windows, expect(...))`, since an expect that goes
  unfulfilled on another leg fails there.
- Every line a command other than the gate prints goes through
  `xtask/src/ui.rs`, so the color policy and the column math live in one place.
  The gate prints to the writer it is handed, so its tests read what it printed.
- A comment explains a constraint the reader can verify today. What was wrong
  before, and why an earlier approach failed, goes in the commit message.

## Tests

- A test that needs a real build is `#[ignore]` and finds the build through the
  environment. `EMBER_SERVER_EXE` names a server executable,
  `EMBER_CLIENT_EXE` a client executable, and `EMBER_SCHEMA` a directory of
  dumps to compare against. Each test skips cleanly when its variable is
  unset, and `cargo test -- --ignored` runs them. Every other test stands on a
  synthetic image built in memory, so the suite runs on a machine with no
  server at all.
- A test never relies on catching a panic from a hook body. Cargo ignores
  `panic = "abort"` for the test profile, so a hook that unwinds under test
  aborts the shipped library.
- A case states its expectation as a literal, never as a value computed from
  the subject under test. An expectation derived from the subject can only
  restate it.
- The archive tests serve a fake bucket on loopback, so they reach no network.

## The gate

One command, and the only one:

```sh
cargo xtask check
```

It runs its rows in order and stops at the first failure. The rows that read
files run before the rows that build and run repository code. The rows, and what
each one covers, are printed by the same table the gate runs:

```sh
cargo xtask check --rows
```

The opening row holds `mise.toml`, `mise.semver.toml` and their lockfiles to
their rules before any tool runs, and `cargo xtask pins` runs that row on its
own. A lockfile entry the rules reject installs whatever its url serves, so a
rule that ran later would report a finding about a binary that had already
executed. It also refuses any link, and any other mise configuration or
lockfile, at the root or under `.config`, `.mise` or `mise`. mise would merge
such a file and its lockfile over the pair the rules read, and follow a link to
whatever it names. `mise.toml` holds `[tools]`, `[tool_config]` and
`[settings]` alone, and each tool entry holds its version and tag prefix alone.
mise runs hooks, tasks and postinstall commands from that file, and no rule
reads them.

The same row runs the tree rules in `xtask/src/tree.rs`. rustfmt,
cargo-deny, taplo, zizmor, Prettier and commitlint each run with their one
config named, and none of them reads another under that flag. clippy searches
upward from the root, where the committed `clippy.toml` stops it, and a root
`.clippy.toml`, which would win beside it, is refused. A program that
finds its config by name with no flag naming one has every other name refused:
a second lefthook config, an actionlint config, a nested `.cargo/config` or
toolchain file, and a root `.config` directory or `package.yaml`, which
commitlint's cosmiconfig reads even under `--config`. `tree.rs` lists every
name. Such a file is refused on disk, tracked or not, so a local run agrees
with continuous integration. A personal file, such as an env file Bun loads or
a `lefthook-local` config, is refused only when tracked, and `.gitignore` lists
it. The rules run again before every later row, since the build and test rows
run repository code. They read the tree through git, and refuse to when the
work tree git names is not the root: a `.git` that holds no repository sends
git to the repository above, and a `core.worktree` setting sends it elsewhere.

What a config holds is for a reviewer to judge, and CODEOWNERS sends every
change to one to a code owner. The rules refuse only a key that runs or
redirects code from a file that reads as data:

- `.cargo/config.toml` holds the `xtask` alias, and nothing else.
- No `package.json` carries a `cosmiconfig` key, which commitlint's cosmiconfig
  reads even under `--config`.
- `rust-toolchain.toml` names a channel and its components, and nothing else.
- No TypeScript project config sets `noCheck`, and one the gate does not name
  is refused.
- No tracked `package.json` carries `patchedDependencies`, since `bun install`
  rewrites each package it names with a patch. The shared `commits` job refuses
  it too, but its check passes a file its `jq` cannot parse.
- No tracked `package.json` or named TypeScript project config repeats a key
  within one object, since Bun keeps the first and a JSON parser the last.

The shared `commits` job refuses the data files that run code before a merge,
reading the committed tree, and the gate does not repeat them: a `bunfig.toml`
key beyond `[install] minimumReleaseAge`, and TypeScript `paths` or `baseUrl`.

Every JavaScript tool a row or a hook starts runs through
`bunx --bun --no-install`, which fetches nothing. bunx runs a copy from a
parent directory or `PATH` when the checkout holds none, so a row first checks
that `node_modules/.bin` holds its tool as a regular file. Every Bun the gate
starts itself, a script it evaluates or `bun test`, carries `--no-env-file`.
bunx passes that flag to no tool it starts, so an untracked env file reaches
those. No child gets `BUN_OPTIONS`, which Bun reads into every process as
flags, a preload or a test filter among them.

`cargo xtask` runs `--locked`, and so does every cargo row, so a manifest edit
with no relock is refused before the gate starts rather than rewriting
`Cargo.lock`. Every row that walks the tree hands its tool the tracked files it
reads, fails when none was handed, and prints the count. Where the tool names
what it read, as rustfmt, taplo, tsc, actionlint and zizmor do, and as
`bun test`'s junit report does, the row reads that back and fails on a file it
skipped. Prettier is handed exactly the files its own file info keeps.
cargo-machete names each crate directory it visited, and the row fails when it
says it could not read one. A tool whose output a row reads runs with
`NO_COLOR` set, and the row strips any color or link code before it reads.

`typecheck` and `tools` cover `tools/`, the repository's own TypeScript. Bun
strips types rather than checking them, so without `typecheck` the gate would
run TypeScript whose types nothing reads. tsc must list every tracked
TypeScript file, so one outside `tools/` fails the row rather than going
unchecked. `bun test` is handed every tracked test file by path, wherever it
sits, and runs with `CI` set, so a committed `test.only` fails. The row also
fails a file where no test ran: bun test exits zero when every test in one
was skipped, a todo or held back by its condition.

`actionlint` checks workflow syntax, runner labels and every expression,
including whether a `needs.<job>.outputs.<name>` names an output that job
declares. `build-watch.yml` hands every decision between its jobs through those
outputs, and a misspelled one reads as an empty string rather than an error.

actionlint shells out to an analyzer it finds on `PATH` and says nothing at all
when it does not, and no flag changes that. ShellCheck runs through a stand-in.
`-shellcheck` names this xtask under a hidden subcommand, which reads each
`run:` script as actionlint decoded it. The stand-in refuses any line holding a
ShellCheck directive, and otherwise runs the ShellCheck mise resolved over the
same bytes. `SHELLCHECK_OPTS` never reaches it. A directive drops a finding
from the report, and YAML escapes and folding hide one from any reading of the
workflow file. The row first runs two canary workflows. One must come back with
`SC2086`, and the other, which carries a directive, must come back refused.
actionlint hands only a bash or sh script to ShellCheck, so the row also reads
every step's `shell:` through Bun's YAML parser and refuses any shell but bash,
sh and pwsh. pyflakes stays off, because no Windows package manager ships it
and leaving it on would have the quiet leg report a pass for an analysis it
never ran.

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
`workflows` job on every pull request. `--config` names `.github/zizmor.yml`,
which holds the hash-pin policy, the Dependabot cooldown threshold and every
waiver, so the environment cannot swap it for another. The opening row refuses
an inline `zizmor: ignore` comment under `.github`, so nothing waives an audit
outside that file. The shared `workflows` job fails unless every job passing
`secrets: inherit` calls a workflow under
`zachthedev/.github/.github/workflows/`. That hold is what lets `zizmor.yml`
waive the audit by file.

`tests` fails when no test ran, a run that skipped every test included, and
reads no nextest user config. No child gets a `NEXTEST_` variable, since one
can pass such a run or retry a failing test into a pass.

`doctests` runs beside `tests`, because `cargo nextest` runs none of them and a
doctest that stops compiling would otherwise pass the gate in silence. It
counts every documented example and fails on a filtered run, on every example
ignored, and on none. A repository that holds none yet declares that with
`NO_DOC_EXAMPLES` beside the step table, and the row fails once it counts one.
No child gets `RUSTDOCFLAGS`, `CARGO_BUILD_RUSTDOCFLAGS` or
`CARGO_ENCODED_RUSTDOCFLAGS`, where a test filter drops every example. `doc`
builds every crate's documentation with warnings denied, so a broken link is a
failure.

A missing tool stops the gate and names itself, because a check that did not
run is not a check that passed.

The pre-push hook and continuous integration call the same command, so neither
can run a different gate. Continuous integration adds what one machine cannot
check:

- It runs the gate on Windows, Linux and macOS, because the pre-push hook runs
  it on whichever host a contributor uses. Code behind `cfg(windows)` builds
  only on the first host, and code behind its inverse only on the others, so
  every leg has to hold. The Linux leg compiles `zstd-sys`, which is C, and the
  ubuntu runner image ships a C toolchain for it.
- `commits` runs commitlint over every commit in a pull request, because the
  `commit-msg` hook checks one commit on one machine and a rebase or a
  `--no-verify` reaches the branch unchecked.

A local run that fails, or passes where CI fails, is covered under
[Troubleshooting](#troubleshooting).

## Commit messages

[Conventional Commits](https://www.conventionalcommits.org), enforced by the
`commit-msg` hook and by the `commits` job. `.github/commit-scopes.json` holds
the scope list, one sentence per scope saying what it covers.
`cargo xtask scopes` prints it, and `commitlint.config.js` enforces it.

Scopes are the workspace crate names past the `ember-` prefix, plus
cross-cutting names no crate owns. A new top-level crate earns a scope. Omit the
scope rather than invent one. A scope that only repeats the type, such as
`ci(ci)`, is never written: the bare type says the same.

The header and every body line stop at 72 columns; commitlint refuses longer.
The subject is imperative and lowercase with no trailing period.

A pull request's title takes the type of its most user-facing commit, and `!`
when any commit breaks something users see. A squash lands the title alone,
and release-plz cannot recover a break the title dropped.

github.com cuts a commit subject at 73 characters, so the 72 applies to the
header that lands. A squash appends ` (#N)` to the title, and the `commits`
job lints the title with it appended. A Dependabot pull request whose landed
header runs past 72 fails that job. It is closed, and its bump is taken by
hand.

A revert is `revert(<scope>): <what is undone, in fresh words>`, with a
`Refs: <sha>` footer naming each reverted commit. git's own
`Revert "<header>"` subject carries no type, so neither the changelog nor the
release decision sees it, and repeating the reverted header overflows 72
columns.

A body says what was wrong, what the change does now, and what a reader needs
that the diff cannot show, such as what was deliberately not done. Past tense
belongs here and nowhere else: a code comment describes the code as it is, and
the commit message carries the history.

## Dependencies

`bunfig.toml` sets the install cooldown for Bun and travels with the clone, so
a container run with no user-level configuration sees the same gate. Cargo has
no cooldown file of its own: every version bump comes through Renovate, and the
preset `.github/renovate.json` extends holds the cooldown. `renovate.json` adds
what is true of this repository alone, and says why beside each entry.

Every gate tool comes through mise, and `mise.lock` is generated, so the gate
holds it to something that is not in it. `xtask/src/pins.rs` carries the owner
and the repository every tool's artifacts come from, and the rules refuse a
lockfile entry whose `url` or `backend` names anything else. A bare registry
key in `mise.toml` names no owner, so for those tools that table is the only
record of the account outside the generated file. Moving a tool to another
account takes an edit there, in the same diff as the lockfile it explains.

`mise.toml` sets `locked` twice, under `[settings]` and under `[tool_config]`,
because they are not the same setting. `MISE_LOCKED=false` and a `locked_scopes`
that drops `project` each turn the `[settings]` one off. The `[tool_config]` one
holds regardless of the environment, and mise reads it from the file alone, so
the gate asserts it from the file.

`taplo` is the one tool whose checksum does not come from its publisher. GitHub
began recording a digest for release assets after the taplo release `mise.toml`
pins was published, so its hashes were computed here and committed. They say the
bytes came from that release URL and that every install since has to match them,
which is narrower than a digest the publisher recorded and is not provenance.
Bumping taplo writes a lockfile entry with no checksum at all, which the gate's
own tests refuse, so whoever bumps it computes and commits the new hashes. A
relock at the same version keeps them, so only a bump drops them. A release the
pin file does not name is refused by name, so a wrong install fails at the gate
rather than reading a script under other rules.

The gate's `deny` row runs `cargo deny check licenses bans sources`.
Advisories are not a row, because an advisory published overnight would turn
a change red that touched nothing. Two legs read the lockfile for them, and
they fail in opposite directions: Dependabot alerts read GitHub's database on
every push, which lacks part of RustSec, and `.github/workflows/audit.yml`
runs `cargo deny check advisories` against RustSec daily, reading the
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
it. A release pull request merges only with every required check green, like
any other.

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

## Troubleshooting

A local run that fails, or that differs from continuous integration:

- **A JavaScript tool is not installed in this checkout.** The gate refuses a
  row whose tool `node_modules/.bin` lacks. Run `bun install --frozen-lockfile`
  after every pull, and in a linked worktree add `--ignore-scripts`, as
  [Hooks](#hooks) says. The `commit-msg` hook does not check, and bunx then
  runs a copy from a parent directory or `PATH`, which can differ from the one
  CI runs.
- **A package `bun.lock` no longer names still loads.** A frozen install never
  removes a package the lockfile dropped, and `node_modules` stays the same
  across a branch switch, so a test can pass here on an import CI refuses.
  Delete `node_modules` and install again.
- **A result differs on your machine alone.** A personal env file at the
  checkout's root reaches Prettier, tsc and commitlint, and the Bun variables
  reach Bun as [Safety](#safety) says. Move the file aside, leave
  `BUN_OPTIONS` and the `BUN_INSPECT` names unset, and run again.
- **An archive test fails to reach its fake bucket.** Those tests serve it on
  `127.0.0.1`. A proxy set in `HTTP_PROXY` or `HTTPS_PROXY` can catch that
  request, so add `127.0.0.1` to `NO_PROXY`.
- **The gate stops before its first row after a manifest edit.** `cargo xtask`
  runs `--locked`, so relock with a plain `cargo build` first.
- **zizmor passes here and fails in CI, or the reverse.** Locally it runs online
  when `gh auth token` answers, and CI's gate runs it offline; CI's shared
  `workflows` job runs the online audits.
- **A tool is missing.** The gate names it. Run `mise install`.

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
