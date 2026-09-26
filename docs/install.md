# Installing Ember

Ember goes beside the dedicated server, never inside a Steam library copy of the
game. [README.md](../README.md) says whether a release exists yet.

## Requirements

The Enshrouded dedicated server, `enshrouded_server.exe`, on Windows, or under
Wine or Proton with the library override
[README.md#how-it-loads](../README.md#how-it-loads) names.

## Install

One proxy library and one `ember/` directory, beside the server executable:

```text
enshrouded_server.exe
POWRPROF.dll        Ember
ember/
  config.json       Ember's own settings
  logs/             The startup report and the running log
  mods/<mod>/       One directory per mod, each with its own config.json
```

A mod's install guide says which files go under its directory.
[usage.md](usage.md#running-a-server) says what Ember writes into `ember/` and
when.

## Check the download

None.

## Upgrade

Replace `POWRPROF.dll`. `ember/` holds your settings and your mods, and an
upgrade reads them where they are.

## Uninstall

Delete `POWRPROF.dll` and the `ember/` directory. Ember keeps nothing outside
them. A mod that writes beside the server's save says so in its own guide.
