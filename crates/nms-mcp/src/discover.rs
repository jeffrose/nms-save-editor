//! Finding the game's save folders and reading their slot lists.
//!
//! The editor is normally pointed at a folder, but an agent asked to "add
//! nanites to my Pioneers save" starts with nothing, so the usual installation
//! layouts are searched in order. `NMSEDIT_SAVE_DIR` wins when set: it is the
//! documented way to point the tools at a copy without touching the originals
//! (and the live-folder guard still applies to whatever it names).

use std::path::{Path, PathBuf};

use nms_format::metadata::{Metadata, Slot};

/// The Steam app id No Man's Sky installs under.
const APP_ID: &str = "275850";

/// Candidate save roots on this machine, in the order they are tried.
///
/// Only directories that exist are returned, so a caller can report "none of
/// the usual locations exist" rather than a list of misses.
pub fn default_save_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut push = |path: PathBuf| {
        if path.is_dir() && !roots.contains(&path) {
            roots.push(path);
        }
    };

    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        // Steam on Linux, in the layouts seen in the wild: the Debian package,
        // the upstream tarball, and the Flatpak.
        for steam in [
            ".steam/debian-installation",
            ".steam/steam",
            ".local/share/Steam",
            ".var/app/com.valvesoftware.Steam/.local/share/Steam",
        ] {
            let compatdata = home.join(steam).join("steamapps").join("compatdata").join(APP_ID);
            push(compatdata.join("pfx/drive_c/users/steamuser/AppData/Roaming/HelloGames/NMS"));
        }
        // The native macOS build.
        push(home.join("Library/Application Support/HelloGames/NMS"));
    }
    // Windows, and anything running under Wine with a normal environment.
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
        push(appdata.join("HelloGames/NMS"));
    }
    roots
}

/// The save directories inside a root: its `st_*` subdirectories, or the root
/// itself when save files sit directly in it.
pub fn save_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if holds_player_saves(root) {
        dirs.push(root.to_path_buf());
    }
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir()
                && entry.file_name().to_string_lossy().starts_with("st_")
                && holds_player_saves(&path)
            {
                dirs.push(path);
            }
        }
    }
    dirs.sort();
    dirs
}

/// One save found in a folder, with what its metadata says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundSave {
    /// The folder the file sits in.
    pub folder: PathBuf,
    /// The storage slot the file name identifies.
    pub slot: Slot,
    /// Path of the data file.
    pub data_path: PathBuf,
    /// Player-chosen save name, when the metadata reads.
    pub save_name: String,
    /// Game mode, when the metadata reads.
    pub game_mode: String,
    /// Location summary, when the metadata reads.
    pub location: String,
    /// Total play time in seconds.
    pub play_time: u64,
    /// Save version word.
    pub version: Option<u32>,
    /// Modification time of the data file, in Unix seconds.
    pub modified_unix: Option<u64>,
}

impl FoundSave {
    /// What the client sees.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "folder": self.folder.display().to_string(),
            "file": self.data_path.display().to_string(),
            "slot": self.slot.to_string(),
            "name": self.save_name,
            "game_mode": self.game_mode,
            "location": self.location,
            "play_time_seconds": self.play_time,
            "version": self.version,
            "modified_unix": self.modified_unix,
        })
    }
}

/// Every player save in a folder, ordered by slot, read from metadata only.
///
/// Scanning all 30 slots costs 30 small file reads, which is the point: a
/// client can list saves without decompressing 9 MB payloads.
pub fn scan(dir: &Path) -> Vec<FoundSave> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut saves = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(slot) = Slot::from_file_name(&name) else { continue };
        if slot.player_number().is_none() {
            // `accountdata.hg` and `usersettings.hg` are settings, not saves.
            continue;
        }
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let metadata = std::fs::read(dir.join(slot.metadata_file_name()))
            .ok()
            .and_then(|bytes| Metadata::read(&bytes, slot).ok());
        saves.push(FoundSave {
            folder: dir.to_path_buf(),
            slot,
            data_path: path.clone(),
            save_name: metadata.as_ref().map(Metadata::save_name).unwrap_or_default(),
            game_mode: metadata.as_ref().map(Metadata::game_mode).unwrap_or_default(),
            location: metadata.as_ref().map(Metadata::location).unwrap_or_default(),
            play_time: metadata.as_ref().map(|metadata| metadata.play_time).unwrap_or(0),
            version: metadata.as_ref().map(|metadata| metadata.version),
            modified_unix: std::fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|age| age.as_secs()),
        });
    }
    saves.sort_by_key(|save| save.slot.index());
    saves
}

fn holds_player_saves(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else { return false };
    entries.flatten().any(|entry| {
        let name = entry.file_name().to_string_lossy().into_owned();
        Slot::from_file_name(&name).is_some_and(|slot| slot.player_number().is_some())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_of_saves_is_scanned_and_settings_are_ignored() {
        let dir = std::env::temp_dir().join(format!("nms-mcp-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        for name in ["save.hg", "save10.hg", "accountdata.hg", "usersettings.hg", "notes.txt"] {
            std::fs::write(dir.join(name), b"{}").expect("write");
        }

        let saves = scan(&dir);
        let names: Vec<String> = saves
            .iter()
            .map(|save| save.data_path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["save.hg", "save10.hg"], "player saves, ordered by slot");

        // Metadata is missing here, so the descriptions are empty rather than absent.
        assert_eq!(saves[0].save_name, "");
        assert!(saves[0].version.is_none());

        let dirs = save_dirs(&dir);
        assert_eq!(dirs, [dir.clone()], "a folder with saves in it is its own save dir");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn steam_id_folders_are_found_under_a_root() {
        let root = std::env::temp_dir().join(format!("nms-mcp-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("st_76561197970834089")).expect("scratch");
        std::fs::write(root.join("st_76561197970834089").join("save.hg"), b"{}").expect("write");
        // A `st_` folder without saves is not a save folder.
        std::fs::create_dir_all(root.join("st_empty")).expect("scratch");

        let dirs = save_dirs(&root);
        assert_eq!(dirs, [root.join("st_76561197970834089")]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
