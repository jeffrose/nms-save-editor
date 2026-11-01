//! Fixture tests for the edit layer.
//!
//! The load-bearing one is [`metadata_mirrors_save_content`]: it re-derives
//! every metadata field the editor understands from the save JSON and checks
//! the result against the game's own metadata across all 30 save pairs. If the
//! derivation were wrong anywhere — a name, a location, a mode, a play time, a
//! version word — that test would catch it.

use std::path::{Path, PathBuf};

use nms_edit::{CommitOptions, SaveBundle, format_timestamp};
use nms_format::metadata::{Metadata, Slot};
use nms_model::{Document, KeyMap};
use serde_json::Value;

fn fixtures_root() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("NMS_FIXTURES") {
        let path = PathBuf::from(path);
        return path.is_dir().then_some(path);
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?;
    let path = root.join("fixtures");
    path.is_dir().then_some(path)
}

fn account_dir() -> Option<PathBuf> {
    let root = fixtures_root()?;
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        if dir.join("save.hg").is_file() {
            return Some(dir);
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    stack.push(entry.path());
                }
            }
        }
    }
    None
}

fn player_slots(dir: &Path) -> Vec<Slot> {
    (Slot::FIRST_PLAYER..=Slot::LAST_PLAYER)
        .filter_map(Slot::player_state_checked)
        .filter(|slot| {
            let (data, metadata) = (dir.join(slot.data_file_name()), dir.join(slot.metadata_file_name()));
            data.is_file() && metadata.is_file()
        })
        .collect()
}

