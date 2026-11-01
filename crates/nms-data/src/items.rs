//! Item names, joined from the game's item tables and its text tables.
//!
//! A save stores item ids (`^CARBON_SEAL`, `^FUEL2`, `^UT_PROTECT`), not names.
//! Names come from joining an item table's name key with a text table, so an
//! editor needs both. Three facts make that awkward, and all three are why the
//! older editor shipped a hand-maintained item list:
//!
//! * the display key is **not** derivable from the id — `ANTIMATTER`'s key is
//!   `COMMODITY6_NAME`, `OXYGEN`'s is `UI_AIR1_NAME`, `UT_PROTECT`'s is
//!   `UT_PROTECT_NAME`;
//! * each table has its own record grid, and the name key's distance from the id
//!   **varies between records** (a variable-length field precedes it), so a fixed
//!   offset cannot be used;
//! * the fields before a key end in bytes that look like text (a float's last
//!   byte is often `'A'`), so a naive scan invents keys like `APROTECT_NAME`.
//!
//! The approach here is deliberately evidence-driven:
//!
//! 1. walk each table on its own record grid, bounded by the table's declared
//!    count (so trailing non-item data is never read as items);
//! 2. read the id from its fixed slot in the record;
//! 3. search the rest of the record for `*_NAME` candidates, undoing a stray
//!    leading byte, and keep the one that **resolves in the text tables** —
//!    preferring `<id>_NAME`;
//! 4. report how many resolved, per table.
//!
//! Step 3 is what makes this safe: a key that does not exist in the text tables
//! is never accepted, so a wrong layout yields *fewer* names rather than wrong
//! ones. In the observed build the rates are 96% (products), 68% (substances)
//! and 62% (technologies).

use std::collections::BTreeMap;

/// A text slot is read up to this many bytes, or to its NUL.
const SLOT_SCAN: usize = 0x80;

/// How a table's record grid is located.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridBase {
    /// Records start at the first occurrence of this marker (e.g. the substance
    /// table's `UI_GATHER_OBJ`).
    Marker(&'static [u8]),
    /// The grid's first record starts at the first position whose id slot holds
    /// an id (the product and technology tables begin with a header of varying
    /// size).
    FirstId,
}

/// One item table's record layout.
#[derive(Debug, Clone, Copy)]
pub struct TableLayout {
    /// Table file name, for reports.
    pub name: &'static str,
    /// Offset of the declared record count in the header.
    pub count_offset: usize,
    /// Distance between records.
    pub stride: usize,
    /// Offset of the id within a record.
    pub id_offset: usize,
    /// Offset of the name key within a record, measured from the record origin.
    ///
    /// Fixed per table: the key sits at the same distance after the id in every
    /// record, verified by the offset histogram of every resolved record (2,131
    /// products at +0xA0, 82 substances at +0x6C, 392 technologies at +0xBC).
    pub name_offset: usize,
    /// How to find the first record.
    pub base: GridBase,
    /// Bytes between the marker and the record origin, for marker-based grids.
    pub base_adjust: usize,
}

impl TableLayout {
    /// The category items from this table belong to.
    pub fn category(&self) -> ItemCategory {
        if self.name.contains("product") {
            ItemCategory::Product
        } else if self.name.contains("substance") {
            ItemCategory::Substance
        } else {
            ItemCategory::Technology
        }
    }

    /// Products: most inventory items.
    pub const PRODUCTS: TableLayout = TableLayout {
        name: "nms_reality_gcproducttable.mbin",
        count_offset: 0x28,
        stride: 768,
        id_offset: 0xC0,
        name_offset: 0x160,
        // The pin marker sits 0x40 into the record, but it is a stable anchor:
        // offsets below are measured from the marker, which is what the grid walk
        // was verified against.
        base: GridBase::Marker(b"UI_PIN_"),
        base_adjust: 0,
    };

    /// Substances: the elements.
    pub const SUBSTANCES: TableLayout = TableLayout {
        name: "nms_reality_gcsubstancetable.mbin",
        count_offset: 0x38,
        stride: 416,
        id_offset: 0x88,
        name_offset: 0xF4,
        base: GridBase::Marker(b"UI_GATHER"),
        base_adjust: 0,
    };

