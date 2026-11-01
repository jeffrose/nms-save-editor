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

## Each table is a grid, and the name key is at a fixed offset

```text
                 stride   id at   name key at     records   named
products          768     +0xC0    +0x160          2,206    96%
substances        416     +0x88    +0xF4             114    71%
technologies      736     +0x138   +0x1F4            393    94%
```

The offsets are *fixed*: with a correct grid, every resolved record in each table
puts its name key at exactly the same distance after the id (verified by the
offset histogram of all 2,131 + 82 + 392 resolved records). The apparent variation
that a naive scan sees comes from the bytes *before* the key — a float's last byte
is usually `'A'`, so a scanner happily invents keys like `APROTECT_NAME`. Reading
the slot at a known offset avoids that entirely.

Each table's record count is declared in its header (`0x28` for products and
technologies, `0x38` for substances), so a walk stops exactly at the end of the
item array and never reads whatever follows it as items.

**The resolution rate is the self-check.** Every key read from a table is looked
up in the text tables, and only a key that exists is accepted: a wrong offset
produces *fewer* names, never wrong ones. A layout change collapses the rate,
which is a loud failure.

## The locate-rules, in the order they are tried

The key is not derivable from the id in general, so a handful of conventions are
tried in order, each accepted only if the text tables actually have it:

| Save id | Resolves via | Text |
|---|---|---|
| `CASING` | `CASING_NAME` | METAL PLATING |
| `ANTIMATTER` | `COMMODITY6_NAME` (from the product table) | ANTIMATTER |
| `OXYGEN` | `UI_AIR1_NAME` (from the substance table) | OXYGEN |
| `FUEL2` | `UI_FUEL_2_NAME` (trailing number separated) | CONDENSED CARBON |
| `UT_PROTECT` | `UT_PROTECT_NAME` (from the technology table) | SHIELD LATTICE |
| `BASE_BONEGARDEN` | `BLD_BONEGARDEN_NAME` | RATTLE SPINE |
| `BASE_BUBBLECLUS` | `BLD_BUBBLECLUSTER_NAME` (truncated stem) | BUBBLE CLUSTER |
| `BUILDBEACON` | `BLD_BEACON_NAME` | SAVE BEACON |
| `MAINT_REFINER` | the technology table's own key | REFINERY OUTPUT |
| `B_SHL_A` | `BLD_BIG_SHL_A_NAME` (corvette module) | DEFLECTOR SHIELD |
| `B_COK_C4` | `BLD_BIG_COK1X2_C4_NAME` (size infix) | VIPER-I-CLASS COCKPIT |
| `FIGHT_COCKAA` | `UI_FIGHTER_COCK_AA_NAME` | OMEGA FUSELAGE |
| `SAIL_BODYA` | `UI_SAIL_BODY_A_NAME` | RAVEN COCKPIT |
| `BUILD_REFINER1` | `REFINER1_NAME` | PORTABLE REFINER |

Every rule is a *candidate*: it is accepted only when the text tables contain the
key, so a convention that changes with a game update loses names rather than
inventing them.

## What the editor shows today

Given the 1,108 distinct item ids in a real save, `nmsedit data item` produces
text for **1,038 of them (93%)**, and each answer says where it came from:

| Source | Count | Meaning |
|---|---|---|
| item table | 484 | a join from products, substances or technologies |
| custom name | 426 | the id field holds a generated name, which *is* the display text |
| derived key | 99 | one of the locate-rules above |
| placeholder | 19 | a damaged or empty slot with no name anywhere |

Nothing is invented: an id that resolves to no text is reported as unresolved
rather than shown as a guess.

Damaged components deserve a note: the technology table names them itself
(`MAINT_REFINER` → REFINERY OUTPUT, `MAINT_TECH5` → BIO-INPUT SENSOR), so the
game's own text wins and the placeholder label is only the fallback. That is the
opposite of what a heuristic would do, and it is why the data-driven order
matters.

## Categories, and what metadata is still missing

**Categories are extracted and verified.** The pack records `Product`,
`Substance` or `Technology` for every id that came from an item table, and that
distinction can be checked against a real save, which stores its own
`InventoryType` per slot: **3,351 slots agree, 0 disagree.**

Two other kinds of metadata are *not* extracted yet, and both are recorded here
with the evidence rather than guessed at:

* **Base value (units).** The save does not store it, so there is no oracle to
  check an extracted number against. `costtable.mbin` is not it either — its
  entries are mission and expedition reward costs (`C_ATLAS_FINAL`,
  `POLOMONEY1`, …). Finding it means identifying the product table's
  `BaseValueUnits` field by some other means.
* **Stack size.** The save *does* record it (`MaxAmount` per slot), and it varies
  by inventory type for the same item (`MAINT_REFINER` appears with both 101 and
  201), so it is derived rather than stored per item — most likely a category and
  inventory-type base multiplied by a per-item factor. The stack constants
  (20, 50, 100, 250, 500, 9999, 10000) appear in many tables, with
  `inventorytable.mbin` the most likely source. The save's own `MaxAmount` values
  are the oracle to validate against once found.
* **Recipes** live in `nms_reality_gcrecipetable.mbin` (322 KB), another grid.

## Next

* **The remaining 80 ids** are corvette module variants with left/right and
  decorative markers (`B_CON_L_0`, `B_CON_R_3`, `B_DECO_N_1`, …), plus a handful
  of ship hull parts whose save ids are truncated (`SCIEN_COCKAB` for
  `UI_SCIENTIFIC_COCK_ABA_NAME`).
* **`maintenancegroupstable.mbin`** explains the `MAINT_*` ids and would let the
  editor show which component a damaged slot belongs to.
* **Base value and stack size**, as above.
