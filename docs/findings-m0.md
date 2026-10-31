# M0 findings: what the real save data proved

The first milestone was a feasibility spike against the maintainer's own save
set. It produced hard numbers that shaped the architecture. Everything below is
reproduced by the test suite in this repository.

## Fixtures

| Item | Count | Notes |
|---|---|---|
| `save*.hg` | 30 | 15 slots × auto/manual, save versions 4735–4743 |
| `mf_save*.hg` | 30 | all 432 bytes, metadata format `0x7D4` |
| `accountdata.hg` + `mf_accountdata.hg` | 2 | account file is plain JSON, not compressed |

Largest payload: 16,073,763 bytes (16 MB of JSON, 751,002 keys, 1,291,351
numeric literals). `save.hg` has 31 LZ4 blocks.

## Four traps that break naive implementations

1. **Payloads are Latin-1.** `serde_json::from_slice` on the raw decompressed
   bytes fails with `invalid unicode code point at line 1 column 8778073`.
   Thousands of bytes ≥ 0x80 exist per save, inside custom names. Decoding must
   be Latin-1 and encoding must map back byte-for-byte.
2. **Numbers are text, not doubles.** 77,321 of 1,291,351 literals (6.0%)
   change text under a standard `f64` round trip, including rewrites into
   scientific notation the game never writes. `arbitrary_precision` fixes this.
3. **Keys are obfuscated.** `"F2P"` is `Version`. Keeping the on-disk spelling
   and mapping only for lookups avoids any lossy reverse-mapping step.
4. **Metadata mirrors content.** Sizes, play time, version word, save name,
   location summary and game mode all live in the metadata file, so editing a
   save without refreshing metadata misdescribes it to the game.

## What the tests now guarantee

* Every data file: re-encoding the container and re-reading it yields the same
  payload and the same stored bytes, including the trailing NUL.
* Every plain JSON file (`accountdata.hg`): byte-exact round trip.
* Every metadata file: decryption + re-encryption reproduces the file
  byte-for-byte.
* Metadata size, play time and version cross-checks against the data files hold
  for all 30 pairs.
* Editing a real 16 MB save changes **zero** untouched numeric literals
  (`edited_save_keeps_untouched_number_text`).

## Corrections to earlier assumptions

* **The container is not byte-reproducible.** Re-compressing produced 4,622,408
  bytes where the game wrote 4,621,727. The payload is identical; the encoder
  output is not. Consequence: an unedited save must not be rewritten at all,
  which is now the implemented behaviour ([`WriteOutcome::Unchanged`]).
* **The metadata version offset is not universal.** `Version − 512` holds for
  player saves; `mf_accountdata.hg` uses `0` with JSON `Version` 4098.
* **`compressed_size == 0` means "uncompressed"**, not "empty": it marks
  `accountdata.hg` / `usersettings.hg`, whose container is plain JSON.

## Still open

* Identify metadata words 2..9 (`mf_accountdata.hg`) and 86..88.
* Confirm in game that an edited save loads and shows the edit — the one check
  that cannot be done offline.
* Verify whether the game validates any metadata digest.
