# GUI plan: NMSSaveEditor parity, on this project's foundations

Goal: every capability of [goatfungus/NMSSaveEditor](https://github.com/goatfungus/NMSSaveEditor)
available in nmsedit's window, with the interface improvements its users have
asked for, and the same operations exposed through the library, the CLI and the
MCP server as they land.

**Status.** G1 in progress: the exosuit stats and the inventory grid operations
(resize, unlock, move-between, clone, per-kind measured bounds) are in the
library, the CLI (`suit`, `resize`, `unlock`, `move-to`, `clone`), the MCP
(`read_suit_stats`, `set_suit_stat`, `resize_inventory`, `move_item_to`) and the
window (grouped navigation, Suit page, grid controls, "Move into" in a slot's
details). Next in G1: the item picker with descriptions, and a staged-change
preview with undo.

## What the reference is, and what we may take from it

The original is a Java application. Its repository publishes **binaries only**
(`NMSSaveEditor.jar`, `.exe`, `.zip` — on `master` and every side branch) and has
**no licence file**: no source is published, and nothing in it — code,
resources, or the data lists it bundles — may be copied into this MIT/Apache
repository. The capability inventory below is built from facts: its README,
FAQ, CHANGELOG, the published screenshots, and our own inspection of real
saves. Features and save layout are facts; the code and its bundled data are
not ours to take, and none of it enters this repository. Where a display name
is needed (item descriptions, stat labels, trait names), it is derived from the
player's own installation the way the text pack and the key mapping already
are — never from another editor's files.

Its top-level tabs are: **Main, Exosuit, Multitool, Ships, Squadron, Freighter,
Frigates, Vehicles, Companions, Bases & Storage, Settlements, Discovery,
Milestones / Reputation, Account**, plus three dialogs: **JSON Editor**,
**Item Details**, **Coordinate Viewer**. That is the parity checklist.

## The rules that shape this plan

1. **Nothing writes outside `nms-edit`.** Every new capability becomes a library
   operation first (with its own tests), then a CLI command and an MCP tool,
   then a control in the window.
2. **Staged edits, one Save.** The window keeps its staging model: edits change
   the in-memory document, a change report previews them, and Save is the only
   write. Backups and verify-before-write are already in that path.
3. **Validation by default.** Every field gets a validated range and a reason.
   Values outside the game's own limits — the original warns and allows them —
   require an explicit "beyond vanilla" toggle and are marked in the preview.
4. **Unresolved names show ids.** A name that cannot be verified from the
   player's install is shown as the id, never invented (already the rule for
   items). New reference lists follow it too.
5. **An unedited save is never rewritten.** New editors must not mark the
   document dirty on load, on selection, or on a refused edit.

## Where the window is today

| Area | Today |
|---|---|
| Shell | Folder box + Browse, slot sidebar (Autosave / Restore Point, newest marked), staging bar with Save / Discard, backup history with restore |
| Tabs | Overview, Inventories, Known items, JSON |
| Library | Currencies, inventory list/add/remove/amount/move-within-one-grid/repair, known lists ×5, arbitrary paths, backups, metadata mirroring |
| Data pack | 84,965 strings per language, 7,957 item names with categories, key mapping |

## The shell: grouped navigation instead of fourteen tabs

Fourteen flat tabs is the original's weakest point: the entities repeat the same
shape (pick one, edit stats, edit its grid) and three of its tab names overlap
what a player actually wants ("Bases & Storage" vs "Settlements" vs "Main").
Proposed grouping, keeping the original's names so its users find things:

| Group | Pages |
|---|---|
| **Player** | Overview, Suit, Multitool, Ships, Squadron, Freighter, Frigates, Vehicles, Companions |
| **Universe** | Bases & Storage, Settlements |
| **Data** | Discovery, Milestones / Reputation, Account |
| **Advanced** | JSON |

The slot sidebar stays on the left; the group row sits above the page tabs.
Every entity page uses one component shape — **instance picker** (dropdown or
table) + **stat form** + **inventory panel** — so Multitool, Ships, Freighter,
Vehicles and Companions are one component with different field lists, and
Frigates/Settlements reuse the table half with their own forms.

