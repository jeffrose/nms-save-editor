//! The GUI's headless model.
//!
//! Everything that reads or writes a file lives here, so the drawing code is
//! only a translation of state into widgets and the interesting parts can be
//! tested without a window.
//!
//! Loading is split in two on purpose: a folder scan reads only the small
//! metadata files, which gives every slot's name, mode and play time in
//! milliseconds, while the 9 MB payload of a save is parsed only when that slot
//! is opened.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use nms_data::DataPack;
use nms_edit::{
    live_save_location, now_unix, BackupStore, Cell, CommitOptions, Currencies, Currency, Inventory, ItemKind,
    KnownList, LiveLocation, NewItem, SaveBundle, SuitStat, SuitStats,
};
use nms_format::metadata::Slot;
use nms_model::{describe_raw_path, KeyMap, RawStep};
use serde_json::Value;

/// What a save's metadata file says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotSummary {
    /// Player-chosen save name.
    pub save_name: String,
    /// Location summary the game shows on the load screen.
    pub location: String,
    /// Game mode, e.g. `Normal` or `Creative`.
    pub game_mode: String,
    /// Total play time in seconds.
    pub play_time: u64,
    /// Save version word.
    pub version: u32,
    /// Modification time of the data file.
    pub modified: Option<SystemTime>,
}

impl SlotSummary {
    /// Play time as `hours:minutes:seconds`.
    pub fn play_time_text(&self) -> String {
        format_duration(self.play_time)
    }
}

/// `hours:minutes:seconds`, with hours allowed to run past 24.
pub fn format_duration(seconds: u64) -> String {
    format!("{}:{:02}:{:02}", seconds / 3600, (seconds % 3600) / 60, seconds % 60)
}

/// A value as one line of JSON, for change reports.
///
/// Numbers keep their exact text (`arbitrary_precision`), so a report never
/// shows a value the file does not hold.
fn compact(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

/// One storage slot found in a save folder.
#[derive(Debug, Clone)]
pub struct SlotEntry {
    /// The storage slot the file name identifies.
    pub slot: Slot,
    /// The user-visible game slot (`1..=15`) this save point belongs to.
    pub game_slot: u8,
    /// Which save point this file is: the slot's autosave or its restore point.
    pub save_point: nms_edit::SavePointKind,
    /// Whether this is the newer of the game slot's two save points.
    pub newest: bool,
    /// When the game wrote this save point, in Unix seconds.
    pub state_time: Option<u64>,
    /// The game slot's other save point.
    pub paired_path: Option<PathBuf>,
    /// When the game wrote the other save point.
    pub paired_time: Option<u64>,
    /// Path of the data file (`save*.hg`).
    pub data_path: PathBuf,
    /// Decrypted summary, when the metadata file could be read.
    pub summary: Option<SlotSummary>,
    /// Why the metadata could not be read.
    pub problem: Option<String>,
}

impl SlotEntry {
    /// The file name, which is how a command line addresses the save.
    pub fn file_name(&self) -> String {
        self.data_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Name to show in the slot list.
    pub fn title(&self) -> String {
        match &self.summary {
            Some(summary) if !summary.save_name.is_empty() => summary.save_name.clone(),
            _ => self.slot.to_string(),
        }
    }

    /// A one-line description to show under the title.
    pub fn subtitle(&self) -> String {
        match &self.summary {
            Some(summary) => {
                let mut parts = Vec::new();
                if !summary.game_mode.is_empty() {
                    parts.push(summary.game_mode.clone());
                }
                if !summary.location.is_empty() {
                    parts.push(summary.location.clone());
                }
                parts.push(summary.play_time_text());
                parts.join(" · ")
            }
            None => "metadata could not be read".to_string(),
        }
    }
}

/// A backup archive of the open save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupSummary {
    /// Archive path.
    pub path: PathBuf,
    /// Archive size in bytes.
    pub size: u64,
    /// Whether the archive is pinned against pruning.
    pub kept: bool,
    /// When the archive was last written.
    pub modified: Option<SystemTime>,
    /// When the game last wrote the state inside, in Unix seconds.
    pub state_time: Option<u64>,
    /// The save's name in that state.
    pub save_name: Option<String>,
    /// Play time in that state, in seconds.
    pub play_time: Option<u64>,
}

/// How many archives to keep per save; the same limit the CLI uses.
pub const BACKUP_KEEP: usize = 10;

/// The folder a save's backups live in.
///
/// The CLI's `--backup-dir` defaults to the same place, so both tools share
/// one history.
fn backup_root(data_path: &Path) -> PathBuf {
    data_path.parent().unwrap_or_else(|| Path::new(".")).join("Save Backups")
}

/// One search hit in the JSON document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonHit {
    /// Exact on-disk path of the node.
    pub steps: Vec<RawStep>,
    /// Readable path, with on-disk keys where the map knows no name.
    pub label: String,
    /// One-line rendering of the value.
    pub preview: String,
}

/// How many hits a search keeps before it stops looking.
pub const JSON_HIT_LIMIT: usize = 300;

/// One known-item list and the ids it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownEntry {
    /// Which list.
    pub list: KnownList,
    /// The ids, in a display order that does not depend on how the file
    /// happened to be written.
    pub ids: Vec<String>,
}

/// A save that has been opened and decoded.
pub struct OpenSave {
    /// The slot entry it was loaded from.
    pub entry: SlotEntry,
    /// The loaded save, its document and its metadata.
    pub bundle: SaveBundle,
    /// Summary fields read from the JSON.
    pub info: nms_model::SaveInfo,
    /// Player balances, when the save has a player state.
    pub currencies: Option<Currencies>,
    /// Why the balances could not be read.
    pub currencies_error: Option<String>,
    /// Exosuit stats, when the save has a player state.
    pub suit_stats: Option<SuitStats>,
    /// Why the exosuit stats could not be read.
    pub suit_stats_error: Option<String>,
    /// Every inventory the player state carries, in file order.
    pub inventories: Vec<Inventory>,
    /// The known-item lists the player state carries, with their ids.
    pub known: Vec<KnownEntry>,
}

impl OpenSave {
    /// Whether the in-memory save differs from the file on disk.
    pub fn dirty(&self) -> bool {
        self.bundle.document.is_dirty()
    }

    /// Read a value by exact on-disk path.
    pub fn value_at(&self, steps: &[RawStep]) -> Option<&Value> {
        self.bundle.document.get_raw(steps)
    }

