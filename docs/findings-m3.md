# M3 findings: the game's own data as the source of truth

M3 replaced "someone maintains a list of names by hand" with "read the player's
installation". Everything below was measured on the installed game (build 7.x,
24 archives, 32 GB of `PCBANKS`).

## What the pipeline does

```
GAMEDATA/PCBANKS/NMSARC.*.pak          nms-pak
  language/nms_loc*_<language>.mbin    nms-data::language
    DataPack { id -> text }            nms-data::pack
```

```console
$ nmsedit data refresh
installation    .../common/No Man's Sky
archives        .../GAMEDATA/PCBANKS

  NMSARC.MetadataEtc.pak          9 tables    84965 entries  exact

wrote 84965 strings (english) to ~/.config/nmsedit/strings.json
pack size       8129277 bytes
```

**137 ms** for the whole install, indexed lazily: opening the 743 MB mesh archive
takes 2 ms and no archive is ever held in memory.

## Format facts established here

* **HGPAK v2 works as documented**, with three corrections that each broke the
  first implementation:
  * compressed chunks are **16-byte aligned**, and the declared size excludes the
    padding;
  * chunks are addressed through **one virtual space of 0x10000-byte chunks**;
  * some archives store payloads **verbatim** (texture archives mix raw and zstd
    chunks; the audio archive sets `is_compressed = 0`).
* **The localisation table layout** (see `docs/format/localisation.md`):
  0x130-byte entries, an id in the first 0x40 bytes, a length at +0x48 that
  counts the NUL, and a blob of NUL-terminated UTF-8 values packed in entry
  order. An empty value has length 0 and contributes no bytes.
* **MBIN text is UTF-8**, while save payloads are Latin-1. Two different
  encodings in the same game, which is exactly the kind of thing an editor has to
  get right per file type rather than globally.

## Why the numbers matter

Nine tables, 84,965 entries, and two independent invariants hold on all of them:
the value blob ends exactly at the file's last byte, and every length field
matches its text. The entry table and the blob are separate regions, so agreement
between them is evidence rather than coincidence — a wrong stride or count cannot
satisfy both.

## Blocked on (next)

* **Item names.** They are not in the language tables. The save holds ids and the
  display names live in `metadata/reality/tables/nms_reality_gc*table.mbin`
  (~87 tables in `NMSARC.Precache.pak`), which are structured MBIN records and
  need a field-level MBIN reader rather than a string-table reader.
* **Metadata archives reference a second data region.** 2,614 of 50,805 entries
  in `NMSARC.MetadataEtc.pak` point past the chunk region; reads of those fail
  with a clear error. Nothing in the data pack lives there.