Improvements over the original, applied throughout:

* search everywhere: a field search on each page, an item/entity search in the
  pickers, and a global jump (type a name like `ShipOwnership` or `Hyperdrive`);
* no truncated labels — full names and ids in tooltips, names never clipped to
  `Superconducting Fib…`;
* an item picker with category filter, live search, description pane and
  quantity, replacing the original's "choose from a long list" dialog;
* staged-edit preview with per-field before/after (we already have the change
  report) and an undo of staged operations before Save;
* validation messages that say *why* a value is refused and what the game's
  limit is;
* keyboard-first flow (Enter stages, Esc cancels, Ctrl+S saves) and a
  light/dark theme toggle.

## Capability matrix

Save paths are the verified ones from the fixture saves (readable names; the
on-disk keys stay the source of truth). "Today" is what exists now.

| Original screen | Save data | Today | Work needed |
|---|---|---|---|
| **Main** — open/reload, slot & file picker, Save Changes / Save As, file details | folder files, `mf_*` metadata, `CommonStateData` | Folder + slot list + staging; no Save As | Save As / duplicate to another slot pair (metadata included) — first verify the save's own slot identity (community reports say one is stored) with a sandbox test; file-details page (modified, description); reload |
| **Exosuit** — Health / Shield / Energy, grid, resize, unlock slots | `PlayerStateData.Health`, `.Shield`, `.Energy`; inventories | Inventory grid + item ops | Stat editors; resize + unlock-slots ops (safe bounds); inventory section picker |
| **Multitool** — list of 6, Name / Type / Class / Seed + Generate, Damage / Mining / Scan, grid, Delete / Export / Import | `PlayerStateData.Multitools[]` (`Name`, `Seed`, `Resource.Filename`, inventory) | Grid only | Entity list; name/type/class/seed setters; stat display; delete/export/import |
| **Ships** — list of 12, Name / Type / Class / Seed, Use Old Colours, Health / Shield / Damage / Hyperdrive / Maneuverability, grid, Delete / Export / Import | `PlayerStateData.ShipOwnership[]` (`Name`, `Resource.{Filename,Seed}`, `Inventory*`), `ArchivedShipOwnership[]` | Nothing (only via JSON tab) | Entity model + the same operations as Multitool; class lives on the inventory container (verify) |
| **Squadron** — 4 wingmen: Enabled / Race / Seed / Ship Type / Ship Seed / Pilot Rank | `PlayerStateData.SquadronPilots[4]` (`NPCResource`, `ShipResource`, `TraitsSeed`, `PilotRank`) | Nothing | Four fixed panels; seed + rank editors |
| **Freighter** — Name / Type / Class / Home Seed / Model Seed, Hyperdrive / Fleet Coordination, grid, Backup / Restore | `PlayerStateData.CurrentFreighter`, `PlayerFreighterName`, `CurrentFreighterHomeSystemSeed`, `FreighterInventory*` | Nothing | Entity page; structure Backup/Restore rides on entity export/import (M7) |
| **Frigates** — table of 30, Name / Type / Class / Race / seeds, 5 traits, 11 stats, totals, Delete / Copy | `PlayerStateData.FleetFrigates[]` (`CustomName`, `FrigateClass`, `Race`, `ResourceSeed`, `HomeSystemSeed`, `ForcedTraitsSeed`, `TraitIDs[5]`, `Stats[11]`, counters) | Nothing | Table + form; trait id → name list is a data task; class shown as derived (beneficial trait count, per the game's own rule) |
| **Vehicles** — picker + Technology grid | `PlayerStateData.VehicleOwnership[]` (`Name`, `Inventory*`) | Grid only | Picker + the shared inventory panel |
| **Companions** — picker, Name, seeds, Predator, Biome, Type, Delete / Export / Import | `PlayerStateData.Pets[]` (`CustomSpeciesName`, `CreatureSeed`, `SpeciesSeed`, `GenusSeed`, `Predator`, `Biome`, `CreatureType`) | Nothing | Picker + form; seeds are mixed representations (arrays vs `0x…` strings) — normalise in the library |
| **Bases & Storage** — base list, name, NPC (race/seed), 10 storage chests, Move Base Computer, Backup / Restore | `PlayerStateData.PersistentPlayerBases[]`, `BaseBuildingObjects[]`, `NPCWorkers[]` | Nothing | Base list + chest grids (they are inventories); NPC form; Move Base Computer (swap with a signal booster position) |
| **Settlements** — picker, Name / Seed, 6 perks + Info, Population / Happiness / Production / Upkeep / Sentinels / Debt / Alert, production items | `PlayerStateData.SettlementStatesV2[]` (`Name`, `SeedValue`, `Perks`, `Stats[8]`, `ProductionState`) | Nothing | Picker + form; perk names are a data task; production item names come from the pack |
| **Discovery** — Known Technology / Products tables with Add / Remove, Known Words matrix (Gek/Vy'keen/Korvax/Atlas), 16 Known Glyph toggles | `KnownTech`, `KnownProducts`, `KnownSpecials`, `SeenBaseBuildingObjects`, `KnownRefinerRecipes`, `KnownWordGroups[]` (`Group`, `Races[9]`), glyphs (field to locate) | Known items tab = five lists with filter + add/remove | Rebuild as the four-panel page; word meanings and glyph storage are data/format tasks (below) |
| **Milestones / Reputation** — 9 milestones, 6 kill counters, 6 factions × standing / missions / words / systems / … | `PlayerStateData.Stats[]` groups: `^GLOBAL_STATS` (472 entries), `^TELEMETRY_STATS`, per-planet groups; entries `{Id, Value}` | Nothing | Group picker + form; stat id → label list is a data task |
| **Account** — season / Twitch / platform reward unlocks (checkboxes) | `accountdata.hg` (plain JSON, own document) | Read-only file for `info`; no editing | Account document type in `nms-edit` (backup + metadata rules apply); unlock toggles; account-wide warning |
| **Item Details dialog** — type, category, name, id, quantity, subtitle, build cost, description | text pack + item tables | Item names + categories in the UI | Resolve description/subtitle/build-cost keys; show in picker and slot tooltip |
| **Coordinate Viewer** — galaxy, galactic address, portal address, 12-glyph strip, Search, Save/Warp | `GalacticAddress` structures throughout the save | Nothing | Address codec (voxel ↔ string ↔ glyphs); search over visited systems (`TeleportEndpoints`, discoveries) |
| **JSON Editor** — tree + raw editor | whole document | JSON tab with search + typed edit | Keep; add page links ("open this entity in JSON") |
| **Look & feel** — dark/light, inventory scaling | — | Fixed egui theme | Theme toggle + scale control |

## Library work packages (`nms-edit`)

1. **Entity containers.** A uniform API over `Ships`, `Multitools`, `Vehicles`,
   `Pets`, `Frigates`, `SquadronPilots`, `Freighter`, `Bases`, `Settlements`:
   list, select, read a typed field, write it, add/remove/clone a record. All
   key access through the key map, all writes staged, every op reporting what it
   changed.
2. **Seed handling.** Seeds appear in three shapes in the save: `[bool, "0x…"]`
   arrays, plain `"0x…"` strings, and 2-element numeric arrays. One normal form
   in the API (`u64` + hex text) with exact round-tripping to the on-disk shape.
3. **Stat groups.** Read/write `Stats[]` groups and entries by id.
4. **Inventory extensions.** Resize (width/height), unlock slots (valid-cell
   set), move between inventories, clone within/between, recharge/refill
   (chargeable tech), with the game's own limits enforced.
5. **Entity export/import.** A versioned JSON envelope per entity (ship, tool,
   frigate, pet, base structure) for save-to-save transfer — the M7 item, and
   the engine behind the original's Backup/Restore buttons.
6. **Account document.** `accountdata.hg` as its own document type: plain JSON,
   own metadata companion, same backup/verify rules, unlock-list editing.
7. **Coordinate codec.** Galactic address ↔ `0373:0082:0746:00B1` text ↔ portal
   glyph indices, with round-trip tests.
8. **Glyph storage.** Locate the glyph-knowledge field (not present in the
   fixture saves as a `KnownGlyphs` array — verify against a save that has
   glyphs unlocked; likely a mask on the portal fields).

## Data work packages (`nms-data`)

Extracted from the player's install like the pack and mapping; ids shown when a
name cannot be verified.

| Data | Source | Status |
|---|---|---|
| Item descriptions, subtitles, build costs | item tables' text keys + text pack | to map (same exercise as item names) |
| Word meanings (`^TRA_A` → "a") | game word tables (MBIN, name TBD) | to locate |
| Stat / milestone labels (`^CARGO_SPECIAL` → "…") | stats table (MBIN, name TBD) or curated from ids | to locate |
| Frigate trait names (`TraitIDs`) | game trait table | to locate |
| Settlement perk names | game perk table | to locate |
| Ship / multitool type enums | `Resource.Filename` model paths + item data | partial (names known) |
| Glyph table (16 glyphs) | game glyph data / portal codec | to locate |

## Phases

Each phase is shippable on its own; each slice: library → CLI/MCP where it
makes sense → GUI → tests (fixture round-trips + headless render tests).

* **G1 — Entity infrastructure + Suit page.** Library: container API, seeds,
  inventory resize/unlock/move-between/clone, recharge. UI: Suit page (stats,
  section picker, improved grid), item picker with search + description
  (pending the description keys), change preview. This phase makes the most
  used screen good and builds the machinery the rest of the fleet reuses.
* **G2 — Fleet pages.** Multitool, Ships, Freighter, Frigates, Vehicles,
  Squadron, Companions, including delete/export/import (the M7 envelope).
  Data: trait names.
* **G3 — Home pages.** Bases & Storage (base list, chests, NPC, Move Base
  Computer, structure Backup/Restore), Settlements (stats, perks, production).
* **G4 — Data pages.** Discovery (four panels, words matrix, glyph toggles),
  Milestones / Reputation, Account. Data: word meanings, stat labels, glyphs.
* **G5 — Dialogs and polish.** Item Details dialog, Coordinate Viewer, Save As,
  theme/scale, global search, page-to-JSON links, keyboard flow.
* **G6 — Platforms (M6).** Xbox Game Pass (WGS), PS4 (Save Wizard), Switch:
  container work in `nms-format`, no UI change beyond opening them.

## Known hazards to design around

From the game itself and the reference's FAQ — each becomes a validation rule or
a documented note rather than a silent failure:

* stack-size edits on the General exosuit inventory are game-breaking (the
  original disables them; we validate them away);
* inventory resizing beyond vanilla limits can break the save — allowed only
  behind the explicit unsafe toggle;
* the Jetpack must exist in the General inventory or the game recreates one;
* frigate class is derived from the count of *beneficial* traits, not a stored
  class;
* freighter colours/NPCs come from the home seed, not the model seed;
* extreme-survival milestones only tick over in-game;
* `accountdata.hg` is account-wide — one edit reaches every save on the account
  and syncs through Steam Cloud; it needs the loudest warning.

## Open questions

1. **Navigation**: grouped pages (recommended above) or a 1:1 fourteen-tab
   strip? Grouped keeps the original names but shortens the strip.
2. **First phase**: G1 as planned (infrastructure + Suit), or jump to Ships /
   Multitool because those are what the original's users reach for most?
3. **Account editing**: include it in G4, or defer while account-wide risk is
   reviewed?
4. **Export/import**: our own JSON envelope (recommended) — and should
   cross-save import be allowed by default, or gated?
5. **Unsafe values**: an explicit toggle per edit (recommended) or a global
   "developer mode"?
6. **Platforms**: keep PC-first and treat PS4/Xbox/Switch as the separate M6?