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
* The edit layer refuses a live location unless `confirm_live` /
  `--confirm-live` is passed (`crates/nms-edit/src/safety.rs`). Do not pass it
  without asking the user first.
* This rule relaxes only when the user says we have enough confidence.

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

## Conventions

* Crates: `nms-format` (bytes), `nms-model` (documents, paths, change reports),
  `nms-edit` (load/edit/plan/commit, metadata mirroring, backups), `nms-cli`,
  `nms-gui` (later). Keep layers separate: no game knowledge in `nms-format`, no
  byte handling in `nms-model`, no CLI concerns in `nms-edit`.
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

## Where the knowledge lives

`docs/format/` is the written spec (containers, Latin-1 payloads, metadata word
layout) and `docs/findings-m0.md` records the measurements behind the design
decisions. Update them when an observation changes; they are the project's
memory of *why* the code is shaped this way.
