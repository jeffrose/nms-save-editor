# M2 findings: metadata mirrors save content, and two test-caught bugs

M2 added the write path: load a save with its metadata, edit, back up, commit,
and refresh the metadata so the game's slot list still describes the save.
Everything here is enforced by tests in `crates/nms-edit/tests/fixtures.rs`.

## Metadata is a content mirror, not just sizes

The metadata file carries a summary of the save that the game reads to draw the
slot list. Re-deriving it from the save JSON and comparing against the game's
own file — for all 30 fixture slots — matched on every word except the ones we
expect to differ:

| Word(s) | Content | Source in the save JSON |
|---|---|---|
| 14, 21 | decompressed size | payload length + 1 (trailing NUL) |
| 15 | compressed size | raw file size, `0` when stored uncompressed |
| 17 | version | `Version` − 512 |
| 19–20 | play time | `CommonStateData.TotalPlayTime` |
| 22..29 | save name | `CommonStateData.SaveName` |
| 54..61+ | location | `PlayerStateData.SaveSummary` |
| 89 | timestamp | data file mtime |
| 91..92 | game mode | `DifficultyState.Preset.DifficultyPresetType` |

Only word 15 differed, and for the right reason: the container is re-encoded
rather than byte-reproduced, so its size changes. That test is deliberately
non-vacuous — it asserts each mirror field was actually derived, and that the
derived value equals what the game stored.

`PlayerStateData` sits inside a context: `BaseContext` for normal saves and
`ExpeditionContext` for expedition saves, so both paths are probed.

## Mirror strings are written conservatively

Field widths are inferred from observation (a 23-byte name, a 34-byte location
string, a 6-byte mode), and a wrong guess must never corrupt a save. So a string
write may only extend into words that are currently zero: words the existing
string already occupies can be overwritten and its tail cleared, but a word
holding anything else aborts the write with `MetadataFieldOccupied`. If that
happens the field is dropped and reported as a warning instead of blocking the
whole commit.

For example, `Permadeath` (10 bytes) exceeds the observed 8-byte extent of the
game-mode field. Rather than guess, the writer reports
`metadata field GameMode not refreshed: ...` and leaves the field alone.

## Two bugs the tests caught

Both were design errors, not typos:

1. **Metadata could be written without the data file.** The first commit path
   refreshed metadata independently, so a save with no edits got a rewritten
   metadata file claiming a compressed size for a file that had not changed.
   Fixed by making metadata write strictly conditional on a new data file being
   written — an unedited save is now untouched, metadata included.

2. **The change report was measuring the wrong thing.** Byte-level diffing by
   position reports ~9 MB of "changes" for a six-byte edit, because everything
   after a shorter field shifts. `ByteDiff` now also reports the changed region
   (common prefix/suffix), and the user-facing report is a semantic JSON tree
   diff: `CommonStateData.TotalPlayTime: 3156876 -> 1`.

## Still open

* Identify metadata words 2..9 (account files) and 86..88 (per-slot, stable).
* Confirm in game that an edited save loads: the only check that cannot be done
  offline, and the gate on trusting the mirror-string writes.
