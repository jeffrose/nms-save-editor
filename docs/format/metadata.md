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
| 22..29 | save name, UTF-8, NUL padded | `Utopia`, `Our Journey Continues`; empty for unnamed saves |
| 54..61 | location summary, UTF-8, NUL padded | `Aboard Laracu XVII Station Beta`, `In the Fasyrini system`; equals JSON `PlayerStateData.SaveSummary` |
| 89 | timestamp | unix seconds; matches the data file's mtime (within a second or two) |
| 91..92 | game mode, UTF-8, NUL padded | `Normal`; matches JSON `DifficultyState.*.DifficultyPresetType` |

Words not listed are zero in every observed file.

## What a writer must do

Sizes and the version word are derivable, so a correct write refreshes at least:
words 14, 15, 17, 19, 20 and 21. Words 22–29, 54–61 and 91–92 mirror editable
JSON fields (`SaveName`, `SaveSummary`, difficulty preset), so a save that
changes those must refresh them too or the slot list goes stale — that is the
reported "metadata size and cloud sync state differ from what the game syncs"
failure mode. Word 89 tracks the file timestamp.

The current implementation patches words 14, 15, 17, 19 and 20 and preserves
everything else byte-for-byte. Deriving the mirrored strings is the next
metadata milestone and is listed in the README roadmap.

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
