# nms-save-editor

A No Man's Sky save editor, written in Rust.

**Status: early.** The format layer is complete and verified against real saves;
the editor itself is being built on top of it. There is no GUI yet — the current
binary reads, verifies and reports.

## Why another one

[goatfungus/NMSSaveEditor](https://github.com/goatfungus/NMSSaveEditor) is the
tool most players use, but it is closed source, needs a Java runtime, and its
game knowledge is hand-maintained: 465 issues are open, most of them "the new
update broke the name mappings" or "the UI is scrambled", and the editor trails
the game by months. A Rust implementation can fix the *causes* rather than the
symptoms:

* **native binaries** for Windows, Linux, macOS and Steam Deck — no JRE, no Wine;
* **save integrity by construction** instead of by careful clicking;
* **a data pipeline** that regenerates game text and item names from the
  player's own install, so a game update is a command, not a wait (84,965 strings
  and 7,640 item names in 137 ms today);
* **automation** — a library and a CLI, not only a window.

## What works today

```console
$ nmsedit info save.hg
file            save.hg
container       lz4-blocks (31 blocks)
file size       4621727 bytes
payload         16073762 bytes (+NUL)
json keys       751002
version         4743
platform        Win|Final
active context  Main
play time       3156876 (876:54:36)
storage slot    2 (PlayerState1)
```

| Command | Purpose |
|---|---|
| `nmsedit info <save.hg>` | container layout plus a summary of the save |
| `nmsedit slots <dir>` | list every slot with platform, version, play time, name |
| `nmsedit get <save.hg> <path>` | read a field by readable path |
| `nmsedit set <save.hg> <path> <value>` | edit a field: back up, write, refresh metadata |
| `nmsedit export <save.hg\|dir> [out]` | readable JSON for review, sharing or diffing |
| `nmsedit dump <save.hg> [out]` | write the raw on-disk payload |
| `nmsedit metadata <mf_*.hg>` | decrypt a metadata file and show every field |
| `nmsedit backup <dir>` | timestamped zip backup of a save directory |
| `nmsedit backups <dir>` | list existing backups |
| `nmsedit restore <zip> --to <dir>` | restore a backup (replaced files are backed up first) |
| `nmsedit verify <dir>` | round-trip fidelity check over a whole save directory |
| `nmsedit mapping <mapping.json>` | load a community key mapping |
| `nmsedit data refresh` | reread the game's archives and rebuild the local data pack |
| `nmsedit data info` | show the installed data pack |
| `nmsedit data lookup <id>` | print game text by id |
| `nmsedit data item <id>` | print item names by id, with the source of each name |

There is also an in-game verification harness that runs the game against a
throwaway Proton prefix, because on PC the saves live inside the Steam
Cloud-synced prefix and a test save there propagates to every machine on the
account: `tools/sandbox-test.sh` (see [`docs/verification.md`](docs/verification.md)).

Editing works on readable, dotted paths that are resolved through the key map,
so what you type is the field name and what lands on disk is the spelling the
game reads:

```console
$ nmsedit set save.hg CommonStateData.SaveName "Voyager Alpha"
save.hg (PlayerState13)
  CommonStateData.SaveName: Our Journey Continues -> Voyager Alpha
CommonStateData.SaveName: Our Journey Continues -> Voyager Alpha (payload 8825445 -> 8825437 bytes; 9 metadata words refreshed)
backup: Save Backups/save13_20261101_011825.zip (1926925 bytes)
wrote: 2605205 bytes (metadata: 432 bytes)
```

## Guarantees

Verified against 62 real files from a live 7.x save set:

1. **Unedited saves are never modified.** Not rewritten, not re-compressed,
   timestamps untouched.
2. **Untouched numbers keep their exact text.** A naive `f64` round trip
   rewrites ~6% of the numeric literals in a real save; this does not.
3. **Latin-1 payloads survive.** Non-UTF-8 bytes in names are preserved, and
   characters above `U+00FF` are written as escapes rather than dropped.
4. **Metadata is mirrored, not regenerated.** Sizes, version, play time, save
   name, location, game mode and timestamp are re-derived from the save (verified
   to match the game's own metadata across all 30 fixture slots), and unknown
   words are copied verbatim.
5. **Metadata is only written with a new data file**, so it can never describe a
   file that did not change.
6. **Every write is backed up first** (timestamped zip, bounded retention) and
   **atomic** (temp file, flush, rename) — never a half-written save. Restores
   keep a copy of what they replace, so undoing an undo is possible too.
7. **Changes are reported by field, not by byte count**
   (`CommonStateData.SaveName: Our Journey Continues -> Voyager Alpha`).
8. **Live save folders are refused by default.** A path that looks like the
   game's own save folder (or a Steam-managed Proton prefix) needs an explicit
   `--confirm-live`, because Steam Cloud would propagate the edit to every
   machine on the account. Scratch copies marked with `.nmsedit-sandbox` are
   exempt.

Details and measurements: [`docs/findings-m0.md`](docs/findings-m0.md),
[`docs/findings-m2.md`](docs/findings-m2.md),
[`docs/format/save-file.md`](docs/format/save-file.md),
[`docs/format/metadata.md`](docs/format/metadata.md).

## Layout

```
crates/nms-format   LZ4 block containers, Latin-1 payloads, XXTEA metadata, atomic writes
crates/nms-model    exact-text JSON documents, key mapping, path access, change reporting
crates/nms-edit     load/edit/plan/commit, metadata mirroring, backups
crates/nms-pak      HGPAK reader for the game's own .pak archives
crates/nms-data     regenerates game text from a local installation
crates/nms-cli      the nmsedit binary
docs/format         the format notes this implementation is built from
tools/              fixture and data-pipeline scripts
```

## Development

```sh
cargo test --workspace              # unit tests always run
tools/fetch-fixtures.sh <save.zip>  # then the fixture tests run too
cargo run -p nms-cli -- info <save.hg>
```

Fixture tests exercise real saves and skip with a message when none are
present. See [`fixtures/README.md`](fixtures/README.md).

## Roadmap

| Milestone | Content |
|---|---|
| **M1 (done)** | format layer, fidelity guarantees, verification CLI |
| **M2 (done)** | edit/commit with backups, metadata mirroring, change reports, readable export, restore |
| **M3 (in progress)** | game data pipeline — 84,965 strings regenerated in 137 ms, item names joined from three item tables (94% of a real save's ids named, every answer labelled with its source), and item categories verified against the save's own inventory types |
| M4 | editing: currencies, inventories, known items — library first, then GUI |
| M5 | GUI (`egui`): inventory grids, JSON editor, slot list from metadata |
| M6 | other platforms: Xbox Game Pass, PS4, Switch |
| M7 | entity export/import, reward-claim correctness, coordinate tooling |

## Licence and provenance

MIT OR Apache-2.0.

Written from scratch against real save files and published format notes. The
reference Rust parser [`nms-save`](https://crates.io/crates/nms-save) (MIT OR
Apache-2.0) was used to cross-check the container and metadata layout; mapping
data comes from the community mapping file distributed with
[MBINCompiler](https://github.com/monkeyman192/MBINCompiler). No code or data
from the GPL/AGPL save editors (NomNom, NMSE) or from the closed-source Java
editor is used — they are consulted as documentation only, and no game assets
are redistributed.

No Man's Sky is a trademark of Hello Games. This is an unofficial, unaffiliated
tool.