/// Copy a slot's files into a scratch directory so tests never touch fixtures.
fn scratch_copy(dir: &Path, slot: Slot, test_name: &str) -> PathBuf {
    let scratch = std::env::temp_dir().join(format!("nms-edit-{}-{}", test_name, std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).unwrap();
    std::fs::copy(dir.join(slot.data_file_name()), scratch.join(slot.data_file_name())).unwrap();
    std::fs::copy(dir.join(slot.metadata_file_name()), scratch.join(slot.metadata_file_name())).unwrap();
    scratch
}

#[test]
fn metadata_mirrors_save_content() {
    let Some(dir) = account_dir() else {
        eprintln!("skipping: no fixtures (run tools/fetch-fixtures.sh)");
        return;
    };

    let mut checked = 0usize;
    for slot in player_slots(&dir) {
        let bundle = SaveBundle::load(&dir.join(slot.data_file_name()), KeyMap::builtin())
            .unwrap_or_else(|e| panic!("{slot}: {e}"));

        // Use the metadata's own timestamp so word 89 is not reported as a change.
        let timestamp = bundle.metadata.as_ref().unwrap().words[89] as u64;
        let plan = bundle.plan(timestamp).unwrap();
        let metadata = plan.metadata.as_ref().expect("player slots have metadata");

        // The mirror fields must actually be derived, otherwise the comparison
        // below would pass without checking anything.
        assert!(metadata.patch.save_name.is_some(), "{slot}: save name was not derived");
        assert!(metadata.patch.location.is_some(), "{slot}: location was not derived");
        assert!(metadata.patch.game_mode.is_some(), "{slot}: game mode was not derived");
        assert!(metadata.patch.play_time.is_some(), "{slot}: play time was not derived");
        assert!(metadata.patch.version.is_some(), "{slot}: version word was not derived");

        // Word 15 is the compressed size: the container is re-encoded rather
        // than byte-reproduced, so a size change there is expected and correct.
        // Every other difference would mean the derivation disagrees with the
        // game, which is the thing this test exists to catch.
        let unexpected: Vec<usize> = metadata
            .changed_words
            .iter()
            .copied()
            .filter(|word| !matches!(word, 15))
            .collect();
        assert!(
            unexpected.is_empty(),
            "{slot}: metadata words {unexpected:?} differ from the game's own file\n  derived: {} | {} | {}\n  stored:  {} | {} | {}\n  warnings: {:?}",
            bundle.metadata.as_ref().unwrap().save_name(),
            bundle.metadata.as_ref().unwrap().location(),
            bundle.metadata.as_ref().unwrap().game_mode(),
            Metadata::read(&metadata.bytes, slot).unwrap().save_name(),
            Metadata::read(&metadata.bytes, slot).unwrap().location(),
            Metadata::read(&metadata.bytes, slot).unwrap().game_mode(),
            metadata.warnings,
        );
        assert!(metadata.warnings.is_empty(), "{slot}: unexpected warnings {:?}", metadata.warnings);

        // And the stored metadata really does carry the same values we derived,
        // i.e. the derivation is not merely "preserve what was there".
        let stored = bundle.metadata.as_ref().unwrap();
        assert_eq!(metadata.patch.save_name.as_deref(), Some(stored.save_name().as_str()), "{slot}: derived name");
        assert_eq!(metadata.patch.location.as_deref(), Some(stored.location().as_str()), "{slot}: derived location");
        assert_eq!(metadata.patch.game_mode.as_deref(), Some(stored.game_mode().as_str()), "{slot}: derived mode");
        assert_eq!(metadata.patch.play_time, Some(stored.play_time), "{slot}: derived play time");

        checked += 1;
    }
    eprintln!("cross-checked metadata derivation for {checked} slots");
    assert!(checked > 0, "no player slots found to check");
}

#[test]
fn unmodified_save_commits_nothing() {
    let Some(dir) = account_dir() else {
        eprintln!("skipping: no fixtures");
        return;
    };
    let slot = Slot::player_state(1);
    let scratch = scratch_copy(&dir, slot, "noop");
    let data_path = scratch.join(slot.data_file_name());
    let before_bytes = std::fs::read(&data_path).unwrap();
    let before_mtime = std::fs::metadata(&data_path).unwrap().modified().unwrap();

    let mut bundle = SaveBundle::load(&data_path, KeyMap::builtin()).unwrap();
    let options = CommitOptions {
        backup_root: Some(scratch.join("backups")),
        keep_backups: 3,
        timestamp: Some(1_793_416_365),
        dry_run: false,
    };
    let report = bundle.commit(&options).unwrap();

    assert_eq!(format!("{:?}", report.data), "Unchanged", "an unedited save must not be written");
    assert_eq!(format!("{:?}", report.metadata), "Unchanged", "metadata must not be rewritten either");
    assert!(report.backup.is_none(), "no backup is needed when nothing is written");
    assert_eq!(std::fs::read(&data_path).unwrap(), before_bytes);
    assert_eq!(std::fs::metadata(&data_path).unwrap().modified().unwrap(), before_mtime);
    assert!(!scratch.join("backups").exists(), "no backup directory should be created");
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn rename_updates_json_and_metadata_and_backs_up() {
    let Some(dir) = account_dir() else {
        eprintln!("skipping: no fixtures");
        return;
    };
    let slot = Slot::player_state(13);
    if !dir.join(slot.data_file_name()).is_file() {
        eprintln!("skipping: {} not present in fixtures", slot.data_file_name());
        return;
    }
    let scratch = scratch_copy(&dir, slot, "rename");
    let data_path = scratch.join(slot.data_file_name());
    let metadata_path = scratch.join(slot.metadata_file_name());
    let backups = scratch.join("backups");

    let mut bundle = SaveBundle::load(&data_path, KeyMap::builtin()).unwrap();
    let before_meta = bundle.metadata.clone().unwrap();
    let before_location = before_meta.location();
    let before_mode = before_meta.game_mode();

    bundle.set("CommonStateData.SaveName", Value::from("Renamed By Test")).unwrap();

    let options = CommitOptions {
        backup_root: Some(backups.clone()),
        keep_backups: 3,
        timestamp: Some(1_793_488_365),
        dry_run: false,
    };
    let report = bundle.commit(&options).unwrap();

    assert!(matches!(report.data, nms_format::WriteOutcome::Written(_)), "data file must be written");
    assert!(matches!(report.metadata, nms_format::WriteOutcome::Written(_)), "metadata must be refreshed");
    assert!(report.warnings.is_empty(), "unexpected warnings: {:?}", report.warnings);

    // The backup holds the pre-edit files.
    let backup = report.backup.expect("a backup must be created");
    assert_eq!(
        backup.path.file_name().unwrap().to_string_lossy(),
        format!("save13_{}.zip", format_timestamp(1_793_488_365))
    );
    let archive = std::fs::File::open(&backup.path).unwrap();
    let zip = zip::ZipArchive::new(archive).unwrap();
    assert_eq!(zip.len(), 2, "backup must contain the data file and its metadata");

    // Re-read everything from disk: the JSON and the metadata must agree.
    let reloaded = SaveBundle::load(&data_path, KeyMap::builtin()).unwrap();
    assert_eq!(reloaded.info().save_name.as_deref(), Some("Renamed By Test"));

    let metadata_bytes = std::fs::read(&metadata_path).unwrap();
    let metadata = Metadata::read(&metadata_bytes, slot).unwrap();
    assert_eq!(metadata.save_name(), "Renamed By Test", "metadata must mirror the new name");
    assert_eq!(metadata.location(), before_location, "location must be preserved when unchanged");
    assert_eq!(metadata.game_mode(), before_mode, "game mode must be preserved when unchanged");
    assert_eq!(metadata.words[100..], before_meta.words[100..], "unmodelled words must be copied verbatim");

    // Sizes must describe the files that actually exist now.
    let data_bytes = std::fs::read(&data_path).unwrap();
    let reparsed = nms_format::SaveFile::from_bytes(&data_bytes).unwrap();
    assert_eq!(metadata.decompressed_size as usize, reparsed.stored_payload().len());
    assert_eq!(metadata.compressed_size as usize, data_bytes.len());
    assert_eq!(metadata.play_time, reloaded.info().play_time.unwrap_or(0));

    // The document layer round-trips the file that was written.
    let document = Document::parse(&reparsed.payload).unwrap();
    assert_eq!(document.payload().unwrap(), reparsed.payload);

    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn small_edit_only_changes_a_few_bytes() {
    let Some(dir) = account_dir() else {
        eprintln!("skipping: no fixtures");
        return;
    };
    let slot = Slot::player_state(1);
    let scratch = scratch_copy(&dir, slot, "small-edit");
    let data_path = scratch.join(slot.data_file_name());

    let mut bundle = SaveBundle::load(&data_path, KeyMap::builtin()).unwrap();
    let before = bundle.save.payload.clone();
    bundle.set("CommonStateData.TotalPlayTime", Value::from(1)).unwrap();
    let plan = bundle.plan(1_793_488_365).unwrap();

    // The semantic report is the meaningful one: exactly one field changed.
    assert_eq!(plan.changes.fields_changed.len(), 1, "{:#?}", plan.changes.fields_changed);
    assert_eq!(plan.changes.fields_changed[0].path, "CommonStateData.TotalPlayTime");
    assert_eq!(plan.changes.fields_changed[0].after.as_deref(), Some("1"));

    // And the raw bytes agree: the edit is a small, local region.
    let changed = nms_model::ByteDiff::compute(&before, &plan.payload);
    assert!(!changed.is_identical(), "the edit must actually change the payload");
    assert_eq!(changed.common_prefix, 65, "everything before the edited field is untouched");
    assert!(
        plan.payload.len() < before.len(),
        "replacing a 7-digit play time with 1 should shorten the payload"
    );
    assert!(
        changed.first_diff.is_some_and(|offset| offset < 200),
        "the edited field sits near the start of the document"
    );

    let _ = std::fs::remove_dir_all(&scratch);
}
