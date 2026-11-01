//! Item names, joined from the game's item tables and its text tables.
//!
//! A save stores item ids (`^CARBON_SEAL`, `^ANTIMATTER`, `^FUEL2`), not names.
//! The names live in the localisation tables under keys that the *item tables*
//! point at, so an editor needs the join. Two facts make that awkward, and both
//! are why the older editor shipped a hand-maintained item list:
//!
//! * the display key is **not** always derived from the id — `ANTIMATTER`'s name
//!   key is `COMMODITY6_NAME`, and `BIOSHIP_COMP5_0`'s is
//!   `UI_BIOSHIP_COMP5_NAME0` — so the key has to be read from the table;
//! * several id namespaces coexist: the substance the save calls `OXYGEN` has
//!   the table id `AIR1` and the name key `UI_AIR1_NAME`.
//!
//! The product table (`nms_reality_gcproducttable.mbin`) is laid out as a grid
//! of fixed-size records, which is what this module exploits:
//!
//! ```text
//! record + 0x000  "UI_CRAFT_OBJ"            (fixed 64-byte text slot)
//! record + 0x040  "UI_PIN_<ID>_OBJ_TIP"     (fixed 32-byte text slot)
//! record + 0x0C0  <ID>                      (fixed 16-byte text slot)
//! record + 0x160  <name key>                (fixed 128-byte text slot)
//! record + 0x1E0  <name key>_L              (lower-case variant)
//! stride          768 bytes
//! ```
//!
//! Records are walked on that grid and a position is skipped when both slots do
//! not hold plausible text, so a record whose shape differs is skipped rather
//! than shifting everything after it. Every extracted key is then resolved
//! against the text tables, and the resolution rate is reported: that rate is
//! the self-check. In the observed build it is 96% over 2,261 records, and a
//! layout change would make it collapse instead of producing wrong names.

use std::collections::BTreeMap;

/// Distance between records in the product table.
const PRODUCT_STRIDE: usize = 768;
/// Offset of the id within a product record.
const PRODUCT_ID_OFFSET: usize = 0xC0;
/// Offset of the name key within a product record.
const PRODUCT_NAME_OFFSET: usize = 0x160;
/// Bytes reserved for a text slot, used to bound reads.
const SLOT_SCAN: usize = 0x80;

/// What an extraction found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemExtraction {
    /// `id -> name key`, in table order.
    pub pairs: Vec<(String, String)>,
    /// Grid positions that held a usable id and name key.
    pub records: usize,
    /// Grid positions skipped because the slots did not hold plausible text.
    pub skipped: usize,
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

/// Read a NUL-terminated text slot, returning `None` when it does not look like
/// a key or id (`A-Z`, `0-9`, `_`, at least two characters).
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
    Some(String::from_utf8_lossy(text).into_owned())
}

/// Extract `(id, name key)` pairs from the product table.
pub fn extract_product_table(data: &[u8]) -> ItemExtraction {
    let mut extraction = ItemExtraction::default();

    // The grid does not start at offset zero: the file header precedes it. The
    // base is the first position within one stride whose id and name slots both
    // hold plausible text, which is stable across builds because the header size
    // is part of the record layout.
    let Some(base) = (0..PRODUCT_STRIDE).find(|offset| {
        offset + PRODUCT_NAME_OFFSET + SLOT_SCAN <= data.len()
            && read_slot(data, offset + PRODUCT_ID_OFFSET).is_some()
            && read_slot(data, offset + PRODUCT_NAME_OFFSET).is_some()
    }) else {
        return extraction;
    };

    let mut position = base;
    while position + PRODUCT_STRIDE <= data.len() {
        match (read_slot(data, position + PRODUCT_ID_OFFSET), read_slot(data, position + PRODUCT_NAME_OFFSET)) {
            (Some(id), Some(key)) => {
                extraction.pairs.push((id, key));
                extraction.records += 1;
            }
            _ => extraction.skipped += 1,
        }
        position += PRODUCT_STRIDE;
    }
    extraction
}

/// An item-name index: save id to display text.
#[derive(Debug, Clone, Default)]
pub struct ItemIndex {
    items: BTreeMap<String, String>,
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

/// Counts for the build report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ItemIndexReport {
    /// Items named from the product table.
    pub from_table: usize,
    /// Items named by deriving `<id>_NAME`.
    pub derived: usize,
    /// Table records whose key did not resolve.
    pub unresolved: usize,
}

