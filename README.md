# nms-save-editor

A No Man's Sky save editor, written in Rust.

**Status: early.** The format layer is complete and verified against real saves;
currencies, inventories and known-item lists are editable from the library, the
CLI, the desktop GUI and an MCP server for agents.

Prebuilt binaries for Linux, Windows and macOS are attached to
[releases](https://github.com/jeffrose/nms-save-editor/releases); tagging `v*`
runs [`.github/workflows/release.yml`](.github/workflows/release.yml).

## Why another one

[goatfungus/NMSSaveEditor](https://github.com/goatfungus/NMSSaveEditor) is the
tool most players use, but it is closed source, needs a Java runtime, and its
game knowledge is hand-maintained: 465 issues are open, most of them "the new
update broke the name mappings" or "the UI is scrambled", and the editor trails
the game by months. A Rust implementation can fix the *causes* rather than the
symptoms:

* **native binaries** for Windows, Linux, macOS and Steam Deck — no JRE, no Wine;
* **save integrity by construction** instead of by careful clicking;
* **a data pipeline** that regenerates game text, item names and item
  descriptions from the player's own install, so a game update is a command,
  not a wait (84,965 strings, 7,957 item names and 3,660 descriptions, in
  137 ms, for any language the game ships) — and the obfuscated field names,
  which are a hash of the name, come from the same pipeline, with a baseline
  for the keys real saves use compiled into the editor
  ([how](docs/findings-mapping.md));
* **automation** — a library, a CLI, and an MCP server so an agent can do the
  clicking (see [MCP](#mcp)).

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
| `nmsedit slots <dir>` | list every save with its save point (Autosave or Restore Point), platform, version, play time and name |
| `nmsedit get <save.hg> <path>` | read a field by readable path |
| `nmsedit set <save.hg> <path> <value>` | edit a field: back up, write, refresh metadata |
| `nmsedit export <save.hg\|dir> [out]` | readable JSON for review, sharing or diffing |
| `nmsedit dump <save.hg> [out]` | write the raw on-disk payload |
| `nmsedit metadata <mf_*.hg>` | decrypt a metadata file and show every field |
| `nmsedit backup <dir>` | timestamped zip backup of a save directory |
| `nmsedit backups <dir>` | list backups with the state each holds, and manage them (`--pin`, `--unpin`, `--remove`, `--prune`) |
| `nmsedit restore <zip> --to <dir>` | restore a backup (replaced files are backed up first) |
| `nmsedit verify <dir>` | round-trip fidelity check over a whole save directory |
| `nmsedit mapping <mapping.json>` | load a key mapping and report it |
| `nmsedit data refresh` | reread the game's archives and rebuild the local data pack and full key mapping |
| `nmsedit data info` | show the installed data pack |
| `nmsedit data lookup <id>` | print game text by id |
| `nmsedit data item <id>` | print item names by id, with the source of each name and the game's description where it names one |
| `nmsedit data mapping` | derive the save-key mapping from the game's own executable (field names live in `NMS.exe`; the key is a hash of the name), merging the installed file; `--saves <dir>` scopes it to those saves and builds the shipped baseline |
| `nmsedit currencies <save.hg>` | show units, nanites and quicksilver |
| `nmsedit currency <save.hg> <name> <value>` | set one balance and commit |
| `nmsedit suit <save.hg>` | show the exosuit's health, shield and energy |
| `nmsedit suit <save.hg> <stat> <value>` | set one of them and commit |
| `nmsedit resize <save.hg> <inv> <width> <height>` | change an inventory's grid size (`--beyond-vanilla` past the game's own) |
| `nmsedit unlock <save.hg> <inv>` | enable every slot in an inventory's grid |
| `nmsedit move-to <save.hg> <inv> <slot> <to-inv>` | move a slot into another inventory |
| `nmsedit clone <save.hg> <inv> <slot> <to-inv>` | copy a slot into another inventory |
| `nmsedit inventories <save.hg> [--all]` | list inventories, their grids and roles |
| `nmsedit inventory <save.hg> <name>` | list one inventory's slots, with item names |
| `nmsedit add-item <save.hg> <inv> <id> <amount>` | add items, filling stacks first (`--kind`, `--max`, `--cell`) |
| `nmsedit remove-item <save.hg> <inv> <slot>` | remove a slot |
| `nmsedit set-amount <save.hg> <inv> <slot> <amount>` | set a slot's amount |
| `nmsedit repair <save.hg> [<inv> <slot>]` | repair one slot, or every damaged slot |
| `nmsedit move-item <save.hg> <inv> <slot> <x> <y>` | move a slot to another cell |
| `nmsedit known <save.hg> [list]` | list learned ids |
| `nmsedit known-add` / `known-remove <save.hg> <list> <id>` | learn or forget an id |

There is also an in-game verification harness that runs the game against a
throwaway Proton prefix, because on PC the saves live inside the Steam
Cloud-synced prefix and a test save there propagates to every machine on the
account: `tools/sandbox-test.sh` (see [`docs/verification.md`](docs/verification.md)).

Editing works on readable, dotted paths that are resolved through the key map,
so what you type is the field name and what lands on disk is the spelling the
game reads. The player-state commands address inventories the same way, by the
label `nmsedit inventories` prints:

```console
$ nmsedit set save.hg CommonStateData.SaveName "Voyager Alpha"
save.hg (PlayerState13)
  CommonStateData.SaveName: Our Journey Continues -> Voyager Alpha
CommonStateData.SaveName: Our Journey Continues -> Voyager Alpha (payload 8825566 -> 8825558 bytes, 21 byte(s) changed at offset 57; 9 metadata words refreshed)
backup: Save Backups/save13_20261102_000607.zip (1926927 bytes)
wrote: 2605172 bytes (metadata: 432 bytes)

$ nmsedit currency save.hg units 123456789
units: 1757937817 -> 123456789
save.hg (PlayerState13)
  BaseContext.PlayerStateData.Units: 1757937817 -> 123456789
BaseContext.PlayerStateData.Units: 1757937817 -> 123456789 (payload 8825445 -> 8825444 bytes, 9 byte(s) changed at offset 943447; 3 metadata words refreshed)
backup: Save Backups/save13_20261102_000544.zip (1926925 bytes)
wrote: 2605298 bytes (metadata: 432 bytes)

$ nmsedit inventory save.hg Inventory | head -3
Inventory  10x12  77/120 slots  group Personal  class C
[  0] ( 0, 5) ^AMMO                      PROJECTILE AMMUNITION             9552/9999
[  1] ( 2, 3) ^ROBOT1                    PUGNEUM                           1210/9999
```

Every balance is limited to the game's own 32-bit range, `--dry-run` plans an
edit without writing, and a refused edit (a full grid, an occupied cell, an
amount the slot cannot hold) leaves the save untouched.

## GUI

```sh
cargo run --release -p nms-gui -- /path/to/HelloGames/NMS/st_<id>
```

Launched without a folder, the window opens the platform's usual save location
itself (Steam's Proton prefix on Linux, `%APPDATA%` on Windows, the Application
Support folder on macOS; `NMSEDIT_SAVE_DIR` overrides it). **Browse…** next to
the folder box opens a picker: a path box, the usual locations as shortcuts,
and a directory list that marks the folders actually holding saves, so the
right one is obvious.

The slot list is built from the metadata files, so every save shows its name,
game mode, location and play time without decompressing anything. Each game
slot holds **two save points** — the game's own **Autosave** and **Restore
Point**, fixed by the file name (`save.hg`/`save3.hg`… are autosaves,
`save2.hg`/`save4.hg`… restore points) — so the list groups them, names each,
marks the newer one (the one the game loads by default), and the overview says
which you are editing and names the other file. Opening a slot adds the save
version, balances, known-item counts and every inventory the
player state carries; the inventory tab draws the grid the game draws, with item
names from the data pack — the game's own text, regenerated locally — and
per-slot details on click. Ids the text tables do not name (some legacy
objects) show the id instead; the editor never invents a name.

The window is organised in groups — **Player** (Overview, Suit, Inventories),
**Data** (Known items) and **Advanced** (JSON) — so the strip stays short while
the original editor's names stay recognisable. More pages from that editor
(Ships, Multitool, Freighter, Frigates, Bases, Settlements, Milestones,
Account) are planned in [`docs/plan-gui-parity.md`](docs/plan-gui-parity.md).

The **Suit** page edits the exosuit's three stats — health, shield and energy —
which are plain fields in the player state; the game reads them as signed
32-bit values, so the window refuses anything larger than 2147483647. The
overview's balances work the same way, capped at the game's own 4294967295.

The **Inventories** page resizes a grid and enables its locked cells, both
staged like every other edit. A resize past the size the game itself writes for
that kind of inventory needs the "beyond vanilla" opt-in, because the game's own
UI cannot address the extra cells and the save can break; the sizes the editor
knows are the ones measured from real saves, and a kind the corpus never showed
fully unlocked is not guessed at. A slot can also be moved or copied into
another inventory ("Move into" in the slot's details): the moved slot keeps
every field the game wrote — damage, installation, stack size — and only its
cell changes, so a damaged technology module survives the trip intact.

**Add item** is a picker over the game's own catalogue: search by name or id,
see the item's category and description (where the game names one — 3,660 of
the 7,957 items), and add it with a stack size taken from the save. An id can
still be typed directly, so an item the pack does not hold — a modded one —
still works.

Editing follows a staging model: balances, suit stats, slot amounts, repairs, and
add/remove/move item actions change only the in-memory document, and a
**Save** button writes them through the same edit layer the CLI uses — a
timestamped zip backup first, then the data file, then patched metadata. A
refused action (a full grid, an occupied cell, an amount the slot cannot hold)
leaves the save clean, so an unedited save is still never rewritten. Switching
slots or folders with staged changes asks first; **Discard** re-reads the file.

The **Known items** tab holds the five learned-id lists — products,
technology, specials, base parts and refiner recipes — with a filter over the
id and the item name, and add/remove actions that stage like every other edit.
The overview's counts jump straight to the list they name.

The **JSON** tab is the escape hatch for everything the typed editors do not
cover: the whole document as a tree, with readable names on the keys and the
on-disk spelling on hover, and a search that finds a field by key or value
(measured at 88 ms over a 9 MB save). A selected field is edited with a control
that matches its kind — text, number or checkbox — and the edit replaces exactly
the node it was found on, so duplicate readable names cannot send it elsewhere.
Nothing is created: a path that does not resolve is refused, so a typo cannot
add junk to a save.

The overview also lists the save's backups, written before every save, with a
restore button that asks first. A restore archives the files it replaces as
`pre-restore`, so undoing a restore works too.

Pointed at the game's own folder the window still works, with a standing
caution: that folder is what Steam Cloud syncs, so an edit there reaches every
machine signed into the account. Every save backs up first, and the overview's
backup list is where to go when something looks wrong.

Field names come from the key mapping compiled into the editor; without a data
pack the item names are absent, and without a full mapping the handful of
fields the derivation cannot resolve keep their on-disk names. The window says
so in both cases.

## MCP

```sh
nmsedit mcp        # speaks the Model Context Protocol on stdin/stdout
```

The same binary is an MCP server, so an agent can answer requests like *"add
100,000 nanites to my Pioneers save"*: it lists the saves, finds the one named
Pioneers, resolves what "nanites" means (a balance, not an item), sets it, and
reports the backup it wrote. Twenty tools cover the editor: slot listing and
search, balances, inventories and their slots, item add/remove/set-amount,
repairs, learned lists, arbitrary fields by path, backups and restore.

Register it with OpenCode (global config, `~/.config/opencode/opencode.json`):

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "servers": {
      "nmsedit": {
        "type": "local",
        "command": ["nmsedit", "mcp"],
      },
    },
  },
}
```

or `opencode mcp add nmsedit --global -- nmsedit mcp`. Any other MCP client
takes the same shape: run `nmsedit mcp` and speak JSON-RPC 2.0 over stdio, one
message per line.

Nothing bypasses the edit layer. Every write takes a timestamped backup first,
is verified in memory before it touches the disk, and an unedited save is never
rewritten. Writing into the game's own folder is allowed; the result carries a
`note:` line saying that folder is the one Steam Cloud syncs, so an agent can
pass that on instead of hiding it. `NMSEDIT_MAPPING` and `NMSEDIT_PACK` select
the full key mapping and data pack, as they do for the CLI and GUI; the
built-in baseline is used when there is no mapping file.

## Backups

Every write — CLI, GUI or MCP — takes a timestamped zip backup of the files it
is about to replace, before anything is written. Backups live in
`Save Backups/` next to the save, the same folder the CLI's `--backup-dir`
defaults to, so all three faces share one history.

```console
$ nmsedit backups .
archive                             size  state from           save                    play  pinned
save13_20261102_132025.zip       1.8 MiB  2026-11-02 13:20:23  Our Journey Conti…  19:13:42
save13_20261102_132021.keep.zip  1.8 MiB  2026-11-02 00:02:30  Our Journey Conti…  19:13:42  kept
2 archive(s) in ./Save Backups
```

The `state from` column comes from the metadata file *inside* each archive, so
a listing never decompresses a save payload, and "which backup is the one from
before I broke it" is a question the listing answers.

Managing them:

| | |
|---|---|
| `nmsedit backups <dir> --pin <archive>` | keep an archive out of pruning (renames it to `*.keep.zip`) |
| `nmsedit backups <dir> --unpin <archive>` | let pruning remove it again |
| `nmsedit backups <dir> --remove <archive>` | delete one archive |
| `nmsedit backups <dir> --prune [--keep <n>]` | apply the retention limit now |
| `nmsedit restore <archive> --to <dir>` | put a state back (what it replaces is backed up first) |

The editor keeps the newest ten unpinned archives per save; pinned ones never
count against that and are never removed. The GUI's overview shows the same
list with Restore, Pin and Delete buttons, and the MCP server exposes the same
through `list_backups`, `pin_backup`, `delete_backup` and `restore_backup`.

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
   Backups are named for the moment they were taken, and each one knows the
   state it holds, so choosing between restore points does not mean guessing.
7. **Every write is verified before it touches the disk.** The planned bytes are
   read back and parsed in memory first, so a save this editor cannot read is
   one it refuses to write; the check runs before the backup, so a refusal
   costs nothing.
8. **Changes are reported by field, not by byte count**
   (`CommonStateData.SaveName: Our Journey Continues -> Voyager Alpha`).
9. **A write into the game's own folder is called out, not blocked.** The
   guard that used to refuse those paths was dropped on 2026-11-02, once every
   write was backed up and verified: the CLI prints a note, the GUI shows a
   banner, and the MCP server returns one, because Steam Cloud will sync the
   edit to every machine on the account. Scratch copies marked with
   `.nmsedit-sandbox` are silent.
10. **Player-state edits are validated before anything changes.** A full grid, an
   occupied cell, an amount outside the slot or an unknown inventory leaves the
   document clean, so a refused edit cannot cause a file to be rewritten. Adding
   an item fills existing stacks first, and stack sizes are measured from the
   save itself rather than guessed from a table.

Details and measurements: [`docs/findings-m0.md`](docs/findings-m0.md),
[`docs/findings-mapping.md`](docs/findings-mapping.md),
[`docs/findings-m2.md`](docs/findings-m2.md),
[`docs/findings-m4.md`](docs/findings-m4.md),
[`docs/format/save-file.md`](docs/format/save-file.md),
[`docs/format/metadata.md`](docs/format/metadata.md),
[`docs/format/player-state.md`](docs/format/player-state.md).

## Layout

```
crates/nms-format   LZ4 block containers, Latin-1 payloads, XXTEA metadata, atomic writes
crates/nms-model    exact-text JSON documents, key mapping, path access, change reporting
crates/nms-edit     load/edit/plan/commit, metadata mirroring, backups, player state
crates/nms-pak      HGPAK reader for the game's own .pak archives
crates/nms-data     regenerates game text from a local installation
crates/nms-cli      the nmsedit binary
crates/nms-gui      the nmsedit-gui window (egui/eframe)
crates/nms-mcp      the MCP server an agent talks to
docs/format         the format notes this implementation is built from
tools/              fixture and data-pipeline scripts
```

## Development

```sh
cargo test --workspace              # unit tests always run
tools/fetch-fixtures.sh <save.zip>  # then the fixture tests run too
cargo run -p nms-cli -- info <save.hg>
cargo run -p nms-gui -- <save-dir>   # or --release; omit the folder to open the default
cargo run -p nms-cli -- mcp          # the MCP server, for agents
```

Fixture tests exercise real saves and skip with a message when none are
present. See [`fixtures/README.md`](fixtures/README.md).

## Roadmap

| Milestone | Content |
|---|---|
| **M1 (done)** | format layer, fidelity guarantees, verification CLI |
| **M2 (done)** | edit/commit with backups, metadata mirroring, change reports, readable export, restore |
| **M3 (done)** | game data pipeline — 84,965 strings regenerated in 137 ms, item names joined from three item tables (94% of a real save's ids named, every answer labelled with its source), and item categories verified against the save's own inventory types |
| **M4 (done)** | player state: currencies, inventory grids and slots, known-item lists — library and CLI, with the measured invariants in [`docs/format/player-state.md`](docs/format/player-state.md) |
| **M5 (done)** | GUI (`egui`): slot browser from metadata, save summary, balances, inventory grids, known-item lists, backup history with restore, staged editing, and the JSON view with exact-path editing |
| **MCP (done)** | `nmsedit mcp`: the editor as an MCP server, so agents read and edit saves through the same backed-up edit layer |
| M6 | other platforms: Xbox Game Pass, PS4, Switch |
| M7 | entity export/import, reward-claim correctness, coordinate tooling |

The window's route to feature parity with the original editor — its fourteen
tabs and three dialogs, mapped to this project's edit layer — is planned in
[`docs/plan-gui-parity.md`](docs/plan-gui-parity.md).

## Licence and provenance

MIT OR Apache-2.0.

Written from scratch against real save files and published format notes. The
reference Rust parser [`nms-save`](https://crates.io/crates/nms-save) (MIT OR
Apache-2.0) was used to cross-check the container and metadata layout. The
field-name baseline compiled into the editor is this project's own derivation
from the game's executable — bare identifiers, not game text — and a community
mapping file distributed with
[MBINCompiler](https://github.com/monkeyman192/MBINCompiler) is merged in when
the player has one; it is never copied into this repository (that project is
LGPL-3.0). No code or data from the GPL/AGPL save editors (NomNom, NMSE) or from
the closed-source Java editor is used — they are consulted as documentation
only, and no game assets are redistributed.

No Man's Sky is a trademark of Hello Games. This is an unofficial, unaffiliated
tool.
