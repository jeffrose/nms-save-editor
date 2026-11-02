# Player state: currencies, inventories, known items

The character lives in `PlayerStateData`, which sits under one of two contexts:

```text
BaseContext.PlayerStateData        normal saves
ExpeditionContext.PlayerStateData  expedition saves
```

A save may carry both (an expedition save keeps the normal context too), so the
reader probes `BaseContext` first and falls back to `ExpeditionContext`. Every
figure below was measured on 30 real saves from a 7.x install; the counts are in
[`../findings-m4.md`](../findings-m4.md).

Field names are resolved through the key mapping, because the file stores
obfuscated keys (`6f=` for `PlayerStateData`, `wGS` for `Units`). A document is
never re-keyed; the mapping is a lookup table only.

## Currencies

| Field | Meaning |
|---|---|
| `Units` | the main currency |
| `Nanites` | the upgrade currency |
| `Specials` | **Quicksilver** — the Nexus currency |

`Specials` is not a name guess: the MIT-licensed [`nms-save`] crate documents it
("Quicksilver is stored as `Specials` in JSON").

Each balance is a **signed 32-bit JSON integer read as unsigned**. The game caps
balances at 4,294,967,295, which it writes as:

```json
"Units": -1
```

The editor therefore exposes `u32` to callers and converts on the boundary:

* read: `value as u32`, so `-1` becomes `4,294,967,295`;
* write: `value as i32` widened to `i64`, so `u32::MAX` is written back as `-1`.

Writing `4294967295` literally would be a different number to the game, which
is why the conversion is centralised instead of left to callers.

## Exosuit stats

The three stats the original editor's Exosuit tab edits are plain fields in
`PlayerStateData`:

| Field | Meaning | Observed values |
|---|---|---|
| `Health` | current health | 180 (a save mid-play), 60 (an expedition start) |
| `Shield` | shield charge | 100 |
| `Energy` | life-support charge | 76, 43 |

Unlike the currencies, these are **not** a two's-complement `u32`: they are
written as ordinary positive integers, and the game reads them as signed
32-bit, so the editor refuses anything above `2147483647` rather than writing a
value the game would read as negative. A malformed value in a save reads as its
nearest legal one instead of failing the whole page.

## Inventory containers

An inventory is any object that has all four of:

| Field | Type | Meaning |
|---|---|---|
| `Slots` | array | the occupied slots (see below) |
| `ValidSlotIndices` | array of `{X, Y}` | cells the grid exposes, in display order |
| `Width` | integer | grid columns |
| `Height` | integer | grid rows |

Anything else — `Class`, `StackSizeGroup`, `BaseStatValues`, `SpecialSlots`,
`Name`, `Version` — is carried along and shown, not interpreted.

They are found by walking the player state; a recognised container is not walked
into, because its `Slots` are not containers. The hosts seen in real saves:

| Host | What it is |
|---|---|
| `Inventory`, `Inventory_TechOnly`, `Inventory_Cargo` | exosuit general, technology and cargo grids |
| `ShipInventory`, `WeaponInventory` | the active ship and multitool (older layout) |
| `ShipOwnership[i].Inventory`, `_Cargo`, `_TechOnly` | every owned ship |
| `Multitools[i].Store` | every owned multitool |
| `VehicleOwnership[i].…` | exocraft |
| `FreighterFleet[i].…` | frigates |
| `ArchivedShipOwnership[i].Ownership.…`, `ArchivedMultitools[i].MultitoolData.Store` | archived ships and tools |
| `Chest1Inventory` … | storage containers and the base capsule |
| `CorvetteStorageInventory`, `GraveInventory`, `CookingIngredientsInventory`, `FoodUnitInventory`, `FishBaitBoxInventory`, `FishPlatformInventory`, `FireteamSessionCount` | other named grids the player state carries |
| `MaintenanceInteractions[i].InventoryContainer`, `PersonalMaintenanceInteractions[i].…`, `RepairTechBuffer[i].MaintenanceContainer.InventoryContainer`, `RefinerBufferData[i].InventoryContainer` | machine-owned grids |

The last group is classified `Auxiliary`: the game uses those grids for repair
ingredients and refiner buffers, and their amounts do not follow the rules the
player-facing grids follow. The classification is made from the on-disk key of
the host, not from the printed label, so it works the same with or without a
mapping loaded.

The labels the editor prints are built from the on-disk path with the context and
`PlayerStateData` removed, e.g. `ShipOwnership[1].Inventory` or
`MaintenanceInteractions[250].InventoryContainer`. Paths address **raw keys and
array indices**, not readable names, because a few readable names occur more than
once in the community mapping.

### Grid size and unlocking

`Width`/`Height` are the grid's extent; `ValidSlotIndices` lists the cells the
game has unlocked, in display order (row-major, measured across every container
in the 30-save corpus). An **empty** `ValidSlotIndices` means the whole grid is
usable — the cargo grids and the machine-owned containers write it that way —
so the editor reads an empty list as every cell, and materialises the list only
when an edit changes the set.

The corpus also bounds what the editor writes:

