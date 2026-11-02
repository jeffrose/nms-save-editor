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

    // What the archive holds can be read back without decompressing the save:
    // the metadata inside says which state it is, which is what choosing
    // between restore points needs.
    let store = nms_edit::BackupStore::new(backups.clone(), 3);
    let summary = store.inspect(&backup.path).expect("inspect the archive");
    assert_eq!(summary.files.len(), 2, "the archive holds the data file and its metadata");
    assert_eq!(summary.slot, Some(slot));
    let inside = summary.metadata.as_ref().expect("the archive holds the metadata file");
    assert_eq!(inside.save_name(), before_meta.save_name(), "the state before the edit");
    assert_eq!(inside.play_time, before_meta.play_time);
    assert_eq!(summary.save_name().as_deref(), Some(before_meta.save_name().as_str()));
    assert_eq!(summary.state_time(), before_meta.timestamp());
    assert!(!summary.kept, "a fresh archive is not pinned");

    // Pinning takes it out of the retention count and renames it, which is the
    // only difference a listing shows.
    let pinned = store.set_kept(&backup.path, true).expect("pin");
    assert!(nms_edit::is_kept(&pinned));
    let label = slot.data_file_name().trim_end_matches(".hg").to_string();
    let listed = store.list(&label).expect("list");
    assert_eq!(listed.len(), 1);
    assert!(listed[0].kept);
    assert_eq!(listed[0].path, pinned);
    let unpinned = store.set_kept(&pinned, false).expect("unpin");
    assert!(!nms_edit::is_kept(&unpinned));

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

/// A write into a live-looking tree goes through, is backed up, and is
/// reported as a live location so the caller can say so out loud.
///
/// The owner of this machine asked for this on 2026-11-02, once every write had
/// a backup and a pre-write verification behind it; what remains of the old
/// refusal is the classification, which [`nms_edit::live_notice`] turns into a
/// warning line.
#[test]
fn a_live_looking_folder_is_written_with_a_backup_and_a_notice() {
    let Some(dir) = account_dir() else {
        eprintln!("skipping: no fixtures");
        return;
    };
    let fake = std::env::temp_dir().join(format!("nms-edit-live-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&fake);
    let live = fake.join("compatdata/275850/pfx/drive_c/users/steamuser/AppData/Roaming/HelloGames/NMS/st_test");
    std::fs::create_dir_all(&live).unwrap();
    let slot = Slot::player_state(1);
    std::fs::copy(dir.join(slot.data_file_name()), live.join(slot.data_file_name())).unwrap();
    std::fs::copy(dir.join(slot.metadata_file_name()), live.join(slot.metadata_file_name())).unwrap();

    let data_path = live.join(slot.data_file_name());
    let mut bundle = SaveBundle::load(&data_path, KeyMap::builtin()).unwrap();
    bundle.set("CommonStateData.SaveName", Value::from("Written To A Live Tree")).unwrap();

    let report = bundle
        .commit(&CommitOptions {
            backup_root: Some(live.join("Save Backups")),
            timestamp: Some(1_793_488_365),
            ..Default::default()
        })
        .unwrap();
    assert!(matches!(report.data, nms_format::WriteOutcome::Written(_)), "the write happens");
    let backup = report.backup.expect("a live write still takes a backup");
    assert!(backup.path.is_file());

    let reloaded = SaveBundle::load(&data_path, KeyMap::builtin()).unwrap();
    assert_eq!(reloaded.info().save_name.as_deref(), Some("Written To A Live Tree"));

    // The classification that drives the warning is still there...
    let notice = nms_edit::live_notice(&data_path).expect("a live path is reported");
    assert!(notice.contains("Steam"), "{notice}");

    // ...and a declared scratch copy is silent, which is how the test harness
    // avoids a warning on a copied prefix.
    nms_edit::mark_sandbox(&fake).unwrap();
    assert_eq!(nms_edit::live_notice(&data_path), None);

    let _ = std::fs::remove_dir_all(&fake);
}

// ---------------------------------------------------------------------------
// M4: player state — currencies, inventories, known items
// ---------------------------------------------------------------------------

/// The mapping the CLI would load: the built-in summary keys plus the
/// community mapping from the usual config location or `NMSEDIT_MAPPING`.
///
/// Inventory fields are not in the built-in map, so these tests cannot run
/// without it; like the fixtures themselves, they skip instead of failing.
fn full_keys() -> Option<KeyMap> {
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
    let loaded = KeyMap::load(&path).ok()?;
    Some(KeyMap::builtin().extended_with(&loaded))
}

/// Count the slot entries the file itself contains, independently of the
/// reader, so a slot that fails to parse cannot silently disappear.
fn raw_slot_count(value: &Value, keys: &KeyMap) -> usize {    fn member<'a>(value: &'a Value, keys: &KeyMap, name: &str) -> Option<&'a Value> {
        let object = value.as_object()?;
        keys.candidates(name).find_map(|candidate| object.get(candidate))
    }
    let is_inventory = ["Slots", "ValidSlotIndices", "Width", "Height"]
        .iter()
        .all(|name| member(value, keys, name).is_some());
    if is_inventory {
        return member(value, keys, "Slots").and_then(Value::as_array).map_or(0, Vec::len);
    }
    match value {
        Value::Object(map) => map.values().map(|child| raw_slot_count(child, keys)).sum(),
        Value::Array(items) => items.iter().map(|child| raw_slot_count(child, keys)).sum(),
        _ => 0,
    }
}

