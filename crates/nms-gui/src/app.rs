//! The window: a slot browser, a save summary, and editable inventory grids.
//!
//! Editing is a staging model: every action changes only the in-memory
//! document, so the file on disk is untouched until **Save** is pressed. A
//! refused action (a full grid, an occupied cell) leaves the document clean,
//! because the edit layer validates against an immutable view first. Save then
//! goes through [`crate::session::Session::save`], which writes a timestamped
//! backup and patches the metadata exactly like the CLI does.
//!
//! Writes into the game's own folder are allowed — the owner asked for that on
//! 2026-11-02, once every write was backed up and verified — but the window says
//! so out loud, because Steam Cloud syncs that folder to every machine signed
//! into the account.

use std::path::{Path, PathBuf};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, RichText, ScrollArea, Sense, Stroke, StrokeKind, Vec2,
};
use nms_data::DataPack;
use nms_edit::{Cell, Currency, Inventory, InventoryRole, ItemKind, ItemSlot, KnownList};
use nms_model::{describe_raw_path, KeyMap, RawStep};
use serde_json::Value;

use crate::session::{
    contains_ci, format_duration, json_summary, BackupSummary, JsonHit, OpenSave, Session, JSON_HIT_LIMIT,
};

/// Which part of a save the central panel is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Inventories,
    Known,
    Json,
}

/// The item kind a new stack should be written as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KindChoice {
    /// Let the edit layer infer it from the save.
    Auto,
    Substance,
    Product,
    Technology,
}

impl KindChoice {
    const ALL: [KindChoice; 4] =
        [KindChoice::Auto, KindChoice::Substance, KindChoice::Product, KindChoice::Technology];

    fn label(self) -> &'static str {
        match self {
            KindChoice::Auto => "from the save",
            KindChoice::Substance => "substance",
            KindChoice::Product => "product",
            KindChoice::Technology => "technology",
        }
    }

    fn item_kind(self) -> Option<ItemKind> {
        match self {
            KindChoice::Auto => None,
            KindChoice::Substance => Some(ItemKind::Substance),
            KindChoice::Product => Some(ItemKind::Product),
            KindChoice::Technology => Some(ItemKind::Technology),
        }
    }
}

/// What the controls hold, as opposed to what the save holds.
struct EditState {
    selected_inventory: usize,
    selected_slot: Option<usize>,
    /// One text box per [`Currency::ALL`] entry, so a balance can be typed.
    currency_inputs: [String; 3],
    amount_input: String,
    add_id: String,
    add_amount: String,
    add_kind: KindChoice,
    add_max: String,
    /// Slot being moved; the next empty-cell click is the destination.
    moving: Option<usize>,
}

impl Default for EditState {
    fn default() -> Self {
        EditState {
            selected_inventory: 0,
            selected_slot: None,
            currency_inputs: Default::default(),
            amount_input: String::new(),
            add_id: String::new(),
            add_amount: String::new(),
            add_kind: KindChoice::Auto,
            add_max: String::new(),
            moving: None,
        }
    }
}

impl EditState {
    /// Fill every control from the loaded save.
    fn sync(&mut self, open: &OpenSave) {
        if self.selected_inventory >= open.inventories.len() {
            self.selected_inventory = 0;
        }
        if let Some(currencies) = open.currencies {
            for (index, currency) in Currency::ALL.iter().enumerate() {
                self.currency_inputs[index] = currencies.get(*currency).to_string();
            }
        }
        let still_there = self.selected_slot.is_some_and(|position| {
            open.inventories
                .get(self.selected_inventory)
                .is_some_and(|inventory| inventory.slots.iter().any(|slot| slot.position == position))
        });
        if !still_there {
            self.selected_slot = None;
        }
        if self.moving.is_some_and(|position| self.selected_slot != Some(position)) {
            self.moving = None;
        }
        self.sync_amount(open);
    }

    /// Put the selected slot's amount into the editor text box.
    fn sync_amount(&mut self, open: &OpenSave) {
        let slot = self.selected_slot.and_then(|position| {
            let inventory = open.inventories.get(self.selected_inventory)?;
            inventory.slots.iter().find(|slot| slot.position == position)
        });
        self.amount_input = slot.map(|slot| slot.amount.to_string()).unwrap_or_default();
    }

    fn select_slot(&mut self, open: &OpenSave, position: usize) {
        self.selected_slot = Some(position);
        self.moving = None;
        self.sync_amount(open);
    }

    fn clear_slot(&mut self) {
        self.selected_slot = None;
        self.amount_input.clear();
        self.moving = None;
    }
}

/// The known-items tab's controls.
struct KnownState {
    /// Which of the five lists is shown.
    list: KnownList,
    /// Filter over id and item name.
    filter: String,
    /// Id of the row the remove button applies to.
    selected: Option<String>,
    /// Text of the add box.
    add_id: String,
}

impl Default for KnownState {
    fn default() -> Self {
        KnownState { list: KnownList::Products, filter: String::new(), selected: None, add_id: String::new() }
    }
}

impl KnownState {
    /// Drop a selection that the document no longer holds, and fall back to a
    /// list that exists.
    fn sync(&mut self, open: &OpenSave) {
        if !open.known.iter().any(|entry| entry.list == self.list) {
            if let Some(first) = open.known.first() {
                self.list = first.list;
            }
        }
        let present = open.known.iter().find(|entry| entry.list == self.list).map(|entry| &entry.ids);
        let still_there = self
            .selected
            .as_ref()
            .is_some_and(|selected| present.is_some_and(|ids| ids.iter().any(|id| id == selected)));
        if !still_there {
            self.selected = None;
        }
    }
}

/// An edit the drawing code asked for, applied after the frame is drawn so the
/// window never holds a borrow of the save while changing it.
enum Action {
    SetCurrency(Currency, u32),
    AddItem {
        inventory: usize,
        id: String,
        amount: i64,
        kind: Option<ItemKind>,
        max: Option<i64>,
    },
    SetAmount {
        inventory: usize,
        position: usize,
        amount: i64,
    },
    Repair {
        inventory: usize,
        position: usize,
    },
    RepairAll,
    Remove {
        inventory: usize,
        position: usize,
    },
    Move {
        inventory: usize,
        position: usize,
        cell: Cell,
    },
    /// Replace an existing value, addressed by exact on-disk path.
    SetField {
        steps: Vec<RawStep>,
        value: Value,
    },
    /// Add an id to a known-item list.
    KnownAdd {
        list: KnownList,
        id: String,
    },
    /// Remove an id from a known-item list.
    KnownRemove {
        list: KnownList,
        id: String,
    },
    /// Switch to the known-items tab showing one list.
    OpenKnownList(KnownList),
    /// Ask to restore one archive over the open save.
    RestoreBackup(PathBuf),
    /// Pin an archive against pruning, or unpin it.
    PinBackup {
        path: PathBuf,
        pinned: bool,
    },
    /// Ask to delete one archive.
    DeleteBackup(PathBuf),
    Save,
    Discard,
}

/// The JSON value kind, so the editor can offer the right control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JsonKind {
    Null,
    Bool,
    Number,
    String,
    Array,
    Object,
}

impl JsonKind {
    fn of(value: &Value) -> JsonKind {
        match value {
            Value::Null => JsonKind::Null,
            Value::Bool(_) => JsonKind::Bool,
            Value::Number(_) => JsonKind::Number,
            Value::String(_) => JsonKind::String,
            Value::Array(_) => JsonKind::Array,
            Value::Object(_) => JsonKind::Object,
        }
    }

    fn label(self) -> &'static str {
        match self {
            JsonKind::Null => "null",
            JsonKind::Bool => "boolean",
            JsonKind::Number => "number",
            JsonKind::String => "string",
            JsonKind::Array => "array",
            JsonKind::Object => "object",
        }
    }
}

/// The JSON tab's controls, as opposed to the document.
struct JsonState {
    filter: String,
    /// The text the hits below were computed for; `None` before a search.
    searched_for: Option<String>,
    hits: Vec<JsonHit>,
    hits_truncated: bool,
    /// Exact path of the selected node.
    selected: Option<Vec<RawStep>>,
    /// Text box for the selected scalar.
    edit_text: String,
    /// Checkbox for the selected boolean.
    edit_bool: bool,
    /// The kind the editor was filled for.
    edit_kind: Option<JsonKind>,
    /// The value the editor was filled from.
    edit_original: Option<Value>,
    /// Ancestor paths to force open, so a hit is visible when it is clicked.
    reveal: Vec<Vec<RawStep>>,
}

