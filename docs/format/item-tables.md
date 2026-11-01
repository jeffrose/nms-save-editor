# Item tables and item names

A save stores **ids**, never names: inventory slots hold `^CARBON_SEAL`,
`^FUEL2`, `^MAINT_REFINER`. Turning those into text needs three things — the
item table that maps an id to a name key, the text table that maps a key to
words, and rules for the ids that have neither.

## Where the data lives

| Data | File |
|---|---|
| Products (most inventory items) | `metadata/reality/tables/nms_reality_gcproducttable.mbin` (2.1 MB) |
| Substances (elements) | `metadata/reality/tables/nms_reality_gcsubstancetable.mbin` |
| Technologies | `metadata/reality/tables/nms_reality_gctechnologytable.mbin` |
| Procedural technologies | `metadata/reality/tables/nms_reality_gcproceduraltechnologytable.mbin` |
| Text | `language/nms_loc*_<language>.mbin` (see [localisation](localisation.md)) |

All of them are inside `NMSARC.Precache.pak` and `NMSARC.MetadataEtc.pak`.

## The product table is a grid

```text
record + 0x000  "UI_CRAFT_OBJ"            fixed 64-byte text slot
record + 0x040  "UI_PIN_<ID>_OBJ_TIP"     fixed 32-byte text slot
record + 0x0C0  <ID>                      fixed 16-byte text slot
record + 0x160  <name key>                fixed 128-byte text slot
record + 0x1E0  <name key>_L              lower-case variant
stride          768 bytes
```

Records are walked on that grid. A position whose id or name slot does not hold
plausible text is skipped rather than shifting everything after it, so a record
with a different shape costs one entry instead of corrupting the rest.

**The resolution rate is the self-check.** Every extracted key is looked up in
the text tables; in the observed build 2,188 of 2,261 keys resolve (96%). A
layout change makes that number collapse, which is a loud failure rather than
wrong names.

## Why the key cannot just be derived from the id

Three real cases, all present in the game's data:

| Save id | Name key | Note |
|---|---|---|
| `CASING` | `CASING_NAME` | the common case |
| `ANTIMATTER` | `COMMODITY6_NAME` | commodities use their own naming |
| `BIOSHIP_COMP5_0` | `UI_BIOSHIP_COMP5_NAME0` | procedural ship parts |
| `FUEL2` | `UI_FUEL_2_NAME` | a trailing number gains an underscore |
| `OXYGEN` | `UI_AIR1_NAME` | **the save's id is not the table's id** |

That last row is the substance namespace: the save calls the element `OXYGEN`
while the substance table's id is `AIR1` and its wiki key is `WIKI_OXYGEN`.
Resolving those needs the substance table's own fields, which is the next step.

## What the editor shows today

Given the 1,108 distinct item ids in a real save, `nmsedit data item` produces
text for 866 of them (78%), and each answer says where it came from:

| Source | Count | Meaning |
|---|---|---|
| custom name | 425 | the id field holds a generated name, which *is* the display text |
| item table | 282 | a real join (`CASING` → `METAL PLATING`) |
| derived key | 80 | `<id>_NAME`, `UI_<id>_NAME`, or the trailing-number form |
| placeholder | 66 | `MAINT_*`: a damaged or empty slot, labelled as such |
| direct key | 2 | the id is itself a text key |

Nothing is invented: an id that resolves to no text is reported as unresolved
rather than shown as a guess.

## Next

* **Substance and technology tables** — different record layouts (the technology
  table's stride is 736 with the name key 0xBB after the id) to close the
  remaining ~240 ids.
* **Item metadata beyond names**: category, base value, stack size, and the
  technology adjacency rules, all of which live in the same tables.