#[test]
fn every_inventory_obeys_the_games_own_rules() {
    let Some(dir) = account_dir() else {
        eprintln!("skipping: no fixtures (run tools/fetch-fixtures.sh)");
        return;
    };
    let Some(keys) = full_keys() else {
        eprintln!("skipping: no community mapping (set NMSEDIT_MAPPING, or run `nmsedit data refresh`)");
        return;
    };

    let mut saves = 0usize;
    let mut containers = 0usize;
    let mut slots = 0usize;
    let mut kinds = std::collections::BTreeMap::new();
    let mut auxiliary_over_max = 0usize;

    for slot in player_slots(&dir) {
        let bundle = SaveBundle::load(&dir.join(slot.data_file_name()), keys.clone())
            .unwrap_or_else(|e| panic!("{slot}: {e}"));
        let inventories = bundle.inventories();
        assert!(!inventories.is_empty(), "{slot}: no inventories found");

        // The reader walks the active player-state context; the oracle must
        // walk exactly the same subtree (CommonStateData also carries season
        // inventories, which are not player-state inventories).
        let context = bundle.player_context().expect("a player context");
        let root = bundle.document.tree().as_object().expect("object root");
        let context_key = keys
            .candidates(context)
            .find(|candidate| root.contains_key(*candidate))
            .unwrap_or(context);
        let raw = raw_slot_count(&root[context_key], &keys);
        let parsed: usize = inventories.iter().map(|inventory| inventory.slots.len()).sum();
        assert_eq!(parsed, raw, "{slot}: {} slots could not be read", raw - parsed);

        for inventory in &inventories {
            containers += 1;
            assert!(inventory.width * inventory.height > 0 || inventory.slots.is_empty(), "{slot}: {}", inventory.path);
            let valid: std::collections::HashSet<_> = inventory.valid_cells.iter().copied().collect();
            let mut occupied = std::collections::HashSet::new();
            for item in &inventory.slots {
                slots += 1;
                *kinds.entry(item.kind).or_insert(0usize) += 1;
                assert!(item.id.starts_with('^'), "{slot}: {} id {:?}", inventory.path, item.id);
                assert!(item.amount >= -1, "{slot}: {} has amount {}", inventory.path, item.amount);
                if inventory.role == nms_edit::InventoryRole::Player {
                    assert!(
                        item.amount <= item.max_amount,
                        "{slot}: {} {} holds {} of a {}-stack",
                        inventory.path,
                        item.id,
                        item.amount,
                        item.max_amount
                    );
                } else if item.amount > item.max_amount {
                    // In maintenance containers `Amount` is a repair-ingredient
                    // quantity, not a stack count, so it may exceed `MaxAmount`.
                    auxiliary_over_max += 1;
                }
                assert!(
                    item.cell.x < inventory.width && item.cell.y < inventory.height,
                    "{slot}: {} slot at {} is outside {}x{}",
                    inventory.path,
                    item.cell,
                    inventory.width,
                    inventory.height
                );
                assert!(
                    occupied.insert(item.cell),
                    "{slot}: {} holds two items at {}",
                    inventory.path,
                    item.cell
                );
                assert!(
                    valid.contains(&item.cell),
                    "{slot}: {} slot at {} is not in ValidSlotIndices",
                    inventory.path,
                    item.cell
                );
            }
        }
        saves += 1;
    }

    eprintln!(
        "checked {slots} slots in {containers} inventories across {saves} saves: {:?} ({auxiliary_over_max} auxiliary slots above MaxAmount)",
        kinds
    );
    // The real fixture set is 30 saves with 52,874 containers and 58,429
    // slots; the floor keeps the test meaningful if fixtures are trimmed.
    assert!(saves >= 5, "expected several saves, found {saves}");
    assert!(containers > 1_000, "expected thousands of containers, found {containers}");
    assert!(slots > 10_000, "expected tens of thousands of slots, found {slots}");
}

