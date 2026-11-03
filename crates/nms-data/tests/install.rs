//! Validation against the real game installation.
//!
//! These tests only run where No Man's Sky is installed (they skip otherwise, so
//! a clean clone stays green). They are the load-bearing evidence for the
//! localisation parser: every table **of every language** must parse with the
//! blob consumed exactly to the end of the file and every length field matching
//! the text it points at. A wrong stride, entry count or language slot cannot
//! satisfy both, so passing on the game's own 153 tables (1.44M entries) is what
//! makes the layout a fact rather than a guess.

use std::collections::HashSet;

use nms_data::{DataPack, Install, language};

/// The language code a table's file name ends with (`nms_loc1_german.mbin` →
/// `german`).
fn language_of(name: &str) -> String {
    name.trim_end_matches(".mbin").rsplit('_').next().unwrap_or(name).to_string()
}

/// Every table of every language parses exactly.
///
/// This is the test that catches a language-layout regression: each record
/// carries one value slot per language and a table fills only its own, so a
/// parser that assumes one fixed slot reads every language but that one as
/// empty. (It did, until the slots were worked out — German came out with all
/// 84,965 values blank.)
#[test]
fn every_language_table_parses_exactly() {
    let Ok(install) = Install::discover(std::env::var("NMS_INSTALL").ok().as_deref().map(std::path::Path::new))
    else {
        eprintln!("skipping: no No Man's Sky installation found (set NMS_INSTALL to point at one)");
        return;
    };

    let mut tables = 0usize;
    let mut entries = 0usize;
    let mut filled_total = 0usize;
    let mut languages: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut samples: Vec<(String, String, String)> = Vec::new();

    for pak_path in install.paks().unwrap() {
        let pak = nms_pak::Pak::open(&pak_path).expect("archive opens");
        let names: Vec<String> = pak
            .entries()
            .iter()
            .filter(|entry| entry.name.starts_with("language/") && entry.name.ends_with(".mbin"))
            .map(|entry| entry.name.clone())
            .collect();

        for name in names {
            let bytes = pak.extract_name(&name).expect("table extracts");
            let table = language::parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));

            assert!(table.stats.declared > 0, "{name}: no entries");
            assert_eq!(table.stats.parsed, table.stats.declared, "{name}: entry count mismatch");
            assert!(
                table.is_exact(),
                "{name}: blob consumed {} of {} bytes, {} offset mismatches — the layout assumption is wrong for this table",
                table.stats.consumed,
                table.stats.file_len,
                table.stats.pointer_mismatches
            );

            // Ids must be printable ASCII and unique; anything else means the
            // stride drifted and we are reading the wrong bytes.
            let mut seen = HashSet::new();
            for (id, _) in &table.entries {
                // Ids are ASCII apart from a handful with typographic
                // apostrophes; the file's own text is UTF-8 (verified: every
                // high-byte case in the game decodes as UTF-8).
                assert!(
                    !id.chars().any(char::is_control),
                    "{name}: id {id:?} contains control characters"
                );
                assert!(seen.insert(id.clone()), "{name}: duplicate id {id:?}");
            }

            // The text is UTF-8; a lossy decode would have produced replacement
            // characters, so check the raw bytes are valid UTF-8.
            for (_, text) in &table.entries {
                assert!(
                    std::str::from_utf8(text.as_bytes()).is_ok() && !text.contains('\u{FFFD}'),
                    "{name}: value {text:?} is not valid UTF-8"
                );
            }

            // The slot was found: a table with text must read some. (One table
            // is genuinely sparse — tencentchinese's loc10 ships 2 translated
            // strings of 2,505 — so the corpus-wide fraction below, not a
            // per-table one, is the check that values are being read.)
            let filled = table.entries.iter().filter(|(_, text)| !text.is_empty()).count();
            assert!(filled > 0, "{name}: not one entry has text — the value slot was not found");

            // Every table fills exactly one language's slot; which slot is the
            // game's language order, and the same language always uses the same
            // one.
            let slot = table.stats.slot.expect("a table with values fills a language slot");
            let code = language_of(&name);
            if let Some(previous) = languages.insert(code.clone(), slot) {
                assert_eq!(previous, slot, "{name}: {code} changed slots between tables");
            }

            if samples.len() < 4 {
                if let Some((id, text)) = table.entries.iter().find(|(_, t)| !t.is_empty()) {
                    samples.push((code.clone(), id.clone(), text.clone()));
                }
            }
            tables += 1;
            entries += table.entries.len();
            filled_total += filled;
        }
    }

    eprintln!("parsed {tables} tables in {} languages, {entries} entries exactly", languages.len());
    for (code, id, text) in &samples {
        eprintln!("  sample ({code}): {id} -> {text:?}");
    }
    assert!(languages.len() >= 10, "expected the game's language set, found {:?}", languages.keys());
    assert!(tables >= 100, "expected the full table set, found {tables}");
    assert!(entries > 1_000_000, "expected a large corpus, found {entries} entries");
    assert!(
        filled_total * 100 / entries >= 90,
        "only {filled_total} of {entries} values are filled — values are being read as empty"
    );

    // The slot order is the game's language order (alphabetical by file name),
    // and English is not the first: pinning two of them documents the
    // observation the fix rests on.
    assert_eq!(languages.get("english"), Some(&2), "english's slot");
    assert_eq!(languages.get("german"), Some(&4), "german's slot");
}

