//! Validation against the real game installation.
//!
//! These tests only run where No Man's Sky is installed (they skip otherwise, so
//! a clean clone stays green). They are the load-bearing evidence for the
//! localisation parser: every table in the game must parse with the blob
//! consumed **exactly** to the end of the file and every length field matching
//! the text it points at. A wrong stride or entry count cannot satisfy both, so
//! passing on the game's own files is what makes the layout a fact rather than a
//! guess.

use std::collections::HashSet;

use nms_data::{DataPack, Install, language};

#[test]
fn every_english_table_parses_exactly() {
    let Ok(install) = Install::discover(std::env::var("NMS_INSTALL").ok().as_deref().map(std::path::Path::new))
    else {
        eprintln!("skipping: no No Man's Sky installation found (set NMS_INSTALL to point at one)");
        return;
    };

    let mut tables = 0usize;
    let mut entries = 0usize;
    let mut samples: Vec<(String, String)> = Vec::new();

    for pak_path in install.paks().unwrap() {
        let pak = nms_pak::Pak::open(&pak_path).expect("archive opens");
        let names: Vec<String> = pak
            .entries()
            .iter()
            .filter(|entry| entry.name.starts_with("language/") && entry.name.ends_with("_english.mbin"))
            .map(|entry| entry.name.clone())
            .collect();

        for name in names {
            let bytes = pak.extract_name(&name).expect("table extracts");
            let table = language::parse(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));

            assert!(table.stats.declared > 0, "{name}: no entries");
            assert_eq!(table.stats.parsed, table.stats.declared, "{name}: entry count mismatch");
            assert!(
                table.is_exact(),
                "{name}: blob consumed {} of {} bytes — the layout assumption is wrong for this table",
                table.stats.consumed,
                table.stats.file_len
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

            // Most entries carry text; a few are intentionally empty.
            let filled = table.entries.iter().filter(|(_, text)| !text.is_empty()).count();
            assert!(
                filled * 100 / table.entries.len() >= 80,
                "{name}: only {filled} of {} entries have text",
                table.entries.len()
            );

            if samples.len() < 4 {
                if let Some((id, text)) = table.entries.iter().find(|(_, t)| !t.is_empty()) {
                    samples.push((id.clone(), text.clone()));
                }
            }
            tables += 1;
            entries += table.entries.len();
        }
    }

    eprintln!("parsed {tables} English tables, {entries} entries exactly");
    for (id, text) in &samples {
        eprintln!("  sample: {id} -> {text:?}");
    }
    assert!(tables >= 3, "expected several localisation tables, found {tables}");
    assert!(entries > 20_000, "expected a large corpus, found {entries} entries");
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
}