| Kind (`StackSizeGroup`) | Largest grid seen | How it was seen |
|---|---|---|
| `Personal` (suit main) | 10x12 | fully unlocked, 120/120 cells |
| `Default` (technology grids) | 10x6 | fully unlocked, 60/60 |
| `Chest` (base storage) | 10x6 | fully unlocked, 60/60 |
| `Vehicle` (exocraft) | 10x5 | fully unlocked, 50/50 |
| `BaseCapsule` | 10x16 | fully unlocked, 160/160 |
| `Ship`, `Freighter` | 10x12 | the grid the save declares |
| `SeasonTransfer` | 8x4 | fully unlocked, 32/32 |

No container in the corpus is wider than 16 columns or taller than 16 rows (the
widest is a machine container at 16x16), so a resize refuses anything larger
outright; a grid past a kind's known size needs the caller's explicit opt-in,
because the game's UI cannot address the extra cells and the original editor's
documentation warns that a save can break. The cargo grids (`PersonalCargo`,
`ShipCargo`, `FreighterCargo`) were still growing in the corpus (7x5, 8x5), so
no size is claimed for them — only the structural bound applies. The game's own
capacity table (`metadata/reality/tables/inventorytable.mbin`) is the eventual
source for exact caps.

## Slots

```json
{
  "Type": {"InventoryType": "Substance"},
  "Id": "^FUEL1",
  "Amount": 1394,
  "MaxAmount": 9999,
  "DamageFactor": 0.0,
  "FullyInstalled": true,
  "AddedAutomatically": false,
  "Index": {"X": 2, "Y": 0}
}
```

| Field | Meaning |
|---|---|
| `Type.InventoryType` | `Product`, `Substance` or `Technology`; nothing else occurs |
| `Id` | the item id, always with a leading `^` |
| `Amount` | how much is in the stack; `-1` on technology means *no charge* |
| `MaxAmount` | stack limit, or a capacity for refiner technology (see below) |
| `DamageFactor` | `1.0` when damaged, `0.0` otherwise |
| `FullyInstalled` | whether the item is installed |
| `Index` | the grid cell |

A slot's `position` in the editor is its index in the `Slots` array, which is
what the CLI addresses. That order has nothing to do with the grid.

### Measured invariants

Across 58,429 slots in 52,874 containers:

* every id starts with `^`;
* every cell lies inside `Width` × `Height` and is unique within its inventory;
* in **player-facing** inventories, every cell is listed in `ValidSlotIndices`,
  and `Amount` never exceeds `MaxAmount`;
* the same id can occupy several slots in one inventory (1,511 containers do) —
  an id is not a unique key, which is why edits go by slot index;
* `DamageFactor` and `FullyInstalled` are independent: 831 slots are damaged but
  installed, and 294 are undamaged but not installed.

### When `ValidSlotIndices` is empty

Machine-owned containers write an empty list *while holding items* — 9,377
maintenance and 2,806 refiner containers do. The grid itself is the list there,
so the reader fills in every cell of `Width` × `Height`. Player-facing
inventories always list their cells.

### Amounts are not always stack counts

`Amount == -1` occurs 8,104 times on technology, where it means "no charge"
(what the game writes for a fully charged tool as well as an uncharged one). The
124 substance and 36 product slots with `-1` all sit in maintenance or refiner
containers.

In those auxiliary containers `Amount` can also exceed `MaxAmount` (237 slots),
because the pair means "how much of a repair ingredient is present" and "how much
the recipe wants". The same slots make `MaxAmount` look like 101, 201, 901 — so
they are excluded from stack-size statistics.

One more `MaxAmount` exception: refiner technology (`^SUIT_REFINER`,
`^SUIT_REFINER2`) sometimes carries the refiner's capacity (112,900 or 367,900)
rather than a stack limit. It is the only player-facing case where the same
(group, kind, id) was seen with more than one `MaxAmount`.

## Stack sizes for new items

Stack size is a property of the item, not of the inventory type: the same
substance is 9,999 in the exosuit, a chest and a freighter. `MaxAmount` is
therefore the only reliable source, and it is read from the save being edited:

1. the same id in the same stack group and kind (`SameItem`);
2. failing that, the most common `MaxAmount` for that group and kind
   (`InventoryKind`).

Both are observations of what the game itself wrote. A caller can always pass an
explicit stack size. The underlying base-per-group × per-item multiplier
(`inventorytable.mbin`, `StackMultiplier`) is **not** extracted yet, so nothing
is guessed from an unvalidated table.

## Creating and editing slots

* A new slot is written with the game's field order and the same key spellings
  the document uses.
* Items are added by filling existing stacks of the same id and kind first, then
  taking free cells in `ValidSlotIndices` order.
* Technology does not stack: adding it takes one cell and writes `Amount: -1`
  ("no charge"), the safest default; a charge can be set afterwards.
* Amounts are validated against the slot: `0..=MaxAmount` normally, `-1` as well
  for technology.
* Every operation validates against an immutable view first and marks the
  document dirty only after the change is made, so a refused edit can never cause
  a save to be rewritten.

## Known-item lists

| Field | Meaning |
|---|---|
| `KnownProducts` | products and base parts the player has learned |
| `KnownTech` | technology blueprints |
| `KnownSpecials` | cosmetics, figurines, titles |
| `SeenBaseBuildingObjects` | base-building parts seen |
| `KnownRefinerRecipes` | refiner recipes |

They are plain arrays of id strings (`^UP_SCAN`, `^BUILD_REFINER1`). A save that
lacks one of the arrays is reported as such rather than having it invented.

[`nms-save`]: https://crates.io/crates/nms-save
