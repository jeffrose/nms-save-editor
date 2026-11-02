//! Player-state editing: currencies, inventories, and knowledge lists.
//!
//! Everything in this module works on the *player state* — the part of a save
//! that belongs to the character rather than to the world. It lives under
//! `BaseContext.PlayerStateData` in a normal save and under
//! `ExpeditionContext.PlayerStateData` in an expedition save; both are probed,
//! in that order, the same way the metadata mirror does.
//!
//! Two rules from the rest of the crate carry over:
//!
//! * nothing is renamed — fields are looked up through the key map and written
//!   back under whatever spelling the file uses;
//! * a failed edit leaves the document **not dirty**, so a rejected operation
//!   can never cause the save to be rewritten.
//!
//! The second one is why every mutating operation validates against an
//! immutable view first and only then takes a mutable one.

use std::collections::BTreeMap;

use nms_model::KeyMap;
use serde_json::{Map, Value};

use crate::bundle::SaveBundle;
use crate::EditError;

// ---------------------------------------------------------------------------
// currencies
// ---------------------------------------------------------------------------

/// The three balances the game keeps in the player state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Currency {
    /// Units, the main currency.
    Units,
    /// Nanites, used for upgrades.
    Nanites,
    /// Quicksilver, the Nexus currency. Stored under the field name `Specials`.
    Quicksilver,
}

impl Currency {
    /// All three, in display order.
    pub const ALL: [Currency; 3] = [Currency::Units, Currency::Nanites, Currency::Quicksilver];

    /// The on-disk field name.
    pub fn field(self) -> &'static str {
        match self {
            Currency::Units => "Units",
            Currency::Nanites => "Nanites",
            Currency::Quicksilver => "Specials",
        }
    }

    /// A lowercase name for command lines and reports.
    pub fn label(self) -> &'static str {
        match self {
            Currency::Units => "units",
            Currency::Nanites => "nanites",
            Currency::Quicksilver => "quicksilver",
        }
    }

    /// Parse a lowercase name.
    pub fn parse(text: &str) -> Option<Currency> {
        match text.to_ascii_lowercase().as_str() {
            "units" | "unit" => Some(Currency::Units),
            "nanites" | "nanite" => Some(Currency::Nanites),
            "quicksilver" | "qs" | "specials" => Some(Currency::Quicksilver),
            _ => None,
        }
    }
}

/// The player's three balances, as unsigned values.
///
/// The game stores each balance as a signed 32-bit JSON integer and treats the
/// bit pattern as unsigned, so a player at the cap of 4,294,967,295 units has
/// `"Units": -1` in the file. Reading and writing through `u32` here means the
/// editor shows and accepts the number the game shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Currencies {
    /// Units.
    pub units: u32,
    /// Nanites.
    pub nanites: u32,
    /// Quicksilver.
    pub quicksilver: u32,
}

impl Currencies {
    /// Read one balance.
    pub fn get(&self, currency: Currency) -> u32 {
        match currency {
            Currency::Units => self.units,
            Currency::Nanites => self.nanites,
            Currency::Quicksilver => self.quicksilver,
        }
    }

    /// Set one balance.
    pub fn set(&mut self, currency: Currency, value: u32) {
        match currency {
            Currency::Units => self.units = value,
            Currency::Nanites => self.nanites = value,
            Currency::Quicksilver => self.quicksilver = value,
        }
    }
}

/// Encode an unsigned balance the way the game writes it: the same bits, read
/// as a signed 32-bit integer.
fn encode_currency(value: u32) -> i64 {
    i64::from(value as i32)
}

/// Decode a signed JSON integer back to the balance the game shows.
fn decode_currency(value: i64) -> u32 {
    value as u32
}

// ---------------------------------------------------------------------------
// inventories
// ---------------------------------------------------------------------------

/// The kind of item a slot holds. The game calls this `InventoryType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ItemKind {
    /// A product.
    Product,
    /// An element.
    Substance,
    /// Installed technology.
    Technology,
}

impl ItemKind {
    /// The on-disk spelling.
    pub fn label(self) -> &'static str {
        match self {
            ItemKind::Product => "Product",
            ItemKind::Substance => "Substance",
            ItemKind::Technology => "Technology",
        }
    }

    /// Parse the on-disk spelling, ignoring case so command lines can use
    /// lowercase without a second parser.
    pub fn parse(text: &str) -> Option<ItemKind> {
        match text.to_ascii_lowercase().as_str() {
            "product" => Some(ItemKind::Product),
            "substance" => Some(ItemKind::Substance),
            "technology" => Some(ItemKind::Technology),
            _ => None,
        }
    }
}

/// One cell of an inventory grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Cell {
    /// Column.
    pub x: u64,
    /// Row.
    pub y: u64,
}

impl Cell {
    /// A cell at `(x, y)`.
    pub fn new(x: u64, y: u64) -> Cell {
        Cell { x, y }
    }
}

impl std::fmt::Display for Cell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}

/// One occupied slot.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemSlot {
    /// Position in the `Slots` array, which is what edit commands address.
    pub position: usize,
    /// Grid cell the slot sits in.
    pub cell: Cell,
    /// Item id, including the `^` marker the game writes.
    pub id: String,
    /// Item kind.
    pub kind: ItemKind,
    /// Current amount. `-1` on technology means "not chargeable" or "full".
    pub amount: i64,
    /// Maximum the slot's stack allows.
    pub max_amount: i64,
    /// Damage factor, `0.0` when undamaged.
    pub damage: f64,
    /// Whether the item is installed.
    pub installed: bool,
}

impl ItemSlot {
    /// Whether the game considers this slot damaged or broken.
    pub fn damaged(&self) -> bool {
        self.damage > 0.0 || !self.installed
    }
}

/// Where an inventory sits and what it is called.
///
/// The steps are **on-disk** keys and indices, not readable names, so resolving
/// a path cannot accidentally land on a different field that happens to share a
/// readable name (the community mapping has a few of those).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryPath {
    steps: Vec<Step>,
    label: String,
}

impl InventoryPath {
    /// The readable path, e.g. `ShipOwnership[1].Inventory_TechOnly`.
    pub fn label(&self) -> &str {
        &self.label
    }
}

impl std::fmt::Display for InventoryPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// Whether an inventory is a player-facing one or an auxiliary container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryRole {
    /// Exosuit, ship, multitool, freighter, exocraft, storage, cooking.
    Player,
    /// Refiner buffers, maintenance containers and other machine-owned grids.
    Auxiliary,
}

/// A discovered inventory.
#[derive(Debug, Clone)]
pub struct Inventory {
    /// Where it lives.
    pub path: InventoryPath,
    /// Grid width.
    pub width: u64,
    /// Grid height.
    pub height: u64,
    /// The game's stack-size group (`Personal`, `Ship`, `Chest`, …).
    pub stack_group: String,
    /// Inventory class (`C`/`B`/`A`/`S`), when present.
    pub class: String,
    /// Occupied slots, in array order.
    pub slots: Vec<ItemSlot>,
    /// Cells the game allows items to sit in.
    ///
    /// Player-facing inventories list these explicitly. Machine-owned
    /// containers (maintenance, refiner buffers) leave `ValidSlotIndices` empty
    /// even while holding items — measured on 9,377 maintenance and 2,806
    /// refiner containers across 30 saves — so the whole grid is used there.
    pub valid_cells: Vec<Cell>,
    /// Whether this is a player-facing inventory.
    pub role: InventoryRole,
}

impl Inventory {
    /// Cells that are valid but empty, in the order the game lists them.
    pub fn free_cells(&self) -> Vec<Cell> {
        let occupied: std::collections::HashSet<Cell> = self.slots.iter().map(|slot| slot.cell).collect();
        self.valid_cells.iter().copied().filter(|cell| !occupied.contains(cell)).collect()
    }

    /// True when every valid cell is taken.
    pub fn is_full(&self) -> bool {
        self.free_cells().is_empty()
    }

    /// Total number of items of one kind.
    pub fn count(&self, kind: ItemKind) -> usize {
        self.slots.iter().filter(|slot| slot.kind == kind).count()
    }
}

/// A suggested maximum stack size for an item that may not be in the save yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StackSize {
    /// The value to write into `MaxAmount`.
    pub value: i64,
    /// Where the value came from.
    pub source: StackSizeSource,
}

/// How a stack size was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackSizeSource {
    /// The same item, observed in an inventory of the same stack group and kind.
    SameItem,
    /// The most common stack size for that inventory group and kind, used when
    /// the item is new to this save.
    InventoryKind,
}

impl StackSizeSource {
    /// A short explanation for reports.
    pub fn describe(self) -> &'static str {
        match self {
            StackSizeSource::SameItem => "same item elsewhere in this save",
            StackSizeSource::InventoryKind => "the usual size for this inventory type",
        }
    }
}

