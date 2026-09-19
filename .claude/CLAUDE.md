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
- **Commit scopes come from `cargo xtask scopes`.** It prints
  `.github/commit-scopes.json`, the file the commit hook reads. Omit the scope
  rather than invent one.

## Where a change belongs

[CONTRIBUTING.md](../CONTRIBUTING.md#where-code-goes) says which crate, and
which repository, a change belongs in.

## The documentation

[README.md](../README.md#documentation) lists every document and what it holds.