/// Item names come from joining each item table's keys with the text tables.
/// The per-table resolution rate is the self-check: a layout change collapses it
/// instead of producing wrong names.
#[test]
fn item_names_resolve_from_the_item_tables() {
    let Ok(install) = Install::discover(std::env::var("NMS_INSTALL").ok().as_deref().map(std::path::Path::new))
    else {
        eprintln!("skipping: no No Man's Sky installation found");
        return;
    };

    let (pack, report) = DataPack::build(&install, "english", 1_793_488_365).expect("pack builds");
    let _strings = &pack.strings;

    eprintln!("item tables:");
    for table in &report.items.tables {
        let rate = table.resolved * 100 / table.records.max(1);
        eprintln!(
            "  {:<34} {:>5} declared, {:>5} records, {:>5} named ({rate}%)",
            table.table, table.declared, table.records, table.resolved
        );
        // A table that parses to nothing means the layout assumption broke.
        assert!(table.records > 100, "{}: only {} records read", table.table, table.records);
        assert!(rate >= 50, "{}: resolution collapsed to {rate}%", table.table);
    }
    assert!(report.items.tables.len() >= 3, "expected three item tables");
    eprintln!("item index: {} names ({} derived from text keys)", pack.item_count(), report.items.derived);

    // Known joins, one per table, including keys not derived from the id.
    assert_eq!(pack.item_name("CASING").as_deref(), Some("METAL PLATING"));
    assert_eq!(pack.item_name("CARBON_SEAL").as_deref(), Some("HERMETIC SEAL"));
    assert_eq!(pack.item_name("ANTIMATTER").as_deref(), Some("ANTIMATTER"));
    assert_eq!(pack.item_name("FUEL1").as_deref(), Some("CARBON"));
    assert_eq!(pack.item_name("OXYGEN").as_deref(), Some("OXYGEN"));
    assert_eq!(pack.item_name("LAUNCHSUB").as_deref(), Some("DI-HYDROGEN"));
    assert_eq!(pack.item_name("UT_PROTECT").as_deref(), Some("SHIELD LATTICE"));
    assert_eq!(pack.item_name("JET1").as_deref(), Some("JETPACK"));
    // Damaged components are named by the technology table itself (the game's
    // own text for `MAINT_REFINER` is the refiner's name), so the table wins over
    // the placeholder label; the label is the fallback for ids no table names.
    assert_eq!(pack.item_name("MAINT_REFINER").as_deref(), Some("REFINERY OUTPUT"));
    assert_eq!(pack.item_name("MAINT_TECH5").as_deref(), Some("BIO-INPUT SENSOR"));
    // Descriptions come from the name key's `_DESC` sibling or the id's own
    // keys, where the game names one.
    assert!(
        pack.item_description("STELLAR2").is_some(),
        "chromatic metal's description is its name key's sibling"
    );
    assert!(pack.item_description("FUEL1").is_some(), "carbon has one");
    assert!(
        pack.item_description("^ANTIMATTER").is_none(),
        "the detail key is a hyperdrive panel, not a description"
    );
    assert!(pack.item_description("NOT_AN_ITEM").is_none());
    // Base parts are named by the BLD_ convention, including truncated stems.
    assert_eq!(pack.item_name("BASE_BONEGARDEN").as_deref(), Some("RATTLE SPINE"));
    assert_eq!(pack.item_name("^BASE_BUBBLECLUS").as_deref(), Some("BUBBLE CLUSTER"));
    assert_eq!(pack.item_name("BUILDBEACON").as_deref(), Some("SAVE BEACON"));
    // A generated name is shown as-is, so the editor never has a blank cell.
    assert_eq!(
        pack.item_name_with_source("^J\u{00C4}#00000").map(|(_, source)| source),
        Some(nms_data::NameSource::Custom)
    );
    assert_eq!(pack.item_name("NOT_AN_ITEM"), None);
}

