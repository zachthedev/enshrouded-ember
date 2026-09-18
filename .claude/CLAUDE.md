# Claude Code in this repository

Everything a contributor needs is in the human-facing files. This one only
points at them.

Read [CONTRIBUTING.md](../CONTRIBUTING.md) and [docs/dev.md](../docs/dev.md)
before changing anything.

## The rules that do not bend

- **The gate is `cargo xtask check`.** Run it, and never run its steps
  separately as a substitute. A missing tool stops it and names itself.
- **Never write to, launch, or inject into a Steam library path.** A dedicated
  server for development is fetched separately into `.cache`.
- **Nothing recovered from a Keen binary is committed.** No schema dump, no
  string table, no protocol registry, no game data. The extractors are
  committed; their output lives under `.cache` and is regenerated. The one
  published exception is the signature table.
- **Never commit an address without the pattern that recovers it.** A table row
  locks a scan result; it does not replace the scan.
- **Commit scopes come from `cargo xtask scopes`.** That list and
  `commitlint.config.js` are the same list. Omit the scope rather than invent
  one.

## Which repository a change belongs in

Ask what the code describes:

- Keen's engine or Keen's game: it goes here
- A mod's own idea: it goes in that mod's repository, such as
  [enshrouded-mods](https://github.com/zachthedev/enshrouded-mods)

Inside this repository: the engine goes in `ember-holistic`, the Enshrouded
server in `ember-enshrouded`, and the operating system in `ember-platform`.

## The documentation

| File                                  | Holds                                      |
| ------------------------------------- | ------------------------------------------ |
| [CONTRIBUTING.md](../CONTRIBUTING.md) | The gate, the commit convention, the hooks |
| [docs/dev.md](../docs/dev.md)         | The first run, end to end                  |
| [README.md](../README.md)             | What Ember is and how it loads             |
