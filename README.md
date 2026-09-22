# Ember

A modding SDK for the Enshrouded dedicated server.

Keen ships no modding SDK and no debugging symbols for `enshrouded_server.exe`.
Ember recovers what a server mod needs from the binary at runtime, loads mods
beside the server, and fails closed when a game update moves something.

Nothing here is affiliated with or endorsed by Keen Games.

This repository is in development. No release is published yet.

## What a mod author gets

A mod is a `cdylib` that links `ember-sdk` and exports the C symbols
`ember_sdk::declare` describes. The host runs the mod only once every symbol it
declared is resolved.

- **A lifecycle.** `Mod::load` runs after resolution and dependency ordering.
  `Mod::ready` runs once every hook is armed. Hooks and events register in
  `load` and nowhere else, so the chain is fixed for the life of the process.
- **Chained hooks.** Several mods hook one target and run in load order, each
  refined by `First`, `Normal` or `Last`. A link continues, stops with a
  substitute return, or fails and is skipped for that call.
- **Engine access, not a proxy.** A mod reads components and reflection through
  its own copy of `ember-holistic` and `ember-enshrouded`, using addresses and
  offsets the host resolved. The host never marshals a read.
- **Per-mod config and logging.** JSON like the server's own file, written from
  defaults when absent, and a log sink the loader owns.
- **Save-cycle hooks.** A mod stages a payload and the host commits it only when
  the server's own save succeeds.
- **Fail closed.** A mod whose symbols are missing is disabled by name and the
  server keeps running vanilla for that feature.

## Crates

Every crate says what it holds in its own `Cargo.toml`, and one command prints
them all:

```sh
cargo xtask crates
```

The engine and game layers are separate because one public game runs on
Holistic, so nothing can test cross-game reuse. Keeping them apart means the
engine crates lift out cleanly if a second one appears.

## How it loads

On Windows, Ember ships as a proxy library placed beside
`enshrouded_server.exe`, named for a library the server already imports. It
forwards every export to the real system copy and starts Ember on the side.

Of the libraries the server imports, `POWRPROF.dll`, `IPHLPAPI.DLL` and
`dbghelp.dll` sit outside the `KnownDLLs` list, so each can be proxied.
`POWRPROF.dll` is the default, because no other public Enshrouded loader claims
it and its exports are all named. `dbghelp.dll` exports some functions by
ordinal only.

A proxy forwards every export of the library it stands in for, not only the ones
the server imports, because anything else loaded into the process may import the
rest.

Wine and Proton prefer their own builtin copies, so a host on Linux sets:

```sh
WINEDLLOVERRIDES="powrprof=n,b"
```

## Surviving game updates

Keen ships builds under a monotonic revision, printed at server startup as
`Game Version (SVN): N`. The four-part version string does not identify a build:
two different binaries shipped as v0.9.1.2.

The revision is not readable when Ember loads: the server formats it into that
log line later in startup. Each table row therefore also carries a fingerprint
taken from the image itself, and the loader matches on that.

Ember resolves every required symbol at startup and refuses to activate a mod
whose symbols are missing, naming each one in the log. The server keeps running
vanilla for that feature rather than starting half hooked.

A release supports the current server build plus the four before it. Every
supported build is fetched from SteamCMD and cached by its Steam build id, and
every change to the table resolves against all of them. No row ships without
being resolved against its real binary.

A build that no row covers stops with a message rather than hooking the wrong
function. Report one with the "New Keen build" issue template.

## Installing

Ember goes beside the dedicated server, never inside a Steam library copy of the
game. [docs/install.md](docs/install.md) has the layout.

## Documentation

| File                               | For                                                                  |
| ---------------------------------- | -------------------------------------------------------------------- |
| [docs/install.md](docs/install.md) | Where Ember goes beside a server, and how it is upgraded and removed |
| [docs/dev.md](docs/dev.md)         | What to install, the first run, the dev server, the tools behind it  |
| [CONTRIBUTING.md](CONTRIBUTING.md) | The gate, the commit convention, where code goes, what never happens |
| [SECURITY.md](SECURITY.md)         | How to report a vulnerability and what is in scope                   |
| [AGENTS.md](AGENTS.md)             | What an agent reads first, runs to verify, and never does            |

## License

MIT. See `LICENSE`.

`LICENSE` carries the legal name, because that line is the legally operative
one. Every manifest field a reader sees carries the brand, so `Cargo.toml`
`authors` reads `ZachTheDev <hey@zachthe.dev>`.