/// An item to add.
#[derive(Debug, Clone)]
pub struct NewItem {
    /// Item id, with or without the `^` marker.
    pub id: String,
    /// How many to add.
    pub amount: i64,
    /// Item kind, when the caller knows it (the data pack can tell). When it is
    /// absent the kind is taken from an existing slot of the same item, and
    /// failing that from the inventory if that holds a single kind.
    pub kind: Option<ItemKind>,
    /// Stack size to write, when the caller knows it.
    pub max_amount: Option<i64>,
    /// Cell to use for the first new slot.
    pub cell: Option<Cell>,
}

impl NewItem {
    /// An item of `amount` units.
    pub fn new(id: impl Into<String>, amount: i64) -> NewItem {
        NewItem { id: id.into(), amount, kind: None, max_amount: None, cell: None }
    }
}

// ---------------------------------------------------------------------------
// errors
// ---------------------------------------------------------------------------

/// A player-state edit that cannot be carried out.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlayerError {
    /// The save has no player state (for example an account-level file).
    #[error("this save has no player state")]
    NoPlayerState,

    /// The requested inventory does not exist.
    #[error("no inventory named {0:?} (run `nmsedit inventories` to list them)")]
    UnknownInventory(String),

    /// The inventory has no room for another slot.
    #[error("inventory {inventory} has no free slot")]
    InventoryFull {
        /// Inventory path.
        inventory: String,
    },

    /// The requested cell is not one the game allows in this inventory.
    #[error("cell {cell} is not a valid slot in {inventory}")]
    CellNotInInventory {
        /// Inventory path.
        inventory: String,
        /// Requested cell.
        cell: Cell,
    },

    /// The requested cell already holds something.
    #[error("cell {cell} in {inventory} is already occupied by slot {position}")]
    CellOccupied {
        /// Inventory path.
        inventory: String,
        /// Requested cell.
        cell: Cell,
        /// Slot that already sits there.
        position: usize,
    },

    /// The requested slot index does not exist.
    #[error("{inventory} has no slot {index} (it has {len})")]
    NoSuchSlot {
        /// Inventory path.
        inventory: String,
        /// Requested position.
        index: usize,
        /// Number of slots.
        len: usize,
    },

    /// No stack size is known for a new item.
    #[error(
        "cannot tell how many {id} fit in one slot: no {kind} of that kind exists in this save, \
         so pass an explicit stack size"
    )]
    UnknownStackSize {
        /// Item id.
        id: String,
        /// Kind that was requested.
        kind: String,
    },

    /// The item's kind is not known and the inventory does not imply one.
    #[error("cannot tell whether {0} is a product, substance or technology; pass an explicit kind")]
    UnknownKind(String),

    /// An amount that is not legal for the slot.
    #[error("{amount} is not a legal amount for {id} (a slot holds 0 to {max})")]
    AmountOutOfRange {
        /// Item id.
        id: String,
        /// Requested amount.
        amount: i64,
        /// Slot maximum.
        max: i64,
    },

    /// A known-item list is missing from the save.
    #[error("this save has no {0} list")]
    UnknownList(String),

    /// The id is not a plausible item id.
    #[error("{0:?} is not an item id")]
    InvalidId(String),
}

// ---------------------------------------------------------------------------
// reading
// ---------------------------------------------------------------------------

/// Look up an object member by readable name, returning the on-disk key too.
fn named<'a>(value: &'a Value, keys: &KeyMap, name: &str) -> Option<(&'a str, &'a Value)> {
    let object = value.as_object()?;
    keys.candidates(name)
        .find_map(|candidate| object.get_key_value(candidate))
        .map(|(key, value)| (key.as_str(), value))
}

/// The on-disk key to create a field under.
fn creation_key(keys: &KeyMap, name: &str) -> String {
    keys.candidates(name).last().unwrap_or(name).to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Key(String),
    Index(usize),
}

fn resolve_mut<'a>(value: &'a mut Value, steps: &[Step]) -> Option<&'a mut Value> {
    match steps.split_first() {
        None => Some(value),
        Some((Step::Key(key), rest)) => resolve_mut(value.as_object_mut()?.get_mut(key)?, rest),
        Some((Step::Index(index), rest)) => resolve_mut(value.as_array_mut()?.get_mut(*index)?, rest),
    }
}

/// Resolve a chain of readable names, honouring the key spelling each level
/// actually uses.
///
/// This is what `Document::get_path` does for reads; writes that need a mutable
/// reference cannot go through that shared borrow, so the chain is walked here
/// instead of being built as raw steps from guesses about the root.
fn resolve_named_mut<'a>(value: &'a mut Value, keys: &KeyMap, names: &[&str]) -> Option<&'a mut Value> {
    let mut current = value;
    for name in names {
        let object = current.as_object_mut()?;
        let key = keys.candidates(name).find(|candidate| object.contains_key(*candidate))?.to_string();
        current = object.get_mut(&key)?;
    }
    Some(current)
}

/// Render a path for humans: readable names where the map knows them, the
/// on-disk spelling where it does not.
///
/// The first step is the context (`BaseContext` or `ExpeditionContext`) and the
/// second is `PlayerStateData`; neither is part of what the player is editing,
/// and dropping both means an inventory keeps one name in normal and expedition
/// saves alike.
fn render_path(steps: &[Step], keys: &KeyMap) -> String {
    let mut out = String::new();
    for step in steps.iter().skip(1) {
        match step {
            Step::Key(key) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(keys.readable_for(key).unwrap_or(key));
            }
            Step::Index(index) => {
                out.push('[');
                out.push_str(&index.to_string());
                out.push(']');
            }
        }
    }
    match out.strip_prefix("PlayerStateData.") {
        Some(rest) => rest.to_string(),
        None => out,
    }
}

fn read_cell(value: &Value, keys: &KeyMap) -> Option<Cell> {
    Some(Cell::new(named(value, keys, "X")?.1.as_u64()?, named(value, keys, "Y")?.1.as_u64()?))
}

/// Whether `path` ends with `label`, either exactly or after a dot.
fn ends_at_boundary(path: &str, label: &str) -> bool {
    match path.strip_suffix(label) {
        Some(rest) => rest.is_empty() || rest.ends_with('.'),
        None => false,
    }
}

/// Whether the path passes through a machine-owned container.
///
/// These grids hold repair ingredients and refiner buffers, whose amounts are
/// recipe quantities rather than stacks, so they are kept out of the statistics
/// used to size new items. Classification looks at the on-disk keys rather than
/// the rendered label: a label is for humans and may be missing names when no
/// mapping is loaded, while the keys are always there.
fn is_auxiliary_host(steps: &[Step], keys: &KeyMap) -> bool {
    const HOSTS: [&str; 4] =
        ["MaintenanceInteractions", "PersonalMaintenanceInteractions", "RepairTechBuffer", "RefinerBufferData"];
    steps.iter().any(|step| match step {
        Step::Key(key) => HOSTS.iter().any(|host| keys.candidates(host).any(|candidate| candidate == key)),
        Step::Index(_) => false,
    })
}

fn read_slot(value: &Value, position: usize, keys: &KeyMap) -> Option<ItemSlot> {
    let kind = named(value, keys, "Type")
        .and_then(|(_, kind)| named(kind, keys, "InventoryType"))
        .and_then(|(_, kind)| kind.as_str())
        .and_then(ItemKind::parse)?;
    let cell = read_cell(named(value, keys, "Index")?.1, keys)?;
    Some(ItemSlot {
        position,
        cell,
        id: named(value, keys, "Id")?.1.as_str()?.to_string(),
        kind,
        amount: named(value, keys, "Amount")?.1.as_i64()?,
        max_amount: named(value, keys, "MaxAmount")?.1.as_i64()?,
        damage: named(value, keys, "DamageFactor").and_then(|(_, v)| v.as_f64()).unwrap_or(0.0),
        installed: named(value, keys, "FullyInstalled").and_then(|(_, v)| v.as_bool()).unwrap_or(true),
    })
}

fn read_inventory(value: &Value, steps: Vec<Step>, keys: &KeyMap) -> Option<Inventory> {
    let slots = named(value, keys, "Slots")?.1.as_array()?;
    let valid = named(value, keys, "ValidSlotIndices")?.1.as_array()?;
    let width = named(value, keys, "Width")?.1.as_u64()?;
    let height = named(value, keys, "Height")?.1.as_u64()?;

    let stack_group = named(value, keys, "StackSizeGroup")
        .and_then(|(_, group)| named(group, keys, "InventoryStackSizeGroup"))
        .and_then(|(_, group)| group.as_str())
        .unwrap_or_default()
        .to_string();
    let class = named(value, keys, "Class")
        .and_then(|(_, class)| named(class, keys, "InventoryClass"))
        .and_then(|(_, class)| class.as_str())
        .unwrap_or_default()
        .to_string();

    let slots: Vec<ItemSlot> =
        slots.iter().enumerate().filter_map(|(position, slot)| read_slot(slot, position, keys)).collect();
    let mut valid_cells: Vec<Cell> = valid.iter().filter_map(|cell| read_cell(cell, keys)).collect();
    // An empty list is not "nothing fits": the machine-owned containers use
    // their whole grid, and their items prove the game accepts it.
    if valid_cells.is_empty() {
        for y in 0..height {
            for x in 0..width {
                valid_cells.push(Cell::new(x, y));
            }
        }
    }
    let label = render_path(&steps, keys);
    let role = if is_auxiliary_host(&steps, keys) { InventoryRole::Auxiliary } else { InventoryRole::Player };

    Some(Inventory {
        path: InventoryPath { steps, label },
        width,
        height,
        stack_group,
        class,
        slots,
        valid_cells,
        role,
    })
}

