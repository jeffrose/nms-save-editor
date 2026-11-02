# AGENTS.md — nms-save-editor

A Rust re-implementation of the No Man's Sky save editor. Workspace conventions
live in `~/Workspace/AGENTS.md`; this file adds project-specific rules.

## Build and test

```sh
cargo test --workspace                    # unit tests (always run)
./tools/fetch-fixtures.sh <save.zip>      # install fixtures (see fixtures/README.md)
cargo test --release --workspace          # includes the fixture tests
cargo run -p nms-cli -- info <save.hg>
```

Note: this toolchain is a system Rust install without `clippy`/`rustfmt`
components, so lints and formatting are maintained by hand. Keep code
`cargo build --workspace` clean with zero warnings.

Fixture tests skip with a message when `fixtures/` is empty, so a clean clone
passes without them. They are the load-bearing tests: run them in release mode
(a full set is ~40 MB of JSON and compression).

## Live save folders are off-limits

**Do not write to the machine's real game save folders, and do not use them as
test input.** On this machine that means anything under
`~/.steam/debian-installation/steamapps/compatdata/275850/...` and
`.../AppData/Roaming/HelloGames/NMS/...`. Reading them only to confirm they are
still untouched is fine.

The reason is Steam Cloud: it syncs that prefix, so a test edit is uploaded and
lands on every machine signed into the account. There is no undo inside this
process, and the failure appears on hardware we cannot reach.

Work on copies instead:

* `fixtures/` holds real saves for reading; tests copy them to temp directories.
* `tools/sandbox-test.sh` runs the game against a throwaway prefix and declares
  that tree a scratch copy with a `.nmsedit-sandbox` marker.
* **The live-folder guard was dropped on 2026-11-02, at the user's explicit
  request**, once every write took a timestamped backup and verified its own
  planned bytes before touching the disk. Writes to the game's folder now go
  through; `nms_edit::live_notice` still classifies the path so the CLI, the
  GUI and the MCP server can say out loud that Steam Cloud will sync it.
* Be careful with tests anyway: they must not edit the real save folder. Use
  `fixtures/` copies or `tools/sandbox-test.sh`, which marks its throwaway
  prefix with `.nmsedit-sandbox`.

## The invariant that must not break

**An unedited save is never modified, and an edited save changes only what was
edited.** Concretely:

* `SaveFile::write` returns `WriteOutcome::Unchanged` and touches nothing when
  `!is_modified()`.
* `Document::payload()` returns the original bytes when the document is not
  dirty.
* Numbers are parsed with `arbitrary_precision` and never reformatted; keys are
  never re-spelled; payloads are Latin-1 at the byte boundary.
* Metadata is **patched**, never regenerated — unknown words are copied, and it
  is only written when the data file itself was rewritten.
* A mirror string may only extend into words that are currently zero; if it
  cannot be written safely the field is dropped with a warning, never forced.

If a change would relax any of these, stop and raise it with the user first.
Any new write path needs a fixture test proving untouched data is byte-identical.

Two more rules the code enforces, not just documents:

* **A write is verified before it happens.** `SaveBundle::commit` decodes and
  parses its own planned bytes and refuses to write if they do not read back.
  Do not add a path to disk that skips it — it is the only thing standing
  between an encoder bug and a save the game cannot open.
* **Backups are not optional in practice.** The CLI, the GUI and the MCP server
  all pass a `backup_root`, so every write has a restore point; keep it that
  way when adding a caller.

## Conventions

* Crates: `nms-format` (bytes), `nms-model` (documents, paths, change reports),
  `nms-edit` (load/edit/plan/commit, metadata mirroring, backups), `nms-pak`
  (the game's .pak container), `nms-data` (regenerating game text from an
  installation), `nms-cli`, `nms-gui` (later). Keep layers separate: no game
  knowledge in `nms-format`, no byte handling in `nms-model`, no CLI concerns in
  `nms-edit`, and no game-text parsing in `nms-pak`.
* Reading the game installation is fine and expected (it is how data packs are
  built); writing to it never happens. Save folders are still off-limits — see
  the section above.
* Errors: `thiserror` enums per crate, no `anyhow` in libraries, no `unwrap` on
  file data. `panic!` is acceptable in tests.
* Doc comments explain *why* a format detail exists (with the observation that
  proved it), not just what the code does. Format claims need a fixture or a
  test to back them.
* Never commit anything from `fixtures/` — it is git-ignored and holds personal
  data. Test data that must be tracked is synthetic and built in code.
* Licence hygiene: MIT/Apache-2.0 sources only. NomNom, NMSE (GPL/AGPL) and the
  closed-source Java editor may be read for format facts, never copied, and no
  Hello Games assets (icons, text tables) get committed — those are generated
  locally from the player's install.
* The one game-derived file that *is* committed is
  `crates/nms-model/data/baseline-mapping.json`: field-name pairs our own tool
  derives from the game's executable (bare identifiers, no game text), so the
  editor names fields on a machine with no game installed. Regenerate it with
  `tools/build-baseline-mapping.sh` when the fixtures or the game change. Never
  copy MBINCompiler's `mapping.json` (LGPL-3.0) into the tree — a copy the
  player has installed is merged at run time only.

## Where the knowledge lives

`docs/format/` is the written spec (containers, Latin-1 payloads, metadata word
layout) and `docs/findings-m0.md` records the measurements behind the design
decisions. Update them when an observation changes; they are the project's
memory of *why* the code is shaped this way.
