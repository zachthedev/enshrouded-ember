## What this changes

<!-- One or two sentences. The why belongs in the commit message. -->

## The gate

Paste the last line the gate prints. `CONTRIBUTING.md` names the command.

```text

```

## Tested against

- Server build id and revision:
- Host: Windows, Wine or Proton
- Loaded as: proxy library, injection, or not loaded

## Every change

- [ ] Each commit follows Conventional Commits, with a scope from `.github/commit-scopes.json` or none.
- [ ] The documentation says what a user sees, where this changes it.
- [ ] A deviation from the handbook is recorded at its drift site.
- [ ] Nothing recovered from a Keen binary is in the diff. The extractors are
      committed and what they produce is not.

## If this touches a signature row

- [ ] Every row resolves against its real binary
- [ ] The pattern that recovers the address is in the diff beside it

<!--
A change that cannot reach a server says so here and names what it did instead.
-->