impl Default for JsonState {
    fn default() -> Self {
        JsonState {
            filter: String::new(),
            searched_for: None,
            hits: Vec::new(),
            hits_truncated: false,
            selected: None,
            edit_text: String::new(),
            edit_bool: false,
            edit_kind: None,
            edit_original: None,
            reveal: Vec::new(),
        }
    }
}

impl JsonState {
    /// Point the editor at a node and fill it from the value.
    fn select(&mut self, steps: &[RawStep], value: &Value) {
        self.selected = Some(steps.to_vec());
        self.edit_kind = Some(JsonKind::of(value));
        self.edit_original = Some(value.clone());
        match value {
            Value::String(text) => self.edit_text = text.clone(),
            Value::Number(_) => self.edit_text = value.to_string(),
            Value::Bool(flag) => self.edit_bool = *flag,
            _ => self.edit_text.clear(),
        }
    }

    /// Re-read the editor from the document after an edit.
    fn sync(&mut self, open: &OpenSave) {
        let Some(steps) = self.selected.clone() else { return };
        match open.value_at(&steps) {
            Some(value) => self.select(&steps, value),
            None => {
                // The edit removed the node (or its parent); nothing to show.
                self.selected = None;
                self.edit_kind = None;
                self.edit_original = None;
                self.edit_text.clear();
            }
        }
    }

    /// Force the ancestors of `steps` open, so a search hit is visible.
    fn reveal(&mut self, steps: &[RawStep]) {
        self.reveal.clear();
        for end in 1..steps.len() {
            self.reveal.push(steps[..end].to_vec());
        }
    }

    /// Replace the hit list with every node whose path or value matches.
    fn search(&mut self, session: &Session) {
        let (hits, truncated) = session.search(&self.filter);
        self.hits = hits;
        self.hits_truncated = truncated;
        self.searched_for = Some(self.filter.clone());
    }
}

/// A switch that would throw away staged edits, held until the user decides.
enum PendingSwitch {
    Slot(usize),
    Folder(PathBuf),
}

/// The application.
pub struct EditorApp {
    session: Session,
    tab: Tab,
    path_input: String,
    edit: EditState,
    known: KnownState,
    json: JsonState,
    /// Backups of the open save, newest first.
    backups: Vec<BackupSummary>,
    /// Why the backup list could not be read, when that happened.
    backups_note: Option<String>,
    /// Last message shown in the bottom bar.
    status: Option<String>,
    pending_switch: Option<PendingSwitch>,
    confirm_save: bool,
    confirm_discard: bool,
    /// Archive the user asked to restore, awaiting confirmation.
    confirm_restore: Option<PathBuf>,
    /// Archive the user asked to delete, awaiting confirmation.
    confirm_delete: Option<PathBuf>,
}

impl EditorApp {
    /// Build the app, optionally opening a save folder given on the command line.
    pub fn new(_cc: &eframe::CreationContext<'_>, dir: Option<PathBuf>) -> EditorApp {
        let session = match &dir {
            Some(dir) => Session::open_dir(dir),
            None => Session::empty(),
        };
        EditorApp::from_session(session)
    }

    /// Build the app around an already-loaded session.
    pub fn from_session(session: Session) -> EditorApp {
        let mut app = EditorApp {
            path_input: session.dir.display().to_string(),
            session,
            tab: Tab::Overview,
            edit: EditState::default(),
            known: KnownState::default(),
            json: JsonState::default(),
            backups: Vec::new(),
            backups_note: None,
            status: None,
            pending_switch: None,
            confirm_save: false,
            confirm_discard: false,
            confirm_restore: None,
            confirm_delete: None,
        };
        app.sync_edit();
        app
    }

