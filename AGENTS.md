# enshrouded-ember

The Ember modding SDK for the Enshrouded dedicated server, a `rust-crates`
repository. [README.md](README.md) says what it is.

## Read first

Read [CONTRIBUTING.md](CONTRIBUTING.md) and [docs/dev.md](docs/dev.md) before
changing anything. They bind an agent as they bind a person.

## Verify

```sh
cargo xtask check          # the gate: every row in order, stopping at the first failure
cargo xtask check --rows   # print the rows and what each covers, and run nothing
cargo xtask pins           # the opening row alone: mise.toml and mise.lock against their rules
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

A comment beside a line that names the handbook records a deliberate
deviation. It is a decision, not a defect.

## Where the rest is

[README.md#documentation](README.md#documentation)