    /// Re-read everything the window shows.
    ///
    /// Every edit goes through the bundle and then through here, so a widget
    /// never draws a value the document no longer holds.
    pub fn refresh(&mut self) {
        self.info = self.bundle.info();
        (self.currencies, self.currencies_error) = match self.bundle.currencies() {
            Ok(currencies) => (Some(currencies), None),
            Err(error) => (None, Some(error.to_string())),
        };
        (self.suit_stats, self.suit_stats_error) = match self.bundle.suit_stats() {
            Ok(stats) => (Some(stats), None),
            Err(error) => (None, Some(error.to_string())),
        };
        self.inventories = self.bundle.inventories();
        self.known = KnownList::ALL
            .into_iter()
            .filter_map(|list| {
                self.bundle.known(list).ok().map(|mut ids| {
                    ids.sort_by_key(|id| id.to_ascii_lowercase());
                    KnownEntry { list, ids }
                })
            })
            .collect();
    }
}

/// A folder of saves and, at most, one opened slot.
pub struct Session {
    /// The folder being browsed.
    pub dir: PathBuf,
    /// Every slot the folder contains, ordered by slot number.
    pub slots: Vec<SlotEntry>,
    /// Key map used to resolve readable field names.
    pub keys: KeyMap,
    /// Names come from the game when a data pack was generated; absent otherwise.
    pub pack: Option<DataPack>,
    /// Human-readable notes about what was or was not found on startup.
    pub notes: Vec<String>,
    /// Folder-level failure, e.g. a missing directory.
    pub error: Option<String>,
    /// The slot currently open, if any.
    pub open: Option<OpenSave>,
}

impl Session {
    /// An empty session, before a folder has been chosen.
    pub fn empty() -> Session {
        Session {
            dir: PathBuf::new(),
            slots: Vec::new(),
            keys: KeyMap::builtin(),
            pack: None,
            notes: Vec::new(),
            error: None,
            open: None,
        }
    }

    /// Open the platform's usual save folder, when one exists.
    ///
    /// The window starts here when it is launched without a folder: the field
    /// is filled in and the slot list appears, instead of an empty window that
    /// asks where the saves are. `NMSEDIT_SAVE_DIR` wins when it is set.
    pub fn open_default() -> Session {
        match nms_edit::default_save_dir() {
            Some(dir) => Session::open_dir(&dir),
            None => {
                let mut session = Session::empty();
                session.notes.push(
                    "no save folder found in the usual locations; type one above, or use Browse".to_string(),
                );
                session
            }
        }
    }

    /// Open a folder with the user's configured key map and data pack.
    pub fn open_dir(dir: &Path) -> Session {
        let mut notes = Vec::new();
        let keys = match default_key_map() {
            Ok(keys) => keys,
            Err(note) => {
                notes.push(note);
                KeyMap::builtin()
            }
        };
        let pack = match default_pack() {
            Ok(pack) => pack,
            Err(note) => {
                notes.push(note);
                None
            }
        };
        Session::open_dir_with(dir, keys, pack, notes)
    }

    /// Open a folder with an explicit key map and data pack.
    pub fn open_dir_with(dir: &Path, keys: KeyMap, pack: Option<DataPack>, notes: Vec<String>) -> Session {
        let mut session = Session {
            dir: dir.to_path_buf(),
            slots: Vec::new(),
            keys,
            pack,
            notes,
            error: None,
            open: None,
        };
        session.rescan();
        session
    }

    /// Re-read the folder listing.
    ///
    /// An open save stays open only if its file is still there; the payload is
    /// not re-read, so a list refresh does not throw away edits in progress.
    pub fn rescan(&mut self) {
        self.error = None;
        if self.dir.as_os_str().is_empty() {
            // No folder chosen yet; do not report that "" cannot be read.
            self.slots.clear();
            self.open = None;
            return;
        }
        self.slots = match scan_dir(&self.dir) {
            Ok(slots) => slots,
            Err(message) => {
                self.error = Some(message);
                Vec::new()
            }
        };
        let open_path = self.open.as_ref().map(|open| open.entry.data_path.clone());
        if let Some(path) = open_path {
            if !self.slots.iter().any(|entry| entry.data_path == path) {
                self.open = None;
            }
        }
    }

    /// Open the slot at `index` in the folder listing.
    pub fn select(&mut self, index: usize) {
        let Some(entry) = self.slots.get(index).cloned() else { return };
        if self.open.as_ref().is_some_and(|open| open.entry.data_path == entry.data_path) {
            return;
        }
        self.open = match SaveBundle::load(&entry.data_path, self.keys.clone()) {
            Ok(bundle) => {
                let info = bundle.info();
                let (currencies, currencies_error) = match bundle.currencies() {
                    Ok(currencies) => (Some(currencies), None),
                    Err(error) => (None, Some(error.to_string())),
                };
                let known = KnownList::ALL
                    .into_iter()
                    .filter_map(|list| {
                        bundle.known(list).ok().map(|mut ids| {
                            ids.sort_by_key(|id| id.to_ascii_lowercase());
                            KnownEntry { list, ids }
                        })
                    })
                    .collect();
                Some(OpenSave {
                    entry,
                    info,
                    bundle,
                    currencies,
                    currencies_error,
                    suit_stats: None,
                    suit_stats_error: None,
                    inventories: Vec::new(),
                    known,
                })
            }
            Err(error) => {
                self.error = Some(format!("{}: {error}", entry.data_path.display()));
                None
            }
        };
        if let Some(open) = self.open.as_mut() {
            open.refresh();
        }
    }

    /// True when the open save has staged edits that are not on disk.
    pub fn dirty(&self) -> bool {
        self.open.as_ref().is_some_and(OpenSave::dirty)
    }

    /// The live location the open save belongs to, if any.
    ///
    /// The window uses this to stay read-only over a cloud-synced folder, the
    /// same classification the edit layer applies when asked to write.
    pub fn live_location(&self) -> Option<LiveLocation> {
        self.open.as_ref().and_then(|open| live_save_location(&open.entry.data_path))
    }

    /// Stage a balance change. Nothing is written until [`Session::save`].
    pub fn set_currency(&mut self, currency: Currency, value: u32) -> Result<String, String> {
        let open = self.open.as_mut().ok_or_else(|| "no save is open".to_string())?;
        let before = open.bundle.currencies().map_err(|error| error.to_string())?.get(currency);
        if before == value {
            return Ok(format!("{} is already {value}", currency.label()));
        }
        open.bundle.set_currency(currency, value).map_err(|error| error.to_string())?;
        open.refresh();
        Ok(format!("{}: {before} -> {value}", currency.label()))
    }

    /// Stage a change to one exosuit stat. Nothing is written until [`Session::save`].
    pub fn set_suit_stat(&mut self, stat: SuitStat, value: u32) -> Result<String, String> {
        let open = self.open.as_mut().ok_or_else(|| "no save is open".to_string())?;
        let before = open.bundle.suit_stats().map_err(|error| error.to_string())?.get(stat);
        if before == value {
            return Ok(format!("{} is already {value}", stat.label()));
        }
        open.bundle.set_suit_stat(stat, value).map_err(|error| error.to_string())?;
        open.refresh();
        Ok(format!("{}: {before} -> {value}", stat.label()))
    }