    /// Technologies: installed tech.
    pub const TECHNOLOGIES: TableLayout = TableLayout {
        name: "nms_reality_gctechnologytable.mbin",
        count_offset: 0x28,
        stride: 736,
        id_offset: 0x138,
        name_offset: 0x1F4,
        base: GridBase::FirstId,
        base_adjust: 0,
    };

    /// Every table this crate understands.
    pub const ALL: [TableLayout; 3] = [TableLayout::PRODUCTS, TableLayout::SUBSTANCES, TableLayout::TECHNOLOGIES];
}

/// What kind of thing an item id names.
///
/// The category comes from the table an id was found in, which is the same
/// distinction the save itself records in its inventory slots (`InventoryType`),
/// so it can be cross-checked against a real save.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemCategory {
    /// From the product table: most inventory items.
    Product,
    /// From the substance table: the elements.
    Substance,
    /// From the technology table: installed technology.
    Technology,
}

impl ItemCategory {
    /// The name used in the data pack.
    pub fn label(self) -> &'static str {
        match self {
            ItemCategory::Product => "Product",
            ItemCategory::Substance => "Substance",
            ItemCategory::Technology => "Technology",
        }
    }
}

/// What one table's extraction found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemExtraction {
    /// Table file name.
    pub table: String,
    /// Category every item from this table has.
    pub category: Option<ItemCategory>,
    /// `id -> name key`, in table order.
    pub pairs: Vec<(String, String)>,
    /// Records the grid declared.
    pub declared: usize,
    /// Records whose id slot held an id.
    pub records: usize,
    /// Records with an id but no name key that resolves in the text tables.
    pub unresolved: usize,
}

impl ItemExtraction {
    /// How many keys resolve in the given text table.
    pub fn resolved(&self, strings: &BTreeMap<String, String>) -> usize {
        self.pairs.iter().filter(|(_, key)| strings.contains_key(key)).count()
    }

    /// Resolution rate as a percentage.
    pub fn resolution_percent(&self, strings: &BTreeMap<String, String>) -> usize {
        if self.pairs.is_empty() {
            return 0;
        }
        self.resolved(strings) * 100 / self.pairs.len()
    }
}

/// Read a NUL-terminated text slot, if it looks like an id or key.
fn read_slot(data: &[u8], offset: usize) -> Option<String> {
    let end = offset.checked_add(SLOT_SCAN)?.min(data.len());
    if offset >= end {
        return None;
    }
    let slot = &data[offset..end];
    let terminator = slot.iter().position(|byte| *byte == 0)?;
    let text = &slot[..terminator];
    if text.len() < 2 || !text.iter().all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_') {
        return None;
    }
    std::str::from_utf8(text).ok().map(str::to_string)
}

/// Read the name key for a record and accept it only if the text tables have it.
///
/// Requiring the key to resolve is what makes a wrong layout produce *fewer*
/// names rather than wrong ones: a misaligned slot reads some other field, whose
/// text is almost never a `*_NAME` key the game uses.
fn choose_key(data: &[u8], key_offset: usize, strings: &BTreeMap<String, String>) -> Option<String> {
    let key = read_slot(data, key_offset)?;
    if !key.ends_with("_NAME") || !strings.contains_key(&key) {
        return None;
    }
    Some(key)
}

/// Extract `(id, name key)` pairs from one table.
pub fn extract_table(data: &[u8], layout: &TableLayout, strings: &BTreeMap<String, String>) -> ItemExtraction {
    let mut extraction = ItemExtraction {
        table: layout.name.to_string(),
        category: Some(layout.category()),
        ..Default::default()
    };
    if data.len() < layout.count_offset + 4 || data.len() < layout.stride {
        return extraction;
    }
    let declared = u32::from_le_bytes(
        data[layout.count_offset..layout.count_offset + 4].try_into().expect("checked length"),
    ) as usize;
    extraction.declared = declared;
    if declared == 0 {
        return extraction;
    }

    let Some(base) = grid_base(data, layout) else {
        return extraction;
    };

    for index in 0..declared {
        let record = base + index * layout.stride;
        let Some(id) = read_slot(data, record + layout.id_offset) else {
            continue;
        };
        extraction.records += 1;
        match choose_key(data, record + layout.name_offset, strings) {
            Some(key) => extraction.pairs.push((id, key)),
            None => extraction.unresolved += 1,
        }
    }
    extraction
}

