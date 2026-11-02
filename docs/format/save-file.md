# NMS save file format (as observed)

Everything here is verified against real files: 30 `save*.hg` data files, 30
`mf_save*.hg` metadata files, `accountdata.hg` and `mf_accountdata.hg` from a
Steam installation on game build 7.x (save version words 4735–4743). Where
something is inferred rather than confirmed, it says so.

## 1. Where files live

| Platform | Layout |
|---|---|
| Steam / GOG | One directory per account (`st_<steamid>`, `gog_<id>`) containing `save.hg`, `save2.hg` … `save30.hg`, one `mf_save*.hg` per data file, plus `accountdata.hg`, `usersettings.hg` and their `mf_` companions |
| Xbox Game Pass | `containers.index` plus GUID-named blob directories |
| PS4 | `memory.dat` monolith, or streaming `savedataNN.hg` + `manifestNN.hg` |
| Switch | Streaming `savedataNN.hg` + `manifestNN.hg` |

Only the Steam/GOG layout is implemented so far. The slot mapping is fixed:

| File | Storage slot | Notes |
|---|---|---|
| `save.hg` | 2 (`PlayerState1`) | game slot 1, Autosave |
| `save2.hg` | 3 (`PlayerState2`) | game slot 1, Restore Point |
| `saveN.hg` | N+1 | `N` runs 1..30, i.e. 15 slots × 2 save points |
| `accountdata.hg` | 1 (`AccountData`) | account unlocks and rewards |
| `usersettings.hg` | 0 (`UserSettings`) | options |

### Two save points per slot

A game slot is two files, not one: `save.hg` and `save2.hg` are slot 1's two
save points — the game's **Autosave** and its **Restore Point** (the manual
save) — two states of the same game at different moments, not two halves of one
file. Fifteen slots × two save points = the thirty `save*.hg` files, which is
why a folder listing looks doubled. The game's own load menu labels the two
(`Reload Autosave` / `Reload Restore Point`, the text-table ids
`UI_RELOAD_AUTO` / `UI_RELOAD_MANUAL`) and loads the newer one by default; the
older one is the previous save point.

Measured across the 15 pairs of the reference save set:

* the two files of a pair always share the save name, game mode and location,
  and differ in play time and timestamp by whatever was played between the two
  saves — seconds in some pairs, weeks in others;
* **nothing in either file says which is auto and which is manual.** All 96
  decrypted metadata words were compared between the halves of every pair: not
  one word separates them, and a full structural diff of the two JSON payloads
  finds no type field either — only game state (positions, timers, inventories).
  The timestamp (metadata word 89) is what orders the pair;
* the role is therefore the **file name**, and the rule — the community's,
  relied on by every save tool, and matching the game's own labels — is: the
  odd file (`save.hg`, `save3.hg`, …) is the **Autosave**, the even one
  (`save2.hg`, `save4.hg`, …) the **Restore Point**. It is the one inference in
  this document: nothing inside a file can confirm it, and nothing conflicts
  with it. If a future build changed it, it is one function to change
  (`SavePointKind::of_player_number`).

That is why `FoundSave` carries `save_point`, `newest`, `paired_path` and
`paired_time`, and why the editor groups a game slot's two files instead of
listing thirty separate slots.

## 2. Data files: two containers, one payload

### Block stream (`save*.hg`)

A sequence of blocks, each with a 16-byte little-endian header:

```
0x00  u32  magic             0xFEEDA1E5
0x04  u32  compressed_size   bytes of LZ4 payload after the header
0x08  u32  decompressed_size <= 0x80000 (512 KiB)
0x0C  u32  reserved          zero in every observed file
0x10  ...  LZ4 block payload (raw block format, not a frame)
```

Blocks concatenate into one payload. Real files use exactly 512 KiB
decompressed chunks (16 MB save → 31 blocks), and the last block is short.

### Plain JSON (`accountdata.hg`, `usersettings.hg`)

No compression and no headers: the file *is* the payload. Detected by a first
byte of `{` (the block magic can never start with `0x7B`).

### Payload encoding

* **Latin-1, not UTF-8.** Every byte is one character code. Real saves contain
  hundreds to thousands of bytes ≥ `0x80` (custom names). Decoding as UTF-8
  fails outright on most saves — `serde_json::from_slice` returns
  `invalid unicode code point`. Decode/encode through
  [`crates/nms-format/src/latin1.rs`](../../crates/nms-format/src/latin1.rs).
* **One trailing NUL byte.** Present in every observed file, and metadata's
  `decompressed_size` counts it.
* **JSON is compact** (no whitespace) and **object keys are obfuscated**:
  `"F2P"` is `Version`, `"8>q"` is `Platform`, `"<h0"` is `CommonStateData`.
  Real saves use this spelling, so the on-disk keys are treated as the source
  of truth and mapping is only applied when looking values up by name.

### Field notes

| JSON path (readable) | Example | Notes |
|---|---|---|
| `Version` | `4743` | save format word; metadata word 17 + 512 |
| `Platform` | `Win\|Final` | |
| `ActiveContext` | `Main` | `Season` for expedition saves |
| `CommonStateData.SaveName` | `Utopia` | empty when the player never named the save |
| `CommonStateData.TotalPlayTime` | `3156876` | seconds; mirrored in metadata word 19 |
| `PlayerStateData.SaveSummary` | `Aboard Laracu XVII Station Beta` | slot-list text; mirrored in metadata |
| `PlayerStateData.DifficultyState.*.DifficultyPresetType` | `Normal` | mirrored in metadata |

## 3. Fidelity rules

The project's core promise, and why each rule exists:

1. **Unedited payloads are byte-identical.** The document layer returns the
   original bytes when nothing was edited, and unedited files are never
   rewritten at all (mtime and cloud-sync state untouched).
2. **Numbers keep their exact text.** About 6% of numeric literals in a real
   save (77,321 of 1,291,351 in one 16 MB file) change under a naive `f64`
   round trip. Examples:
   `0.30000001192092898` → `0.30000001192092896`, and
   `0.000031987179681891578` → `3.1987179681891575e-05`.
   The game writes plain decimal where standard serialisers emit scientific
   notation, so the tree is parsed with `arbitrary_precision`.
3. **The compressed container is not byte-reproducible.** The game's LZ4
   encoder output cannot be replicated; a re-encoded container is valid but its
   bytes differ (observed: 4,622,408 vs 4,621,727 bytes). This is harmless —
   the game decompresses rather than compares — but it is why "no edit means no
   write" matters.
4. **Unknown fields survive.** Nothing is dropped or reordered: key order and
   all unmodelled keys are preserved.

One residual cosmetic difference for *edited* saves: `serde_json` writes
`\u001a` where the game wrote `\u001A` (52 escapes in 16 MB, 8 of them
uppercase). It is semantically inert, and irrelevant for unedited saves because
those are not re-serialised.

## 4. Cross-file invariants (enforced by tests)

* metadata `decompressed_size` == payload length + 1 (the NUL)
* metadata `compressed_size` == raw data file size, or `0` when the file is
  stored uncompressed
* metadata `play_time` == JSON `CommonStateData.TotalPlayTime`
* metadata `version` == JSON `Version` − 512 (player saves only)
