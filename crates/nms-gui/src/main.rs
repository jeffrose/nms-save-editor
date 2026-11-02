//! `nmsedit-gui` — a desktop editor for No Man's Sky save files.
//!
//! Usage: `nmsedit-gui [save-folder]`. The folder can also be typed into the
//! window. Nothing is written yet: this milestone is the slot browser and the
//! read-only inventory view.

mod app;
mod session;

use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    let dir = std::env::args().nth(1).map(PathBuf::from);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([860.0, 520.0])
            .with_title("nmsedit"),
        ..Default::default()
    };
    eframe::run_native(
        "nmsedit",
        options,
        Box::new(move |cc| Ok(Box::new(app::EditorApp::new(cc, dir)))),
    )
}