    /// Stage a new item in the inventory at `inventory`.
    ///
    /// Stack sizes come from the save unless `max_amount` is given, which is
    /// the same rule the CLI follows.
    pub fn add_item(
        &mut self,
        inventory: usize,
        id: &str,
        amount: i64,
        kind: Option<ItemKind>,
        max_amount: Option<i64>,
    ) -> Result<String, String> {
        let open = self.open.as_mut().ok_or_else(|| "no save is open".to_string())?;
        let path = open
            .inventories
            .get(inventory)
            .ok_or_else(|| "no such inventory".to_string())?
            .path
            .clone();
        let mut item = NewItem::new(id.trim(), amount);
        item.kind = kind;
        item.max_amount = max_amount;
        let slots = open.bundle.add_item(&path, &item).map_err(|error| error.to_string())?;
        open.refresh();
        let id = nms_edit::normalize_id(id.trim());
        Ok(format!("added {amount} x {id} to {} ({} slot(s))", path.label(), slots.len()))
    }

    /// Stage a new amount for one slot.
    pub fn set_amount(&mut self, inventory: usize, position: usize, amount: i64) -> Result<String, String> {
        let open = self.open.as_mut().ok_or_else(|| "no save is open".to_string())?;
        let path = open
            .inventories
            .get(inventory)
            .ok_or_else(|| "no such inventory".to_string())?
            .path
            .clone();
        let slot = open.bundle.set_amount(&path, position, amount).map_err(|error| error.to_string())?;
        open.refresh();
        Ok(format!("{}: amount set to {amount}", slot.id))
    }

    /// Stage a repair of one damaged slot.
    pub fn repair(&mut self, inventory: usize, position: usize) -> Result<String, String> {
        let open = self.open.as_mut().ok_or_else(|| "no save is open".to_string())?;
        let path = open
            .inventories
            .get(inventory)
            .ok_or_else(|| "no such inventory".to_string())?
            .path
            .clone();
        let repaired = open.bundle.repair(&path, position).map_err(|error| error.to_string())?;
        open.refresh();
        Ok(if repaired { "slot repaired".to_string() } else { "that slot was not damaged".to_string() })
    }

    /// Stage a repair of every damaged slot in the save.
    pub fn repair_all(&mut self) -> Result<String, String> {
        let open = self.open.as_mut().ok_or_else(|| "no save is open".to_string())?;
        let repaired = open.bundle.repair_all().map_err(|error| error.to_string())?;
        open.refresh();
        Ok(format!("{repaired} slot(s) repaired"))
    }

    /// Stage the removal of one slot.
    pub fn remove_item(&mut self, inventory: usize, position: usize) -> Result<String, String> {
        let open = self.open.as_mut().ok_or_else(|| "no save is open".to_string())?;
        let path = open
            .inventories
            .get(inventory)
            .ok_or_else(|| "no such inventory".to_string())?
            .path
            .clone();
        let slot = open.bundle.remove_item(&path, position).map_err(|error| error.to_string())?;
        open.refresh();
        Ok(format!("removed {} x{}", slot.id, slot.amount))
    }

    /// Stage a move of one slot to another cell.
    pub fn move_item(&mut self, inventory: usize, position: usize, cell: Cell) -> Result<String, String> {
        let open = self.open.as_mut().ok_or_else(|| "no save is open".to_string())?;
        let path = open
            .inventories
            .get(inventory)
            .ok_or_else(|| "no such inventory".to_string())?
            .path
            .clone();
        let moved = open.bundle.move_item(&path, position, cell).map_err(|error| error.to_string())?;
        open.refresh();
        Ok(if moved { format!("moved to {cell}") } else { "that slot is already there".to_string() })
    }

    /// Stage a grid resize. Nothing is written until [`Session::save`].
    pub fn resize_inventory(
        &mut self,
        inventory: usize,
        width: u64,
        height: u64,
        beyond_vanilla: bool,
    ) -> Result<String, String> {
        let open = self.open.as_mut().ok_or_else(|| "no save is open".to_string())?;
        let current = open.inventories.get(inventory).ok_or_else(|| "no such inventory".to_string())?;
        let path = current.path.clone();
        let (before_width, before_height) = (current.width, current.height);
        let resized = open
            .bundle
            .resize_inventory(&path, width, height, beyond_vanilla)
            .map_err(|error| error.to_string())?;
        open.refresh();
        Ok(if resized {
            format!("grid {before_width}x{before_height} -> {width}x{height}")
        } else {
            format!("the grid is already {width}x{height}")
        })
    }

    /// Stage enabling every cell of an inventory's grid.
    pub fn unlock_all_slots(&mut self, inventory: usize) -> Result<String, String> {
        let open = self.open.as_mut().ok_or_else(|| "no save is open".to_string())?;
        let path = open
            .inventories
            .get(inventory)
            .ok_or_else(|| "no such inventory".to_string())?
            .path
            .clone();
        let added = open.bundle.unlock_all_slots(&path).map_err(|error| error.to_string())?;
        open.refresh();
        Ok(if added == 0 {
            "every slot in that grid is already enabled".to_string()
        } else {
            format!("enabled {added} slot(s)")
        })
    }

    /// Render an on-disk path for display, using readable names where known.
    pub fn path_label(&self, steps: &[RawStep]) -> String {
        describe_raw_path(steps, &self.keys)
    }

    /// Find every node whose on-disk key, readable name or scalar value
    /// contains `filter` (case-insensitive).
    ///
    /// Returns the hits, capped at [`JSON_HIT_LIMIT`], and whether the walk
    /// stopped early. The whole document is walked — measured at 88 ms for a
    /// 9 MB save in release — so this runs on a button or Enter rather than on
    /// every keystroke.
    pub fn search(&self, filter: &str) -> (Vec<JsonHit>, bool) {
        let needle = filter.trim().to_lowercase();
        let mut hits = Vec::new();
        let mut truncated = false;
        if needle.is_empty() {
            return (hits, truncated);
        }
        if let Some(open) = self.open.as_ref() {
            let mut steps = Vec::new();
            let root = open.bundle.document.tree();
            collect_hits(root, &mut steps, &self.keys, &needle, &mut hits, &mut truncated);
        }
        (hits, truncated)
    }

    /// Stage a replacement of an existing value, addressed by exact path.
    ///
    /// Nothing is created: a path that does not resolve is refused, so a typo
    /// cannot add a field the game does not expect.
    pub fn set_field(&mut self, steps: &[RawStep], value: Value) -> Result<String, String> {
        let label = self.path_label(steps);
        let open = self.open.as_mut().ok_or_else(|| "no save is open".to_string())?;
        let before = open
            .bundle
            .document
            .get_raw(steps)
            .ok_or_else(|| format!("{label} is no longer there"))?
            .clone();
        if before == value {
            return Ok(format!("{label} is unchanged"));
        }
        open.bundle.document.set_raw(steps, value.clone()).map_err(|error| error.to_string())?;
        open.refresh();
        Ok(format!("{label}: {} -> {}", compact(&before), compact(&value)))
    }

