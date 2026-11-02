//! The window: a slot browser, a save summary, and inventory grids.
//!
//! This first slice is **read-only**. Every widget below draws state from
//! [`crate::session::Session`]; nothing in this file opens a file or touches a
//! save, so the edit layer's safety rules apply unchanged when editing is added.

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

/// The application.
pub struct EditorApp {
    session: Session,
    tab: Tab,
    path_input: String,
    /// Index into `OpenSave::inventories`.
    selected_inventory: usize,
    /// Position of the clicked slot in the on-disk `Slots` array, which is what
    /// edit commands will address once editing lands.
    selected_slot: Option<usize>,
    /// Last message shown in the bottom bar.
    status: Option<String>,
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
        EditorApp {
            path_input: session.dir.display().to_string(),
            session,
            tab: Tab::Overview,
            selected_inventory: 0,
            selected_slot: None,
            status: None,
        }
    }

    /// Draw one frame.
    ///
    /// Separate from `eframe::App::update` so a test can drive the window with
    /// a bare egui context, without a window or a GPU.
    pub fn ui(&mut self, ctx: &egui::Context) {
        self.top_bar(ctx);
        self.bottom_bar(ctx);
        self.slot_panel(ctx);
        self.central(ctx);
    }

    fn open_folder(&mut self) {
        if self.path_input.trim().is_empty() {
            self.status = Some("type a save folder first".to_string());
            return;
        }
        self.session = Session::open_dir(&PathBuf::from(self.path_input.trim()));
        self.selected_inventory = 0;
        self.selected_slot = None;
        self.status = self.session.error.clone();
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
                }
            });
            ui.add_space(2.0);
        });
    }

    fn slot_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("slots").resizable(true).default_width(260.0).show(ctx, |ui| {
            ui.add_space(4.0);
            ui.label(RichText::new("SLOTS").small().strong());
            ui.add_space(4.0);
            if self.session.slots.is_empty() {
                ui.label(RichText::new("No player saves found in this folder.").weak());
                return;
            }

            let mut clicked = None;
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for (index, entry) in self.session.slots.iter().enumerate() {
                    let selected = self
                        .session
                        .open
                        .as_ref()
                        .is_some_and(|open| open.entry.data_path == entry.data_path);
                    let title = RichText::new(entry.title()).strong();
                    if ui.selectable_label(selected, title).clicked() {
                        clicked = Some(index);
                    }
                    ui.label(RichText::new(entry.subtitle()).small().weak());
                    if let Some(problem) = &entry.problem {
                        let color = Color32::from_rgb(200, 140, 90);
                        ui.label(RichText::new(problem).small().color(color));
                    }
                    ui.add_space(6.0);
                }
            });
            if let Some(index) = clicked {
                self.session.select(index);
                self.selected_inventory = 0;
                self.selected_slot = None;
            }
        });
    }

    fn central(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(error) = &self.session.error {
                ui.colored_label(Color32::from_rgb(220, 110, 110), error);
                ui.separator();
            }
            let Some(open) = self.session.open.as_ref() else {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    ui.heading("No save open");
                    ui.label(
                        RichText::new("Type the game's save folder above, or launch with a folder path.")
                            .weak(),
                    );
                });
                for note in &self.session.notes {
                    ui.add_space(4.0);
                    ui.label(RichText::new(note).small().weak());
                }
                return;
            };

            for note in &self.session.notes {
                ui.label(RichText::new(note).small().weak());
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

            match self.tab {
                Tab::Overview => overview(ui, open),
                Tab::Inventories => inventories_tab(
                    ui,
                    open,
                    self.session.pack.as_ref(),
                    &mut self.selected_inventory,
                    &mut self.selected_slot,
                ),
            }
        });
    }

    fn bottom_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(self.session.dir.display().to_string()).small().weak());
                ui.separator();
                match self.session.open.as_ref() {
                    Some(open) => {
                        let text = format!(
                            "{}  ·  {} inventories  ·  read-only preview",
                            open.entry.slot,
                            open.inventories.len()
                        );
                        ui.label(RichText::new(text).small().weak());
                    }
                    None => {
                        ui.label(RichText::new("no save open").small().weak());
                    }
                }
                if self.session.pack.is_none() {
                    ui.separator();
                    ui.label(RichText::new("no data pack: item names unavailable").small().weak());
                }
                if let Some(status) = &self.status {
                    ui.separator();
                    ui.label(RichText::new(status).small());
                }
            });
        });
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

