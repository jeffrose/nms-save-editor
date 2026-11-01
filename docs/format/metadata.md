# Metadata files (`mf_*.hg`)

Every data file has a 432-byte companion (earlier formats: 104, 360, 384 bytes)
that the game reads to describe save slots without decompressing the save. It
is XXTEA-encrypted with a key derived from the storage slot.

## Encryption

* Word width: 32-bit little-endian `u32`.
* Rounds: **8** for the 104-byte vanilla layout, **6** for every later layout.
* Key words 0..3, where `slot` is the storage index (see the save-file notes):

```
key[0] = rotate_left(slot ^ 0x1422CB8C, 13) * 5 + 0xE6546B64   (wrapping)
key[1] = 0x44415645
key[2] = 0x5259414E
key[3] = 0x47524E54
```

The cipher is a fixed-round variant of XXTEA (`delta = 0x9E3779B9`) and is
deterministic, so **encrypting an unmodified word array reproduces the original
file byte for byte** — verified across all 31 fixture metadata files. That is
what makes "patch the words you understand, copy the rest" safe.

The first decrypted word is a sentinel, `0xEEEEEEBE`. Because only the first
key word varies with the slot, a metadata file can be recognised as belonging
to an account-level slot (index 0–1) or a player slot (2–31) even when the
expected slot is wrong.

## Word layout (432-byte format `0x7D4`)

| Word(s) | Meaning | Observed |
|---|---|---|
| 0 | sentinel | `0xEEEEEEBE` |
| 1 | format version | `0x7D0` vanilla, `0x7D1` foundation, `0x7D2`–`0x7D4` later |
| 2..9 | 32-byte blob | zero for player saves; non-zero in `mf_accountdata.hg` (see unknowns) |
| 14 | decompressed size | payload length + 1 (trailing NUL) |
| 15 | compressed size | raw data file size; `0` = stored uncompressed |
| 16 | profile hash | `0` in every observed file |
| 17 | save version | JSON `Version` − 512 (player saves) |
| 18 | flag | `1` in every observed file |
| 19–20 | play time, seconds | low/high word; equals JSON `TotalPlayTime` |
| 21 | decompressed size (again) | duplicates word 14 |
| 22.. | save name, UTF-8, NUL padded | `Utopia`, `Our Journey Continues`; empty for unnamed saves |
| 54.. | location summary, UTF-8, NUL padded | `Aboard Laracu XVII Station Beta`, `In the Fasyrini system`; equals JSON `PlayerStateData.SaveSummary` |
| 89 | timestamp | unix seconds; matches the data file's mtime (within a second or two) |
| 91.. | game mode, UTF-8, NUL padded | `Normal`; matches JSON `DifficultyState.*.DifficultyPresetType` |

Words not listed are zero in every observed file. Text fields start where shown
and end at their NUL: the longest name seen is 23 bytes and the longest location
34 bytes, so the declared capacities (32 / 40 / 16 bytes) are the observed
extents rounded up, and writing treats them as a ceiling rather than a fact.

## What a writer must do

Every field in the table above is derived from the save and rewritten on commit:
sizes, version, play time, save name, location, game mode and timestamp. Leaving
any of them stale is the reported "metadata size and cloud sync state differ
from what the game syncs" failure mode — the game draws its slot list from this
file, not from the save.

Rules the implementation follows:

* **Mirror, never regenerate.** Unknown words (2..9, 86..88, everything not in
  the table) are copied verbatim because the cipher is deterministic.
* **Metadata is only written alongside a new data file.** An unedited save is
  left completely alone, metadata included.
* **A string may only extend into empty words.** Overwriting a word that holds
  something other than the current string aborts that field, which is then
  reported as a warning instead of blocking the commit.
* **Sizes describe the file that was actually written**, including the
  re-encoded container size.

`PlayerStateData` is reached through a context — `BaseContext` for normal saves,
`ExpeditionContext` for expedition saves — so the mirror reads the first of
those two paths that resolves.

## Unknowns

* **Words 2..9 in `mf_accountdata.hg`** hold 32 non-zero bytes (plus four more
  words, 10..13). They are not SHA-256, SHA-384, SHA-512 or MD5 of
  `accountdata.hg`, and they survive unmodified. They are preserved verbatim
  until identified.
* **Words 86..88** are non-zero in some files only (`mf_save.hg`: word 86 = `2`,
  words 87–88 = an 8-byte value). Possibly a checksum or an id tied to the
  current station; preserved verbatim.
* Whether the game *validates* any of these values is untested. Testing an
  edited save in game is the verification step for that.