#[test]
fn currency_edits_commit_and_round_trip() {
    let Some(dir) = account_dir() else {
        eprintln!("skipping: no fixtures");
        return;
    };
    let Some(keys) = full_keys() else {
        eprintln!("skipping: no community mapping");
        return;
    };
    let slot = Slot::player_state(1);
    let scratch = scratch_copy(&dir, slot, "m4-currency");
    let data_path = scratch.join(slot.data_file_name());

    let mut bundle = SaveBundle::load(&data_path, keys.clone()).unwrap();
    let before = bundle.currencies().expect("player currencies");
    let quicksilver = if before.quicksilver == 4242 { 4243 } else { 4242 };
    // One fixture save is already at the unit cap, which the game writes as
    // `-1`; pick a value that is certainly different.
    let units = if before.units == u32::MAX { u32::MAX - 1 } else { u32::MAX };

    assert!(bundle.set_currency(nms_edit::Currency::Units, units).expect("set units"));
    assert!(bundle
        .set_currency(nms_edit::Currency::Quicksilver, quicksilver)
        .expect("set quicksilver"));

    // The cap is written as the signed integer the game itself wrote.
    let units_path = format!("{}.PlayerStateData.Units", bundle.player_context().unwrap());
    let expected = if units == u32::MAX { -1 } else { i64::from(units as i32) };
    assert_eq!(bundle.get(&units_path).unwrap().as_i64(), Some(expected), "balances use the game's signed form");

    let plan = bundle.plan(1_793_488_365).unwrap();
    let changed: Vec<&str> = plan.changes.fields_changed.iter().map(|field| field.path.as_str()).collect();
    assert!(
        changed.iter().any(|path| path.ends_with(".Units")),
        "units must be reported as changed: {changed:?}"
    );
    assert!(
        changed.iter().any(|path| path.ends_with(".Specials")),
        "quicksilver must be reported as changed: {changed:?}"
    );

    let report = bundle
        .commit(&CommitOptions {
            backup_root: Some(scratch.join("backups")),
            keep_backups: 3,
            timestamp: Some(1_793_488_365),
            dry_run: false,
        })
        .unwrap();
    assert!(matches!(report.data, nms_format::WriteOutcome::Written(_)));
    assert!(report.warnings.is_empty(), "unexpected warnings: {:?}", report.warnings);

    let reloaded = SaveBundle::load(&data_path, keys).unwrap();
    let after = reloaded.currencies().expect("currencies after reload");
    assert_eq!(after.units, units, "the balance must survive a round trip");
    assert_eq!(after.quicksilver, quicksilver);
    assert_eq!(after.nanites, before.nanites, "untouched balances must not move");

    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn item_edits_commit_and_reload() {
    let Some(dir) = account_dir() else {
        eprintln!("skipping: no fixtures");
        return;
    };
    let Some(keys) = full_keys() else {
        eprintln!("skipping: no community mapping");
        return;
    };
    let slot = Slot::player_state(13);
    if !dir.join(slot.data_file_name()).is_file() {
        eprintln!("skipping: {} not present", slot.data_file_name());
        return;
    }
    let scratch = scratch_copy(&dir, slot, "m4-items");
    let data_path = scratch.join(slot.data_file_name());

    let mut bundle = SaveBundle::load(&data_path, keys.clone()).unwrap();
    let inventory = bundle.inventory("Inventory").expect("the exosuit inventory");
    let path = inventory.path.clone();
    let before_slots = inventory.slots.len();
    if inventory.free_cells().is_empty() {
        eprintln!("skipping: the exosuit inventory has no free cells");
        let _ = std::fs::remove_dir_all(&scratch);
        return;
    }

    // 1. Top up a stack that has room, which must not add a slot.
    let fill = inventory
        .slots
        .iter()
        .find(|item| item.kind != nms_edit::ItemKind::Technology && item.amount >= 0 && item.amount < item.max_amount)
        .cloned()
        .expect("a partly filled stack in the exosuit inventory");

    // 2. Put an item the exosuit does not carry yet into a free cell. The id
    //    comes from another inventory, so this is real save data rather than a
    //    made-up string the game would not recognise.
    let carried: std::collections::HashSet<String> =
        inventory.slots.iter().map(|item| item.id.clone()).collect();
    let newcomer = bundle
        .inventories()
        .iter()
        .filter(|other| other.role == nms_edit::InventoryRole::Player)
        .flat_map(|other| other.slots.iter())
        .filter(|item| item.kind != nms_edit::ItemKind::Technology)
        .map(|item| (item.id.clone(), item.kind))
        .find(|(id, kind)| !carried.contains(id) && bundle.suggest_stack_size(&path, id, *kind).is_some())
        .expect("an item from elsewhere in the save");

    let mut item = nms_edit::NewItem::new(fill.id.clone(), 1);
    item.kind = Some(fill.kind);
    let filled = bundle.add_item(&path, &item).expect("top up a stack");
    assert_eq!(filled.len(), 1);
    assert_eq!(filled[0].position, fill.position);
    assert_eq!(filled[0].amount, fill.amount + 1);

    let mut item = nms_edit::NewItem::new(newcomer.0.clone(), 1);
    item.kind = Some(newcomer.1);
    let created = bundle.add_item(&path, &item).expect("add a new stack");
    assert_eq!(created.len(), 1);
    let created_cell = created[0].cell;

    let report = bundle
        .commit(&CommitOptions {
            backup_root: Some(scratch.join("backups")),
            keep_backups: 3,
            timestamp: Some(1_793_488_365),
            dry_run: false,
        })
        .unwrap();
    assert!(matches!(report.data, nms_format::WriteOutcome::Written(_)));
    assert!(report.warnings.is_empty(), "unexpected warnings: {:?}", report.warnings);

    let reloaded = SaveBundle::load(&data_path, keys).unwrap();
    let inventory = reloaded.inventory("Inventory").expect("the exosuit inventory");
    assert_eq!(inventory.slots.len(), before_slots + 1, "exactly one slot was added");
    assert_eq!(inventory.slots[fill.position].amount, fill.amount + 1, "the existing stack grew");
    let added = inventory
        .slots
        .iter()
        .find(|item| item.id == newcomer.0)
        .expect("the new item");
    assert_eq!(added.amount, 1);
    assert_eq!(added.kind, newcomer.1);
    assert_eq!(added.cell, created_cell);

    // The grid must still be consistent after the edit.
    let mut cells = std::collections::HashSet::new();
    for item in &inventory.slots {
        assert!(cells.insert(item.cell), "two items at {}", item.cell);
        assert!(inventory.valid_cells.contains(&item.cell), "{} is not a valid cell", item.cell);
    }

    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn repair_and_move_commit_and_reload() {
    let Some(dir) = account_dir() else {
        eprintln!("skipping: no fixtures");
        return;
    };
    let Some(keys) = full_keys() else {
        eprintln!("skipping: no community mapping");
        return;
    };
    let slot = Slot::player_state(1);
    let scratch = scratch_copy(&dir, slot, "m4-repair-move");
    let data_path = scratch.join(slot.data_file_name());

    let mut bundle = SaveBundle::load(&data_path, keys.clone()).unwrap();

    // Repair whatever is damaged in a player-facing inventory.
    let damaged: Vec<(nms_edit::InventoryPath, usize)> = bundle
        .inventories()
        .into_iter()
        .filter(|inventory| inventory.role == nms_edit::InventoryRole::Player)
        .flat_map(|inventory| {
            let path = inventory.path.clone();
            inventory
                .slots
                .into_iter()
                .filter(|item| item.damaged())
                .map(move |item| (path.clone(), item.position))
        })
        .collect();
    for (path, index) in &damaged {
        assert!(bundle.repair(path, *index).unwrap(), "slot {index} of {path} should repair");
    }

    // Move a slot to a free cell in the exosuit inventory.
    let inventory = bundle.inventory("Inventory").expect("the exosuit inventory");
    let path = inventory.path.clone();
    let free = inventory.free_cells();
    let movable = inventory.slots.first().cloned();
    let (movable, target) = match (movable, free.first().copied()) {
        (Some(item), Some(cell)) => (item, cell),
        _ => {
            eprintln!("skipping moves: no slot or free cell");
            let _ = std::fs::remove_dir_all(&scratch);
            return;
        }
    };
    assert!(bundle.move_item(&path, movable.position, target).unwrap());

    let report = bundle
        .commit(&CommitOptions {
            backup_root: Some(scratch.join("backups")),
            keep_backups: 3,
            timestamp: Some(1_793_488_365),
            dry_run: false,
        })
        .unwrap();
    assert!(matches!(report.data, nms_format::WriteOutcome::Written(_)));

    let reloaded = SaveBundle::load(&data_path, keys).unwrap();
    for (path, index) in &damaged {
        let inventory = reloaded.inventories().into_iter().find(|inv| inv.path == *path).expect("inventory");
        let item = inventory.slots.iter().find(|item| item.position == *index).expect("slot");
        assert!(!item.damaged(), "repaired slot must stay repaired");
    }
    let inventory = reloaded.inventory("Inventory").expect("the exosuit inventory");
    let moved = inventory.slots.iter().find(|item| item.position == movable.position).expect("moved slot");
    assert_eq!(moved.cell, target, "the slot must be in its new cell");
    assert_eq!(moved.id, movable.id, "moving must not change the item");

    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn known_lists_accept_and_forget_items_in_every_save() {
    let Some(dir) = account_dir() else {
        eprintln!("skipping: no fixtures");
        return;
    };
    let Some(keys) = full_keys() else {
        eprintln!("skipping: no community mapping");
        return;
    };

    let mut checked = 0usize;
    for slot in player_slots(&dir) {
        let mut bundle = SaveBundle::load(&dir.join(slot.data_file_name()), keys.clone())
            .unwrap_or_else(|e| panic!("{slot}: {e}"));
        for list in nms_edit::KnownList::ALL {
            let before = bundle.known(list).unwrap_or_else(|e| panic!("{slot}: {}: {e}", list.label()));
            let probe = format!("^NMSEDIT_PROBE_{}", list.field().to_ascii_uppercase());
            assert!(!before.contains(&probe), "{slot}: fixture already contains the probe id");
            let added = bundle
                .set_known(list, &probe, true)
                .unwrap_or_else(|e| panic!("{slot}: {} add: {e}", list.label()));
            assert!(added, "{slot}: {} add reported no change", list.label());
            assert!(bundle.known(list).unwrap().contains(&probe), "{slot}: {} add did not stick", list.label());
            assert!(!bundle.set_known(list, &probe, true).unwrap(), "{slot}: {} duplicate add", list.label());
            assert!(bundle.set_known(list, &probe, false).unwrap(), "{slot}: {} remove", list.label());
            assert_eq!(bundle.known(list).unwrap(), before, "{slot}: {} remove changed the list", list.label());
        }
        checked += 1;
    }
    eprintln!("checked known lists in {checked} saves");
    assert!(checked > 0, "no saves checked");
}