fn overview(ui: &mut egui::Ui, open: &OpenSave) {
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
                for currency in Currency::ALL {
                    ui.label(currency.label());
                    let value = currencies.get(currency);
                    let mut text = thousands(value);
                    if value == u32::MAX {
                        text.push_str("  (cap)");
                    }
                    ui.label(RichText::new(text).strong());
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
}

fn inventories_tab(
    ui: &mut egui::Ui,
    open: &OpenSave,
    pack: Option<&DataPack>,
    selected_inventory: &mut usize,
    selected_slot: &mut Option<usize>,
) {
    if open.inventories.is_empty() {
        ui.add_space(16.0);
        ui.label("This save has no readable inventories.");
        ui.label(RichText::new("Inventory fields need the community key mapping; see the readme.").weak());
        return;
    }
    *selected_inventory = (*selected_inventory).min(open.inventories.len() - 1);

    egui::SidePanel::left("inventory-list").resizable(true).default_width(240.0).show_inside(ui, |ui| {
        ui.label(RichText::new("INVENTORIES").small().strong());
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (index, inventory) in open.inventories.iter().enumerate() {
                let mut text = inventory.path.label().to_string();
                if inventory.role == InventoryRole::Auxiliary {
                    text.push_str("  (machine)");
                }
                if ui.selectable_label(*selected_inventory == index, text).clicked() {
                    *selected_inventory = index;
                    *selected_slot = None;
                }
            }
        });
    });

    let inventory = &open.inventories[*selected_inventory];
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
    });
    ui.add_space(6.0);

    if inventory.width == 0 || inventory.height == 0 {
        ui.label(RichText::new("this grid is empty").weak());
        return;
    }
    grid(ui, inventory, pack, selected_slot);

    if let Some(position) = *selected_slot {
        if let Some(slot) = inventory.slots.iter().find(|slot| slot.position == position) {
            ui.add_space(8.0);
            ui.separator();
            slot_details(ui, slot, pack);
        }
    }
}

/// Draw an inventory grid and record the clicked slot.
fn grid(ui: &mut egui::Ui, inventory: &Inventory, pack: Option<&DataPack>, selected: &mut Option<usize>) {
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
    let outline = if dark { Color32::from_rgb(60, 62, 68) } else { Color32::from_rgb(196, 198, 204) };
    let strong = ui.visuals().strong_text_color();
    let weak = ui.visuals().weak_text_color();

    for y in 0..inventory.height {
        for x in 0..inventory.width {
            let cell = Cell::new(x, y);
            let min = rect.min + Vec2::new(x as f32 * (CELL + GAP), y as f32 * (CELL + GAP));
            let cell_rect = egui::Rect::from_min_size(min, Vec2::splat(CELL));
            let slot = inventory.slots.iter().find(|slot| slot.cell == cell);
            let fill = match slot {
                Some(slot) => match slot.kind {
                    ItemKind::Substance => substance,
                    ItemKind::Product => product,
                    ItemKind::Technology => technology,
                },
                None => empty,
            };

            let painter = ui.painter();
            painter.rect_filled(cell_rect, CornerRadius::same(4), fill);
            let stroke = match slot {
                Some(slot) if slot.damaged() => Stroke::new(2.0, damaged),
                Some(slot) if *selected == Some(slot.position) => Stroke::new(2.0, strong),
                _ => Stroke::new(1.0, outline),
            };
            painter.rect_stroke(cell_rect, CornerRadius::same(4), stroke, StrokeKind::Inside);

            let response = ui.interact(cell_rect, ui.id().with((x, y, "cell")), Sense::click());
            let Some(slot) = slot else {
                if response.clicked() {
                    *selected = None;
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
            let response = response.on_hover_text(format!(
                "{name}\n{}\n{} / {}\n{}\n\nclick for details",
                slot.id, slot.amount, slot.max_amount, slot.cell
            ));
            if response.clicked() {
                *selected = Some(slot.position);
            }
        }
    }
}

fn slot_details(ui: &mut egui::Ui, slot: &ItemSlot, pack: Option<&DataPack>) {
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
                    "ValidSlotIndices": [{"X":0,"Y":0},{"X":1,"Y":0}],
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
    fn the_window_draws_the_save_it_opens() {
        let dir = scratch("overview");
        let mut app = app(&dir);
        app.session.select(0);

        let text = drawn_text(&mut app);
        for expected in ["nmsedit", "PlayerState1", "CURRENCIES", "units", "1 200", "KNOWN ITEMS"] {
            assert!(text.contains(expected), "the overview should draw {expected:?}:\n{text}");
        }

        app.tab = Tab::Inventories;
        let text = drawn_text(&mut app);
        for expected in ["INVENTORIES", "Inventory", "CARBON"] {
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
}
