# Developing Ember

## Prerequisites

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
person gets the convenience.

`mise.lock` is generated, so the gate holds it to something that is not in it.
`xtask/src/pins.rs` carries the owner and the repository every tool's artifacts
come from, and the rules refuse a lockfile entry whose `url` or `backend` names
anything else. A bare registry key in `mise.toml` names no owner, so for those
tools that table is the only record of the account outside the generated file.
Moving a tool to another account takes an edit there, in the same diff as the
lockfile it explains.

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

## First run

From a fresh clone to a green gate:

```sh
git clone https://github.com/zachthedev/enshrouded-ember.git
cd enshrouded-ember
mise install                  # the gate's tools, at the releases mise.lock records
bun install                   # the hooks and the markup formatter
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

`cargo xtask check --rows` prints the gate's rows and what each covers.
[CONTRIBUTING.md#the-gate](../CONTRIBUTING.md#the-gate) says what the gate does
when a tool is missing. `pre-push` runs the same command, and so does
continuous integration.

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

## Generated files

| File                       | Regenerated by                                                |
| -------------------------- | ------------------------------------------------------------- |
| `Cargo.lock`               | any cargo build after a manifest edit; the gate runs locked   |
| `bun.lock`                 | `bun install` after a `package.json` edit                     |
| `mise.lock`                | `mise lock` after a `mise.toml` edit                          |
| `mise.semver.lock`         | `MISE_ENV=semver mise lock` after a `mise.semver.toml` edit   |
| `data/steam-builds.jsonl`  | the build watcher workflows, one appended row per change      |
| `data/build-digests.jsonl` | `bun run archive emit` or `archive record`, one row per build |

Nobody hand-edits a row of either record. `mise.lock` keeps the hand-computed
taplo hashes across a relock at the same version, as Prerequisites says.

## Tests that need a real thing

Tests that need a real build are `#[ignore]` and find it through the
environment. `EMBER_SERVER_EXE` names a server executable, `EMBER_CLIENT_EXE` a
client executable, and `EMBER_SCHEMA` a directory of dumps to compare against.
Each test skips cleanly when its variable is unset, and
`cargo test -- --ignored` runs them. Every other test stands on a synthetic
image built in memory, so the whole suite runs on a machine with no server at
all.

A test that drives a real detour runs under unwinding rules the shipped library
does not have, because cargo ignores `panic = "abort"` for the test profile.
Never rely on catching a panic from a hook body.

## The localization tables

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

## The server and schema commands

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

## The signature diff

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

## The build watcher and the archive

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
cancelled by the next one arriving. Cases hold both.

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

## Building a mod against this checkout

To build a mod in another checkout against this one, put a `[patch.crates-io]`
block in a `.cargo/config.toml` in a directory **above** both. Cargo merges
config from every parent directory, so nothing in either repository has to
change.