    /// Stage an addition to a known-item list.
    pub fn known_add(&mut self, list: KnownList, id: &str) -> Result<String, String> {
        let id = nms_edit::normalize_id(id.trim());
        let open = self.open.as_mut().ok_or_else(|| "no save is open".to_string())?;
        let changed = open.bundle.set_known(list, &id, true).map_err(|error| error.to_string())?;
        open.refresh();
        Ok(if changed {
            format!("added {id} to {}", list.label())
        } else {
            format!("{id} is already in {}", list.label())
        })
    }

    /// Stage a removal from a known-item list.
    pub fn known_remove(&mut self, list: KnownList, id: &str) -> Result<String, String> {
        let id = nms_edit::normalize_id(id.trim());
        let open = self.open.as_mut().ok_or_else(|| "no save is open".to_string())?;
        let changed = open.bundle.set_known(list, &id, false).map_err(|error| error.to_string())?;
        open.refresh();
        Ok(if changed {
            format!("removed {id} from {}", list.label())
        } else {
            format!("{id} is not in {}", list.label())
        })
    }

    /// Existing backups of the open save, newest first.
    pub fn backups(&self) -> Result<Vec<BackupSummary>, String> {
        let Some(open) = self.open.as_ref() else { return Ok(Vec::new()) };
        let Some(label) = open.entry.data_path.file_stem().map(|stem| stem.to_string_lossy().into_owned())
        else {
            return Ok(Vec::new());
        };
        let store = BackupStore::new(backup_root(&open.entry.data_path), BACKUP_KEEP);
        let entries = store.list(&label).map_err(|error| error.to_string())?;
        Ok(entries
            .into_iter()
            .map(|entry| {
                // Inspecting reads the metadata file inside each archive, which
                // is what tells two restore points apart; the save payload is
                // not decompressed.
                let summary = store.inspect(&entry.path).ok();
                BackupSummary {
                    modified: std::fs::metadata(&entry.path).and_then(|meta| meta.modified()).ok(),
                    kept: entry.kept,
                    state_time: summary.as_ref().and_then(|summary| summary.state_time()),
                    save_name: summary.as_ref().and_then(|summary| summary.save_name()),
                    play_time: summary.as_ref().and_then(|summary| summary.play_time()),
                    path: entry.path,
                    size: entry.size,
                }
            })
            .collect())
    }

    /// Pin an archive against pruning, or unpin it.
    pub fn pin_backup(&mut self, archive: &Path, pinned: bool) -> Result<String, String> {
        let store = BackupStore::new(backup_root(archive), BACKUP_KEEP);
        let moved = store.set_kept(archive, pinned).map_err(|error| error.to_string())?;
        let name = moved.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(if pinned { format!("pinned {name}") } else { format!("unpinned {name}") })
    }

    /// Delete one archive.
    pub fn delete_backup(&mut self, archive: &Path) -> Result<String, String> {
        let store = BackupStore::new(backup_root(archive), BACKUP_KEEP);
        store.delete(archive).map_err(|error| error.to_string())?;
        let name = archive.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(format!("deleted {name}"))
    }

    /// Restore a backup archive over the open save.
    ///
    /// The files being replaced are archived first (under the `pre-restore`
    /// label), so a restore is itself undoable; the open save is re-read
    /// afterwards.
    pub fn restore_backup(&mut self, archive: &Path) -> Result<String, String> {
        let (path, folder) = {
            let open = self.open.as_ref().ok_or_else(|| "no save is open".to_string())?;
            let path = open.entry.data_path.clone();
            let folder = path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
            (path, folder)
        };
        let store = BackupStore::new(backup_root(&path), BACKUP_KEEP);
        let restored = nms_edit::backup::restore_archive(archive, &folder, &store, now_unix())
            .map_err(|error| error.to_string())?;

        // The window was showing the files the restore just replaced.
        self.open = None;
        if let Some(index) = self.slots.iter().position(|entry| entry.data_path == path) {
            self.select(index);
        }
        self.rescan();
        let name = archive.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(format!("restored {} file(s) from {name}", restored.len()))
    }

    /// Re-read the open slot from disk, throwing away staged edits.
    pub fn reload(&mut self) -> Result<String, String> {
        let path = self
            .open
            .as_ref()
            .map(|open| open.entry.data_path.clone())
            .ok_or_else(|| "no save is open".to_string())?;
        let index = self
            .slots
            .iter()
            .position(|entry| entry.data_path == path)
            .ok_or_else(|| format!("{} is no longer in the folder", path.display()))?;
        self.open = None;
        self.error = None;
        self.select(index);
        match self.open {
            Some(_) => Ok("changes discarded".to_string()),
            None => Err(self.error.take().unwrap_or_else(|| "the save could not be re-read".to_string())),
        }
    }

    /// Write staged edits through the edit layer: backup, data file, metadata.
    ///
    /// The game's own folder is written like any other: the banner above the
    /// overview and the status bar are what say where the write landed, and
    /// every write takes a timestamped backup first.
    pub fn save(&mut self) -> Result<String, String> {
        let (path, backup_root) = {
            let open = self.open.as_ref().ok_or_else(|| "no save is open".to_string())?;
            let path = open.entry.data_path.clone();
            let root = backup_root(&path);
            (path, root)
        };

        let options = CommitOptions {
            backup_root: Some(backup_root),
            keep_backups: BACKUP_KEEP,
            timestamp: Some(now_unix()),
            dry_run: false,
        };
        let open = self.open.as_mut().ok_or_else(|| "no save is open".to_string())?;
        let report = open.bundle.commit(&options).map_err(|error| error.to_string())?;
        let mut message = match report.data {
            nms_format::WriteOutcome::Unchanged => "nothing to write".to_string(),
            nms_format::WriteOutcome::Written(bytes) => {
                format!("{} · wrote {bytes} bytes", report.changes.summary())
            }
        };
        if let Some(backup) = &report.backup {
            message.push_str(&format!(" · backup {}", backup.path.display()));
        }
        if !report.warnings.is_empty() {
            message.push_str(&format!(" · {} metadata warning(s)", report.warnings.len()));
        }

        // The commit rewrote the metadata file (size, version, timestamp), so
        // the sidebar summary is re-read from it; the document itself was
        // already re-parsed by the commit.
        if let Ok(entries) = scan_dir(&self.dir) {
            self.slots = entries;
            if let Some(entry) = self.slots.iter().find(|entry| entry.data_path == path).cloned() {
                if let Some(open) = self.open.as_mut() {
                    open.entry = entry;
                    open.refresh();
                }
            }
        }
        Ok(message)
    }
}