impl ItemIndex {
    /// Build the index from the product table and the text tables.
    ///
    /// The table's own keys win over derived ones, because a table key is what
    /// the game displays.
    pub fn build(product_table: &[u8], strings: &BTreeMap<String, String>) -> ItemIndex {
        let extraction = extract_product_table(product_table);
        let mut items = BTreeMap::new();
        let mut report = ItemIndexReport::default();

        for (id, key) in &extraction.pairs {
            match strings.get(key) {
                Some(text) if !text.is_empty() => {
                    items.insert(id.clone(), text.clone());
                    report.from_table += 1;
                }
                _ => report.unresolved += 1,
            }
        }

        // Every `<id>_NAME` key names an item whose id the save may use, which
        // covers items the product grid does not hold (substances, technologies,
        // and ids that only appear in other tables).
        for (key, text) in strings {
            let Some(id) = key.strip_suffix("_NAME") else { continue };
            if id.is_empty() || text.is_empty() || items.contains_key(id) {
                continue;
            }
            items.insert(id.to_string(), text.clone());
            report.derived += 1;
        }

        ItemIndex { items, report }
    }

    /// Number of named items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// True when nothing is named.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Look up an id, accepting the save's `^` and `!` prefixes.
    pub fn lookup(&self, id: &str) -> Option<(&str, NameSource)> {
        let bare = id.trim_start_matches(['^', '!']);
        if let Some(text) = self.items.get(bare) {
            return Some((text.as_str(), NameSource::Table));
        }
        None
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
    /// not hold: `<id>_NAME`, `UI_<id>_NAME`, then the id as a key.
    pub fn resolve_against<'a>(
        id: &str,
        strings: &'a BTreeMap<String, String>,
    ) -> Option<(&'a str, NameSource)> {
        let bare = id.trim_start_matches(['^', '!']);
        // The game separates a trailing number with an underscore in some keys:
        // the substance `FUEL2` is named by `UI_FUEL_2_NAME`.
        let digits = bare.trim_end_matches(|c: char| c.is_ascii_digit());
        let number = &bare[digits.len()..];
        let mut candidates = vec![
            format!("{bare}_NAME"),
            format!("UI_{bare}_NAME"),
        ];
        if !number.is_empty() && !digits.is_empty() {
            candidates.push(format!("{digits}_{number}_NAME"));
            candidates.push(format!("UI_{digits}_{number}_NAME"));
        }
        for candidate in candidates {
            if let Some(text) = strings.get(&candidate) {
                if !text.is_empty() {
                    return Some((text.as_str(), NameSource::Derived));
                }
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

    /// Build a product table in the real layout: header, then a grid of records.
    fn synthetic_product_table(records: &[(&str, &str)]) -> Vec<u8> {
        let header = 0x80;
        let mut data = vec![0u8; header + records.len() * PRODUCT_STRIDE];
        data[0..8].copy_from_slice(&0xCCCC_CCCC_CCCC_CCCCu64.to_le_bytes());
        for (index, (id, key)) in records.iter().enumerate() {
            let base = header + index * PRODUCT_STRIDE;
            data[base + 0x00..base + 0x00 + 12].copy_from_slice(b"UI_CRAFT_OBJ");
            data[base + PRODUCT_ID_OFFSET..base + PRODUCT_ID_OFFSET + id.len()].copy_from_slice(id.as_bytes());
            data[base + PRODUCT_NAME_OFFSET..base + PRODUCT_NAME_OFFSET + key.len()].copy_from_slice(key.as_bytes());
        }
        data
    }

    fn strings(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn extracts_pairs_on_the_record_grid() {
        let table = synthetic_product_table(&[
            ("CASING", "CASING_NAME"),
            ("ANTIMATTER", "COMMODITY6_NAME"),
            ("CARBON_SEAL", "CARBON_SEAL_NAME"),
        ]);
        let extraction = extract_product_table(&table);
        assert_eq!(extraction.pairs.len(), 3, "{:?}", extraction.pairs);
        assert_eq!(extraction.pairs[0], ("CASING".into(), "CASING_NAME".into()));
        assert_eq!(extraction.pairs[1], ("ANTIMATTER".into(), "COMMODITY6_NAME".into()));
    }

    #[test]
    fn skips_positions_whose_slots_are_empty_or_padding() {
        let mut table = synthetic_product_table(&[("A_ITEM", "A_ITEM_NAME")]);
        // A second record with an empty id slot is skipped, not misread.
        table.resize(table.len() + PRODUCT_STRIDE, 0);
        let extraction = extract_product_table(&table);
        assert_eq!(extraction.records, 1);
        assert_eq!(extraction.skipped, 1);
    }

    #[test]
    fn joins_table_keys_with_text_and_derives_the_rest() {
        let table = synthetic_product_table(&[("CASING", "CASING_NAME"), ("ANTIMATTER", "COMMODITY6_NAME")]);
        let strings = strings(&[
            ("CASING_NAME", "METAL PLATING"),
            ("COMMODITY6_NAME", "ANTIMATTER"),
            ("FUEL2_NAME", "CONDENSED CARBON"),
            ("UI_UNUSED_NAME", ""),
        ]);
        let index = ItemIndex::build(&table, &strings);

        assert_eq!(index.lookup("CASING"), Some(("METAL PLATING", NameSource::Table)));
        // The save writes ids with a type prefix.
        assert_eq!(index.lookup("^CASING"), Some(("METAL PLATING", NameSource::Table)));
        assert_eq!(index.lookup("ANTIMATTER").map(|(t, _)| t), Some("ANTIMATTER"));
        // An item the grid does not hold is still named by derivation.
        assert_eq!(index.lookup("FUEL2").map(|(t, _)| t), Some("CONDENSED CARBON"));
        assert_eq!(index.lookup("MISSING"), None);
        assert_eq!(index.report.from_table, 2);
        assert!(index.report.derived >= 1);
    }

    #[test]
    fn reports_unresolved_table_keys_instead_of_inventing_names() {
        let table = synthetic_product_table(&[("GHOST", "GHOST_NAME")]);
        let index = ItemIndex::build(&table, &strings(&[]));
        assert!(index.is_empty());
        assert_eq!(index.report.unresolved, 1);
    }

    #[test]
    fn falls_back_to_direct_key_lookups() {
        let strings = strings(&[("ACCESS3_NAME", "ATLASPASS v3"), ("UI_SEASON_23_NAME", "Season 23")]);
        assert_eq!(
            ItemIndex::resolve_against("^ACCESS3", &strings),
            Some(("ATLASPASS v3", NameSource::Derived))
        );
        assert_eq!(
            ItemIndex::resolve_against("UI_SEASON_23_NAME", &strings),
            Some(("Season 23", NameSource::Direct))
        );
    }

    #[test]
    fn labels_placeholders_and_custom_names() {
        let strings = strings(&[("CASING_NAME", "METAL PLATING"), ("UI_FUEL_2_NAME", "CONDENSED CARBON")]);
        let index = ItemIndex::build(&[], &strings);

        assert_eq!(
            index.resolve("^MAINT_REFINER", &strings),
            Some((PLACEHOLDER_LABEL.to_string(), NameSource::Placeholder))
        );
        // A generated name is shown as-is, because that is what the game shows.
        assert_eq!(
            index.resolve("^J\u{00C4}#00000", &strings),
            Some(("J\u{00C4}#00000".to_string(), NameSource::Custom))
        );
        // CASING_NAME is folded into the index by derivation, so it is a table hit;
        // FUEL2 has no FUEL2_NAME key and is found by the trailing-number rule.
        assert_eq!(
            index.resolve("^CASING", &strings),
            Some(("METAL PLATING".to_string(), NameSource::Table))
        );
        assert_eq!(
            index.resolve("FUEL2", &strings),
            Some(("CONDENSED CARBON".to_string(), NameSource::Derived))
        );
        assert_eq!(index.resolve("^NOTHING_HERE", &strings), None);
    }

    #[test]
    fn finds_keys_that_separate_a_trailing_number() {
        // The substance FUEL2 is named by UI_FUEL_2_NAME, not FUEL2_NAME.
        let strings = strings(&[("UI_FUEL_2_NAME", "CONDENSED CARBON")]);
        assert_eq!(
            ItemIndex::resolve_against("FUEL2", &strings),
            Some(("CONDENSED CARBON", NameSource::Derived))
        );
        assert_eq!(ItemIndex::resolve_against("FUEL", &strings), None);
    }
}
