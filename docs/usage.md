# Using Ember

This page is for whoever runs a server with Ember and whoever writes a mod
against it. [install.md](install.md) covers putting Ember beside a server.

## Running a server

Ember writes `ember/config.json` from defaults when it is absent, and each mod
does the same for its own under `ember/mods/<mod>/`. The startup report and the
running log land in `ember/logs/`.

At startup Ember resolves every required symbol and refuses to activate a mod
whose symbols are missing, naming each one in the log. The server keeps running
vanilla for that feature rather than starting half hooked.

A build that no table row covers stops with a message rather than hooking the
wrong function. Report one with the "New Keen build" issue template.
[README.md](../README.md#surviving-game-updates) says how Ember matches a build
and which builds a release supports.

## Writing a mod

A mod is a `cdylib` that takes the SDK with `cargo add ember-sdk` and exports
the C symbols `ember_sdk::declare` describes. The host runs the mod only once
every symbol it declared is resolved.

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
