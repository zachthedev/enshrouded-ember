# enshrouded-ember

The Ember modding SDK for the Enshrouded dedicated server, a `rust-crates`
repository. [README.md](README.md) says what it is.

## Read first

Read these before changing anything, in order. They bind an agent as they bind a
person.

1. [README.md](README.md)
2. [CONTRIBUTING.md](CONTRIBUTING.md)
3. [SECURITY.md](SECURITY.md)
4. [docs/install.md](docs/install.md)

## Verify

```sh
cargo xtask check          # the gate: every row in order, stopping at the first failure
cargo xtask check --rows   # print the rows and what each covers, and run nothing
cargo xtask pins           # the opening row alone: both mise pin files against their rules
```

[CONTRIBUTING.md#the-gate](CONTRIBUTING.md#the-gate) says what the rows cover
and what the gate does when a tool is missing.

## Never

- Never write to, launch or inject into a path under a Steam library.
  ([What never happens](CONTRIBUTING.md#what-never-happens))
- Never commit anything recovered from a Keen binary.
  ([What never happens](CONTRIBUTING.md#what-never-happens))
- Never commit an address without the pattern that recovers it.
  ([What never happens](CONTRIBUTING.md#what-never-happens))
- Never hand-edit a version in a manifest or a crate's `CHANGELOG.md`.
  ([What never happens](CONTRIBUTING.md#what-never-happens))
- Never write a commit message outside the convention.
  ([What never happens](CONTRIBUTING.md#what-never-happens))

## Deviations

A comment that calls a line a deliberate deviation records a decision, not a
defect. Leave the line as it is.

## Where the rest is

[README.md#documentation](README.md#documentation)