/// Collect every inventory under `value`, without descending into one once it
/// is recognised (its slots are not containers).
fn collect(value: &Value, steps: &mut Vec<Step>, keys: &KeyMap, out: &mut Vec<Inventory>) {
    if let Some(inventory) = read_inventory(value, steps.clone(), keys) {
        out.push(inventory);
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                steps.push(Step::Key(key.clone()));
                collect(child, steps, keys, out);
                steps.pop();
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                steps.push(Step::Index(index));
                collect(child, steps, keys, out);
                steps.pop();
            }
        }
        _ => {}
    }
}

impl SaveBundle {
    /// The player-state context this save uses.
    pub fn player_context(&self) -> Option<&'static str> {
        self.context_key()
    }

    /// Every inventory in the save, in file order.
    pub fn inventories(&self) -> Vec<Inventory> {
        let Some(context) = self.player_context() else { return Vec::new() };
        let Some(root) = self.document.tree().as_object() else { return Vec::new() };
        let Some((key, context_value)) =
            self.keys.candidates(context).find_map(|candidate| root.get_key_value(candidate))
        else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut steps = vec![Step::Key(key.to_string())];
        collect(context_value, &mut steps, &self.keys, &mut out);
        out
    }

    /// Look an inventory up by its label, accepting an optional
    /// `PlayerStateData.` prefix.
    pub fn inventory(&self, name: &str) -> Option<Inventory> {
        let wanted = name.trim().strip_prefix("PlayerStateData.").unwrap_or(name.trim());
        let inventories = self.inventories();
        if let Some(inventory) =
            inventories.iter().find(|inventory| inventory.path.label().eq_ignore_ascii_case(wanted))
        {
            return Some(inventory.clone());
        }
        // A longer path — one copied out of the JSON tree, say — resolves when
        // it ends in exactly one label at a dot boundary.
        let wanted = wanted.to_ascii_lowercase();
        if wanted.is_empty() {
            return None;
        }
        let mut matches = inventories.into_iter().filter(|inventory| {
            let label = inventory.path.label().to_ascii_lowercase();
            !label.is_empty() && ends_at_boundary(&wanted, &label)
        });
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }

    /// The player's balances.
    pub fn currencies(&self) -> Result<Currencies, PlayerError> {
        let mut currencies = Currencies::default();
        for currency in Currency::ALL {
            let value = self
                .player_state_value(currency.field())
                .and_then(Value::as_i64)
                .ok_or(PlayerError::NoPlayerState)?;
            currencies.set(currency, decode_currency(value));
        }
        Ok(currencies)
    }

    /// Set one balance, writing the two's-complement form the game uses for
    /// values above `i32::MAX`.
    pub fn set_currency(&mut self, currency: Currency, value: u32) -> Result<bool, EditError> {
        let context = self.player_context().ok_or(PlayerError::NoPlayerState)?;
        let path = format!("{context}.PlayerStateData.{}", currency.field());
        let current = self.get(&path).ok().and_then(Value::as_i64).map(decode_currency).unwrap_or(0);
        if current == value {
            return Ok(false);
        }
        self.set(&path, Value::from(encode_currency(value)))?;
        Ok(true)
    }

    /// Suggest a maximum stack size for an item in a given inventory.
    ///
    /// The value is always derived from this save: first the same item in an
    /// inventory of the same stack group and kind, then the most common size for
    /// that group and kind. Both are measurements of what the game itself
    /// wrote, not a table copied from somewhere else.
    pub fn suggest_stack_size(&self, path: &InventoryPath, id: &str, kind: ItemKind) -> Option<StackSize> {
        let id = normalize_id(id);
        let inventories = self.inventories();
        let target = inventories.iter().find(|inventory| inventory.path == *path)?;
        let same_group = |inventory: &&Inventory| {
            inventory.role == InventoryRole::Player && inventory.stack_group == target.stack_group
        };

        let mut exact = BTreeMap::<i64, usize>::new();
        for inventory in inventories.iter().filter(same_group) {
            for slot in inventory.slots.iter().filter(|slot| slot.kind == kind && slot.id == id) {
                *exact.entry(slot.max_amount).or_default() += 1;
            }
        }
        if let Some((value, _)) = exact.iter().max_by_key(|(value, count)| (**count, **value)) {
            return Some(StackSize { value: *value, source: StackSizeSource::SameItem });
        }

        let mut common = BTreeMap::<i64, usize>::new();
        for inventory in inventories.iter().filter(same_group) {
            for slot in inventory.slots.iter().filter(|slot| slot.kind == kind) {
                *common.entry(slot.max_amount).or_default() += 1;
            }
        }
        common
            .iter()
            .max_by_key(|(value, count)| (**count, **value))
            .map(|(value, _)| StackSize { value: *value, source: StackSizeSource::InventoryKind })
    }

    /// Add items to an inventory, filling existing stacks of the same item
    /// first and then allocating free cells.
    ///
    /// Returns the final state of every slot that was touched.
    pub fn add_item(&mut self, path: &InventoryPath, item: &NewItem) -> Result<Vec<ItemSlot>, PlayerError> {
        let id = normalize_id(&item.id);
        if id.len() <= 1 {
            return Err(PlayerError::InvalidId(item.id.clone()));
        }
        let amount = item.amount;
        if amount <= 0 {
            return Err(PlayerError::AmountOutOfRange {
                id: id.clone(),
                amount,
                max: i64::MAX,
            });
        }

        // Work from a fresh immutable view so the plan below is based on the
        // document as it is now, not as it was when the caller listed it.
        let inventory = self
            .inventories()
            .into_iter()
            .find(|inventory| inventory.path == *path)
            .ok_or_else(|| PlayerError::UnknownInventory(path.label().to_string()))?;

        let kind = match item.kind {
            Some(kind) => kind,
            // The item is already in this save: its own slots say what kind it
            // is, which is more reliable than guessing from the inventory.
            None => inventory
                .slots
                .iter()
                .find(|slot| slot.id == id)
                .map(|slot| slot.kind)
                .or_else(|| {
                    self.inventories()
                        .iter()
                        .flat_map(|other| other.slots.iter())
                        .find(|slot| slot.id == id)
                        .map(|slot| slot.kind)
                })
                .or_else(|| single_kind(&inventory))
                .ok_or_else(|| PlayerError::UnknownKind(id.clone()))?,
        };

        let max = match item.max_amount {
            Some(max) => max,
            None => self
                .suggest_stack_size(path, &id, kind)
                .ok_or_else(|| PlayerError::UnknownStackSize { id: id.clone(), kind: kind.label().to_string() })?
                .value,
        };
        if max <= 0 {
            return Err(PlayerError::UnknownStackSize { id: id.clone(), kind: kind.label().to_string() });
        }

        // Plan: (existing slot position -> new amount) and (new cell -> amount).
        let mut fills: Vec<(usize, i64)> = Vec::new();
        let mut remaining = amount;
        if kind != ItemKind::Technology {
            for slot in inventory.slots.iter().filter(|slot| slot.id == id && slot.kind == kind) {
                if remaining == 0 {
                    break;
                }
                let room = slot.max_amount.saturating_sub(slot.amount.max(0));
                if room > 0 {
                    let added = room.min(remaining);
                    fills.push((slot.position, slot.amount + added));
                    remaining -= added;
                }
            }
        }

        let mut free = inventory.free_cells();
        if let Some(cell) = item.cell {
            if !inventory.valid_cells.contains(&cell) {
                return Err(PlayerError::CellNotInInventory {
                    inventory: path.label().to_string(),
                    cell,
                });
            }
            if let Some(slot) = inventory.slots.iter().find(|slot| slot.cell == cell) {
                return Err(PlayerError::CellOccupied {
                    inventory: path.label().to_string(),
                    cell,
                    position: slot.position,
                });
            }
            free.retain(|candidate| *candidate != cell);
            free.insert(0, cell);
        }

        let mut creations: Vec<(Cell, i64)> = Vec::new();
        if kind == ItemKind::Technology {
            // Technology does not stack: one slot, and the game writes `-1` for
            // "no charge" (which is also what a fully charged tool holds).
            if amount != 1 {
                return Err(PlayerError::AmountOutOfRange { id: id.clone(), amount, max: 1 });
            }
            let cell = free.first().copied().ok_or_else(|| PlayerError::InventoryFull {
                inventory: path.label().to_string(),
            })?;
            creations.push((cell, -1));
        } else {
            let mut left = remaining;
            for cell in free {
                if left == 0 {
                    break;
                }
                let stack = left.min(max);
                creations.push((cell, stack));
                left -= stack;
            }
            if left > 0 {
                return Err(PlayerError::InventoryFull { inventory: path.label().to_string() });
            }
        }

        // The plan is complete and verified; apply it.
        let tree = self.document.tree_raw_mut();
        let Some(inventory_value) = resolve_mut(tree, &path.steps) else {
            return Err(PlayerError::UnknownInventory(path.label().to_string()));
        };
        let slots = inventory_value
            .as_object_mut()
            .and_then(|object| named_key_mut(object, &self.keys, "Slots"))
            .and_then(Value::as_array_mut)
            .ok_or_else(|| PlayerError::UnknownInventory(path.label().to_string()))?;

        for (position, new_amount) in &fills {
            if let Some(slot) = slots.get_mut(*position) {
                set_field(slot, &self.keys, "Amount", Value::from(*new_amount));
            }
        }

        let mut created = Vec::new();
        for (cell, amount) in creations {
            let slot = new_slot_value(&self.keys, &id, kind, cell, amount, max, false);
            slots.push(slot);
            created.push(cell);
        }

        self.document.mark_dirty();

        // Report from the freshly written document so the caller sees exactly
        // what landed, including the created slot positions.
        let updated = self
            .inventories()
            .into_iter()
            .find(|inventory| inventory.path == *path)
            .ok_or_else(|| PlayerError::UnknownInventory(path.label().to_string()))?;
        let wanted: Vec<usize> = fills
            .iter()
            .map(|(position, _)| *position)
            .chain(updated.slots.iter().filter(|slot| created.contains(&slot.cell)).map(|slot| slot.position))
            .collect();
        Ok(updated.slots.into_iter().filter(|slot| wanted.contains(&slot.position)).collect())
    }

    /// Remove the slot at `index` (a position in the `Slots` array).
    pub fn remove_item(&mut self, path: &InventoryPath, index: usize) -> Result<ItemSlot, PlayerError> {
        let inventory = self
            .inventories()
            .into_iter()
            .find(|inventory| inventory.path == *path)
            .ok_or_else(|| PlayerError::UnknownInventory(path.label().to_string()))?;
        let slot = inventory.slots.iter().find(|slot| slot.position == index).cloned().ok_or(
            PlayerError::NoSuchSlot {
                inventory: path.label().to_string(),
                index,
                len: inventory.slots.len(),
            },
        )?;

        let tree = self.document.tree_raw_mut();
        let slots = resolve_mut(tree, &path.steps)
            .and_then(|value| value.as_object_mut())
            .and_then(|object| named_key_mut(object, &self.keys, "Slots"))
            .and_then(Value::as_array_mut)
            .ok_or_else(|| PlayerError::UnknownInventory(path.label().to_string()))?;
        slots.remove(index);
        self.document.mark_dirty();
        Ok(slot)
    }

    /// Set the amount in a slot.
    pub fn set_amount(&mut self, path: &InventoryPath, index: usize, amount: i64) -> Result<ItemSlot, PlayerError> {
        let inventory = self
            .inventories()
            .into_iter()
            .find(|inventory| inventory.path == *path)
            .ok_or_else(|| PlayerError::UnknownInventory(path.label().to_string()))?;
        let slot = inventory.slots.iter().find(|slot| slot.position == index).ok_or(PlayerError::NoSuchSlot {
            inventory: path.label().to_string(),
            index,
            len: inventory.slots.len(),
        })?;
        // Technology uses -1 for "no charge"; everything else is 0..=max.
        let lowest = if slot.kind == ItemKind::Technology { -1 } else { 0 };
        if amount < lowest || amount > slot.max_amount {
            return Err(PlayerError::AmountOutOfRange {
                id: slot.id.clone(),
                amount,
                max: slot.max_amount,
            });
        }
        if slot.amount == amount {
            return Ok(slot.clone());
        }

        let tree = self.document.tree_raw_mut();
        let slot_value = resolve_mut(tree, &path.steps)
            .and_then(|value| value.as_object_mut())
            .and_then(|object| named_key_mut(object, &self.keys, "Slots"))
            .and_then(Value::as_array_mut)
            .and_then(|slots| slots.get_mut(index))
            .ok_or_else(|| PlayerError::NoSuchSlot {
                inventory: path.label().to_string(),
                index,
                len: inventory.slots.len(),
            })?;
        set_field(slot_value, &self.keys, "Amount", Value::from(amount));
        self.document.mark_dirty();
        let mut slot = slot.clone();
        slot.amount = amount;
        Ok(slot)
    }

    /// Repair a damaged slot: clear its damage factor and mark it installed.
    pub fn repair(&mut self, path: &InventoryPath, index: usize) -> Result<bool, PlayerError> {
        let inventory = self
            .inventories()
            .into_iter()
            .find(|inventory| inventory.path == *path)
            .ok_or_else(|| PlayerError::UnknownInventory(path.label().to_string()))?;
        let slot = inventory.slots.iter().find(|slot| slot.position == index).ok_or(PlayerError::NoSuchSlot {
            inventory: path.label().to_string(),
            index,
            len: inventory.slots.len(),
        })?;
        if !slot.damaged() {
            return Ok(false);
        }

        let tree = self.document.tree_raw_mut();
        let slot_value = resolve_mut(tree, &path.steps)
            .and_then(|value| value.as_object_mut())
            .and_then(|object| named_key_mut(object, &self.keys, "Slots"))
            .and_then(Value::as_array_mut)
            .and_then(|slots| slots.get_mut(index))
            .ok_or_else(|| PlayerError::NoSuchSlot {
                inventory: path.label().to_string(),
                index,
                len: inventory.slots.len(),
            })?;
        set_field(slot_value, &self.keys, "DamageFactor", Value::from(0.0));
        set_field(slot_value, &self.keys, "FullyInstalled", Value::Bool(true));
        self.document.mark_dirty();
        Ok(true)
    }

    /// Repair every damaged slot in every player-facing inventory.
    ///
    /// Returns the number of slots repaired.
    pub fn repair_all(&mut self) -> Result<usize, EditError> {
        let damaged: Vec<(InventoryPath, usize)> = self
            .inventories()
            .into_iter()
            .filter(|inventory| inventory.role == InventoryRole::Player)
            .flat_map(|inventory| {
                let path = inventory.path.clone();
                inventory
                    .slots
                    .into_iter()
                    .filter(ItemSlot::damaged)
                    .map(move |slot| (path.clone(), slot.position))
            })
            .collect();
        let mut repaired = 0usize;
        for (path, index) in damaged {
            if self.repair(&path, index)? {
                repaired += 1;
            }
        }
        Ok(repaired)
    }

    /// Move a slot to another cell in the same inventory.
    pub fn move_item(&mut self, path: &InventoryPath, index: usize, cell: Cell) -> Result<bool, PlayerError> {
        let inventory = self
            .inventories()
            .into_iter()
            .find(|inventory| inventory.path == *path)
            .ok_or_else(|| PlayerError::UnknownInventory(path.label().to_string()))?;
        let slot = inventory.slots.iter().find(|slot| slot.position == index).ok_or(PlayerError::NoSuchSlot {
            inventory: path.label().to_string(),
            index,
            len: inventory.slots.len(),
        })?;
        if !inventory.valid_cells.contains(&cell) {
            return Err(PlayerError::CellNotInInventory { inventory: path.label().to_string(), cell });
        }
        // The slot itself sits in the target cell when nothing is to be done;
        // that check has to come before the occupancy check, which would
        // otherwise report the slot as blocking itself.
        if slot.cell == cell {
            return Ok(false);
        }
        if let Some(other) = inventory.slots.iter().find(|other| other.cell == cell) {
            return Err(PlayerError::CellOccupied {
                inventory: path.label().to_string(),
                cell,
                position: other.position,
            });
        }

        let tree = self.document.tree_raw_mut();
        let index_value = resolve_mut(tree, &path.steps)
            .and_then(|value| value.as_object_mut())
            .and_then(|object| named_key_mut(object, &self.keys, "Slots"))
            .and_then(Value::as_array_mut)
            .and_then(|slots| slots.get_mut(index))
            .and_then(|slot| named_key_mut(slot.as_object_mut()?, &self.keys, "Index"))
            .ok_or_else(|| PlayerError::NoSuchSlot {
                inventory: path.label().to_string(),
                index,
                len: inventory.slots.len(),
            })?;
        set_field(index_value, &self.keys, "X", Value::from(cell.x));
        set_field(index_value, &self.keys, "Y", Value::from(cell.y));
        self.document.mark_dirty();
        Ok(true)
    }

    // -- knowledge lists ---------------------------------------------------

    /// The ids in one of the "known" lists.
    pub fn known(&self, list: KnownList) -> Result<Vec<String>, PlayerError> {
        let value = self.known_value(list)?;
        let items = value.as_array().ok_or_else(|| PlayerError::UnknownList(list.label().to_string()))?;
        Ok(items.iter().filter_map(Value::as_str).map(str::to_string).collect())
    }

    /// Add or remove an id in a "known" list. Returns whether anything changed.
    pub fn set_known(&mut self, list: KnownList, id: &str, known: bool) -> Result<bool, EditError> {
        let value = normalize_id(id);
        if value.len() <= 1 {
            return Err(PlayerError::InvalidId(id.to_string()).into());
        }
        let current = self.known(list)?;
        let present = current.iter().any(|entry| entry.eq_ignore_ascii_case(&value));
        if present == known {
            return Ok(false);
        }

        let context = self.player_context().ok_or(PlayerError::NoPlayerState)?;
        let tree = self.document.tree_raw_mut();
        let list_value = resolve_named_mut(tree, &self.keys, &[context, "PlayerStateData", list.field()])
            .ok_or_else(|| PlayerError::UnknownList(list.label().to_string()))?;
        let items = match list_value {
            Value::Array(items) => items,
            _ => return Err(PlayerError::UnknownList(list.label().to_string()).into()),
        };
        if known {
            items.push(Value::String(value));
        } else {
            items.retain(|entry| entry.as_str().is_none_or(|entry| !entry.eq_ignore_ascii_case(&value)));
        }
        self.document.mark_dirty();
        Ok(true)
    }

    fn known_value(&self, list: KnownList) -> Result<&Value, PlayerError> {
        self.player_state_value(list.field()).ok_or_else(|| PlayerError::UnknownList(list.label().to_string()))
    }
}