    /// Draw one frame.
    ///
    /// Separate from `eframe::App::update` so a test can drive the window with
    /// a bare egui context, without a window or a GPU.
    pub fn ui(&mut self, ctx: &egui::Context) {
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.edit.moving = None;
            self.json.reveal.clear();
            self.confirm_save = false;
            self.confirm_discard = false;
            self.confirm_restore = None;
            self.confirm_delete = None;
            self.pending_switch = None;
        }
        self.top_bar(ctx);
        self.bottom_bar(ctx);
        self.slot_panel(ctx);
        self.central(ctx);
        self.dialogs(ctx);
        // A reveal is a one-shot: the forced-open headers stored their state
        // this frame, so they stay open without being forced on every frame.
        self.json.reveal.clear();
    }

    /// True when the open save may be edited.
    ///
    /// That includes the game's own folder: every write is backed up and
    /// verified, which is what the owner of this machine asked for on
    /// 2026-11-02. The banner says when a write will sync.
    fn editable(&self) -> bool {
        self.session.open.is_some()
    }

    fn sync_edit(&mut self) {
        if let Some(open) = self.session.open.as_ref() {
            self.edit.sync(open);
            self.known.sync(open);
            self.json.sync(open);
        }
        match self.session.backups() {
            Ok(backups) => {
                self.backups = backups;
                self.backups_note = None;
            }
            Err(message) => {
                self.backups.clear();
                self.backups_note = Some(message);
            }
        }
    }

    /// Restore an archive and re-read everything the window shows.
    fn restore_backup(&mut self, archive: &Path) {
        let result = self.session.restore_backup(archive);
        match result {
            Ok(message) => self.status = Some(message),
            Err(message) => self.status = Some(format!("error: {message}")),
        }
        self.sync_edit();
    }

    /// Apply one staged action and refresh the views it touched.
    fn apply(&mut self, action: Action) {
        let result = match action {
            Action::SetCurrency(currency, value) => self.session.set_currency(currency, value),
            Action::AddItem { inventory, id, amount, kind, max } => {
                self.session.add_item(inventory, &id, amount, kind, max)
            }
            Action::SetAmount { inventory, position, amount } => {
                self.session.set_amount(inventory, position, amount)
            }
            Action::Repair { inventory, position } => self.session.repair(inventory, position),
            Action::RepairAll => self.session.repair_all(),
            Action::Remove { inventory, position } => {
                if self.edit.selected_slot == Some(position) {
                    self.edit.clear_slot();
                }
                self.session.remove_item(inventory, position)
            }
            Action::Move { inventory, position, cell } => self.session.move_item(inventory, position, cell),
            Action::SetField { steps, value } => self.session.set_field(&steps, value),
            Action::KnownAdd { list, id } => self.session.known_add(list, &id),
            Action::KnownRemove { list, id } => self.session.known_remove(list, &id),
            Action::OpenKnownList(list) => {
                self.known.list = list;
                self.tab = Tab::Known;
                Ok(format!("editing {}", list.label()))
            }
            Action::RestoreBackup(path) => {
                self.confirm_restore = Some(path);
                Ok("the restore needs confirmation".to_string())
            }
            Action::PinBackup { path, pinned } => self.session.pin_backup(&path, pinned),
            Action::DeleteBackup(path) => {
                self.confirm_delete = Some(path);
                Ok("the delete needs confirmation".to_string())
            }
            Action::Save => {
                self.edit.moving = None;
                self.session.save()
            }
            Action::Discard => {
                self.edit = EditState::default();
                self.session.reload()
            }
        };
        match result {
            Ok(message) => self.status = Some(message),
            Err(message) => self.status = Some(format!("error: {message}")),
        }
        self.sync_edit();
    }

    fn open_folder(&mut self) {
        let text = self.path_input.trim().to_string();
        if text.is_empty() {
            self.status = Some("type a save folder first".to_string());
            return;
        }
        if self.session.dirty() {
            self.pending_switch = Some(PendingSwitch::Folder(PathBuf::from(text)));
            return;
        }
        self.load_folder(PathBuf::from(text));
    }

    fn load_folder(&mut self, path: PathBuf) {
        self.session = Session::open_dir(&path);
        self.edit = EditState::default();
        self.known = KnownState::default();
        self.json = JsonState::default();
        self.tab = Tab::Overview;
        self.status = self.session.error.clone();
        self.sync_edit();
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.heading("nmsedit");
                ui.separator();
                ui.label("Save folder:");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.path_input)
                        .hint_text("/path/to/HelloGames/NMS/st_<id>")
                        .desired_width(460.0),
                );
                let submitted =
                    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if ui.button("Open").clicked() || submitted {
                    self.open_folder();
                }
                if ui.button("Rescan").clicked() {
                    self.session.rescan();
                    self.status = Some(format!("{} slot(s) found", self.session.slots.len()));
                    self.sync_edit();
                }
            });
            ui.add_space(2.0);
        });
    }

    fn slot_panel(&mut self, ctx: &egui::Context) {
        let dirty = self.session.dirty();
        let mut requested = None;
        egui::SidePanel::left("slots").resizable(true).default_width(260.0).show(ctx, |ui| {
            ui.add_space(4.0);
            ui.label(RichText::new("SLOTS").small().strong());
            ui.add_space(4.0);
            if self.session.slots.is_empty() {
                ui.label(RichText::new("No player saves found in this folder.").weak());
                return;
            }

            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for (index, entry) in self.session.slots.iter().enumerate() {
                    let selected = self
                        .session
                        .open
                        .as_ref()
                        .is_some_and(|open| open.entry.data_path == entry.data_path);
                    let title = RichText::new(entry.title()).strong();
                    if ui.selectable_label(selected, title).clicked() {
                        requested = Some(index);
                    }
                    ui.label(RichText::new(entry.subtitle()).small().weak());
                    if let Some(problem) = &entry.problem {
                        let color = Color32::from_rgb(200, 140, 90);
                        ui.label(RichText::new(problem).small().color(color));
                    }
                    ui.add_space(6.0);
                }
            });
        });
        if let Some(index) = requested {
            if dirty {
                self.pending_switch = Some(PendingSwitch::Slot(index));
            } else {
                self.session.select(index);
                self.edit.clear_slot();
                self.tab = Tab::Overview;
                self.status = None;
                self.sync_edit();
            }
        }
    }

    fn central(&mut self, ctx: &egui::Context) {
        let editable = self.editable();
        let notes = self.session.notes.clone();
        let mut action = None;
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(error) = &self.session.error {
                ui.colored_label(Color32::from_rgb(220, 110, 110), error);
                ui.separator();
            }
            // Missing mapping or data pack notes, in both the empty and the
            // loaded state: they explain why names may be absent.
            for note in &notes {
                ui.label(RichText::new(note).small().weak());
            }
            if self.session.open.is_none() {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.heading("No save open");
                    ui.label(
                        RichText::new("Type the game's save folder above, or launch with a folder path.")
                            .weak(),
                    );
                });
                return;
            }

            ui.horizontal(|ui| {
                if ui.selectable_label(self.tab == Tab::Overview, "Overview").clicked() {
                    self.tab = Tab::Overview;
                }
                if ui.selectable_label(self.tab == Tab::Inventories, "Inventories").clicked() {
                    self.tab = Tab::Inventories;
                }
                if ui.selectable_label(self.tab == Tab::Known, "Known items").clicked() {
                    self.tab = Tab::Known;
                }
                if ui.selectable_label(self.tab == Tab::Json, "JSON").clicked() {
                    self.tab = Tab::Json;
                }
            });
            ui.separator();

            let Some(open) = self.session.open.as_ref() else { return };
            if !editable {
                live_banner(ui);
            }
            action = match self.tab {
                Tab::Overview => overview(
                    ui,
                    open,
                    &mut self.edit,
                    editable,
                    &self.backups,
                    self.backups_note.as_deref(),
                ),
                Tab::Inventories => inventories_tab(
                    ui,
                    open,
                    &mut self.edit,
                    editable,
                    self.session.pack.as_ref(),
                ),
                Tab::Known => {
                    known_tab(ui, open, &mut self.known, editable, self.session.pack.as_ref())
                }
                Tab::Json => json_tab(ui, &self.session, &mut self.json, editable),
            };
        });
        if let Some(action) = action {
            self.apply(action);
        }
    }

    fn bottom_bar(&mut self, ctx: &egui::Context) {
        let dirty = self.session.dirty();
        let live = self.session.live_location();
        let dir = if self.session.dir.as_os_str().is_empty() {
            "no folder".to_string()
        } else {
            self.session.dir.display().to_string()
        };
        let open_text = self.session.open.as_ref().map(|open| {
            format!("{} · {} inventories", open.entry.slot, open.inventories.len())
        });
        let status = self.status.clone();
        let mut save = false;
        let mut discard = false;
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(dir).small().weak());
                ui.separator();
                match &open_text {
                    Some(text) => ui.label(RichText::new(text).small().weak()),
                    None => ui.label(RichText::new("no save open").small().weak()),
                };
                if live.is_some() {
                    ui.separator();
                    ui.label(RichText::new("live folder · syncs via Steam Cloud").small().weak());
                }
                if dirty {
                    ui.separator();
                    ui.label(RichText::new("staged changes").small());
                    if live.is_none() {
                        if ui.button("Save").clicked() {
                            save = true;
                        }
                        if ui.button("Discard").clicked() {
                            discard = true;
                        }
                    }
                }
                if let Some(status) = &status {
                    ui.separator();
                    ui.label(RichText::new(status).small());
                }
            });
        });
        if save {
            self.confirm_save = true;
        }
        if discard {
            self.confirm_discard = true;
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let mut switch = false;
        if self.pending_switch.is_some() {
            let mut discard = false;
            let mut cancel = false;
            egui::Window::new("Discard staged changes?")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label("The open save has changes that have not been written.");
                    ui.horizontal(|ui| {
                        if ui.button("Discard and switch").clicked() {
                            discard = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
            if discard {
                switch = true;
            }
            if cancel {
                self.pending_switch = None;
            }
        }
        if switch {
            match self.pending_switch.take() {
                Some(PendingSwitch::Slot(index)) => {
                    self.session.select(index);
                    self.edit.clear_slot();
                    self.tab = Tab::Overview;
                    self.status = None;
                    self.sync_edit();
                }
                Some(PendingSwitch::Folder(path)) => self.load_folder(path),
                None => {}
            }
        }

        if self.confirm_save {
            let mut do_save = false;
            let mut cancel = false;
            let file = self
                .session
                .open
                .as_ref()
                .map(|open| open.entry.data_path.display().to_string())
                .unwrap_or_default();
            egui::Window::new("Save changes")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label(format!("Write the staged changes to {file}?"));
                    ui.label(RichText::new("A timestamped zip backup is written first.").weak());
                    ui.label(
                        RichText::new("An unedited save is never rewritten; only changed fields move.")
                            .weak(),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            do_save = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
            if do_save {
                self.confirm_save = false;
                self.apply(Action::Save);
            }
            if cancel {
                self.confirm_save = false;
            }
        }

        if let Some(archive) = self.confirm_delete.clone() {
            let mut do_delete = false;
            let mut cancel = false;
            let name = archive
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            egui::Window::new("Delete backup")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label(format!("Delete the restore point {name}?"));
                    ui.label(
                        RichText::new(
                            "The save itself is untouched; this only removes the state the \
                             archive holds. A pinned archive can be unpinned instead.",
                        )
                        .weak(),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Delete").clicked() {
                            do_delete = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
            if do_delete {
                self.confirm_delete = None;
                let result = self.session.delete_backup(&archive);
                match result {
                    Ok(message) => self.status = Some(message),
                    Err(message) => self.status = Some(format!("error: {message}")),
                }
                self.sync_edit();
            }
            if cancel {
                self.confirm_delete = None;
            }
        }

        if let Some(archive) = self.confirm_restore.clone() {
            let mut do_restore = false;
            let mut cancel = false;
            let name = archive
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            egui::Window::new("Restore backup")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label(format!("Replace the open save with {name}?"));
                    ui.label(
                        RichText::new(
                            "The files being replaced are archived first (as pre-restore), and any \
                             staged edits are discarded.",
                        )
                        .weak(),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Restore").clicked() {
                            do_restore = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
            if do_restore {
                self.confirm_restore = None;
                self.restore_backup(&archive);
            }
            if cancel {
                self.confirm_restore = None;
            }
        }

        if self.confirm_discard {
            let mut do_discard = false;
            let mut cancel = false;
            egui::Window::new("Discard changes")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label("Throw away every staged change and re-read the file from disk?");
                    ui.horizontal(|ui| {
                        if ui.button("Discard").clicked() {
                            do_discard = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    });
                });
            if do_discard {
                self.confirm_discard = false;
                self.apply(Action::Discard);
            }
            if cancel {
                self.confirm_discard = false;
            }
        }
    }
}

impl eframe::App for EditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui(ctx);
    }
}

// ---------------------------------------------------------------------------
// drawing helpers
// ---------------------------------------------------------------------------

fn live_banner(ui: &mut egui::Ui) {
    ui.add_space(4.0);
    ui.label(
        RichText::new(
            "This is the game's own save folder, which Steam Cloud syncs: an edit here reaches \
             every machine signed into the account. Every save writes a timestamped backup \
             first, and the overview lists them.",
        )
        .small()
        .color(Color32::from_rgb(200, 140, 90)),
    );
    ui.add_space(4.0);
}

fn overview(
    ui: &mut egui::Ui,
    open: &OpenSave,
    edit: &mut EditState,
    editable: bool,
    backups: &[BackupSummary],
    backups_note: Option<&str>,
) -> Option<Action> {
    let mut action = None;
    ui.add_space(6.0);
    ui.heading(open.entry.title());
    ui.label(RichText::new(open.entry.subtitle()).weak());
    ui.add_space(10.0);

    egui::Grid::new("overview-facts").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
        fact(ui, "Version", open.info.version.map(|version| version.to_string()));
        fact(ui, "Platform", open.info.platform.clone());
        fact(ui, "Active context", open.info.active_context.clone());
        fact(ui, "Play time", open.info.play_time.map(format_duration));
        fact(
            ui,
            "Modified",
            open.entry.summary.as_ref().and_then(|summary| summary.modified).map(age_text),
        );
        fact(ui, "File", Some(open.entry.data_path.display().to_string()));
        fact(ui, "Storage slot", Some(open.entry.slot.to_string()));
    });

    ui.add_space(16.0);
    ui.label(RichText::new("CURRENCIES").small().strong());
    ui.add_space(4.0);
    match &open.currencies {
        Some(currencies) => {
            egui::Grid::new("currencies").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                for (index, currency) in Currency::ALL.iter().enumerate() {
                    let current = currencies.get(*currency);
                    ui.label(currency.label());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(thousands(current)).strong());
                        if editable {
                            ui.add_space(8.0);
                            ui.add(
                                egui::TextEdit::singleline(&mut edit.currency_inputs[index])
                                    .desired_width(110.0),
                            );
                            let parsed = parse_u32(&edit.currency_inputs[index]);
                            if ui.add_enabled(parsed.is_some(), egui::Button::new("Set")).clicked() {
                                if let Some(value) = parsed {
                                    action = Some(Action::SetCurrency(*currency, value));
                                }
                            }
                        }
                        if current == u32::MAX {
                            ui.label(RichText::new("(cap)").weak());
                        }
                    });
                    ui.end_row();
                }
            });
        }
        None => {
            let message = open.currencies_error.as_deref().unwrap_or("no player state in this save");
            ui.label(RichText::new(message).weak());
        }
    }

    ui.add_space(16.0);
    ui.add_space(16.0);
    ui.label(RichText::new("KNOWN ITEMS").small().strong());
    ui.add_space(4.0);
    if open.known.is_empty() {
        ui.label(RichText::new("this save has no known-item lists").weak());
    } else {
        egui::Grid::new("known").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
            for entry in &open.known {
                ui.label(entry.list.label());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(entry.ids.len().to_string()).strong());
                    if editable && ui.button("edit").clicked() {
                        action = Some(Action::OpenKnownList(entry.list));
                    }
                });
                ui.end_row();
            }
        });
    }

    ui.add_space(16.0);
    ui.label(RichText::new("BACKUPS").small().strong());
    ui.add_space(4.0);
    if let Some(note) = backups_note {
        ui.label(RichText::new(note).small().color(Color32::from_rgb(200, 140, 90)));
    } else if backups.is_empty() {
        ui.label(RichText::new("no backups yet — the first save writes one").weak());
    } else {
        ui.label(
            RichText::new(
                "Written before every save. Pinned archives are never pruned; deleting one only \
                 removes the restore point, not the save.",
            )
            .small()
            .weak(),
        );
        ui.add_space(4.0);
        egui::Grid::new("backups").num_columns(6).spacing([14.0, 4.0]).show(ui, |ui| {
            for backup in backups {
                let name = backup
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                // The state's own facts, when the archive's metadata reads: the
                // save's name, when the game last wrote it, and how long it had
                // been played.
                let mut hover = format!("{name}\n{}", backup.path.display());
                if let Some(state) = backup.state_time {
                    hover.push_str(&format!("\nstate from {}", nms_edit::format_utc(state)));
                }
                if let Some(save_name) = &backup.save_name {
                    hover.push_str(&format!("\nsave name {save_name:?}"));
                }
                if let Some(play) = backup.play_time {
                    hover.push_str(&format!("\nplay time {}", format_duration(play)));
                }
                if backup.kept {
                    hover.push_str("\npinned: kept out of pruning");
                }
                let mut label = RichText::new(&name).monospace().small();
                if backup.kept {
                    label = label.strong();
                }
                ui.label(label).on_hover_text(hover);

                let state = backup
                    .state_time
                    .map(nms_edit::format_utc)
                    .unwrap_or_else(|| "-".to_string());
                ui.label(RichText::new(state).small().weak());
                ui.label(RichText::new(bytes_text(backup.size)).small().weak());

                if ui
                    .add_enabled(editable, egui::Button::new("Restore").small())
                    .on_hover_text("Replace the open save with this state (what it replaces is backed up first)")
                    .clicked()
                {
                    action = Some(Action::RestoreBackup(backup.path.clone()));
                }
                let pin_label = if backup.kept { "Unpin" } else { "Pin" };
                let pin_hover = if backup.kept {
                    "Let pruning remove this archive again"
                } else {
                    "Keep this archive out of pruning"
                };
                if ui
                    .add_enabled(editable, egui::Button::new(pin_label).small())
                    .on_hover_text(pin_hover)
                    .clicked()
                {
                    action = Some(Action::PinBackup { path: backup.path.clone(), pinned: !backup.kept });
                }
                if ui
                    .add_enabled(editable, egui::Button::new("Delete").small())
                    .on_hover_text("Delete this restore point")
                    .clicked()
                {
                    action = Some(Action::DeleteBackup(backup.path.clone()));
                }
                ui.end_row();
            }
        });
    }

    action
}

