# Fixtures

Save files are **not** tracked in this repository: real saves contain player
names, account IDs and platform GUIDs. `fixtures/` is git-ignored except for
this README.

## Getting fixtures

```sh
tools/fetch-fixtures.sh ~/Downloads/nms-saves.zip     # an archive of a save dir
tools/fetch-fixtures.sh "$HOME/.steam/steam/steamapps/compatdata/275850/pfx/drive_c/users/steamuser/AppData/Roaming/HelloGames/NMS"
```

Default destination is `fixtures/steam/`; override with `NMS_FIXTURES_DEST`.
Tests look for a `fixtures/` directory at the repository root, or wherever
`NMS_FIXTURES` points. When no fixtures are present the fixture tests skip
(with a message) instead of failing.

## What to collect

| Platform | Files |
|---|---|
| Steam / GOG | the whole account directory: `save*.hg`, `mf_save*.hg`, `accountdata.hg`, `usersettings.hg`, `mf_accountdata.hg`, `mf_usersettings.hg` |
| Xbox Game Pass | `containers.index` plus the GUID blob directories |
| PS4 | `memory.dat` (homebrew) or a Save Wizard export / `savedataNN.hg` + `manifestNN.hg` |
| Switch | `savedataNN.hg` + `manifestNN.hg` |

A save directory from each platform, and preferably from more than one game
version, is what makes the format tests meaningful. Never commit them.

## Synthetic fixtures

Tests that must be tracked use small synthetic payloads built in code (wrong
magic, truncated blocks, unknown metadata lengths, and so on). If a synthetic
file is ever needed on disk, keep it obviously fake — no real player data.
