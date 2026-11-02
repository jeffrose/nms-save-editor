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
    live_save_location, now_unix, Cell, CommitOptions, Currencies, Currency, Inventory, ItemKind, KnownList,
    LiveLocation, NewItem, SaveBundle,
};
use nms_format::metadata::{Metadata, Slot};
use nms_model::KeyMap;

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

/// One storage slot found in a save folder.
#[derive(Debug, Clone)]
pub struct SlotEntry {
    /// The storage slot the file name identifies.
    pub slot: Slot,
    /// Path of the data file (`save*.hg`).
    pub data_path: PathBuf,
    /// Decrypted summary, when the metadata file could be read.
    pub summary: Option<SlotSummary>,
    /// Why the metadata could not be read.
    pub problem: Option<String>,
}

impl SlotEntry {
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
    /// Every inventory the player state carries, in file order.
    pub inventories: Vec<Inventory>,
    /// How many ids each known-item list holds, when the list exists.
    pub known: Vec<(KnownList, usize)>,
}

impl OpenSave {
    /// Whether the in-memory save differs from the file on disk.
    pub fn dirty(&self) -> bool {
        self.bundle.document.is_dirty()
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
        self.inventories = self.bundle.inventories();
        self.known = KnownList::ALL
            .into_iter()
            .filter_map(|list| self.bundle.known(list).ok().map(|ids| (list, ids.len())))
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
                    .filter_map(|list| bundle.known(list).ok().map(|ids| (list, ids.len())))
                    .collect();
                Some(OpenSave {
                    entry,
                    info,
                    bundle,
                    currencies,
                    currencies_error,
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
    /// A live save folder is refused outright. The CLI can be told to override
    /// that, but the window deliberately has no such switch yet: the safe path
    /// is to copy the folder and open the copy.
    pub fn save(&mut self) -> Result<String, String> {
        let (path, backup_root) = {
            let open = self.open.as_ref().ok_or_else(|| "no save is open".to_string())?;
            let path = open.entry.data_path.clone();
            if let Some(live) = live_save_location(&path) {
                return Err(format!(
                    "refusing to write: {} is {}. Copy the folder and open the copy.",
                    path.display(),
                    live.describe()
                ));
            }
            let backup_root = path.parent().unwrap_or_else(|| Path::new(".")).join("Save Backups");
            (path, backup_root)
        };

        let options = CommitOptions {
            backup_root: Some(backup_root),
            keep_backups: 10,
            timestamp: Some(now_unix()),
            dry_run: false,
            confirm_live: false,
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

/// List the save slots in a folder, ordered by slot number.
pub fn scan_dir(dir: &Path) -> Result<Vec<SlotEntry>, String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    let mut slots = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(slot) = Slot::from_file_name(&name) else { continue };
        if slot.player_number().is_none() {
            // `accountdata.hg` and `usersettings.hg` are settings, not saves.
            continue;
        }
        let metadata_path = path.with_file_name(slot.metadata_file_name());
        let modified = std::fs::metadata(&path).and_then(|meta| meta.modified()).ok();
        let (summary, problem) = match read_summary(&metadata_path, slot) {
            Ok(mut summary) => {
                summary.modified = modified;
                (Some(summary), None)
            }
            Err(message) => (None, Some(message)),
        };
        slots.push(SlotEntry { slot, data_path: path, summary, problem });
    }
    slots.sort_by_key(|entry| entry.slot.index());
    Ok(slots)
}

fn read_summary(path: &Path, slot: Slot) -> Result<SlotSummary, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let metadata = Metadata::read(&bytes, slot).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(SlotSummary {
        save_name: metadata.save_name(),
        location: metadata.location(),
        game_mode: metadata.game_mode(),
        play_time: metadata.play_time,
        version: metadata.version,
        modified: None,
    })
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
        _ => Err("no community key mapping found; field and inventory names will be limited \
                  (set NMSEDIT_MAPPING or install ~/.config/nmsedit/mapping.json)"
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

        let known: Vec<(KnownList, usize)> = open.known.clone();
        assert!(known.contains(&(KnownList::Products, 2)));
        assert!(known.contains(&(KnownList::Technology, 0)));
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
    fn a_live_folder_is_refused_by_the_window() {
        let scratch = Scratch::new("live");
        let live = scratch
            .0
            .join("steamapps/compatdata/275850/pfx/drive_c/users/steamuser/AppData/Roaming/HelloGames/NMS/st_1");
        std::fs::create_dir_all(&live).expect("create the live-looking folder");
        std::fs::write(live.join("save.hg"), SAVE.as_bytes()).expect("write the save");

        let mut session = session(&live);
        session.select(0);
        assert!(session.live_location().is_some(), "the path classifies as a live save folder");

        session.set_currency(Currency::Units, 5).expect("staging stays in memory");
        assert!(session.dirty());
        let error = session.save().expect_err("the window refuses to write a live folder");
        assert!(error.contains("refusing to write"), "{error}");

        // The file on disk still holds the original balance.
        let on_disk = SaveBundle::load(&live.join("save.hg"), KeyMap::builtin()).expect("reload");
        assert_eq!(on_disk.currencies().expect("currencies").units, 1200);
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
    fn a_real_save_stages_and_saves() {
        let Some(account) = fixture_account_dir() else {
            eprintln!("skipping: no fixtures (run tools/fetch-fixtures.sh)");
            return;
        };
        let Some(keys) = fixture_keys() else {
            eprintln!(
                "skipping: no community mapping (set NMSEDIT_MAPPING or install ~/.config/nmsedit/mapping.json)"
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
    }
}