fn inventories_tab(
    ui: &mut egui::Ui,
    open: &OpenSave,
    edit: &mut EditState,
    editable: bool,
    pack: Option<&DataPack>,
) -> Option<Action> {
    let mut action = None;
    if open.inventories.is_empty() {
        ui.add_space(16.0);
        ui.label("This save has no readable inventories.");
        ui.label(RichText::new("Inventory fields need the community key mapping; see the readme.").weak());
        return None;
    }
    edit.selected_inventory = edit.selected_inventory.min(open.inventories.len() - 1);

    egui::SidePanel::left("inventory-list").resizable(true).default_width(240.0).show_inside(ui, |ui| {
        ui.label(RichText::new("INVENTORIES").small().strong());
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (index, inventory) in open.inventories.iter().enumerate() {
                let mut text = inventory.path.label().to_string();
                if inventory.role == InventoryRole::Auxiliary {
                    text.push_str("  (machine)");
                }
                if ui.selectable_label(edit.selected_inventory == index, text).clicked() {
                    if edit.selected_inventory != index {
                        edit.selected_inventory = index;
                        edit.clear_slot();
                    }
                }
            }
        });
    });

    let inventory = &open.inventories[edit.selected_inventory];
    ui.add_space(6.0);
    let details = format!(
        "{}x{}  ·  {}/{} slots  ·  {}  ·  class {}",
        inventory.width,
        inventory.height,
        inventory.slots.len(),
        inventory.valid_cells.len(),
        inventory.stack_group,
        inventory.class
    );
    ui.horizontal(|ui| {
        ui.heading(inventory.path.label());
        ui.label(RichText::new(details).weak());
        if editable
            && ui
                .button("Repair all")
                .on_hover_text("Repairs every damaged slot in the save, not just this inventory")
                .clicked()
        {
            action = Some(Action::RepairAll);
        }
    });
    if inventory.role == InventoryRole::Auxiliary {
        ui.label(
            RichText::new(
                "Machine-owned grid: its amounts are recipe quantities, so they are left out of \
                 stack-size statistics.",
            )
            .small()
            .weak(),
        );
    }
    ui.add_space(6.0);

    if editable {
        if let Some(add) = add_item_form(ui, edit, pack) {
            action = Some(add);
        }
        ui.add_space(6.0);
    }

    if let Some(position) = edit.moving {
        ui.label(
            RichText::new(format!(
                "Moving slot {position}: click an empty cell to drop it, or press Esc to cancel."
            ))
            .small()
            .color(Color32::from_rgb(120, 180, 120)),
        );
        ui.add_space(4.0);
    }

    if inventory.width == 0 || inventory.height == 0 {
        ui.label(RichText::new("this grid is empty").weak());
        return action;
    }

    let free = inventory.free_cells();
    if let Some(cell) = grid(ui, inventory, pack, edit.selected_slot, edit.moving, &free) {
        match edit.moving {
            Some(position) => {
                let occupied_elsewhere = inventory
                    .slots
                    .iter()
                    .any(|slot| slot.cell == cell && slot.position != position);
                if !occupied_elsewhere {
                    action = Some(Action::Move { inventory: edit.selected_inventory, position, cell });
                }
                edit.moving = None;
            }
            None => match inventory.slots.iter().find(|slot| slot.cell == cell) {
                Some(slot) => edit.select_slot(open, slot.position),
                None => edit.clear_slot(),
            },
        }
    }

    if let Some(position) = edit.selected_slot {
        if let Some(slot) = inventory.slots.iter().find(|slot| slot.position == position) {
            ui.add_space(8.0);
            ui.separator();
            if let Some(slot_action) =
                slot_details(ui, slot, pack, edit, editable, inventory.is_full())
            {
                action = Some(slot_action);
            }
        }
    }
    action
}

