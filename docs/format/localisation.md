# Localisation tables

`language/nms_loc*_<language>.mbin` and `language/nms_update*_<language>.mbin`
inside `NMSARC.MetadataEtc.pak` hold the game's text: 9 tables per language,
and 17 languages in the installed build — 153 tables, 1,444,405 entries.

## Layout

```text
0x00  8 bytes  MBIN magic (0xCCCCCCCC twice)
0x08  u32      template version (0x0CE4 in the observed build)
0x0C  16 bytes template hash
0x28  u32      entry count
0x30  entries  entry_count × 0x130 bytes:
         +0x00  id, NUL-terminated, fits in 0x20 bytes
         +0x20  value slots: one per language, 16 bytes each, in the game's
                language order (alphabetical by file name):
                  +0x00  u64  value offset, relative to the slot's own position
                  +0x08  u32  value length including the terminating NUL
                  +0x0C  u32  marker (0xAAAAAA01)
       then   the value blob: each non-empty value's text plus its NUL, in
              entry order, packed with no padding
```

**Each file fills only its own language's slot.** The slot order is the game's
language order, which is the file-name order alphabetically: `0`
brazilianportuguese, `1` dutch, `2` english, `3` french, `4` german, `5`
italian, `6` japanese, `7` korean, `8` latinamericanspanish, `9` polish, `10`
portuguese, `11` russian, `12` simplifiedchinese, `13` spanish, `14`
tencentchinese, `15` traditionalchinese, `16` usenglish.

English is slot 2 (`+0x40`), which is why a parser that assumed the first slot
read English correctly and every other language as empty strings — the bug this
layout was worked out to fix. The parser therefore *detects* the filled slot
(the offset and length bytes, not the marker: every slot carries the marker
whether it is filled or not) instead of assuming one.

An entry whose slot length is `0` contributes nothing to the blob; a few are
intentionally empty, and one table is genuinely sparse (tencentchinese's loc10
ships 2 translated strings of 2,505 — the game falls back for the rest). Text
is **UTF-8** — a handful of ids have typographic apostrophes (`EXP_VY’KEEN`)
and 90 English values contain `°`, `©`, `À`, `õ` … — unlike save payloads,
which are Latin-1 and would mis-decode such bytes.

## Why the parse can be trusted

Three invariants are checked on every table, and all three hold on all 153:

1. **The blob is consumed exactly to the end of the file.** The last value's
   NUL is the file's last byte. A wrong stride, count, length or slot would
   leave leftovers.
2. **Every length field matches the text it points at**, and no id repeats.
3. **Every stored offset equals the value's position minus the slot's own
   position.** Counted, not fatal: it corroborates the walk, which needs only
   the lengths.

A parse that satisfies them across 1.44M entries cannot be reading the wrong
bytes: the entry table and the blob are two independent parts of the file that
have to agree. The parser also fails loudly rather than guessing — see the
error cases in `crates/nms-data/src/language.rs`.

## Regenerating in a game update

```sh
nmsedit data refresh --language english      # 137 ms for the whole install
nmsedit data refresh --language german       # any of the game's languages
nmsedit data info
nmsedit data lookup UI_GUIDE_TITLE NOOXYGEN_L
```

The pack is written to `~/.config/nmsedit/strings.json` (8.1 MB for English)
and stays on the machine: no game text is redistributed, and a game update is a
re-run rather than a wait for someone else to publish a data file. That is the
whole point of the pipeline — the older editor's dominant complaint was months
of "name mapping not found" after each patch.

The ids are language-independent — the same 84,965 in every language — and only
the values change. Item names follow the same rule; how ids become words is
[item-tables.md](item-tables.md).