/// Walk the whole tree once, collecting matching nodes.
///
/// This runs over hundreds of thousands of nodes on a real save, so the checks
/// avoid allocating: paths and previews are only built for the few nodes that
/// match, and the case-insensitive compare borrows both sides.
fn collect_hits(
    value: &Value,
    steps: &mut Vec<RawStep>,
    keys: &KeyMap,
    needle: &str,
    hits: &mut Vec<JsonHit>,
    truncated: &mut bool,
) {
    if hits.len() >= JSON_HIT_LIMIT {
        *truncated = true;
        return;
    }
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                steps.push(RawStep::Key(key.clone()));
                // A field matches by its on-disk key, its readable name, or a
                // scalar value. Descending through it keeps working, so the
                // fields *under* a match are not reported as matches too.
                let readable = keys.readable_for(key).unwrap_or(key);
                let named = contains_ci(key, needle) || contains_ci(readable, needle);
                if named || scalar_matches(child, needle) {
                    hits.push(hit(steps, keys, child));
                }
                collect_hits(child, steps, keys, needle, hits, truncated);
                steps.pop();
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                steps.push(RawStep::Index(index));
                // An index is not worth matching ("3" would find every third
                // item); only the value is.
                if scalar_matches(child, needle) {
                    hits.push(hit(steps, keys, child));
                }
                collect_hits(child, steps, keys, needle, hits, truncated);
                steps.pop();
            }
        }
        _ => {}
    }
}

/// Whether a scalar's text contains the needle. Containers never match by
/// value; their children are visited on their own.
fn scalar_matches(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(text) => contains_ci(text, needle),
        Value::Bool(true) => contains_ci("true", needle),
        Value::Bool(false) => contains_ci("false", needle),
        Value::Null => contains_ci("null", needle),
        Value::Number(number) => contains_ci(number.as_str(), needle),
        Value::Array(_) | Value::Object(_) => false,
    }
}

fn hit(steps: &[RawStep], keys: &KeyMap, value: &Value) -> JsonHit {
    JsonHit { steps: steps.to_vec(), label: describe_raw_path(steps, keys), preview: json_summary(value) }
}

/// Case-insensitive `contains`, ASCII rules, with the needle already lowered.
///
/// Save keys are ASCII; a non-ASCII needle still matches exact bytes, which is
/// what a name search needs anyway.
pub(crate) fn contains_ci(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    needle.len() <= haystack.len()
        && haystack.windows(needle.len()).any(|window| window.eq_ignore_ascii_case(needle))
}

/// A one-line rendering of a value for the tree and search hits.
pub fn json_summary(value: &Value) -> String {
    match value {
        Value::String(text) => shorten(text, 80),
        Value::Null => "null".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::Array(items) => format!("[{} items]", items.len()),
        Value::Object(map) => format!("{{{} fields}}", map.len()),
    }
}

/// Cut a long string for display, keeping the front.
fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}…")
}

/// List the save slots in a folder, ordered by slot number.
pub fn scan_dir(dir: &Path) -> Result<Vec<SlotEntry>, String> {
    // `nms_edit::scan` treats a folder it cannot read as a folder with no
    // saves; the window says so instead of showing an empty list.
    if !dir.is_dir() {
        return Err(format!("cannot read {}: no such folder", dir.display()));
    }
    // The edit layer does the scanning: it knows how the two save points of a
    // game slot relate, and the window must not disagree with it about which
    // one the game will load.
    let found = nms_edit::scan(dir);
    Ok(found
        .into_iter()
        .map(|save| {
            let summary = save.metadata.as_ref().map(|metadata| SlotSummary {
                save_name: metadata.save_name(),
                location: metadata.location(),
                game_mode: metadata.game_mode(),
                play_time: metadata.play_time,
                version: metadata.version,
                modified: save
                    .modified_unix
                    .map(|seconds| std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds)),
            });
            let problem = summary.is_none().then(|| {
                format!("{}: metadata could not be read", save.slot.metadata_file_name())
            });
            SlotEntry {
                slot: save.slot,
                game_slot: save.game_slot,
                save_point: save.save_point,
                newest: save.newest,
                state_time: save.state_time,
                paired_path: save.paired_path,
                paired_time: save.paired_time,
                data_path: save.data_path,
                summary,
                problem,
            }
        })
        .collect())
}

/// The key map the CLI would load (`--mapping` has no GUI equivalent yet, so
/// the environment and config locations are used).
fn default_key_map() -> Result<KeyMap, String> {
    let path = std::env::var("NMSEDIT_MAPPING")
        .ok()
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(default_mapping_path);
    match path {
        Some(path) if path.is_file() => {
            let loaded = KeyMap::load(&path).map_err(|e| format!("mapping {}: {e}", path.display()))?;
            Ok(KeyMap::builtin().extended_with(&loaded))
        }
        _ => Err("no full key mapping found; using the built-in baseline \
                  (run `nmsedit data refresh` to derive the full mapping from your game, or set NMSEDIT_MAPPING)"
            .to_string()),
    }
}

fn default_mapping_path() -> Option<PathBuf> {
    let base = match std::env::var("XDG_CONFIG_HOME") {
        Ok(value) if !value.is_empty() => PathBuf::from(value),
        _ => PathBuf::from(std::env::var("HOME").ok()?).join(".config"),
    };
    Some(base.join("nmsedit").join("mapping.json"))
}

