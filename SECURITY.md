# Security

Ember loads a library into a running dedicated server process, and its
continuous integration writes to an archive other people fetch executables from.
This file says what counts as a vulnerability in that arrangement, and how to
report one without publishing it first.

## Reporting

Open a private advisory:
<https://github.com/zachthedev/enshrouded-ember/security/advisories/new>

Or write to <hey@zachthe.dev>. The advisory is the better channel, because
the fix and the credit land beside the report.

Never open a public issue for a vulnerability. Everything else belongs in the
issue tracker.

A report is most useful with the Steam build id, the Ember version, and the
smallest reproduction you have. Keep a proof of concept inert. Something that
writes to standard output proves the hole is reachable as well as something that
acts on it.

## What is supported

The newest release on the
[Releases page](https://github.com/zachthedev/enshrouded-ember/releases) is
supported, and `main` is until the first one ships. The server builds a release
covers are in the README's
[Surviving game updates](README.md#surviving-game-updates). A build outside them
is unsupported, and Ember refuses to run against it by design.

## In scope

- The loader and the proxy library. Anything that runs code inside the server
  process that the operator did not put there.
- The SDK boundary. A mod reaching past what its manifest declared, or reading
  what the host did not hand it.
- Signature resolution. A wrong match that hooks a function the row does not
  name.
- The build pipeline and the archive. A way to place bytes in the archive, or to
  make a digest row agree with bytes it is supposed to refuse.
- This repository's own tooling, where it runs on a contributor's machine.

## Out of scope

- A defect in Enshrouded or in Keen's engine that Ember does not introduce.
  Report those to Keen Games.
- Anything that needs write access to the server directory. Ember loads from
  beside `enshrouded_server.exe`, so whoever can write there can already replace
  the executable.
- Anything that needs administrator access to the machine running the server.
- A mod doing what its author wrote it to do. An operator chooses which mods to
  install, and Ember names every one it loaded in the startup report.
- Cheating or griefing by a player on a server whose operator installed a mod
  that allows it.
- A build no signature row covers. Ember stops rather than hooking the wrong
  function, which is the designed behavior.
- Denial of service that a vanilla server has too.

## After a report

There is no bounty. A report gets an acknowledgment, a fix, and a credit in the
advisory unless you ask to stay anonymous.