/// The ids the game records as learned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownList {
    /// Known products and base parts (`KnownProducts`).
    Products,
    /// Known technology (`KnownTech`).
    Technology,
    /// Known specials — cosmetics, figurines, titles (`KnownSpecials`).
    Specials,
    /// Base-building parts seen (`SeenBaseBuildingObjects`).
    BaseParts,
    /// Refiner recipes (`KnownRefinerRecipes`).
    RefinerRecipes,
}

impl KnownList {
    /// All lists, in display order.
    pub const ALL: [KnownList; 5] =
        [KnownList::Products, KnownList::Technology, KnownList::Specials, KnownList::BaseParts, KnownList::RefinerRecipes];

    /// The on-disk field name.
    pub fn field(self) -> &'static str {
        match self {
            KnownList::Products => "KnownProducts",
            KnownList::Technology => "KnownTech",
            KnownList::Specials => "KnownSpecials",
            KnownList::BaseParts => "SeenBaseBuildingObjects",
            KnownList::RefinerRecipes => "KnownRefinerRecipes",
        }
    }

    /// A lowercase name for command lines.
    pub fn label(self) -> &'static str {
        match self {
            KnownList::Products => "products",
            KnownList::Technology => "technology",
            KnownList::Specials => "specials",
            KnownList::BaseParts => "base parts",
            KnownList::RefinerRecipes => "refiner recipes",
        }
    }

    /// Parse a command-line name.
    pub fn parse(text: &str) -> Option<KnownList> {
        match text.to_ascii_lowercase().replace(['-', '_'], " ").as_str() {
            "products" | "product" | "known" => Some(KnownList::Products),
            "technology" | "tech" => Some(KnownList::Technology),
            "specials" | "special" => Some(KnownList::Specials),
            "base parts" | "baseparts" | "parts" => Some(KnownList::BaseParts),
            "refiner recipes" | "refiner" | "recipes" => Some(KnownList::RefinerRecipes),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn named_key_mut<'a>(object: &'a mut Map<String, Value>, keys: &KeyMap, name: &str) -> Option<&'a mut Value> {
    let key = keys.candidates(name).find(|candidate| object.contains_key(*candidate))?.to_string();
    object.get_mut(&key)
}