fn add_item_form(ui: &mut egui::Ui, edit: &mut EditState, pack: Option<&DataPack>) -> Option<Action> {
    let mut action = None;
    egui::CollapsingHeader::new("Add item").default_open(false).show(ui, |ui| {
        egui::Grid::new("add-item").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label("Item id");
            ui.add(
                egui::TextEdit::singleline(&mut edit.add_id)
                    .hint_text("^CARBON")
                    .desired_width(240.0),
            );
            ui.end_row();
            ui.label("Amount");
            ui.add(
                egui::TextEdit::singleline(&mut edit.add_amount)
                    .hint_text("1")
                    .desired_width(100.0),
            );
            ui.end_row();
            ui.label("Kind");
            egui::ComboBox::from_id_salt("add-kind")
                .selected_text(edit.add_kind.label())
                .show_ui(ui, |ui| {
                    for choice in KindChoice::ALL {
                        ui.selectable_value(&mut edit.add_kind, choice, choice.label());
                    }
                });
            ui.end_row();
            ui.label("Stack size");
            ui.add(
                egui::TextEdit::singleline(&mut edit.add_max)
                    .hint_text("from the save")
                    .desired_width(100.0),
            );
            ui.end_row();
        });

        let name = pack.and_then(|pack| pack.item_name(edit.add_id.trim()));
        if let Some(name) = name.filter(|name| !name.is_empty()) {
            ui.label(RichText::new(name).small().weak());
        }
        let amount = parse_i64(&edit.add_amount);
        let valid = !edit.add_id.trim().is_empty() && amount.is_some();
        if ui.add_enabled(valid, egui::Button::new("Add to this inventory")).clicked() {
            if let Some(amount) = amount {
                action = Some(Action::AddItem {
                    inventory: edit.selected_inventory,
                    id: edit.add_id.trim().to_string(),
                    amount,
                    kind: edit.add_kind.item_kind(),
                    max: parse_i64(&edit.add_max),
                });
            }
        }
    });
    action
}

/// Draw an inventory grid, returning the cell that was clicked.
fn grid(
    ui: &mut egui::Ui,
    inventory: &Inventory,
    pack: Option<&DataPack>,
    selected: Option<usize>,
    moving: Option<usize>,
    free: &[Cell],
) -> Option<Cell> {
    const CELL: f32 = 46.0;
    const GAP: f32 = 4.0;

    let width = inventory.width as f32 * (CELL + GAP);
    let height = inventory.height as f32 * (CELL + GAP);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());

    let dark = ui.visuals().dark_mode;
    let (empty, substance, product, technology) = if dark {
        (
            Color32::from_rgb(32, 34, 38),
            Color32::from_rgb(40, 76, 58),
            Color32::from_rgb(40, 60, 92),
            Color32::from_rgb(92, 68, 34),
        )
    } else {
        (
            Color32::from_rgb(238, 238, 238),
            Color32::from_rgb(198, 232, 210),
            Color32::from_rgb(200, 218, 244),
            Color32::from_rgb(244, 226, 190),
        )
    };
    let damaged = Color32::from_rgb(206, 84, 84);
    let moving_color = Color32::from_rgb(120, 180, 120);
    let outline = if dark { Color32::from_rgb(60, 62, 68) } else { Color32::from_rgb(196, 198, 204) };
    let strong = ui.visuals().strong_text_color();
    let weak = ui.visuals().weak_text_color();

    let mut clicked = None;
    for y in 0..inventory.height {
        for x in 0..inventory.width {
            let cell = Cell::new(x, y);
            let min = rect.min + Vec2::new(x as f32 * (CELL + GAP), y as f32 * (CELL + GAP));
            let cell_rect = egui::Rect::from_min_size(min, Vec2::splat(CELL));
            let slot = inventory.slots.iter().find(|slot| slot.cell == cell);
            let is_move_target = moving.is_some() && free.contains(&cell);
            let fill = if is_move_target {
                if dark {
                    Color32::from_rgb(34, 58, 40)
                } else {
                    Color32::from_rgb(214, 238, 208)
                }
            } else {
                match slot {
                    Some(slot) => match slot.kind {
                        ItemKind::Substance => substance,
                        ItemKind::Product => product,
                        ItemKind::Technology => technology,
                    },
                    None => empty,
                }
            };

            let painter = ui.painter();
            painter.rect_filled(cell_rect, CornerRadius::same(4), fill);
            let stroke = match slot {
                Some(slot) if moving == Some(slot.position) => Stroke::new(2.0, moving_color),
                Some(slot) if slot.damaged() => Stroke::new(2.0, damaged),
                Some(slot) if selected == Some(slot.position) => Stroke::new(2.0, strong),
                _ if is_move_target => Stroke::new(2.0, moving_color),
                _ => Stroke::new(1.0, outline),
            };
            painter.rect_stroke(cell_rect, CornerRadius::same(4), stroke, StrokeKind::Inside);

            let id = ui.id().with((inventory.path.label(), x, y));
            let response = ui.interact(cell_rect, id, Sense::click());
            let Some(slot) = slot else {
                if response.clicked() {
                    clicked = Some(cell);
                }
                continue;
            };

            let short: String = slot.id.trim_start_matches(['^', '!']).chars().take(6).collect();
            painter.text(
                cell_rect.center() - Vec2::new(0.0, 6.0),
                Align2::CENTER_CENTER,
                short,
                FontId::proportional(10.0),
                strong,
            );
            let amount = if slot.kind == ItemKind::Technology && slot.amount < 0 {
                "—".to_string()
            } else {
                slot.amount.to_string()
            };
            painter.text(
                cell_rect.center() + Vec2::new(0.0, 9.0),
                Align2::CENTER_CENTER,
                amount,
                FontId::proportional(11.0),
                weak,
            );

            let name = pack.and_then(|pack| pack.item_name(&slot.id)).unwrap_or_else(|| slot.id.clone());
            let hover = if moving.is_some() {
                format!("{name}\n{}\nclick to cancel the move", slot.id)
            } else {
                format!(
                    "{name}\n{}\n{} / {}\n{}\n\nclick for details",
                    slot.id, slot.amount, slot.max_amount, slot.cell
                )
            };
            if response.on_hover_text(hover).clicked() {
                clicked = Some(cell);
            }
        }
    }
    clicked
}

