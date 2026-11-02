# M4 findings: player state, measured before it was edited

M4 adds currencies, inventories and known-item lists. As in M0–M3, the rules
below were measured on the fixture set first and only then turned into code:
**30 saves, 52,874 inventory containers, 58,429 slots**, read through the
community key mapping.

## What the reader found

| Measurement | Value |
|---|---|
| saves | 30 |
| inventory containers | 52,874 |
| slots | 58,429 |
| `Technology` slots | 41,887 |
| `Product` slots | 10,611 |
| `Substance` slots | 5,931 |
| slots with `Amount == -1` | 8,264 |
| damaged but installed (`DamageFactor: 1.0`) | 831 |
| undamaged but not installed (`FullyInstalled: false`) | 294 |
| containers whose grid holds the same id twice | 1,511 |

The container count includes empty grids: every owned ship, freighter frigate,
archived tool and maintenance object carries an inventory object even when it
holds nothing, so the file's shape is much larger than what a player sees.

Slot types are exactly three spellings — `{"InventoryType": "Product"}`,
`"Substance"` and `"Technology"` — which is the same partition the data pack
publishes for item ids (`docs/format/item-tables.md`). That agreement is why the
CLI can infer a new item's kind from the pack.

## Four surprises, each of which changed the code

1. **`ValidSlotIndices` can be empty while slots exist.** 9,377 maintenance and
   2,806 refiner containers do this. An empty list is not "no cells"; it means
   the whole `Width` × `Height` grid. Player-facing containers always list their
   cells (1,822 of them, with zero slots outside the list).
2. **Auxiliary amounts are recipe quantities, not stack counts.** 237 maintenance
   slots hold more than `MaxAmount` (`^PLANT_SNOW` 3,547 in a 401 stack,
   `^MAINT_FRIG8` 101 in a 1 stack). Those same containers are what put 101, 201,
   901 into a naive `MaxAmount` histogram, so they are excluded from stack-size
   statistics and classified `InventoryRole::Auxiliary`.
3. **`-1` is not only a technology charge value.** 8,104 technology slots use it
   for "no charge", but 124 substance and 36 product slots do too — all of them
   in maintenance and refiner containers where the amount is an ingredient count.
   Outside those containers, `-1` is technology-only.
4. **`MaxAmount` is a capacity for refiner technology.** `^SUIT_REFINER` appears
   with both 100 and 112,900; `^SUIT_REFINER2` with 100 and 367,900. It is the
   only player-facing (group, kind, id) combination seen with two different
   values, so it is documented rather than special-cased: the editor reports the
   source of a suggested stack size, and callers can override it.

## What the 30 saves say about currencies

`Units` is `-1` in two fixture slots — the game's representation of
4,294,967,295. `Specials` holds Quicksilver (named by the `nms-save` crate), and
`Nanites` the third balance. Every balance is written back in the same signed
form, so an edit at the cap survives a round trip.

## Discovery rules, and why paths are raw

Inventories are recognised by shape (`Slots`, `ValidSlotIndices`, `Width`,
`Height`), not by a list of known field names, and the walk stops at each
container. That is why a game update that adds another inventory host needs no
code change: 52,874 containers were found across hosts as different as
`Chest3Inventory`, `ShipOwnership[1].Inventory_TechOnly` and
`RefinerBufferData[0].InventoryContainer`.

Edits address the **on-disk path** (`Step::Key` / `Step::Index`), with readable
names only for display. The community mapping contains duplicate readable names,
and two keys that read as the same field can sit in different hosts; addressing
raw steps makes a mis-targeted write impossible rather than unlikely.

## Safety properties added in M4

* Every operation plans against an immutable view, then takes a mutable one;
  `Document::tree_raw_mut()` exists so that a failed plan cannot leave a
  document dirty.
* A refused edit (full grid, occupied cell, out-of-range amount, unknown
  inventory) changes nothing and writes nothing.
* Adding an item fills stacks of the same id and kind before allocating a cell,
  and derives the stack size from the save itself (`SameItem`, else the modal
  value for that group and kind).
* Technology is added as one slot with `Amount: -1`, because technology does not
  stack and `-1` is the safe "no charge" value.

## What the tests now prove

Unit tests build synthetic saves for the rules that are hard to find in
fixtures (currency bit patterns, stacking splits, full-grid refusal, cell
validation, repair, move, known lists).

Fixture tests then check the real data:

* every inventory obeys the invariants above across all 30 saves, and the
  number of slots parsed equals an independent count from the JSON;
* a currency edit commits and round-trips, including the `-1` cap;
* adding to an existing stack plus creating one new stack commits, reloads and
  still leaves unique cells inside `ValidSlotIndices`;
* repair and move commit and reload;
* every known list accepts a probe id, refuses a duplicate, and forgets it
  again in every save.

The M4 fixture tests need the community key mapping (the built-in map only
covers summary fields); without it they skip with a message, like the other
fixture tests do when `fixtures/` is empty.

## Still open

* **Stack multipliers are not extracted.** `inventorytable.mbin`
  (`StackMultiplier`) is the obvious lead; until it is read and verified
  against `MaxAmount`, suggestions come from the save only.
* **Season inventories are not exposed.** `CommonStateData.SeasonData.Inventory`
  and `SeasonTransferInventoryData.Inventory` hold real grids outside
  `PlayerStateData`; they were left alone in M4 because editing them belongs to
  the season-transfer feature, not to the player state.
* **Base item values are still not extracted.** The save stores no price oracle
  and `costtable.mbin` turned out to be mission costs, not item prices.