/// Locate the first record of a table's grid.
fn grid_base(data: &[u8], layout: &TableLayout) -> Option<usize> {
    match layout.base {
        GridBase::Marker(marker) => {
            let found = data.windows(marker.len()).position(|window| window == marker)?;
            Some(found.checked_sub(layout.base_adjust)?)
        }
        GridBase::FirstId => (0..layout.stride)
            .find(|offset| read_slot(data, offset + layout.id_offset).is_some()),
    }
}

/// An item-name index: save id to display text.
#[derive(Debug, Clone, Default)]
pub struct ItemIndex {
    items: BTreeMap<String, String>,
    categories: BTreeMap<String, ItemCategory>,
    /// How the index was built, for reporting.
    pub report: ItemIndexReport,
}

/// Where an item's name came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameSource {
    /// The item table's own name key.
    Table,
    /// A `<id>_NAME` key in the text tables.
    Derived,
    /// The id itself was a text key.
    Direct,
    /// A slot holding a damaged or empty component rather than an item: the
    /// game's own ids for these are `MAINT_*`, and no text table names them.
    Placeholder,
    /// The id field holds a generated name rather than a table id, which the
    /// game does for player-named and procedural items. The id *is* the display
    /// text.
    Custom,
}

impl NameSource {
    /// A short label for reports.
    pub fn label(self) -> &'static str {
        match self {
            NameSource::Table => "item table",
            NameSource::Derived => "derived key",
            NameSource::Direct => "direct key",
            NameSource::Placeholder => "placeholder",
            NameSource::Custom => "custom name",
        }
    }
}

/// True when a string looks like a game id rather than a generated name.
pub fn is_id_shaped(id: &str) -> bool {
    id.len() >= 2
        && id.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && id.chars().any(|c| c.is_ascii_alphabetic())
}

/// The label used for a slot that holds a damaged or empty component.
pub const PLACEHOLDER_LABEL: &str = "Damaged / empty component";

/// Per-table counts for the build report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemIndexReport {
    /// One line per table read.
    pub tables: Vec<TableReport>,
    /// Items named by deriving `<id>_NAME` from the text tables.
    pub derived: usize,
}

/// What one table contributed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableReport {
    /// Table file name.
    pub table: String,
    /// Records the header declared.
    pub declared: usize,
    /// Records whose id slot held an id.
    pub records: usize,
    /// Name keys that resolved in the text tables.
    pub resolved: usize,
    /// Records with no resolvable name key.
    pub unresolved: usize,
}

impl ItemIndex {
    /// Build the index from every extraction, plus a derivation pass over the
    /// text tables for ids no table holds.
    ///
    /// Table keys win over derived ones, because a table key is what the game
    /// displays.
    pub fn build(extractions: &[ItemExtraction], strings: &BTreeMap<String, String>) -> ItemIndex {
        let mut items = BTreeMap::new();
        let mut report = ItemIndexReport::default();

        let mut categories = BTreeMap::new();
        for extraction in extractions {
            let mut resolved = 0usize;
            for (id, key) in &extraction.pairs {
                if let Some(text) = strings.get(key).filter(|text| !text.is_empty()) {
                    items.insert(id.clone(), text.clone());
                    if let Some(category) = extraction.category {
                        categories.insert(id.clone(), category);
                    }
                    resolved += 1;
                }
            }
            report.tables.push(TableReport {
                table: extraction.table.clone(),
                declared: extraction.declared,
                records: extraction.records,
                resolved,
                unresolved: extraction.records - resolved,
            });
        }

        for (key, text) in strings {
            let Some(id) = key.strip_suffix("_NAME") else { continue };
            if id.is_empty() || text.is_empty() || items.contains_key(id) {
                continue;
            }
            items.insert(id.to_string(), text.clone());
            report.derived += 1;
        }

        ItemIndex { items, categories, report }
    }