fn slot_details(
    ui: &mut egui::Ui,
    slot: &ItemSlot,
    pack: Option<&DataPack>,
    edit: &mut EditState,
    editable: bool,
    inventory_full: bool,
) -> Option<Action> {
    let mut action = None;
    let name = pack.and_then(|pack| pack.item_name(&slot.id)).unwrap_or_else(|| slot.id.clone());
    ui.horizontal(|ui| {
        ui.label(RichText::new(name).heading());
        ui.label(RichText::new(&slot.id).weak());
    });
    egui::Grid::new("slot-details").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
        fact(ui, "Kind", Some(slot.kind.label().to_string()));
        fact(ui, "Cell", Some(slot.cell.to_string()));
        fact(ui, "Amount", Some(format!("{} / {}", slot.amount, slot.max_amount)));
        fact(ui, "Position in Slots array", Some(slot.position.to_string()));
        fact(ui, "Damage", Some(format!("{:.0}%", slot.damage * 100.0)));
        fact(ui, "Installed", Some(if slot.installed { "yes" } else { "no" }.to_string()));
    });

    if !editable {
        return None;
    }

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label("Amount");
        ui.add(egui::TextEdit::singleline(&mut edit.amount_input).desired_width(100.0));
        if ui.add_enabled(parse_i64(&edit.amount_input).is_some(), egui::Button::new("Set")).clicked() {
            if let Some(amount) = parse_i64(&edit.amount_input) {
                let position = slot.position;
                let inventory = edit.selected_inventory;
                action = Some(Action::SetAmount { inventory, position, amount });
            }
        }
        if ui.add_enabled(slot.damaged(), egui::Button::new("Repair")).clicked() {
            action = Some(Action::Repair { inventory: edit.selected_inventory, position: slot.position });
        }
        if ui.add_enabled(!inventory_full, egui::Button::new("Move")).clicked() {
            edit.moving = Some(slot.position);
        }
        if ui.button("Remove").clicked() {
            action = Some(Action::Remove { inventory: edit.selected_inventory, position: slot.position });
        }
    });
    action
}

/// The known-items tab: the five learned-id lists, with add and remove.
///
/// The lists are long (1,478 products on the sampled save), so the rows are
/// drawn with a virtualised scroll area and filtered by id or item name.
fn known_tab(
    ui: &mut egui::Ui,
    open: &OpenSave,
    state: &mut KnownState,
    editable: bool,
    pack: Option<&DataPack>,
) -> Option<Action> {
    if open.known.is_empty() {
        ui.add_space(16.0);
        ui.label("This save has no readable known-item lists.");
        return None;
    }

    let mut action = None;
    egui::SidePanel::left("known-lists").resizable(true).default_width(220.0).show_inside(ui, |ui| {
        ui.label(RichText::new("LISTS").small().strong());
        ui.add_space(4.0);
        for entry in &open.known {
            let text = format!("{}  ({})", entry.list.label(), entry.ids.len());
            if ui.selectable_label(state.list == entry.list, text).clicked() && state.list != entry.list {
                state.list = entry.list;
                state.selected = None;
            }
        }
    });

    let Some(entry) = open.known.iter().find(|entry| entry.list == state.list).or_else(|| open.known.first())
    else {
        return None;
    };
    state.list = entry.list;

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.heading(entry.list.label());
        ui.label(RichText::new(format!("{} ids", entry.ids.len())).weak());
    });
    ui.add_space(4.0);

    if editable {
        ui.horizontal(|ui| {
            ui.label("Add");
            ui.add(
                egui::TextEdit::singleline(&mut state.add_id)
                    .hint_text("^CARBON")
                    .desired_width(200.0),
            );
            let id = state.add_id.trim().to_string();
            if ui.add_enabled(!id.is_empty(), egui::Button::new("Add")).clicked() {
                action = Some(Action::KnownAdd { list: entry.list, id });
            }
        });
    }

    ui.horizontal(|ui| {
        ui.label("Filter");
        ui.add(egui::TextEdit::singleline(&mut state.filter).desired_width(240.0));
        let removable = editable && state.selected.is_some();
        if ui.add_enabled(removable, egui::Button::new("Remove selected")).clicked() {
            if let Some(id) = state.selected.clone() {
                action = Some(Action::KnownRemove { list: entry.list, id });
            }
        }
        if let Some(selected) = &state.selected {
            ui.label(RichText::new(selected).small().weak());
        }
    });
    ui.add_space(4.0);

    let needle = state.filter.trim().to_ascii_lowercase();
    let visible: Vec<&String> = entry
        .ids
        .iter()
        .filter(|id| {
            if needle.is_empty() {
                return true;
            }
            // A row shows the id and (when a data pack is loaded) the item
            // name, so both are worth searching.
            let name = pack.and_then(|pack| pack.item_name(id));
            contains_ci(id, &needle) || name.is_some_and(|name| contains_ci(&name, &needle))
        })
        .collect();
    if !needle.is_empty() && visible.len() != entry.ids.len() {
        ui.label(RichText::new(format!("{} of {} shown", visible.len(), entry.ids.len())).small().weak());
    }

    // The virtualised list needs a uniform row height; take it from the theme
    // so a different text size or zoom cannot make rows overlap.
    let row_height = ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y;
    egui::ScrollArea::vertical()
        .id_salt("known-ids")
        .auto_shrink([false, false])
        .show_rows(ui, row_height, visible.len(), |ui, range| {
            for index in range {
                let id = visible[index];
                let name = pack.and_then(|pack| pack.item_name(id)).unwrap_or_default();
                let text = if name.is_empty() { id.clone() } else { format!("{id}   {name}") };
                let response = ui.selectable_label(state.selected.as_deref() == Some(id.as_str()), text);
                if response.clicked() {
                    state.selected = Some(id.clone());
                }
                response.on_hover_text(id);
            }
        });
    action
}

/// The JSON tab: a lazy tree, a search over the whole document, and a typed
/// editor for the selected node.
///
/// The tree only walks what is open, so a 9 MB save costs nothing until it is
/// explored. Search walks everything, which is why it runs on Enter or the
/// button rather than on every keystroke.
fn json_tab(ui: &mut egui::Ui, session: &Session, state: &mut JsonState, editable: bool) -> Option<Action> {
    let Some(open) = session.open.as_ref() else { return None };
    let keys = &session.keys;
    let root = open.bundle.document.tree();

    ui.horizontal(|ui| {
        ui.label("Find");
        let response = ui.add(
            egui::TextEdit::singleline(&mut state.filter)
                .hint_text("key or value")
                .desired_width(260.0),
        );
        let entered = response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
        if ui.button("Search").clicked() || entered {
            state.search(session);
        }
        if ui.button("Clear").clicked() {
            state.filter.clear();
            state.searched_for = None;
            state.hits.clear();
            state.hits_truncated = false;
        }
        if let Some(searched) = &state.searched_for {
            let text = if state.hits.is_empty() {
                format!("no match for {searched:?}")
            } else if state.hits_truncated {
                format!("{} matches (first {JSON_HIT_LIMIT} shown)", state.hits.len())
            } else {
                format!("{} match(es)", state.hits.len())
            };
            ui.label(RichText::new(text).small().weak());
        }
    });

    if !state.hits.is_empty() {
        let mut clicked = None;
        egui::ScrollArea::vertical()
            .id_salt("json-hits")
            .max_height(140.0)
            .show(ui, |ui| {
                for (index, hit) in state.hits.iter().enumerate() {
                    let text = format!("{}  =  {}", hit.label, hit.preview);
                    if ui.selectable_label(false, text).clicked() {
                        clicked = Some(index);
                    }
                }
            });
        if let Some(index) = clicked {
            let steps = state.hits[index].steps.clone();
            if let Some(value) = open.value_at(&steps) {
                state.select(&steps, value);
                state.reveal(&steps);
            }
        }
    }

    ui.add_space(4.0);
    ui.label(
        RichText::new(
            "Edits here are staged like every other edit: nothing is written until Save. A wrong \
             value can make the save unreadable, so the backup is what to fall back on.",
        )
        .small()
        .weak(),
    );
    ui.add_space(4.0);

    let tree_height = (ui.available_height() - 170.0).max(120.0);
    egui::ScrollArea::vertical()
        .id_salt("json-tree")
        .max_height(tree_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut steps = Vec::new();
            json_node(ui, root, &mut steps, keys, state);
        });

    ui.add_space(6.0);
    ui.separator();
    json_editor(ui, open, keys, state, editable)
}

/// Draw the document root and everything below it.
fn json_node(
    ui: &mut egui::Ui,
    value: &Value,
    steps: &mut Vec<RawStep>,
    keys: &KeyMap,
    state: &mut JsonState,
) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                steps.push(RawStep::Key(key.clone()));
                let label = keys.readable_for(key).unwrap_or(key).to_string();
                json_child(ui, child, steps, keys, state, &label, key);
                steps.pop();
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                steps.push(RawStep::Index(index));
                let label = format!("[{index}]");
                json_child(ui, child, steps, keys, state, &label, &label);
                steps.pop();
            }
        }
        _ => {}
    }
}

