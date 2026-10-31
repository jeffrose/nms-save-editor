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
* **a data pipeline** that regenerates item names, recipes and key mappings from
  the player's own game install, so a game update is a command, not a wait;
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
| `nmsedit metadata <mf_*.hg>` | decrypt a metadata file and show every field |
| `nmsedit slots <dir>` | list every slot with platform, version, play time, name |
| `nmsedit dump <save.hg> [out]` | write the raw JSON payload |
| `nmsedit verify <dir>` | round-trip fidelity check over a whole save directory |
| `nmsedit mapping <mapping.json>` | load a community key mapping |

## Guarantees

Verified against 62 real files from a live 7.x save set:

1. **Unedited saves are never modified.** Not rewritten, not re-compressed,
   timestamps untouched.
2. **Untouched numbers keep their exact text.** A naive `f64` round trip
   rewrites ~6% of the numeric literals in a real save; this does not.
3. **Latin-1 payloads survive.** Non-UTF-8 bytes in names are preserved, and
   characters above `U+00FF` are written as escapes rather than dropped.
4. **Metadata is patched, not regenerated.** Unknown fields are copied verbatim
   and the cipher is deterministic, so unchanged metadata re-encrypts to the
   original bytes.
5. **Writes are atomic.** Temp file, flush, rename — never a half-written save.

Details and measurements: [`docs/findings-m0.md`](docs/findings-m0.md),
[`docs/format/save-file.md`](docs/format/save-file.md),
[`docs/format/metadata.md`](docs/format/metadata.md).

## Layout

```
crates/nms-format   LZ4 block containers, Latin-1 payloads, XXTEA metadata, atomic writes
crates/nms-model    exact-text JSON documents, key mapping, payload diffing
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
| M2 | metadata refresh (save name, location, mode, timestamp), backups, diff report, `nmsedit export` |
| M3 | game data pipeline: item database, localisation, key mapping generated from the game install |
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