fn set_field(object_value: &mut Value, keys: &KeyMap, name: &str, value: Value) {
    if let Some(object) = object_value.as_object_mut() {
        let key = match keys.candidates(name).find(|candidate| object.contains_key(*candidate)) {
            Some(existing) => existing.to_string(),
            None => creation_key(keys, name),
        };
        object.insert(key, value);
    }
}

/// The item id as the game writes it: uppercase, with the `^` marker.
pub fn normalize_id(id: &str) -> String {
    format!("^{}", id.trim().trim_start_matches(['^', '!']).to_ascii_uppercase())
}

/// The one item kind an inventory holds, if it holds only one.
fn single_kind(inventory: &Inventory) -> Option<ItemKind> {
    let mut kinds = inventory.slots.iter().map(|slot| slot.kind);
    let first = kinds.next()?;
    kinds.all(|kind| kind == first).then_some(first)
}

/// Build a slot object with the game's field order.
fn new_slot_value(
    keys: &KeyMap,
    id: &str,
    kind: ItemKind,
    cell: Cell,
    amount: i64,
    max_amount: i64,
    damage: bool,
) -> Value {
    let mut slot_type = Map::new();
    slot_type.insert(creation_key(keys, "InventoryType"), Value::String(kind.label().to_string()));

    let mut index = Map::new();
    index.insert(creation_key(keys, "X"), Value::from(cell.x));
    index.insert(creation_key(keys, "Y"), Value::from(cell.y));

    let mut slot = Map::new();
    slot.insert(creation_key(keys, "Type"), Value::Object(slot_type));
    slot.insert(creation_key(keys, "Id"), Value::String(id.to_string()));
    slot.insert(creation_key(keys, "Amount"), Value::from(amount));
    slot.insert(creation_key(keys, "MaxAmount"), Value::from(max_amount));
    slot.insert(creation_key(keys, "DamageFactor"), Value::from(if damage { 1.0 } else { 0.0 }));
    slot.insert(creation_key(keys, "FullyInstalled"), Value::Bool(true));
    slot.insert(creation_key(keys, "AddedAutomatically"), Value::Bool(false));
    slot.insert(creation_key(keys, "Index"), Value::Object(index));
    Value::Object(slot)
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use nms_format::metadata::Slot;
    use nms_format::SaveFile;
    use nms_model::Document;
    use std::path::PathBuf;

    /// A synthetic save with the shapes a real one has: an exosuit inventory
    /// with two occupied cells, an empty ship grid with a separate technology
    /// grid, a multitool store, a machine-owned maintenance container whose
    /// stack sizes are deliberately unusual, and the knowledge lists.
    const SAVE: &str = r#"{
        "Version": 4743,
        "Platform": "Win|Final",
        "ActiveContext": "Main",
        "BaseContext": {
            "GameMode": "Normal",
            "PlayerStateData": {
                "Units": -1,
                "Nanites": 500,
                "Specials": 25,
                "Inventory": {
                    "Slots": [
                        {"Type":{"InventoryType":"Substance"},"Id":"^CARBON","Amount":10,"MaxAmount":20,"DamageFactor":0.0,"FullyInstalled":true,"AddedAutomatically":false,"Index":{"X":0,"Y":0}},
                        {"Type":{"InventoryType":"Product"},"Id":"^WARP","Amount":3,"MaxAmount":5,"DamageFactor":1.0,"FullyInstalled":false,"AddedAutomatically":false,"Index":{"X":1,"Y":0}}
                    ],
                    "ValidSlotIndices": [{"X":0,"Y":0},{"X":1,"Y":0},{"X":0,"Y":1},{"X":1,"Y":1}],
                    "Class": {"InventoryClass": "C"},
                    "StackSizeGroup": {"InventoryStackSizeGroup": "Personal"},
                    "Width": 2,
                    "Height": 2
                },
                "ShipOwnership": [
                    {
                        "Inventory": {
                            "Slots": [],
                            "ValidSlotIndices": [{"X":0,"Y":0},{"X":1,"Y":0}],
                            "Class": {"InventoryClass": "B"},
                            "StackSizeGroup": {"InventoryStackSizeGroup": "Ship"},
                            "Width": 2,
                            "Height": 1
                        },
                        "Inventory_TechOnly": {
                            "Slots": [],
                            "ValidSlotIndices": [{"X":0,"Y":0}],
                            "Class": {"InventoryClass": "B"},
                            "StackSizeGroup": {"InventoryStackSizeGroup": "Ship"},
                            "Width": 1,
                            "Height": 1
                        }
                    }
                ],
                "Multitools": [
                    {
                        "Store": {
                            "Slots": [],
                            "ValidSlotIndices": [{"X":0,"Y":0}],
                            "Class": {"InventoryClass": "A"},
                            "StackSizeGroup": {"InventoryStackSizeGroup": "Default"},
                            "Width": 1,
                            "Height": 1
                        }
                    }
                ],
                "MaintenanceInteractions": [
                    {
                        "InventoryContainer": {
                            "Slots": [
                                {"Type":{"InventoryType":"Substance"},"Id":"^CARBON","Amount":-1,"MaxAmount":101,"DamageFactor":0.0,"FullyInstalled":true,"AddedAutomatically":false,"Index":{"X":0,"Y":0}}
                            ],
                            "ValidSlotIndices": [],
                            "Class": {"InventoryClass": "C"},
                            "StackSizeGroup": {"InventoryStackSizeGroup": "MaintenanceObject"},
                            "Width": 16,
                            "Height": 1
                        }
                    }
                ],
                "KnownProducts": ["^A", "^B"],
                "KnownTech": ["^T"],
                "KnownSpecials": [],
                "SeenBaseBuildingObjects": [],
                "KnownRefinerRecipes": []
            }
        }
    }"#;

    fn bundle_from_payload(payload: &[u8]) -> SaveBundle {
        let save = SaveFile::from_bytes(payload).expect("synthetic save parses");
        let document = Document::parse(&save.payload).expect("synthetic payload parses");
        SaveBundle {
            slot: Slot::from_file_name("save.hg").expect("slot name"),
            data_path: PathBuf::from("save.hg"),
            metadata_path: PathBuf::from("mf_save.hg"),
            save,
            document,
            metadata: None,
            keys: KeyMap::builtin(),
        }
    }

    fn bundle(json: &str) -> SaveBundle {
        bundle_from_payload(json.as_bytes())
    }

    /// Serialise and re-parse, so a test can prove a change survives the file
    /// format rather than only the in-memory tree.
    fn reload(bundle: &SaveBundle) -> SaveBundle {
        let payload = bundle.document.payload().expect("serialise edited document");
        bundle_from_payload(&payload)
    }

    fn inv(bundle: &SaveBundle, name: &str) -> Inventory {
        bundle.inventory(name).unwrap_or_else(|| panic!("inventory {name} not found"))
    }

    // -- currencies --------------------------------------------------------

    #[test]
    fn currencies_decode_the_cap_as_unsigned() {
        let bundle = bundle(SAVE);
        let currencies = bundle.currencies().expect("currencies");
        assert_eq!(currencies.units, u32::MAX, "the game's `-1` is the u32 cap");
        assert_eq!(currencies.nanites, 500);
        assert_eq!(currencies.quicksilver, 25, "quicksilver lives in `Specials`");
    }

    #[test]
    fn setting_a_currency_writes_the_games_signed_form() {
        let mut bundle = bundle(SAVE);
        assert!(bundle.set_currency(Currency::Units, 1234).expect("set units"));
        let text = String::from_utf8(bundle.document.payload().unwrap()).unwrap();
        assert!(text.contains("\"Units\":1234"), "units written: {text}");
        assert_eq!(reload(&bundle).currencies().unwrap().units, 1234);

        assert!(bundle.set_currency(Currency::Nanites, u32::MAX).expect("set nanites"));
        let text = String::from_utf8(bundle.document.payload().unwrap()).unwrap();
        assert!(text.contains("\"Nanites\":-1"), "the cap must be written as -1: {text}");
        assert_eq!(reload(&bundle).currencies().unwrap().nanites, u32::MAX);
    }

    #[test]
    fn setting_a_currency_to_its_current_value_changes_nothing() {
        let mut bundle = bundle(SAVE);
        assert!(!bundle.set_currency(Currency::Nanites, 500).expect("no-op"));
        assert!(!bundle.document.is_dirty(), "a no-op must not mark the document dirty");
    }

    #[test]
    fn a_save_without_player_state_reports_it() {
        let mut bundle = bundle(r#"{"Version":4743}"#);
        assert!(matches!(bundle.currencies(), Err(PlayerError::NoPlayerState)));
        assert!(bundle.inventories().is_empty());
        assert!(matches!(
            bundle.set_currency(Currency::Units, 1),
            Err(EditError::Player(PlayerError::NoPlayerState))
        ));
    }

    #[test]
    fn expedition_saves_use_the_same_player_state_paths() {
        let expedition = SAVE.replace("\"BaseContext\"", "\"ExpeditionContext\"");
        let bundle = bundle(&expedition);
        assert_eq!(bundle.player_context(), Some("ExpeditionContext"));
        assert_eq!(bundle.currencies().unwrap().units, u32::MAX);
        let inventory = inv(&bundle, "Inventory");
        assert_eq!(inventory.path.label(), "Inventory");
    }

    // -- discovery ---------------------------------------------------------

    #[test]
    fn inventories_are_found_where_the_game_puts_them() {
        let bundle = bundle(SAVE);
        let found: Vec<(String, InventoryRole)> = bundle
            .inventories()
            .into_iter()
            .map(|inventory| (inventory.path.label().to_string(), inventory.role))
            .collect();
        let expected = [
            ("Inventory".to_string(), InventoryRole::Player),
            ("ShipOwnership[0].Inventory".to_string(), InventoryRole::Player),
            ("ShipOwnership[0].Inventory_TechOnly".to_string(), InventoryRole::Player),
            ("Multitools[0].Store".to_string(), InventoryRole::Player),
            ("MaintenanceInteractions[0].InventoryContainer".to_string(), InventoryRole::Auxiliary),
        ];
        assert_eq!(found, expected);
    }

    #[test]
    fn an_inventory_can_be_named_with_or_without_the_player_state_prefix() {
        let bundle = bundle(SAVE);
        assert!(bundle.inventory("Inventory").is_some());
        assert!(bundle.inventory("PlayerStateData.Inventory").is_some());
        assert!(bundle.inventory("ShipOwnership[0].Inventory_TechOnly").is_some());
        assert!(bundle.inventory("NotAnInventory").is_none());
    }

    #[test]
    fn free_cells_are_the_valid_cells_without_items() {
        let bundle = bundle(SAVE);
        let inventory = inv(&bundle, "Inventory");
        assert_eq!(inventory.width, 2);
        assert_eq!(inventory.height, 2);
        assert_eq!(inventory.stack_group, "Personal");
        assert_eq!(inventory.class, "C");
        assert_eq!(inventory.free_cells(), vec![Cell::new(0, 1), Cell::new(1, 1)]);
        assert!(!inventory.is_full());
        assert_eq!(inventory.count(ItemKind::Substance), 1);
        assert_eq!(inventory.count(ItemKind::Product), 1);
    }

    #[test]
    fn an_empty_valid_slot_list_means_the_whole_grid() {
        // Machine-owned containers write `ValidSlotIndices: []` even though
        // they hold items, so the grid itself has to stand in for the list.
        let bundle = bundle(SAVE);
        let inventory = inv(&bundle, "MaintenanceInteractions[0].InventoryContainer");
        assert_eq!(inventory.valid_cells.len(), 16);
        assert_eq!(inventory.slots.len(), 1);
        assert_eq!(inventory.free_cells().len(), 15);
        assert!(!inventory.is_full());
    }

    #[test]
    fn machine_owned_containers_are_auxiliary() {
        // All four hosts that hold repair ingredients or refiner buffers. Their
        // role keeps them out of stack-size statistics, so a host that is
        // classified wrongly would silently pollute the next new item.
        let hosts = r#"{
            "Version": 4743,
            "BaseContext": {
                "PlayerStateData": {
                    "MaintenanceInteractions": [{"InventoryContainer": CONTAINER}],
                    "PersonalMaintenanceInteractions": [{"InventoryContainer": CONTAINER}],
                    "RefinerBufferData": [{"InventoryContainer": CONTAINER}],
                    "RepairTechBuffer": [{"MaintenanceContainer": {"InventoryContainer": CONTAINER}}],
                    "Inventory": CONTAINER
                }
            }
        }"#
        .replace("CONTAINER", r#"{"Slots":[],"ValidSlotIndices":[],"Width":1,"Height":1}"#);
        let bundle = bundle(&hosts);
        let roles: Vec<(String, InventoryRole)> = bundle
            .inventories()
            .into_iter()
            .map(|inventory| (inventory.path.label().to_string(), inventory.role))
            .collect();
        for (label, role) in &roles {
            let expected = if label == "Inventory" { InventoryRole::Player } else { InventoryRole::Auxiliary };
            assert_eq!(*role, expected, "{label} has the wrong role");
        }
        assert_eq!(roles.len(), 5);
    }

    #[test]
    fn an_inventory_resolves_by_label_or_by_a_longer_path() {
        // One player inventory and one machine container, whose labels share a
        // word but not a path boundary.
        let payload = r#"{
            "Version": 4743,
            "BaseContext": {
                "PlayerStateData": {
                    "Inventory": {"Slots":[],"ValidSlotIndices":[],"Width":1,"Height":1},
                    "RepairTechBuffer": [
                        {"MaintenanceContainer": {"Slots":[],"ValidSlotIndices":[],"Width":1,"Height":1}}
                    ]
                }
            }
        }"#;
        let bundle = bundle(payload);

        let labels: Vec<String> =
            bundle.inventories().iter().map(|inventory| inventory.path.label().to_string()).collect();
        assert!(labels.contains(&"Inventory".to_string()), "{labels:?}");

        // Exact label, with or without the player-state prefix.
        assert!(bundle.inventory("Inventory").is_some());
        assert!(bundle.inventory("PlayerStateData.Inventory").is_some());
        assert!(bundle.inventory("inventory").is_some(), "labels compare case-insensitively");

        // A path copied out of the JSON tree ends in exactly one label.
        assert!(bundle.inventory("BaseContext.PlayerStateData.Inventory").is_some());
        // A bare word that only shares a suffix with a label does not match.
        assert!(bundle.inventory("Container").is_none(), "must match at a dot boundary");
        assert!(bundle.inventory("ventory").is_none());
    }

    #[test]
    fn a_slot_reports_damage_and_installation_independently() {
        let bundle = bundle(SAVE);
        let inventory = inv(&bundle, "Inventory");
        let warp = inventory.slots.iter().find(|slot| slot.id == "^WARP").unwrap();
        assert_eq!(warp.damage, 1.0);
        assert!(!warp.installed);
        assert!(warp.damaged());
        let carbon = inventory.slots.iter().find(|slot| slot.id == "^CARBON").unwrap();
        assert!(!carbon.damaged());
    }

    // -- stack sizes -------------------------------------------------------

    #[test]
    fn stack_sizes_come_from_the_save_itself() {
        let bundle = bundle(SAVE);
        let inventory = inv(&bundle, "Inventory");
        assert_eq!(
            bundle.suggest_stack_size(&inventory.path, "carbon", ItemKind::Substance),
            Some(StackSize { value: 20, source: StackSizeSource::SameItem })
        );
        assert_eq!(
            bundle.suggest_stack_size(&inventory.path, "oxygen", ItemKind::Substance),
            Some(StackSize { value: 20, source: StackSizeSource::InventoryKind })
        );
        let ship = inv(&bundle, "ShipOwnership[0].Inventory");
        assert_eq!(bundle.suggest_stack_size(&ship.path, "oxygen", ItemKind::Substance), None);
    }

    #[test]
    fn maintenance_containers_do_not_supply_stack_sizes() {
        let bundle = bundle(SAVE);
        // The maintenance container holds CARBON at 101, which must never be
        // mistaken for the item's real stack size.
        let inventory = inv(&bundle, "Inventory");
        assert_eq!(
            bundle.suggest_stack_size(&inventory.path, "carbon", ItemKind::Substance),
            Some(StackSize { value: 20, source: StackSizeSource::SameItem })
        );
    }

    // -- adding ------------------------------------------------------------

    #[test]
    fn adding_an_item_fills_an_existing_stack_then_uses_free_cells() {
        let mut bundle = bundle(SAVE);
        let inventory = inv(&bundle, "Inventory");
        let added = bundle
            .add_item(&inventory.path, &NewItem::new("carbon", 15))
            .expect("add carbon");
        assert_eq!(added.len(), 2);
        assert_eq!((added[0].position, added[0].amount), (0, 20));
        assert_eq!((added[1].cell, added[1].amount), (Cell::new(0, 1), 5));
        assert!(bundle.document.is_dirty());

        let reloaded = reload(&bundle);
        let inventory = inv(&reloaded, "Inventory");
        assert_eq!(inventory.slots.len(), 3);
        assert_eq!(inventory.slots[0].amount, 20);
        assert_eq!(inventory.slots[2].cell, Cell::new(0, 1));
        assert_eq!(inventory.slots[2].amount, 5);
        assert_eq!(inventory.slots[2].max_amount, 20);
        assert_eq!(inventory.slots[2].kind, ItemKind::Substance);
    }

    #[test]
    fn adding_more_than_fits_is_refused_without_touching_the_save() {
        let mut bundle = bundle(SAVE);
        let inventory = inv(&bundle, "Inventory");
        // 10 + 65 = 75 needs 4 stacks of 20 (80) but only 3 free cells exist.
        let error = bundle.add_item(&inventory.path, &NewItem::new("carbon", 65)).unwrap_err();
        assert_eq!(error, PlayerError::InventoryFull { inventory: "Inventory".to_string() });
        assert!(!bundle.document.is_dirty(), "a refused edit must not dirty the document");
        assert_eq!(inv(&bundle, "Inventory").slots.len(), 2, "no partial write");
    }

    #[test]
    fn a_new_item_needs_a_kind_when_the_inventory_does_not_imply_one() {
        let mut bundle = bundle(SAVE);
        let inventory = inv(&bundle, "Inventory");
        assert_eq!(
            bundle.add_item(&inventory.path, &NewItem::new("oxygen", 5)).unwrap_err(),
            PlayerError::UnknownKind("^OXYGEN".to_string())
        );
        assert!(!bundle.document.is_dirty());
    }

    #[test]
    fn a_new_item_with_a_kind_gets_the_inventorys_stack_size() {
        let mut bundle = bundle(SAVE);
        let inventory = inv(&bundle, "Inventory");
        let mut item = NewItem::new("oxygen", 5);
        item.kind = Some(ItemKind::Substance);
        let added = bundle.add_item(&inventory.path, &item).expect("add oxygen");
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].max_amount, 20);
        assert_eq!(added[0].amount, 5);
    }

    #[test]
    fn a_failed_add_can_be_followed_by_a_valid_one() {
        let mut bundle = bundle(SAVE);
        let inventory = inv(&bundle, "Inventory");
        assert!(bundle.add_item(&inventory.path, &NewItem::new("carbon", 65)).is_err());
        // The refused plan must not have filled the existing stack either.
        let added = bundle.add_item(&inventory.path, &NewItem::new("carbon", 5)).expect("second add");
        assert_eq!(added.len(), 1);
        assert_eq!((added[0].position, added[0].amount), (0, 15));
    }

    #[test]
    fn an_item_can_be_put_in_a_named_cell() {
        let mut bundle = bundle(SAVE);
        let inventory = inv(&bundle, "Inventory");
        let mut item = NewItem::new("oxygen", 5);
        item.kind = Some(ItemKind::Substance);
        item.cell = Some(Cell::new(1, 1));
        let added = bundle.add_item(&inventory.path, &item).expect("add to cell");
        assert_eq!(added[0].cell, Cell::new(1, 1));

        let mut item = NewItem::new("oxygen", 5);
        item.kind = Some(ItemKind::Substance);
        item.cell = Some(Cell::new(0, 0));
        assert_eq!(
            bundle.add_item(&inventory.path, &item).unwrap_err(),
            PlayerError::CellOccupied { inventory: "Inventory".to_string(), cell: Cell::new(0, 0), position: 0 }
        );

        let mut item = NewItem::new("oxygen", 5);
        item.kind = Some(ItemKind::Substance);
        item.cell = Some(Cell::new(7, 7));
        assert_eq!(
            bundle.add_item(&inventory.path, &item).unwrap_err(),
            PlayerError::CellNotInInventory { inventory: "Inventory".to_string(), cell: Cell::new(7, 7) }
        );
    }

    #[test]
    fn a_ship_inventory_accepts_an_explicit_stack_size() {
        let mut bundle = bundle(SAVE);
        let inventory = inv(&bundle, "ShipOwnership[0].Inventory");
        let mut item = NewItem::new("^WIDGET", 7);
        item.kind = Some(ItemKind::Product);
        item.max_amount = Some(7);
        let added = bundle.add_item(&inventory.path, &item).expect("add to ship");
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].max_amount, 7);
        assert_eq!(added[0].kind, ItemKind::Product);
    }

    #[test]
    fn technology_gets_one_slot_with_no_charge() {
        let mut bundle = bundle(SAVE);
        let inventory = inv(&bundle, "Multitools[0].Store");
        let mut item = NewItem::new("up_scan", 1);
        item.kind = Some(ItemKind::Technology);
        item.max_amount = Some(100);
        let added = bundle.add_item(&inventory.path, &item).expect("add technology");
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].amount, -1, "the game writes -1 for uncharged technology");
        assert_eq!(added[0].max_amount, 100);
        assert_eq!(added[0].id, "^UP_SCAN");

        let reloaded = reload(&bundle);
        let inventory = inv(&reloaded, "Multitools[0].Store");
        assert_eq!(inventory.slots[0].amount, -1);
        assert_eq!(inventory.slots[0].kind, ItemKind::Technology);
    }

    #[test]
    fn technology_cannot_be_added_in_quantities() {
        let mut bundle = bundle(SAVE);
        let inventory = inv(&bundle, "Multitools[0].Store");
        let mut item = NewItem::new("up_scan", 2);
        item.kind = Some(ItemKind::Technology);
        item.max_amount = Some(100);
        assert_eq!(
            bundle.add_item(&inventory.path, &item).unwrap_err(),
            PlayerError::AmountOutOfRange { id: "^UP_SCAN".to_string(), amount: 2, max: 1 }
        );
        assert!(!bundle.document.is_dirty());
    }

    #[test]
    fn a_full_inventory_refuses_new_items() {
        let mut bundle = bundle(SAVE);
        let inventory = inv(&bundle, "ShipOwnership[0].Inventory_TechOnly");
        let mut item = NewItem::new("widget", 100);
        item.kind = Some(ItemKind::Product);
        item.max_amount = Some(5);
        assert_eq!(
            bundle.add_item(&inventory.path, &item).unwrap_err(),
            PlayerError::InventoryFull { inventory: "ShipOwnership[0].Inventory_TechOnly".to_string() }
        );
        assert!(!bundle.document.is_dirty());
    }

    #[test]
    fn an_unknown_inventory_is_reported_by_name() {
        let mut bundle = bundle(SAVE);
        // A path that cannot resolve, built by hand so it is certainly absent.
        let other = InventoryPath { steps: vec![Step::Key("Nope".to_string())], label: "Nope".to_string() };
        assert_eq!(
            bundle.add_item(&other, &NewItem::new("carbon", 1)).unwrap_err(),
            PlayerError::UnknownInventory("Nope".to_string())
        );
    }

    // -- removing, amounts, repair, moving ---------------------------------

    #[test]
    fn removing_a_slot_deletes_exactly_that_slot() {
        let mut bundle = bundle(SAVE);
        let inventory = inv(&bundle, "Inventory");
        let removed = bundle.remove_item(&inventory.path, 0).expect("remove");
        assert_eq!(removed.id, "^CARBON");
        assert!(bundle.document.is_dirty());

        let reloaded = reload(&bundle);
        let inventory = inv(&reloaded, "Inventory");
        assert_eq!(inventory.slots.len(), 1);
        assert_eq!(inventory.slots[0].id, "^WARP");
    }

    #[test]
    fn removing_a_missing_slot_fails_without_dirtying() {
        let mut bundle = bundle(SAVE);
        let inventory = inv(&bundle, "Inventory");
        assert_eq!(
            bundle.remove_item(&inventory.path, 9).unwrap_err(),
            PlayerError::NoSuchSlot { inventory: "Inventory".to_string(), index: 9, len: 2 }
        );
        assert!(!bundle.document.is_dirty());
    }

    #[test]
    fn amounts_are_validated_against_the_slot() {
        let mut bundle = bundle(SAVE);
        let inventory = inv(&bundle, "Inventory");
        assert_eq!(
            bundle.set_amount(&inventory.path, 1, 6).unwrap_err(),
            PlayerError::AmountOutOfRange { id: "^WARP".to_string(), amount: 6, max: 5 }
        );
        assert_eq!(
            bundle.set_amount(&inventory.path, 0, -1).unwrap_err(),
            PlayerError::AmountOutOfRange { id: "^CARBON".to_string(), amount: -1, max: 20 }
        );
        assert!(!bundle.document.is_dirty());

        let slot = bundle.set_amount(&inventory.path, 1, 0).expect("set to zero");
        assert_eq!(slot.amount, 0);
        assert_eq!(reload(&bundle).inventory("Inventory").unwrap().slots[1].amount, 0);
    }

    #[test]
    fn technology_accepts_the_no_charge_amount() {
        let mut bundle = bundle(SAVE);
        let inventory = inv(&bundle, "Multitools[0].Store");
        let mut item = NewItem::new("up_scan", 1);
        item.kind = Some(ItemKind::Technology);
        item.max_amount = Some(100);
        bundle.add_item(&inventory.path, &item).expect("add technology");
        let slot = bundle.set_amount(&inventory.path, 0, 50).expect("charge");
        assert_eq!(slot.amount, 50);
        bundle.set_amount(&inventory.path, 0, -1).expect("no charge");
        assert_eq!(reload(&bundle).inventory("Multitools[0].Store").unwrap().slots[0].amount, -1);
    }

    #[test]
    fn repair_clears_damage_and_installation() {
        let mut bundle = bundle(SAVE);
        let inventory = inv(&bundle, "Inventory");
        assert!(bundle.repair(&inventory.path, 1).expect("repair"));
        assert!(bundle.document.is_dirty());
        assert!(!bundle.repair(&inventory.path, 1).expect("second repair"), "already repaired");

        let reloaded = reload(&bundle);
        let inventory = inv(&reloaded, "Inventory");
        assert_eq!(inventory.slots[1].damage, 0.0);
        assert!(inventory.slots[1].installed);
        assert!(!inventory.slots[1].damaged());
    }

    #[test]
    fn repair_all_covers_player_inventories_only() {
        let mut bundle = bundle(SAVE);
        assert_eq!(bundle.repair_all().expect("repair all"), 1);
        // The maintenance container is damaged nowhere; its `-1` substance
        // amount is normal there and must not be interpreted as damage.
        let inventory = inv(&bundle, "MaintenanceInteractions[0].InventoryContainer");
        assert_eq!(inventory.slots[0].amount, -1);
    }

    #[test]
    fn moving_a_slot_validates_the_target_cell() {
        let mut bundle = bundle(SAVE);
        let inventory = inv(&bundle, "Inventory");
        assert_eq!(
            bundle.move_item(&inventory.path, 0, Cell::new(5, 5)).unwrap_err(),
            PlayerError::CellNotInInventory { inventory: "Inventory".to_string(), cell: Cell::new(5, 5) }
        );
        assert_eq!(
            bundle.move_item(&inventory.path, 0, Cell::new(1, 0)).unwrap_err(),
            PlayerError::CellOccupied { inventory: "Inventory".to_string(), cell: Cell::new(1, 0), position: 1 }
        );
        assert!(!bundle.move_item(&inventory.path, 0, Cell::new(0, 0)).expect("same cell"));
        assert!(!bundle.document.is_dirty(), "rejected moves must not dirty the document");

        assert!(bundle.move_item(&inventory.path, 0, Cell::new(1, 1)).expect("move"));
        let reloaded = reload(&bundle);
        assert_eq!(inv(&reloaded, "Inventory").slots[0].cell, Cell::new(1, 1));
    }

    // -- known lists -------------------------------------------------------

    #[test]
    fn known_lists_can_be_read_and_edited() {
        let mut bundle = bundle(SAVE);
        assert_eq!(bundle.known(KnownList::Products).unwrap(), vec!["^A", "^B"]);

        assert!(bundle.set_known(KnownList::Products, "c", true).expect("learn"));
        assert_eq!(bundle.known(KnownList::Products).unwrap(), vec!["^A", "^B", "^C"]);
        assert!(!bundle.set_known(KnownList::Products, "^c", true).expect("already known"));

        assert!(bundle.set_known(KnownList::Products, "^C", false).expect("forget"));
        assert_eq!(bundle.known(KnownList::Products).unwrap(), vec!["^A", "^B"]);
        assert!(!bundle.set_known(KnownList::Products, "^C", false).expect("not known"));

        let reloaded = reload(&bundle);
        assert_eq!(reloaded.known(KnownList::Products).unwrap(), vec!["^A", "^B"]);
    }

    #[test]
    fn every_known_list_is_reachable() {
        let bundle = bundle(SAVE);
        for list in KnownList::ALL {
            assert!(bundle.known(list).is_ok(), "{} should be readable", list.label());
        }
    }

    #[test]
    fn a_missing_known_list_is_reported() {
        let mut bundle = bundle(&SAVE.replace("\"KnownProducts\": [\"^A\", \"^B\"],", ""));
        assert!(matches!(bundle.known(KnownList::Products), Err(PlayerError::UnknownList(_))));
        assert!(matches!(
            bundle.set_known(KnownList::Products, "^C", true),
            Err(EditError::Player(PlayerError::UnknownList(_)))
        ));
    }

    #[test]
    fn known_ids_are_normalised_like_slot_ids() {
        assert_eq!(normalize_id(" carbon "), "^CARBON");
        assert_eq!(normalize_id("^Carbon"), "^CARBON");
        assert_eq!(normalize_id("!CARBON"), "^CARBON");
        assert!(matches!(
            { let mut bundle = bundle(SAVE); bundle.set_known(KnownList::Products, "!", true).map_err(EditError::from) },
            Err(EditError::Player(PlayerError::InvalidId(_)))
        ));
    }

    #[test]
    fn currency_and_list_names_parse() {
        assert_eq!(Currency::parse("QS"), Some(Currency::Quicksilver));
        assert_eq!(Currency::parse("specials"), Some(Currency::Quicksilver));
        assert_eq!(Currency::parse("units"), Some(Currency::Units));
        assert_eq!(Currency::parse("credits"), None);
        assert_eq!(KnownList::parse("base-parts"), Some(KnownList::BaseParts));
        assert_eq!(KnownList::parse("recipes"), Some(KnownList::RefinerRecipes));
        assert_eq!(KnownList::parse("nonsense"), None);
        assert_eq!(ItemKind::parse("Substance"), Some(ItemKind::Substance));
        assert_eq!(ItemKind::parse("substance"), Some(ItemKind::Substance));
        assert_eq!(ItemKind::parse("element"), None);
    }
}