/// The data pack the CLI would use, when one has been generated.
fn default_pack() -> Result<Option<DataPack>, String> {
    let path = std::env::var("NMSEDIT_PACK")
        .ok()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(DataPack::default_path);
    let Some(path) = path else { return Ok(None) };
    if !path.is_file() {
        return Err(format!(
            "no data pack at {}; run `nmsedit data refresh` to name items",
            path.display()
        ));
    }
    DataPack::load(&path).map(Some).map_err(|e| format!("data pack {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    const SAVE: &str = r#"{
        "Version": 4743,
        "Platform": "Win|Final",
        "ActiveContext": "Main",
        "BaseContext": {
            "PlayerStateData": {
                "Units": 1200,
                "Nanites": 40,
                "Specials": 7,
                "Inventory": {
                    "Slots": [
                        {"Type":{"InventoryType":"Substance"},"Id":"^CARBON","Amount":10,"MaxAmount":20,"DamageFactor":0.0,"FullyInstalled":true,"AddedAutomatically":false,"Index":{"X":0,"Y":0}}
                    ],
                    "ValidSlotIndices": [{"X":0,"Y":0},{"X":1,"Y":0}],
                    "Class": {"InventoryClass": "C"},
                    "StackSizeGroup": {"InventoryStackSizeGroup": "Personal"},
                    "Width": 2,
                    "Height": 1
                },
                "KnownProducts": ["^A", "^B"],
                "KnownTech": [],
                "KnownSpecials": [],
                "SeenBaseBuildingObjects": [],
                "KnownRefinerRecipes": []
            }
        }
    }"#;

    static NEXT: AtomicU32 = AtomicU32::new(0);

    /// A scratch directory removed when the test's guard is dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let unique = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("nmsedit-gui-{}-{}-{unique}", std::process::id(), name));
            std::fs::create_dir_all(&path).expect("create scratch directory");
            Scratch(path)
        }

        fn write(&self, name: &str, contents: &[u8]) {
            std::fs::write(self.0.join(name), contents).expect("write scratch file");
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn session(dir: &Path) -> Session {
        Session::open_dir_with(dir, KeyMap::builtin(), None, Vec::new())
    }

    #[test]
    fn a_folder_listing_finds_saves_and_ignores_everything_else() {
        let scratch = Scratch::new("listing");
        scratch.write("save2.hg", SAVE.as_bytes());
        scratch.write("save10.hg", SAVE.as_bytes());
        scratch.write("save.hg", SAVE.as_bytes());
        scratch.write("accountdata.hg", SAVE.as_bytes());
        scratch.write("mf_save2.hg", b"not metadata");
        scratch.write("notes.txt", b"hello");

        let session = session(&scratch.0);
        assert_eq!(session.error, None);
        let names: Vec<String> =
            session
                .slots
                .iter()
                .map(|entry| entry.data_path.file_name().unwrap().to_string_lossy().into_owned())
                .collect();
        assert_eq!(names, ["save.hg", "save2.hg", "save10.hg"], "player saves are ordered by slot number");

        // The metadata file is not real, so the summary is missing and the
        // problem is kept for display rather than hidden.
        assert!(session.slots[0].summary.is_none());
        assert!(session.slots[0].problem.is_some());
        assert_eq!(session.slots[0].title(), "PlayerState1");
        assert_eq!(session.slots[0].subtitle(), "metadata could not be read");
    }

    #[test]
    fn opening_a_slot_reads_the_player_state() {
        let scratch = Scratch::new("open");
        scratch.write("save2.hg", SAVE.as_bytes());

        let mut session = session(&scratch.0);
        session.select(0);
        let open = session.open.as_ref().expect("the save opens");

        let currencies = open.currencies.expect("currencies");
        assert_eq!(currencies.units, 1200);
        assert_eq!(currencies.nanites, 40);
        assert_eq!(currencies.quicksilver, 7);

        assert_eq!(open.info.version, Some(4743));
        assert_eq!(open.info.platform.as_deref(), Some("Win|Final"));
        assert_eq!(open.inventories.len(), 1);
        assert_eq!(open.inventories[0].slots.len(), 1);
        assert_eq!(open.inventories[0].slots[0].id, "^CARBON");

        let known = open.known.clone();
        let ids = |list: KnownList| -> Option<usize> {
            known.iter().find(|entry| entry.list == list).map(|entry| entry.ids.len())
        };
        assert_eq!(ids(KnownList::Products), Some(2));
        assert_eq!(ids(KnownList::Technology), Some(0));
    }

    #[test]
    fn selecting_the_open_slot_again_keeps_the_loaded_save() {
        let scratch = Scratch::new("reselect");
        scratch.write("save2.hg", SAVE.as_bytes());
        let mut session = session(&scratch.0);
        session.select(0);
        session.select(0);
        assert!(session.open.is_some());
    }

    #[test]
    fn a_rescan_keeps_the_open_save_while_its_file_exists() {
        let scratch = Scratch::new("rescan");
        scratch.write("save2.hg", SAVE.as_bytes());
        let mut session = session(&scratch.0);
        session.select(0);

        session.rescan();
        assert_eq!(session.slots.len(), 1);
        assert!(session.open.is_some(), "a plain refresh must not close the save");

        std::fs::remove_file(scratch.0.join("save2.hg")).expect("remove the save");
        session.rescan();
        assert!(session.open.is_none(), "a save that disappeared is closed");
    }

    #[test]
    fn rescanning_before_a_folder_is_chosen_reports_nothing() {
        let mut session = Session::empty();
        session.rescan();
        assert_eq!(session.error, None);
        assert!(session.slots.is_empty());
    }

    #[test]
    fn a_missing_folder_is_reported_and_lists_nothing() {
        let missing = std::env::temp_dir().join("nmsedit-gui-does-not-exist");
        let session = Session::open_dir_with(&missing, KeyMap::builtin(), None, Vec::new());
        assert!(session.error.is_some());
        assert!(session.slots.is_empty());
    }

    #[test]
    fn durations_are_hours_minutes_seconds() {
        assert_eq!(format_duration(0), "0:00:00");
        assert_eq!(format_duration(3661), "1:01:01");
        assert_eq!(format_duration(3_156_876), "876:54:36");
    }

    #[test]
    fn a_staged_balance_is_only_written_by_save() {
        let scratch = Scratch::new("stage");
        scratch.write("save.hg", SAVE.as_bytes());
        let mut session = session(&scratch.0);
        session.select(0);

        session.set_currency(Currency::Units, 4321).expect("staging a balance");
        assert!(session.dirty(), "a staged change marks the save");
        let on_disk = SaveBundle::load(&scratch.0.join("save.hg"), KeyMap::builtin()).expect("reload");
        assert_eq!(on_disk.currencies().expect("currencies").units, 1200, "nothing on disk yet");

        let report = session.save().expect("save the staged change");
        assert!(report.contains("wrote"), "{report}");
        assert!(!session.dirty(), "saving clears the staged state");
        let on_disk = SaveBundle::load(&scratch.0.join("save.hg"), KeyMap::builtin()).expect("reload");
        assert_eq!(on_disk.currencies().expect("currencies").units, 4321);
        assert!(scratch.0.join("Save Backups").is_dir(), "a backup is written first");
    }

    #[test]
    fn a_refused_edit_leaves_the_save_clean() {
        let scratch = Scratch::new("refused");
        scratch.write("save.hg", SAVE.as_bytes());
        let mut session = session(&scratch.0);
        session.select(0);

        let error = session.set_amount(0, 0, 999).expect_err("999 does not fit a stack of 20");
        assert!(error.contains("999"), "{error}");
        assert!(!session.dirty(), "a refused edit must not dirty the document");
        assert!(session.save().expect("save with nothing staged").contains("nothing to write"));
    }

    #[test]
    fn a_live_folder_is_editable_and_reported() {
        let scratch = Scratch::new("live");
        let live = scratch
            .0
            .join("steamapps/compatdata/275850/pfx/drive_c/users/steamuser/AppData/Roaming/HelloGames/NMS/st_1");
        std::fs::create_dir_all(&live).expect("create the live-looking folder");
        std::fs::write(live.join("save.hg"), SAVE.as_bytes()).expect("write the save");

        let mut session = session(&live);
        session.select(0);
        // The window still says where it is writing: the banner and the status
        // bar both hang off this.
        let live_kind = session.live_location().expect("the path classifies as a live save folder");
        assert!(live_kind.describe().contains("Steam"), "{}", live_kind.describe());

        session.set_currency(Currency::Units, 5).expect("stage");
        let report = session.save().expect("a live folder is written like any other");
        assert!(report.contains("wrote"), "{report}");
        assert!(report.contains("backup"), "the write is backed up: {report}");

        let on_disk = SaveBundle::load(&live.join("save.hg"), KeyMap::builtin()).expect("reload");
        assert_eq!(on_disk.currencies().expect("currencies").units, 5);
    }

    #[test]
    fn discarding_rereads_the_file() {
        let scratch = Scratch::new("discard");
        scratch.write("save.hg", SAVE.as_bytes());
        let mut session = session(&scratch.0);
        session.select(0);

        session.add_item(0, "^IRON", 5, None, None).expect("stage a new item");
        assert!(session.dirty());
        assert_eq!(session.open.as_ref().expect("open").inventories[0].slots.len(), 2);

        session.reload().expect("discard");
        assert!(!session.dirty());
        assert_eq!(
            session.open.as_ref().expect("open").inventories[0].slots.len(),
            1,
            "the staged item is gone"
        );
        assert_eq!(session.open.as_ref().expect("open").currencies.expect("currencies").units, 1200);
    }

    #[test]
    fn an_added_item_saves_and_reloads() {
        let scratch = Scratch::new("add");
        scratch.write("save.hg", SAVE.as_bytes());
        let mut session = session(&scratch.0);
        session.select(0);

        session.add_item(0, "^IRON", 5, None, None).expect("stage a new item");
        session.save().expect("save the new item");

        // Re-open from disk: the item is there and the stack size came from the
        // save's own `^CARBON` slot rather than a guessed table.
        session.reload().expect("reload");
        let slots = &session.open.as_ref().expect("open").inventories[0].slots;
        assert_eq!(slots.len(), 2);
        let iron = slots.iter().find(|slot| slot.id == "^IRON").expect("the new slot");
        assert_eq!(iron.amount, 5);
        assert_eq!(iron.max_amount, 20);
    }

    #[test]
    fn a_json_field_edit_stages_and_saves() {
        let scratch = Scratch::new("json");
        scratch.write("save.hg", SAVE.as_bytes());
        let mut session = session(&scratch.0);
        session.select(0);

        let steps = [
            RawStep::Key("BaseContext".to_string()),
            RawStep::Key("PlayerStateData".to_string()),
            RawStep::Key("Units".to_string()),
        ];
        let open = session.open.as_ref().expect("open");
        assert_eq!(open.value_at(&steps).and_then(Value::as_i64), Some(1200));
        assert_eq!(session.path_label(&steps), "BaseContext.PlayerStateData.Units");

        let report = session.set_field(&steps, Value::from(4321)).expect("stage the edit");
        assert!(report.contains("1200 -> 4321"), "{report}");
        assert!(session.dirty());
        // The derived views follow the document, not a stale copy.
        assert_eq!(session.open.as_ref().expect("open").currencies.expect("currencies").units, 4321);

        session.save().expect("save the edit");
        session.reload().expect("reload");
        let open = session.open.as_ref().expect("open");
        assert_eq!(open.value_at(&steps).and_then(Value::as_i64), Some(4321));
    }

    #[test]
    fn backups_are_listed_and_restore_round_trips() {
        let scratch = Scratch::new("backup");
        scratch.write("save.hg", SAVE.as_bytes());
        let mut session = session(&scratch.0);
        session.select(0);
        assert!(session.backups().expect("list backups").is_empty(), "nothing saved yet");

        session.set_currency(Currency::Units, 111).expect("stage");
        session.save().expect("first save");
        let after_first = session.backups().expect("list backups");
        assert_eq!(after_first.len(), 1);
        assert!(after_first[0].size > 0);

        // A second save in the same second must not overwrite the first
        // restore point; the oldest archive holds the state before the first
        // edit.
        session.set_currency(Currency::Units, 222).expect("stage");
        session.save().expect("second save");
        let all = session.backups().expect("list backups");
        assert_eq!(all.len(), 2, "each save keeps its own archive: {all:?}");
        let oldest = all.last().expect("the older archive").path.clone();

        let report = session.restore_backup(&oldest).expect("restore");
        assert!(report.contains("restored 1 file(s)"), "{report}");
        assert!(!session.dirty(), "a restored save is clean");
        assert_eq!(session.open.as_ref().expect("open").currencies.expect("currencies").units, 1200);
    }

    #[test]
    fn known_list_edits_stage_and_save() {
        let scratch = Scratch::new("known");
        scratch.write("save.hg", SAVE.as_bytes());
        let mut session = session(&scratch.0);
        session.select(0);

        let products = |session: &Session| -> Vec<String> {
            session
                .open
                .as_ref()
                .expect("open")
                .known
                .iter()
                .find(|entry| entry.list == KnownList::Products)
                .map(|entry| entry.ids.clone())
                .expect("the products list")
        };
        assert_eq!(products(&session), ["^A", "^B"], "ids are sorted for display");

        let report = session.known_add(KnownList::Products, "carbon").expect("stage an addition");
        assert!(report.contains("added ^CARBON"), "{report}");
        assert!(session.dirty());
        assert_eq!(products(&session), ["^A", "^B", "^CARBON"]);

        // Adding again is a no-op, case-insensitively, like the edit layer.
        let report = session.known_add(KnownList::Products, "^carbon").expect("no-op");
        assert!(report.contains("already"), "{report}");

        let report = session.known_remove(KnownList::Products, "^A").expect("stage a removal");
        assert!(report.contains("removed ^A"), "{report}");
        assert_eq!(products(&session), ["^B", "^CARBON"]);

        session.save().expect("save the edits");
        session.reload().expect("reload");
        assert_eq!(products(&session), ["^B", "^CARBON"]);
        assert!(!session.dirty());
    }

    #[test]
    fn a_refused_json_edit_leaves_the_save_clean() {
        let scratch = Scratch::new("json-refused");
        scratch.write("save.hg", SAVE.as_bytes());
        let mut session = session(&scratch.0);
        session.select(0);

        let missing = [RawStep::Key("BaseContext".to_string()), RawStep::Key("Nope".to_string())];
        let error = session.set_field(&missing, Value::from(1)).expect_err("no such field");
        assert!(error.contains("no longer there"), "{error}");
        assert!(!session.dirty(), "a refused JSON edit must not dirty the document");
    }

    // -----------------------------------------------------------------------
    // fixture test: real saves, real metadata
    // -----------------------------------------------------------------------

    /// The repo's fixture folder, when real saves have been installed.
    fn fixture_dir() -> Option<PathBuf> {
        if let Ok(path) = std::env::var("NMS_FIXTURES") {
            let path = PathBuf::from(path);
            return path.is_dir().then_some(path);
        }
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?;
        let path = root.join("fixtures");
        path.is_dir().then_some(path)
    }

    /// The first account folder that holds a player save.
    fn fixture_account_dir() -> Option<PathBuf> {
        let root = fixture_dir()?;
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            if dir.join("save.hg").is_file() {
                return Some(dir);
            }
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                        stack.push(entry.path());
                    }
                }
            }
        }
        None
    }

    /// The community mapping, when one is installed.
    ///
    /// Inventory fields are not in the built-in map, so the fixture test cannot
    /// run without it; like the fixtures themselves, it skips instead.
    fn fixture_keys() -> Option<KeyMap> {
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
        KeyMap::load(&path).ok().map(|loaded| KeyMap::builtin().extended_with(&loaded))
    }

    #[test]
    fn a_real_save_can_be_searched() {
        let Some(account) = fixture_account_dir() else {
            eprintln!("skipping: no fixtures (run tools/fetch-fixtures.sh)");
            return;
        };
        let Some(keys) = fixture_keys() else {
            eprintln!("skipping: no community mapping");
            return;
        };
        let slot = (Slot::FIRST_PLAYER..=Slot::LAST_PLAYER)
            .filter_map(Slot::player_state_checked)
            .find(|slot| {
                account.join(slot.data_file_name()).is_file()
                    && account.join(slot.metadata_file_name()).is_file()
            })
            .expect("an account folder with a save has one complete player slot");

        let scratch = Scratch::new("fixture-search");
        std::fs::copy(account.join(slot.data_file_name()), scratch.0.join(slot.data_file_name()))
            .expect("copy the save");
        let mut session = Session::open_dir_with(&scratch.0, keys, None, Vec::new());
        session.select(0);

        // The player's balance lives under its readable name in a real save,
        // and the walk has to find it through the key map.
        let (hits, truncated) = session.search("Units");
        assert!(!truncated, "a single name should not hit the cap");
        let open = session.open.as_ref().expect("open");
        let units = open.currencies.expect("currencies").units;
        // Another context could hold a `Units` field too, so pick the one that
        // is the balance the player-state reader returned.
        let hit = hits
            .iter()
            .find(|hit| {
                hit.label.ends_with(".Units")
                    && open.value_at(&hit.steps).and_then(Value::as_i64).map(|stored| stored as u32)
                        == Some(units)
            })
            .unwrap_or_else(|| panic!("no Units hit matching {units} among {} hits", hits.len()));
        assert!(!hit.steps.is_empty());

        // Searching is case-insensitive and can match a value too.
        let (hits, _) = session.search("wIn|FiNaL");
        assert!(hits.iter().any(|hit| hit.preview.contains("Win|Final")), "value search finds the platform");
    }

    #[test]
    fn a_real_save_stages_and_saves() {
        let Some(account) = fixture_account_dir() else {
            eprintln!("skipping: no fixtures (run tools/fetch-fixtures.sh)");
            return;
        };
        let Some(keys) = fixture_keys() else {
            eprintln!(
                "skipping: no community mapping (set NMSEDIT_MAPPING, or run `nmsedit data refresh`)"
            );
            return;
        };
        let slot = (Slot::FIRST_PLAYER..=Slot::LAST_PLAYER)
            .filter_map(Slot::player_state_checked)
            .find(|slot| {
                account.join(slot.data_file_name()).is_file()
                    && account.join(slot.metadata_file_name()).is_file()
            })
            .expect("an account folder with a save has one complete player slot");

        // Never touch the fixture: copy the pair into a scratch folder first.
        let scratch = Scratch::new("fixture");
        std::fs::copy(account.join(slot.data_file_name()), scratch.0.join(slot.data_file_name()))
            .expect("copy the save");
        std::fs::copy(account.join(slot.metadata_file_name()), scratch.0.join(slot.metadata_file_name()))
            .expect("copy the metadata");

        let mut session = Session::open_dir_with(&scratch.0, keys, None, Vec::new());
        assert_eq!(session.error, None);
        let index = session.slots.iter().position(|entry| entry.slot == slot).expect("the slot is listed");
        assert!(session.slots[index].summary.is_some(), "the real metadata decrypts");

        session.select(index);
        let open = session.open.as_ref().expect("the save opens");
        assert!(!open.inventories.is_empty(), "a player save carries inventories");
        assert!(!open.dirty());
        let before = open.currencies.expect("currencies").units;

        // Move units by one; any value is legal as long as it differs.
        let target = if before == 0 { 1 } else { before - 1 };
        session.set_currency(Currency::Units, target).expect("stage the balance");

        // A known-item list round-trips too, on a real save with real lists.
        let list = KnownList::Products;
        let id = nms_edit::normalize_id("^NMSEDIT_TEST");
        let products = |session: &Session| -> Vec<String> {
            session
                .open
                .as_ref()
                .expect("open")
                .known
                .iter()
                .find(|entry| entry.list == list)
                .map(|entry| entry.ids.clone())
                .expect("the products list")
        };
        assert!(!products(&session).contains(&id), "{id} must not already be known");
        session.known_add(list, &id).expect("stage a known id");
        assert!(session.dirty());
        let report = session.save().expect("save the staged balance");
        assert!(report.contains("wrote"), "{report}");
        assert!(report.contains("metadata"), "a real save has metadata to refresh: {report}");
        assert!(!session.dirty());

        session.reload().expect("reload from disk");
        let open = session.open.as_ref().expect("the save re-opens");
        assert_eq!(open.currencies.expect("currencies").units, target);
        assert!(open.entry.summary.is_some(), "the refreshed metadata still decrypts");
        assert!(!open.inventories.is_empty(), "the inventories still load after a save");
        assert!(products(&session).contains(&id), "the known id survived the round trip");

        // The backup the save wrote knows which state it holds: its metadata
        // file is inside the archive, and inspecting reads it without
        // decompressing the save payload.
        let backups = session.backups().expect("list backups");
        let backup = backups.first().expect("the save wrote an archive");
        assert!(backup.state_time.is_some(), "the archive knows when the state was written");
        assert!(backup.play_time.is_some(), "the archive knows the play time");
        // A slot can have no chosen name; the accessor says so rather than
        // returning an empty string.
        assert!(backup.save_name.as_deref().is_none_or(|name| !name.is_empty()));
        assert!(!backup.kept);
    }
}