#[test]
fn builds_a_pack_from_the_installation() {
    let Ok(install) = Install::discover(std::env::var("NMS_INSTALL").ok().as_deref().map(std::path::Path::new))
    else {
        eprintln!("skipping: no No Man's Sky installation found");
        return;
    };

    let (pack, report) = DataPack::build(&install, "english", 1_793_488_365).expect("pack builds");
    assert!(pack.len() > 20_000, "pack looks short: {} strings", pack.len());
    assert!(pack.item_count() > 2_000, "item index looks short: {}", pack.item_count());
    assert!(!report.sources.is_empty());
    assert!(
        report.sources.iter().all(|source| source.exact),
        "a table did not parse exactly: {:?}",
        report.sources
    );

    // Every value that exists is a string; the merge must not invent entries.
    let filled = pack.strings.values().filter(|text| !text.is_empty()).count();
    assert!(filled * 100 / pack.len() >= 90, "only {filled} of {} values are filled", pack.len());
    eprintln!(
        "pack: {} strings from {} archives ({:?}), {} archives skipped",
        pack.len(),
        report.sources.len(),
        report.sources.iter().map(|s| s.entries).collect::<Vec<_>>(),
        report.skipped.len()
    );
    for skipped in report.skipped.iter().take(3) {
        eprintln!("  skipped: {skipped}");
    }

    // The same build in another language: the id set is language-independent,
    // the values are the language file's own. Before the per-language value
    // slots were worked out, this build produced 84,965 empty strings.
    let (german, _) = DataPack::build(&install, "german", 1_793_488_365).expect("German pack builds");
    let english_keys: std::collections::BTreeSet<&String> = pack.strings.keys().collect();
    let german_keys: std::collections::BTreeSet<&String> = german.strings.keys().collect();
    assert_eq!(german_keys, english_keys, "the id set is language-independent");
    let german_filled = german.strings.values().filter(|text| !text.is_empty()).count();
    assert!(
        german_filled * 100 / german.len() >= 90,
        "German: only {german_filled} of {} values are filled",
        german.len()
    );
    let differing = pack
        .strings
        .iter()
        .filter(|(id, english)| german.strings.get(*id) != Some(*english))
        .count();
    assert!(
        differing > pack.len() / 2,
        "German text is mostly identical to English: only {differing} values differ"
    );
    assert!(german.item_count() > 2_000, "German item index looks short: {}", german.item_count());
    eprintln!("german pack: {} strings, {} item names", german.len(), german.item_count());
}
