# Localisation tables

`language/nms_loc*_<language>.mbin` and `language/nms_update*_<language>.mbin`
inside `NMSARC.MetadataEtc.pak` hold the game's text: 9 English tables, 84,965
entries in the installed build.

## Layout

```text
0x00  8 bytes  MBIN magic (0xCCCCCCCC twice)
0x08  u32      template version (0x0CE4 in the observed build)
0x0C  16 bytes template hash
0x28  u32      entry count
0x30  entries  entry_count × 0x130 bytes:
         +0x00  id, NUL-terminated, fits in 0x40 bytes
         +0x40  u64, purpose unknown (not a hash of the id; unused)
         +0x48  u32, value length including the terminating NUL
       then   the value blob: each non-empty value's text plus its NUL, in
              entry order, packed with no padding
```

An entry whose length field is `0` contributes nothing to the blob. Text is
**UTF-8** — a handful of ids have typographic apostrophes (`EXP_VY’KEEN`) and 90
English values contain `°`, `©`, `À`, `õ` … — unlike save payloads, which are
Latin-1 and would mis-decode such bytes.

## Why the parse can be trusted

Two invariants are checked on every table, and both hold on all nine:

1. **The blob is consumed exactly to the end of the file.** The last value's NUL
   is the file's last byte. A wrong stride, count or length would leave leftovers.
2. **Every length field matches the text it points at**, and no id repeats.

A parse that satisfies both across 84,965 entries cannot be reading the wrong
bytes: the entry table and the blob are two independent parts of the file that
have to agree. The parser also fails loudly rather than guessing — see the error
cases in `crates/nms-data/src/language.rs`.

## Regenerating in a game update

```sh
nmsedit data refresh --language english      # 137 ms for the whole install
nmsedit data info
nmsedit data lookup UI_GUIDE_TITLE NOOXYGEN_L
```

The pack is written to `~/.config/nmsedit/strings.json` (8.1 MB for English) and
stays on the machine: no game text is redistributed, and a game update is a
re-run rather than a wait for someone else to publish a data file. That is the
whole point of the pipeline — the older editor's dominant complaint was months of
"name mapping not found" after each patch.

## Still missing: item names

Item names are **not** in these tables. The save stores item ids
(`ANTIMATTER`, `CARBON_SEAL`, `HYPERFUEL1`, `^`-prefixed in the payload), and the
display name for each comes from the item tables — `metadata/reality/tables/
nms_reality_gc*table.mbin` inside `NMSARC.Precache.pak` (87 table files, including
`nms_reality_gcproceduraltechnologytable.mbin`). Those are structured MBIN
tables, not string tables, so they need a real MBIN field reader; that is the
next step, and it is what turns "id" into "Sulphur" in the UI.
