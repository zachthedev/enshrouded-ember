# Ember

A modding SDK for the Enshrouded dedicated server.

Keen ships no modding SDK and no debugging symbols for `enshrouded_server.exe`.
Ember recovers what a server mod needs from the binary at runtime, loads mods
beside the server, and fails closed when a game update moves something.

Nothing here is affiliated with or endorsed by Keen Games.

## Crates

| Crate              | Holds                                                              |
| ------------------ | ------------------------------------------------------------------ |
| `ember-platform`   | Operating system seam: image access, inline hooks, library loading |
| `ember-sigs`       | Per-build signature and layout tables, as data                     |
| `ember-holistic`   | The Holistic engine: reflection registry, components, systems      |
| `ember-enshrouded` | The game above the engine: sessions, chat, saves, items, recipes   |
| `ember-sdk`        | What a mod author writes against                                   |
| `ember-loader`     | The library the server loads                                       |
| `ember-testkit`    | Development only: a local channel for driving a test server        |
| `xtask`            | Repository automation                                              |

The engine and game layers are separate because one public game runs on
Holistic, so nothing can test cross-game reuse. Keeping them apart means the
engine crates lift out cleanly if a second one appears.

## How it loads

On Windows, Ember ships as a proxy library placed beside
`enshrouded_server.exe`, named for a library the server already imports. It
forwards every export to the real system copy and starts Ember on the side.
`POWRPROF.dll` is the default: the server imports it, it exports two functions,
and neither other public Enshrouded loader claims it. Rename the same file to
`IPHLPAPI.dll`, `WINMM.dll` or `dbghelp.dll` to sit beside a server already
running one of those.

Wine and Proton prefer their own builtin copies, so a host on Linux sets:

```sh
WINEDLLOVERRIDES="powrprof=n,b"
```

## Surviving game updates

Keen ships builds under a monotonic revision, printed at server startup as
`Game Version (SVN): N`. The four-part version string does not identify a build:
two different binaries shipped as v0.9.1.2.

Ember keys its signature table on the platform and that revision, resolves every
required symbol at startup, and refuses to activate a mod whose symbols are
missing, naming each one in the log. The server keeps running vanilla for that
feature rather than starting half hooked.

A release supports the current server build plus the four before it.

## Requirements

- Rust 1.98.1, pinned in `rust-toolchain.toml`
- A C toolchain, for the MinHook engine. On Windows, Visual Studio Build Tools.

## License

MIT. See `LICENSE`.