/// Draw one node: a collapsing header for a container, a selectable row for a
/// leaf. `label` is what the user sees, `raw` the on-disk spelling.
fn json_child(
    ui: &mut egui::Ui,
    value: &Value,
    steps: &mut Vec<RawStep>,
    keys: &KeyMap,
    state: &mut JsonState,
    label: &str,
    raw: &str,
) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            let id = ui.make_persistent_id(("json-node", steps.as_slice()));
            let mut collapsing =
                egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
            if state.reveal.iter().any(|ancestor| ancestor.as_slice() == steps.as_slice()) {
                collapsing.set_open(true);
            }
            let header = collapsing.show_header(ui, |ui| {
                ui.label(format!("{label}  {{{}}}", map.len())).on_hover_text(raw);
            });
            header.body(|ui| {
                for (key, child) in map {
                    steps.push(RawStep::Key(key.clone()));
                    let child_label = keys.readable_for(key).unwrap_or(key).to_string();
                    json_child(ui, child, steps, keys, state, &child_label, key);
                    steps.pop();
                }
            });
        }
        Value::Array(items) if !items.is_empty() => {
            let id = ui.make_persistent_id(("json-node", steps.as_slice()));
            let mut collapsing =
                egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
            if state.reveal.iter().any(|ancestor| ancestor.as_slice() == steps.as_slice()) {
                collapsing.set_open(true);
            }
            let header = collapsing.show_header(ui, |ui| {
                ui.label(format!("{label}  [{}]", items.len())).on_hover_text(raw);
            });
            header.body(|ui| {
                for (index, child) in items.iter().enumerate() {
                    steps.push(RawStep::Index(index));
                    let child_label = format!("[{index}]");
                    json_child(ui, child, steps, keys, state, &child_label, &child_label);
                    steps.pop();
                }
            });
        }
        _ => {
            let selected = state.selected.as_deref() == Some(steps.as_slice());
            let response = ui.selectable_label(selected, format!("{label}: {}", json_summary(value)));
            if response.clicked() {
                state.select(steps, value);
            }
            response.on_hover_text(raw);
        }
    }
}

/// The editor for the selected node: a control that matches the value's kind.
fn json_editor(
    ui: &mut egui::Ui,
    open: &OpenSave,
    keys: &KeyMap,
    state: &mut JsonState,
    editable: bool,
) -> Option<Action> {
    let Some(steps) = state.selected.clone() else {
        ui.label(RichText::new("Click a value in the tree, or search for one.").weak());
        return None;
    };
    let Some(value) = open.value_at(&steps).cloned() else {
        ui.label(RichText::new("That field is no longer in the document.").weak());
        return None;
    };
    let kind = JsonKind::of(&value);

    ui.horizontal(|ui| {
        ui.label(RichText::new("Path").small().strong());
        ui.label(RichText::new(describe_raw_path(&steps, keys)).monospace());
        if let Some(RawStep::Key(raw)) = steps.last() {
            ui.label(RichText::new(format!("on-disk key {raw:?}")).small().weak());
        }
    });

    let mut action = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new(kind.label()).small().strong());
        match kind {
            JsonKind::String => {
                ui.add(egui::TextEdit::singleline(&mut state.edit_text).desired_width(420.0));
                if editable && ui.button("Set").clicked() {
                    let value = Value::String(state.edit_text.clone());
                    action = Some(Action::SetField { steps: steps.clone(), value });
                }
            }
            JsonKind::Number => {
                ui.add(egui::TextEdit::singleline(&mut state.edit_text).desired_width(220.0));
                let number = serde_json::from_str::<Value>(&state.edit_text)
                    .ok()
                    .filter(Value::is_number);
                if number.is_none() {
                    ui.label(RichText::new("not a number").small().color(Color32::from_rgb(206, 84, 84)));
                }
                if ui.add_enabled(editable && number.is_some(), egui::Button::new("Set")).clicked() {
                    if let Some(number) = number {
                        action = Some(Action::SetField { steps: steps.clone(), value: number });
                    }
                }
            }
            JsonKind::Bool => {
                let text = if state.edit_bool { "true" } else { "false" };
                ui.checkbox(&mut state.edit_bool, text);
                if editable && ui.button("Set").clicked() {
                    let value = Value::Bool(state.edit_bool);
                    action = Some(Action::SetField { steps: steps.clone(), value });
                }
            }
            JsonKind::Null => {
                ui.label(RichText::new("null").weak());
                ui.label(RichText::new("(a null field is left alone here)").small().weak());
            }
            JsonKind::Array | JsonKind::Object => {
                ui.label(RichText::new(json_summary(&value)).weak());
                ui.label(RichText::new("(containers are shown, not replaced)").small().weak());
            }
        }
    });
    action
}

fn fact(ui: &mut egui::Ui, name: &str, value: Option<String>) {
    ui.label(name);
    match value {
        Some(value) if !value.is_empty() => ui.label(value),
        _ => ui.label(RichText::new("—").weak()),
    };
    ui.end_row();
}

/// A byte count in the unit that keeps it short.
fn bytes_text(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    if value < KIB {
        format!("{bytes} B")
    } else if value < KIB * KIB {
        format!("{:.1} KiB", value / KIB)
    } else {
        format!("{:.1} MiB", value / (KIB * KIB))
    }
}

/// Group digits in threes, as the game's own UI does for large balances.
fn thousands(value: u32) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(' ');
        }
        out.push(ch);
    }
    out
}

/// Parse a balance, ignoring the separators the display uses.
fn parse_u32(text: &str) -> Option<u32> {
    let digits: String = text.chars().filter(|ch| !ch.is_whitespace() && *ch != '_' && *ch != ',').collect();
    digits.parse().ok()
}

/// Parse a slot amount; technology uses `-1` for "not chargeable".
fn parse_i64(text: &str) -> Option<i64> {
    let digits: String = text.chars().filter(|ch| !ch.is_whitespace() && *ch != '_' && *ch != ',').collect();
    digits.parse().ok()
}