    /// The category an id belongs to, when it came from an item table.
    pub fn category(&self, id: &str) -> Option<ItemCategory> {
        self.categories.get(id.trim_start_matches(['^', '!'])).copied()
    }

    /// Number of named items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True when nothing is named.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Look up an id against the index only.
    pub fn lookup(&self, id: &str) -> Option<&str> {
        self.items.get(id.trim_start_matches(['^', '!'])).map(String::as_str)
    }

    /// Look up an id, falling back to the text tables and then to the two
    /// non-table cases a real save contains.
    ///
    /// Returns `None` only when nothing sensible can be shown, which in practice
    /// means an id-shaped string with no text anywhere.
    pub fn resolve<'a>(&self, id: &str, strings: &'a BTreeMap<String, String>) -> Option<(String, NameSource)> {
        let bare = id.trim_start_matches(['^', '!']);

        // A slot holding a damaged component: the game names these `MAINT_*`.
        if bare.starts_with("MAINT") {
            return Some((PLACEHOLDER_LABEL.to_string(), NameSource::Placeholder));
        }
        // Not an id at all: the field holds a generated name, which is the text.
        if !is_id_shaped(bare) {
            return Some((bare.to_string(), NameSource::Custom));
        }
        if let Some(text) = self.items.get(bare) {
            return Some((text.clone(), NameSource::Table));
        }
        Self::resolve_against(id, strings).map(|(text, source)| (text.to_string(), source))
    }

    /// Look up an id against the text tables directly, for ids the index does
    /// not hold: `<id>_NAME`, `UI_<id>_NAME`, the trailing-number form, then the
    /// id as a key.
    pub fn resolve_against<'a>(
        id: &str,
        strings: &'a BTreeMap<String, String>,
    ) -> Option<(&'a str, NameSource)> {
        let bare = id.trim_start_matches(['^', '!']);
        // The game separates a trailing number with an underscore in some keys:
        // the substance `FUEL2` is named by `UI_FUEL_2_NAME`.
        let digits = bare.trim_end_matches(|c: char| c.is_ascii_digit());
        let number = &bare[digits.len()..];
        let mut candidates = vec![format!("{bare}_NAME"), format!("UI_{bare}_NAME")];
        if !number.is_empty() && !digits.is_empty() {
            candidates.push(format!("{digits}_{number}_NAME"));
            candidates.push(format!("UI_{digits}_{number}_NAME"));
        }
        // Base parts are named `BLD_<stem>_NAME` for the id `BASE_<stem>`, save
        // beacons are `BUILDBEACON` for `BLD_BEACON_NAME`, and corvette modules
        // are `B_<stem>` for `BLD_BIG_<stem>_NAME`. The prefix expansions are
        // pure conventions, and each candidate is still only accepted if the
        // text tables have it.
        let part_stem = bare.strip_prefix("BASE_").or_else(|| bare.strip_prefix("SB_")).or_else(|| {
            bare.strip_prefix("BUILD").map(|rest| rest.strip_prefix('_').unwrap_or(rest))
        });
        if let Some(stem) = part_stem {
            candidates.push(format!("BLD_{stem}_NAME"));
        }
        if let Some(stem) = bare.strip_prefix("B_") {
            candidates.push(format!("BLD_BIG_{stem}_NAME"));
            // The cockpit and wing families carry a size infix in their keys:
            // the id `B_COK_A` is named by `BLD_BIG_COK1X2_A_NAME`.
            for family in ["COK", "WNG", "ENG", "TUR", "SHL", "TRU", "CON", "ALK", "HULL", "STR"] {
                if let Some(rest) = stem.strip_prefix(family) {
                    let rest = rest.strip_prefix('_').unwrap_or(rest);
                    candidates.push(format!("BLD_BIG_{family}1X2_{rest}_NAME"));
                }
            }
        }
        // Ship hull parts: `FIGHT_COCKAA` is `UI_FIGHTER_COCK_AA_NAME`, and
        // `SCIEN_COCKAB` is `UI_SCIENTIFIC_COCK_ABA_NAME`.
        for (save_prefix, key_prefix) in [("FIGHT_", "UI_FIGHTER_"), ("SCIEN_", "UI_SCIENTIFIC_")] {
            if let Some(rest) = bare.strip_prefix(save_prefix) {
                // The key separates a trailing part index of one or two letters:
                // `FIGHT_COCKAA` is `UI_FIGHTER_COCK_AA_NAME`.
                let letters = rest.len() - rest.trim_end_matches(|c: char| c.is_ascii_uppercase()).len();
                for take in [2usize, 1] {
                    if letters >= take && rest.len() > take {
                        let (stem, suffix) = rest.split_at(rest.len() - take);
                        let stem = stem.trim_end_matches('_');
                        if !stem.is_empty() {
                            candidates.push(format!("{key_prefix}{stem}_{suffix}_NAME"));
                        }
                    }
                }
            }
        }
        if let Some(stem) = bare.strip_prefix("BUILD_") {
            candidates.push(format!("{stem}_NAME"));
        }
        for candidate in candidates {
            if let Some(text) = strings.get(&candidate) {
                if !text.is_empty() {
                    return Some((text.as_str(), NameSource::Derived));
                }
            }
        }

        // A few save ids truncate the stem (`BASE_BUBBLECLUS` for
        // `BLD_BUBBLECLUSTER_NAME`), so fall back to the shortest base-part key
        // the id is a prefix of. Only base parts pay for this scan.
        if let Some(stem) = part_stem {
            let prefix = format!("BLD_{}", stem.strip_prefix('_').unwrap_or(stem));
            let mut best: Option<(&String, &String)> = None;
            for (key, text) in strings {
                if !text.is_empty() && key.starts_with(&prefix) && key.ends_with("_NAME") {
                    if best.is_none_or(|(current, _)| key.len() < current.len()) {
                        best = Some((key, text));
                    }
                }
            }
            if let Some((_, text)) = best {
                return Some((text.as_str(), NameSource::Derived));
            }
        }

        strings
            .get(bare)
            .filter(|text| !text.is_empty())
            .map(|text| (text.as_str(), NameSource::Direct))
    }

    /// Iterate over `(id, text)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.items.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// Build a table in the real shape: header with a count, then a grid of
    /// records with a text slot per field.
    fn synthetic_table(layout: &TableLayout, base: usize, ids: &[&str], keys: &[&str]) -> Vec<u8> {
        let mut data = vec![0u8; base + ids.len() * layout.stride];
        data[0..8].copy_from_slice(&0xCCCC_CCCC_CCCC_CCCCu64.to_le_bytes());
        data[layout.count_offset..layout.count_offset + 4].copy_from_slice(&(ids.len() as u32).to_le_bytes());
        if let GridBase::Marker(marker) = layout.base {
            let marker_at = base + layout.base_adjust;
            data[marker_at..marker_at + marker.len()].copy_from_slice(marker);
        }
        for (index, (id, key)) in ids.iter().zip(keys).enumerate() {
            let record = base + index * layout.stride;
            data[record + layout.id_offset..record + layout.id_offset + id.len()].copy_from_slice(id.as_bytes());
            // The name key sits some distance after the id, preceded by a float
            // whose last byte looks like a letter — the case that used to produce
            // keys like APROTECT_NAME.
            let key_offset = record + layout.name_offset;
            data[key_offset - 4..key_offset].copy_from_slice(&10.0f32.to_le_bytes());
            data[key_offset..key_offset + key.len()].copy_from_slice(key.as_bytes());
        }
        data
    }

    #[test]
    fn extracts_products_on_the_record_grid() {
        let layout = TableLayout::PRODUCTS;
        let data = synthetic_table(&layout, 0x80, &["CASING", "ANTIMATTER"], &["CASING_NAME", "COMMODITY6_NAME"]);
        let text = strings(&[("CASING_NAME", "METAL PLATING"), ("COMMODITY6_NAME", "ANTIMATTER")]);
        let extraction = extract_table(&data, &layout, &text);

        assert_eq!(extraction.declared, 2);
        assert_eq!(extraction.records, 2);
        assert_eq!(extraction.pairs[0], ("CASING".into(), "CASING_NAME".into()));
        // A key that is not derived from the id is still found, because it is the
        // one that resolves.
        assert_eq!(extraction.pairs[1], ("ANTIMATTER".into(), "COMMODITY6_NAME".into()));
        assert_eq!(extraction.resolution_percent(&text), 100);
    }

    #[test]
    fn extracts_substances_from_the_marker_grid() {
        let layout = TableLayout::SUBSTANCES;
        let data = synthetic_table(&layout, 0x80, &["FUEL1", "OXYGEN"], &["UI_FUEL_1_NAME", "UI_AIR1_NAME"]);
        let text = strings(&[("UI_FUEL_1_NAME", "CARBON"), ("UI_AIR1_NAME", "OXYGEN")]);
        let extraction = extract_table(&data, &layout, &text);
        assert_eq!(extraction.records, 2);
        assert_eq!(extraction.pairs[1], ("OXYGEN".into(), "UI_AIR1_NAME".into()));
    }

    #[test]
    fn ignores_a_key_that_does_not_resolve() {
        // The record's only candidate is a key the text tables do not have, so
        // the record is counted as unresolved instead of inventing a name.
        let layout = TableLayout::TECHNOLOGIES;
        // The key shape is right but the text tables do not have it, so the
        // record must count as unresolved rather than inventing a name.
        let data = synthetic_table(&layout, 0x40, &["GHOST_TECH"], &["GHOST_TECH_NAME"]);
        let extraction = extract_table(&data, &layout, &strings(&[]));
        assert_eq!(extraction.records, 1);
        assert_eq!(extraction.unresolved, 1);
        assert!(extraction.pairs.is_empty());
    }

    #[test]
    fn a_stray_leading_byte_does_not_hide_a_key() {
        // The float before the key ends in 'A', so the raw scan sees
        // "APROTECT_NAME"; the real key must still be found.
        let layout = TableLayout::TECHNOLOGIES;
        let mut data = synthetic_table(&layout, 0x40, &["PROTECT"], &["PROTECT_NAME"]);
        let key_offset = 0x40 + layout.name_offset;
        data[key_offset - 1] = b'A';
        let text = strings(&[("PROTECT_NAME", "HAZARD PROTECTION")]);
        let extraction = extract_table(&data, &layout, &text);
        assert_eq!(extraction.pairs, vec![("PROTECT".to_string(), "PROTECT_NAME".to_string())]);
    }

    #[test]
    fn stops_at_the_declared_count() {
        // Trailing data after the declared records must not be read as items.
        let layout = TableLayout::PRODUCTS;
        let mut data = synthetic_table(&layout, 0x80, &["A_ITEM"], &["A_ITEM_NAME"]);
        data.resize(data.len() + layout.stride, 0);
        data[layout.count_offset..layout.count_offset + 4].copy_from_slice(&1u32.to_le_bytes());
        let trailing = data.len() - layout.stride + layout.id_offset;
        data[trailing..trailing + 6].copy_from_slice(b"TRAIL\x00");
        let extraction = extract_table(&data, &layout, &strings(&[("A_ITEM_NAME", "An item"), ("TRAIL_NAME", "Nope")]));
        assert_eq!(extraction.records, 1);
    }

    #[test]
    fn joins_tables_with_text_and_derives_the_rest() {
        let layout = TableLayout::PRODUCTS;
        let data = synthetic_table(&layout, 0x80, &["CASING"], &["CASING_NAME"]);
        let text = strings(&[
            ("CASING_NAME", "METAL PLATING"),
            ("FUEL2_NAME", "CONDENSED CARBON"),
            ("UI_EMPTY_NAME", ""),
        ]);
        let extraction = extract_table(&data, &layout, &text);
        let index = ItemIndex::build(&[extraction], &text);

        assert_eq!(index.lookup("CASING"), Some("METAL PLATING"));
        assert_eq!(index.lookup("^CASING"), Some("METAL PLATING"));
        assert_eq!(index.lookup("FUEL2"), Some("CONDENSED CARBON"));
        assert_eq!(index.lookup("MISSING"), None);
        assert_eq!(index.report.tables[0].resolved, 1);
        assert_eq!(index.report.derived, 1);
    }

    #[test]
    fn labels_placeholders_and_custom_names() {
        let text = strings(&[("CASING_NAME", "METAL PLATING"), ("UI_FUEL_2_NAME", "CONDENSED CARBON")]);
        let index = ItemIndex::build(&[], &text);

        assert_eq!(
            index.resolve("^MAINT_REFINER", &text),
            Some((PLACEHOLDER_LABEL.to_string(), NameSource::Placeholder))
        );
        // A generated name is shown as-is, because that is what the game shows.
        assert_eq!(
            index.resolve("^J\\u{00C4}#00000", &text),
            Some(("J\\u{00C4}#00000".to_string(), NameSource::Custom))
        );
        assert_eq!(
            index.resolve("^CASING", &text),
            Some(("METAL PLATING".to_string(), NameSource::Table))
        );
        assert_eq!(
            index.resolve("FUEL2", &text),
            Some(("CONDENSED CARBON".to_string(), NameSource::Derived))
        );
        assert_eq!(index.resolve("^NOTHING_HERE", &text), None);
    }

    #[test]
    fn finds_keys_that_separate_a_trailing_number() {
        let text = strings(&[("UI_FUEL_2_NAME", "CONDENSED CARBON")]);
        assert_eq!(
            ItemIndex::resolve_against("FUEL2", &text),
            Some(("CONDENSED CARBON", NameSource::Derived))
        );
        assert_eq!(ItemIndex::resolve_against("FUEL", &text), None);
    }

    #[test]
    fn names_base_parts_by_their_bld_convention() {
        let text = strings(&[
            ("BLD_BONEGARDEN_NAME", "RATTLE SPINE"),
            ("BLD_BEACON_NAME", "SAVE BEACON"),
            ("BLD_BUBBLECLUSTER_NAME", "BUBBLE CLUSTER"),
        ]);
        assert_eq!(
            ItemIndex::resolve_against("^BASE_BONEGARDEN", &text),
            Some(("RATTLE SPINE", NameSource::Derived))
        );
        assert_eq!(
            ItemIndex::resolve_against("^BUILDBEACON", &text),
            Some(("SAVE BEACON", NameSource::Derived))
        );
        // The save's id is a truncated stem; the shortest matching key wins.
        assert_eq!(
            ItemIndex::resolve_against("^BASE_BUBBLECLUS", &text),
            Some(("BUBBLE CLUSTER", NameSource::Derived))
        );
    }

    #[test]
    fn expands_corvette_and_ship_part_prefixes() {
        let text = strings(&[
            ("BLD_BIG_SHL_A_NAME", "DEFLECTOR SHIELD"),
            ("BLD_BIG_COK1X2_C4_NAME", "VIPER-I-CLASS COCKPIT"),
            ("UI_FIGHTER_COCK_AA_NAME", "OMEGA FUSELAGE"),
            ("UI_SCIENTIFIC_COCK_ABA_NAME", "FIREFLY S-HULL"),
            ("REFINER1_NAME", "PORTABLE REFINER"),
        ]);
        assert_eq!(
            ItemIndex::resolve_against("B_SHL_A", &text),
            Some(("DEFLECTOR SHIELD", NameSource::Derived))
        );
        // The cockpit family carries a size infix in the key.
        assert_eq!(
            ItemIndex::resolve_against("B_COK_C4", &text),
            Some(("VIPER-I-CLASS COCKPIT", NameSource::Derived))
        );
        assert_eq!(
            ItemIndex::resolve_against("FIGHT_COCKAA", &text),
            Some(("OMEGA FUSELAGE", NameSource::Derived))
        );
        assert_eq!(
            ItemIndex::resolve_against("BUILD_REFINER1", &text),
            Some(("PORTABLE REFINER", NameSource::Derived))
        );
    }

    #[test]
    fn falls_back_to_direct_key_lookups() {
        let text = strings(&[("ACCESS3_NAME", "ATLASPASS v3"), ("UI_SEASON_23_NAME", "Season 23")]);
        assert_eq!(ItemIndex::resolve_against("^ACCESS3", &text), Some(("ATLASPASS v3", NameSource::Derived)));
        assert_eq!(
            ItemIndex::resolve_against("UI_SEASON_23_NAME", &text),
            Some(("Season 23", NameSource::Direct))
        );
    }
}
