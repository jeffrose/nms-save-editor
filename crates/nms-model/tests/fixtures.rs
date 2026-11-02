//! Golden tests against real save files.
//!
//! These run only when fixtures are present (see `tools/fetch-fixtures.sh` and
//! `fixtures/README.md`). They are the load-bearing tests for the project's
//! core promise: **reading a real save and writing it back without edits is
//! byte-identical** — for the data files, for the metadata files, and for the
//! JSON document layer.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use nms_format::metadata::{Metadata, Slot};
use nms_format::SaveFile;
use nms_model::{ByteDiff, Document, KeyMap};

fn fixtures_root() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("NMS_FIXTURES") {
        let path = PathBuf::from(path);
        return path.is_dir().then_some(path);
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?;
    let path = root.join("fixtures");
    path.is_dir().then_some(path)
}

/// Directories containing `save.hg`, one per account/platform.
fn account_dirs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if dir.join("save.hg").is_file() {
            out.push(dir.clone());
            continue;
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    stack.push(entry.path());
                }
            }
        }
    }
    out.sort();
    out
}

fn hg_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().map(|e| e == "hg").unwrap_or(false))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

#[test]
fn every_data_file_round_trips_byte_identically() {
    let Some(root) = fixtures_root() else {
        eprintln!("skipping: no fixtures/ directory (run tools/fetch-fixtures.sh)");
        return;
    };
    let mut checked = 0usize;
    for dir in account_dirs(&root) {
        for path in hg_files(&dir) {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if name.starts_with("mf_") {
                continue;
            }
            let raw = std::fs::read(&path).unwrap();
            let save = SaveFile::from_bytes(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert!(!save.is_modified(), "{}: a freshly read save must not report modifications", path.display());

            // The container must carry the payload faithfully: re-encode and re-read.
            let re_encoded = save.to_bytes();
            let reparsed = SaveFile::from_bytes(&re_encoded)
                .unwrap_or_else(|e| panic!("{}: re-encoded container is unreadable: {e}", path.display()));
            assert_eq!(
                reparsed.payload,
                save.payload,
                "{}: payload changed when re-encoding the container",
                path.display()
            );
            assert_eq!(
                reparsed.stored_payload(),
                save.stored_payload(),
                "{}: stored payload (including trailing NUL) changed",
                path.display()
            );

            // For plain JSON files the container *is* the payload, so the whole
            // file must round-trip byte-exactly.
            if save.container == nms_format::Container::PlainJson {
                assert_eq!(re_encoded, raw, "{}: plain JSON file must round-trip byte-exactly", path.display());
            }

            // The document layer must also return the original payload untouched.
            let doc = Document::parse(&save.payload).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert!(!doc.is_dirty(), "{}: a freshly parsed document must not be dirty", path.display());
            let payload = doc.payload().unwrap();
            assert_eq!(payload, save.payload, "{}: unedited payload must be byte-identical", path.display());
            assert!(doc.key_count() > 100, "{}: suspiciously few JSON keys", path.display());

            // Summary fields should be present in a real player save.
            // Account-level files (accountdata.hg) have a different shape.
            if let Some(slot) = Slot::from_file_name(&name) {
                if slot.player_number().is_some() {
                    let info = doc.info(&KeyMap::builtin());
                    assert!(info.version.is_some(), "{}: no Version field", path.display());
                    assert!(info.platform.is_some(), "{}: no Platform field", path.display());
                }
            }

            checked += 1;
        }
    }
    eprintln!("checked {checked} data files");
    if checked == 0 {
        eprintln!("skipping: no save files found under {}", root.display());
    }
}

#[test]
fn every_metadata_file_round_trips_byte_identically() {
    let Some(root) = fixtures_root() else {
        eprintln!("skipping: no fixtures/ directory");
        return;
    };
    let mut checked = 0usize;
    for dir in account_dirs(&root) {
        for path in hg_files(&dir) {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let Some(slot) = Slot::from_metadata_file_name(&name) else { continue };
            let raw = std::fs::read(&path).unwrap();
            let meta = Metadata::read(&raw, slot).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let re_encrypted = meta.to_bytes().unwrap();
            let diff = ByteDiff::compute(&raw, &re_encrypted);
            assert!(
                diff.is_identical(),
                "{}: re-encrypting unchanged metadata changed bytes: {}",
                path.display(),
                diff.summary()
            );
            checked += 1;
        }
    }
    eprintln!("checked {checked} metadata files");
}

/// Metadata must describe its companion data file, otherwise the game shows
/// stale slot information (the reported "metadata size differs" failure mode).
#[test]
fn metadata_sizes_match_their_data_files() {
    let Some(root) = fixtures_root() else {
        eprintln!("skipping: no fixtures/ directory");
        return;
    };
    let mut checked = 0usize;
    for dir in account_dirs(&root) {
        for path in hg_files(&dir) {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let Some(slot) = Slot::from_metadata_file_name(&name) else { continue };
            let data_path = dir.join(slot.data_file_name());
            if !data_path.is_file() {
                continue;
            }
            let raw = std::fs::read(&path).unwrap();
            let meta = Metadata::read(&raw, slot).unwrap();
            let data = std::fs::read(&data_path).unwrap();
            let save = SaveFile::from_bytes(&data).unwrap();

            assert_eq!(
                meta.decompressed_size as usize,
                save.stored_payload().len(),
                "{}: decompressed_size does not match the payload of {}",
                path.display(),
                data_path.display()
            );
            if meta.compressed_size == 0 {
                // Zero means "stored uncompressed" (accountdata.hg, usersettings.hg).
                assert_eq!(
                    save.container,
                    nms_format::Container::PlainJson,
                    "{}: compressed_size 0 but {} is compressed",
                    path.display(),
                    data_path.display()
                );
            } else {
                assert_eq!(
                    meta.compressed_size as usize,
                    data.len(),
                    "{}: compressed_size does not match {}",
                    path.display(),
                    data_path.display()
                );
            }

            // Play time in metadata mirrors the JSON field.
            let doc = Document::parse(&save.payload).unwrap();
            let info = doc.info(&KeyMap::builtin());
            if let Some(play_time) = info.play_time {
                assert_eq!(
                    meta.play_time, play_time,
                    "{}: metadata play time does not match the save JSON",
                    path.display()
                );
            }

            // Version word is the JSON version minus a fixed offset, for player
            // saves only (account files use a different scheme).
            if slot.player_number().is_some() {
                if let Some(version) = info.version {
                    assert_eq!(
                        u64::from(meta.version) + 512,
                        version,
                        "{}: metadata version offset changed",
                        path.display()
                    );
                }
            }
            checked += 1;
        }
    }
    eprintln!("cross-checked {checked} metadata/data pairs");
}

/// Editing a field must change only that field's bytes, and untouched numbers
/// must keep their exact original text.
#[test]
fn edited_save_keeps_untouched_number_text() {
    let Some(root) = fixtures_root() else {
        eprintln!("skipping: no fixtures/ directory");
        return;
    };
    let Some(dir) = account_dirs(&root).into_iter().next() else { return };
    let path = dir.join("save.hg");
    let raw = std::fs::read(&path).unwrap();
    let save = SaveFile::from_bytes(&raw).unwrap();
    let mut doc = Document::parse(&save.payload).unwrap();

    // Collect every numeric literal in the original payload.
    let original = nms_format::latin1::decode(&save.payload);
    let numbers = numeric_literals(&original);
    assert!(numbers.len() > 10_000, "expected a large number of literals, got {}", numbers.len());

    // Make one small, well-formed edit: append a key to the root object.
    doc.tree_mut()["NmsEditProbe"] = serde_json::Value::from("round-trip probe");
    let edited = doc.payload().unwrap();
    let edited_text = nms_format::latin1::decode(&edited);

    let after = numeric_literals(&edited_text);
    let (mut same, mut changed) = (0usize, 0usize);
    for (a, b) in numbers.iter().zip(after.iter()) {
        if a == b {
            same += 1;
        } else {
            changed += 1;
        }
    }
    assert_eq!(changed, 0, "editing must not rewrite untouched numeric literals ({changed} changed)");
    assert!(same > 10_000, "expected most literals to be compared, only {same}");
    assert!(edited_text.contains("NmsEditProbe"), "the edit itself must be present");
}

/// The compiled-in baseline is what a user gets before running `data refresh`,
/// so it must name the bulk of what real saves use — and it must never
/// disagree with an installed community mapping, which is maintained
/// independently of our derivation.
///
/// The 70% floor sits below the measured coverage (76% when the committed file
/// was generated from these fixtures) on purpose: the exact number depends on
/// which saves are installed, and adding saves to `fixtures/` should not break
/// the suite — it should prompt a regeneration, which
/// `tools/build-baseline-mapping.sh` does.
#[test]
fn the_builtin_baseline_names_real_saves() {
    let Some(root) = fixtures_root() else {
        eprintln!("skipping: no fixtures/ directory");
        return;
    };
    let builtin = KeyMap::builtin();
    let mut all_keys = BTreeSet::new();
    let mut checked = 0usize;
    for dir in account_dirs(&root) {
        for path in hg_files(&dir) {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let Some(slot) = Slot::from_file_name(&name) else { continue };
            if slot.player_number().is_none() {
                continue;
            }
            let raw = std::fs::read(&path).unwrap();
            let save = SaveFile::from_bytes(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let doc = Document::parse(&save.payload).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            collect_json_keys(doc.tree(), &mut all_keys);
            checked += 1;
        }
    }
    assert!(checked > 0, "no player saves found under {}", root.display());

    let named = all_keys.iter().filter(|key| builtin.readable_for(key).is_some()).count();
    let percent = named * 100 / all_keys.len().max(1);
    eprintln!("built-in baseline names {named}/{} keys ({percent}%) across {checked} player saves", all_keys.len());
    assert!(
        percent >= 70,
        "the built-in baseline names only {percent}% of the keys real saves use; \
         regenerate it with tools/build-baseline-mapping.sh"
    );

    let Some(community) = community_mapping() else {
        eprintln!("no community mapping installed; skipping the cross-check");
        return;
    };
    let mut compared = 0usize;
    let mut disagreements = Vec::new();
    for key in &all_keys {
        let (Some(ours), Some(theirs)) = (builtin.readable_for(key), community.readable_for(key)) else {
            continue;
        };
        compared += 1;
        if ours != theirs {
            disagreements.push(format!("{key}: built-in says {ours:?}, the community file says {theirs:?}"));
        }
    }
    assert!(
        disagreements.is_empty(),
        "the built-in baseline disagrees with the community mapping:\n{}",
        disagreements.join("\n")
    );
    eprintln!("cross-checked {compared} keys against the community mapping");
}

/// The community mapping, when one is installed, for cross-checking.
fn community_mapping() -> Option<KeyMap> {
    let path = std::env::var("NMSEDIT_MAPPING")
        .ok()
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| {
            let base = match std::env::var("XDG_CONFIG_HOME") {
                Ok(value) if !value.is_empty() => PathBuf::from(value),
                _ => PathBuf::from(std::env::var("HOME").ok()?).join(".config"),
            };
            Some(base.join("nmsedit").join("mapping.json"))
        })?;
    KeyMap::load(&path).ok()
}

fn collect_json_keys(value: &serde_json::Value, keys: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, child) in object {
                keys.insert(key.clone());
                collect_json_keys(child, keys);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_json_keys(item, keys);
            }
        }
        _ => {}
    }
}

/// Extract numeric literals, ignoring digits inside strings. Approximate but
/// sufficient: both sides of the comparison are the same document.
fn numeric_literals(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut in_string = false;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 1,
            b'"' => in_string = !in_string,
            b'-' | b'0'..=b'9' if !in_string => {
                let start = i;
                if bytes[i] == b'-' {
                    i += 1;
                }
                while i < bytes.len() && (bytes[i].is_ascii_digit() || matches!(bytes[i], b'.' | b'e' | b'E' | b'+' | b'-')) {
                    i += 1;
                }
                out.push(text[start..i].to_string());
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    out
}