fn age_text(time: std::time::SystemTime) -> String {
    let seconds = time.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let age = now.saturating_sub(seconds);
    // A local-time rendering would need a timezone crate; the age is what tells
    // two saved copies apart.
    match age {
        0..=59 => format!("{age} seconds ago"),
        60..=3599 => format!("{} minutes ago", age / 60),
        3600..=86_399 => format!("{} hours ago", age / 3600),
        _ => format!("{} days ago", age / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::Session;
    use nms_model::KeyMap;
    use std::path::Path;

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
                    "ValidSlotIndices": [{"X":0,"Y":0}, {"X":1,"Y":0}],
                    "Class": {"InventoryClass": "C"},
                    "StackSizeGroup": {"InventoryStackSizeGroup": "Personal"},
                    "Width": 2,
                    "Height": 1
                },
                "KnownProducts": ["^A"],
                "KnownTech": [],
                "KnownSpecials": [],
                "SeenBaseBuildingObjects": [],
                "KnownRefinerRecipes": []
            }
        }
    }"#;

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("nmsedit-gui-ui-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch directory");
        std::fs::write(path.join("save.hg"), SAVE.as_bytes()).expect("write save");
        path
    }

    fn app(dir: &Path) -> EditorApp {
        EditorApp::from_session(Session::open_dir_with(dir, KeyMap::builtin(), None, Vec::new()))
    }

    /// Run two frames (egui settles panel sizes on the second) and collect every
    /// string the window drew.
    fn drawn_text(app: &mut EditorApp) -> String {
        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1180.0, 760.0))),
            ..Default::default()
        };
        let mut text = String::new();
        for _ in 0..2 {
            let output = ctx.run(input(), |ctx| app.ui(ctx));
            text = collect_text(&output.shapes);
        }
        text
    }

    fn collect_text(shapes: &[egui::epaint::ClippedShape]) -> String {
        fn walk(shape: &egui::Shape, out: &mut String) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push_str(&text.galley.job.text);
                    out.push('\n');
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = String::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    #[test]
    fn large_balances_get_thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1 000");
        assert_eq!(thousands(1_234_567), "1 234 567");
        assert_eq!(thousands(u32::MAX), "4 294 967 295");
    }

    #[test]
    fn balances_accept_the_separators_the_window_draws() {
        assert_eq!(parse_u32("1 234 567"), Some(1_234_567));
        assert_eq!(parse_u32("4_294_967_295"), Some(u32::MAX));
        assert_eq!(parse_u32("12x"), None);
        assert_eq!(parse_i64("-1"), Some(-1));
    }

    #[test]
    fn the_window_draws_the_save_it_opens() {
        let dir = scratch("overview");
        let mut app = app(&dir);
        app.session.select(0);
        app.sync_edit();

        let text = drawn_text(&mut app);
        for expected in ["nmsedit", "PlayerState1", "CURRENCIES", "units", "1 200", "Set", "KNOWN ITEMS"] {
            assert!(text.contains(expected), "the overview should draw {expected:?}:\n{text}");
        }

        app.tab = Tab::Inventories;
        let text = drawn_text(&mut app);
        for expected in ["INVENTORIES", "Inventory", "CARBON", "Add item", "Repair all"] {
            assert!(text.contains(expected), "the inventory tab should draw {expected:?}:\n{text}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_session_draws_a_hint_instead_of_failing() {
        let mut app = EditorApp::from_session(Session::empty());
        let text = drawn_text(&mut app);
        assert!(text.contains("No save open"), "expected the empty-state hint:\n{text}");
    }

    #[test]
    fn the_overview_lists_backups_and_confirms_a_restore() {
        let dir = scratch("backup");
        let mut app = app(&dir);
        app.session.select(0);
        app.sync_edit();
        assert!(app.backups.is_empty(), "nothing saved yet");

        app.apply(Action::SetCurrency(Currency::Units, 555));
        app.apply(Action::Save);
        assert_eq!(app.backups.len(), 1, "a save writes one archive");
        assert_eq!(
            app.session.open.as_ref().expect("open").currencies.expect("currencies").units,
            555
        );

        // The overview draws the archive's name and a restore button.
        let name = app.backups[0].path.file_name().unwrap().to_string_lossy().into_owned();
        let text = drawn_text(&mut app);
        for expected in ["BACKUPS", name.as_str(), "Restore"] {
            assert!(text.contains(expected), "the overview should draw {expected:?}:\n{text}");
        }

        // The button only asks; the restore itself puts the old file back.
        let archive = app.backups[0].path.clone();
        app.apply(Action::RestoreBackup(archive.clone()));
        assert_eq!(app.confirm_restore.as_deref(), Some(archive.as_path()));
        app.confirm_restore = None;
        app.restore_backup(&archive);
        assert!(!app.session.dirty());
        assert_eq!(
            app.session.open.as_ref().expect("open").currencies.expect("currencies").units,
            1200
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn backups_can_be_pinned_and_deleted_from_the_overview() {
        let dir = scratch("backup-manage");
        let mut app = app(&dir);
        app.session.select(0);
        app.sync_edit();

        // Two edits, so there are two restore points.
        app.apply(Action::SetCurrency(Currency::Units, 111));
        app.apply(Action::Save);
        app.apply(Action::SetCurrency(Currency::Units, 222));
        app.apply(Action::Save);
        assert_eq!(app.backups.len(), 2);
        let oldest = app.backups.last().expect("the older archive").path.clone();
        assert!(!app.backups.iter().any(|backup| backup.kept));

        // The panel draws the state each archive holds, with the buttons.
        let text = drawn_text(&mut app);
        for expected in ["BACKUPS", "Restore", "Pin", "Delete"] {
            assert!(text.contains(expected), "the overview should draw {expected:?}:\n{text}");
        }
        // (This save has no metadata file, so the archive carries no state
        // facts; the real-save test covers those.)
        // Pinning renames the archive and survives a refresh.
        app.apply(Action::PinBackup { path: oldest.clone(), pinned: true });
        let pinned = app
            .backups
            .iter()
            .find(|backup| backup.kept)
            .expect("one pinned archive")
            .path
            .clone();
        assert!(pinned.to_string_lossy().ends_with(".keep.zip"), "{}", pinned.display());
        let text = drawn_text(&mut app);
        assert!(text.contains("Unpin"), "a pinned archive offers unpinning:\n{text}");

        // Deleting asks first, then removes exactly that archive.
        app.apply(Action::DeleteBackup(pinned.clone()));
        assert_eq!(app.confirm_delete.as_deref(), Some(pinned.as_path()));
        app.confirm_delete = None;
        let report = app.session.delete_backup(&pinned).expect("delete");
        assert!(report.contains("deleted"), "{report}");
        app.sync_edit();
        assert_eq!(app.backups.len(), 1);
        assert!(!app.backups[0].path.to_string_lossy().contains(".keep."));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_known_tab_lists_and_stages_edits() {
        let dir = scratch("known-tab");
        let mut app = app(&dir);
        app.session.select(0);
        app.sync_edit();
        app.tab = Tab::Known;

        let text = drawn_text(&mut app);
        for expected in ["LISTS", "products", "Add", "Filter", "Remove selected", "^A"] {
            assert!(text.contains(expected), "the known tab should draw {expected:?}:\n{text}");
        }

        app.apply(Action::KnownAdd { list: KnownList::Products, id: "^NEW".to_string() });
        assert!(app.session.dirty());
        let text = drawn_text(&mut app);
        assert!(text.contains("^NEW"), "the added id should appear in the list:\n{text}");

        app.apply(Action::KnownRemove { list: KnownList::Products, id: "^A".to_string() });
        let ids = &app
            .session
            .open
            .as_ref()
            .expect("open")
            .known
            .iter()
            .find(|entry| entry.list == KnownList::Products)
            .expect("products")
            .ids;
        assert!(!ids.iter().any(|id| id == "^A"), "the removed id is gone: {ids:?}");
        assert!(ids.iter().any(|id| id == "^NEW"));

        // The overview's edit buttons jump straight to the list they name.
        app.apply(Action::OpenKnownList(KnownList::Technology));
        assert_eq!(app.tab, Tab::Known);
        assert_eq!(app.known.list, KnownList::Technology);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_json_tab_draws_the_tree_and_finds_a_value() {
        let dir = scratch("json-tab");
        let mut app = app(&dir);
        app.session.select(0);
        app.sync_edit();
        app.tab = Tab::Json;

        let text = drawn_text(&mut app);
        for expected in ["JSON", "Find", "BaseContext", "Version: 4743"] {
            assert!(text.contains(expected), "the JSON tab should draw {expected:?}:\n{text}");
        }

        // Search runs on the button or Enter, so drive it directly here.
        app.json.filter = "Units".to_string();
        app.json.search(&app.session);
        assert_eq!(app.json.hits.len(), 1, "one Units field");
        let text = drawn_text(&mut app);
        assert!(
            text.contains("BaseContext.PlayerStateData.Units"),
            "the hit list should show the path:\n{text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_json_edit_stages_and_marks_the_save() {
        let dir = scratch("json-edit");
        let mut app = app(&dir);
        app.session.select(0);
        app.sync_edit();
        app.tab = Tab::Json;

        let steps = vec![
            RawStep::Key("BaseContext".to_string()),
            RawStep::Key("PlayerStateData".to_string()),
            RawStep::Key("Units".to_string()),
        ];
        let value = app.session.open.as_ref().expect("open").value_at(&steps).cloned().expect("Units");
        app.json.select(&steps, &value);

        let text = drawn_text(&mut app);
        for expected in ["Path", "BaseContext.PlayerStateData.Units", "1200", "Set"] {
            assert!(text.contains(expected), "the editor should draw {expected:?}:\n{text}");
        }

        app.apply(Action::SetField { steps: steps.clone(), value: Value::from(7777) });
        assert!(app.session.dirty());
        let open = app.session.open.as_ref().expect("open");
        assert_eq!(open.value_at(&steps).and_then(Value::as_i64), Some(7777));
        assert_eq!(open.currencies.expect("currencies").units, 7777);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn staged_edits_show_a_save_and_discard_button() {
        let dir = scratch("staged");
        let mut app = app(&dir);
        app.session.select(0);
        app.sync_edit();
        // Stage a change through the same path the widgets use.
        app.apply(Action::SetCurrency(Currency::Units, 4321));
        assert!(app.session.dirty());

        let text = drawn_text(&mut app);
        for expected in ["staged changes", "Save", "Discard"] {
            assert!(text.contains(expected), "expected {expected:?} once something is staged:\n{text}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
