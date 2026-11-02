//! The window: a slot browser, a save summary, and editable inventory grids.
//!
//! Editing is a staging model: every action changes only the in-memory
//! document, so the file on disk is untouched until **Save** is pressed. A
//! refused action (a full grid, an occupied cell) leaves the document clean,
//! because the edit layer validates against an immutable view first. Save then
//! goes through [`crate::session::Session::save`], which writes a timestamped
//! backup and patches the metadata exactly like the CLI does.
//!
//! The window refuses to write to a live, cloud-synced save folder. That is the
//! same rule the edit layer enforces, and skipping it would put an edit on every
//! machine signed into the account.

use std::path::PathBuf;

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, RichText, ScrollArea, Sense, Stroke, StrokeKind, Vec2,
};
use nms_data::DataPack;
use nms_edit::{Cell, Currency, Inventory, InventoryRole, ItemKind, ItemSlot};

use crate::session::{format_duration, OpenSave, Session};

/// Which part of a save the central panel is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Overview,
    Inventories,
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
    Save,
    Discard,
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
    /// Last message shown in the bottom bar.
    status: Option<String>,
    pending_switch: Option<PendingSwitch>,
    confirm_save: bool,
    confirm_discard: bool,
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
            status: None,
            pending_switch: None,
            confirm_save: false,
            confirm_discard: false,
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
            self.confirm_save = false;
            self.confirm_discard = false;
            self.pending_switch = None;
        }
        self.top_bar(ctx);
        self.bottom_bar(ctx);
        self.slot_panel(ctx);
        self.central(ctx);
        self.dialogs(ctx);
    }

    /// True when the open save may be edited.
    ///
    /// A live folder is read-only here; see the module comment.
    fn editable(&self) -> bool {
        self.session.open.is_some() && self.session.live_location().is_none()
    }

    fn sync_edit(&mut self) {
        if let Some(open) = self.session.open.as_ref() {
            self.edit.sync(open);
        }
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
            });
            ui.separator();

            let Some(open) = self.session.open.as_ref() else { return };
            if !editable {
                live_banner(ui);
            }
            action = match self.tab {
                Tab::Overview => overview(ui, open, &mut self.edit, editable),
                Tab::Inventories => inventories_tab(
                    ui,
                    open,
                    &mut self.edit,
                    editable,
                    self.session.pack.as_ref(),
                ),
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
                    ui.label(RichText::new("live folder · read-only").small().weak());
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
            "This is the game's live save folder, so the window is read-only here. \
             Copy the folder somewhere else and open the copy to edit it.",
        )
        .small()
        .color(Color32::from_rgb(200, 140, 90)),
    );
    ui.add_space(4.0);
}

fn overview(ui: &mut egui::Ui, open: &OpenSave, edit: &mut EditState, editable: bool) -> Option<Action> {
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
    ui.label(RichText::new("KNOWN ITEMS").small().strong());
    ui.add_space(4.0);
    if open.known.is_empty() {
        ui.label(RichText::new("this save has no known-item lists").weak());
    } else {
        egui::Grid::new("known").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
            for (list, count) in &open.known {
                ui.label(list.label());
                ui.label(RichText::new(count.to_string()).strong());
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

fn fact(ui: &mut egui::Ui, name: &str, value: Option<String>) {
    ui.label(name);
    match value {
        Some(value) if !value.is_empty() => ui.label(value),
        _ => ui.label(RichText::new("—").weak()),
    };
    ui.end_row();
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
