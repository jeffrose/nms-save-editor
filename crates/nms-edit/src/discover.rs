//! Finding the game's save folders and reading their slot lists.
//!
//! Every face starts from nothing at least once: the window with no argument,
//! an agent asked to "add nanites to my Pioneers save", a listing of a machine
//! nobody has configured. So the usual installation layouts are searched in
//! order, and the first folder that actually holds saves is what
//! [`default_save_dir`] returns. `NMSEDIT_SAVE_DIR` wins over all of it, which
//! is the documented way to point the tools at a copy.

use std::path::{Path, PathBuf};

use nms_format::metadata::{Metadata, Slot};

/// The Steam app id No Man's Sky installs under.
const APP_ID: &str = "275850";

/// The locations the game uses on this platform, whether or not they exist
/// yet.
///
/// This is what the window puts in its folder field before anything has been
/// found: the answer to "where would my saves be" on a fresh machine is the
/// same path, it just is not there yet.
pub fn platform_save_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

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
            roots.push(compatdata.join("pfx/drive_c/users/steamuser/AppData/Roaming/HelloGames/NMS"));
        }
        // The native macOS build.
        roots.push(home.join("Library/Application Support/HelloGames/NMS"));
    }
    // Windows, and anything running under Wine with a normal environment.
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
        roots.push(appdata.join("HelloGames/NMS"));
    }
    // The Steam layouts overlap: on Debian's package `~/.steam/steam` is a
    // symlink to `~/.steam/debian-installation`, so both spellings would list
    // the same saves twice.
    unique_paths(roots)
}

/// Keep the first spelling of each path, comparing what it resolves to.
fn unique_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut unique: Vec<PathBuf> = Vec::new();
    for path in paths {
        let key = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let seen = unique
            .iter()
            .any(|existing| std::fs::canonicalize(existing).unwrap_or_else(|_| existing.clone()) == key);
        if !seen {
            unique.push(path);
        }
    }
    unique
}

/// Candidate save roots on this machine, in the order they are tried.
///
/// Only directories that exist are returned, so a caller can report "none of
/// the usual locations exist" rather than a list of misses.
pub fn default_save_roots() -> Vec<PathBuf> {
    platform_save_roots().into_iter().filter(|path| path.is_dir()).collect()
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

/// How many player saves a folder holds, without reading any of them.
///
/// Used by folder listings: `accountdata.hg`, `usersettings.hg` and the
/// metadata files are not saves.
pub fn player_save_count(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            Slot::from_file_name(&name).is_some_and(|slot| slot.player_number().is_some())
        })
        .count()
}

/// The first of `roots` that holds saves, looking inside each for `st_*`
/// folders too.
///
/// Split out from [`default_save_dir`] so the choice can be tested without the
/// machine's real folders.
pub fn first_save_dir_in(roots: &[PathBuf]) -> Option<PathBuf> {
    roots.iter().find_map(|root| save_dirs(root).into_iter().next())
}

/// The folder a face should open when the user named none.
///
/// `NMSEDIT_SAVE_DIR` wins when set; otherwise the usual locations are
/// searched and the first folder that holds saves is returned. `None` means
/// "ask the user", not "there are no saves anywhere".
pub fn default_save_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("NMSEDIT_SAVE_DIR") {
        if !dir.is_empty() {
            let dir = PathBuf::from(dir);
            if dir.is_dir() {
                // A configured root may hold `st_*` folders or the saves
                // themselves; open whichever it is.
                return save_dirs(&dir).into_iter().next().or(Some(dir));
            }
        }
    }
    first_save_dir_in(&default_save_roots())
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
    fn the_platform_roots_are_a_superset_of_the_existing_ones() {
        let platform = platform_save_roots();
        let existing = default_save_roots();
        assert!(!platform.is_empty(), "a platform with HOME set knows where saves go");
        assert!(
            existing.iter().all(|root| platform.contains(root)),
            "an existing root must be one of the platform's: {existing:?} vs {platform:?}"
        );
        assert!(
            existing.iter().all(|root| root.is_dir()),
            "default_save_roots only reports folders that are there"
        );
    }

    #[cfg(unix)]
    #[test]
    fn one_save_folder_is_not_listed_twice_through_a_symlink() {
        let root = std::env::temp_dir().join(format!("nms-edit-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let install = root.join("install");
        std::fs::create_dir_all(&install).unwrap();
        std::os::unix::fs::symlink(&install, root.join("link")).unwrap();

        let unique = unique_paths(vec![root.join("link"), install.clone(), install.clone()]);
        assert_eq!(unique.len(), 1, "one folder, one entry: {unique:?}");
        assert_eq!(
            std::fs::canonicalize(&unique[0]).unwrap(),
            std::fs::canonicalize(&install).unwrap(),
            "the kept spelling names the same folder"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_first_root_that_holds_saves_wins() {
        let root = std::env::temp_dir().join(format!("nms-edit-first-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let empty = root.join("empty");
        let first = root.join("first/st_1");
        let second = root.join("second/st_2");
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("save.hg"), b"{}").unwrap();
        std::fs::write(second.join("save2.hg"), b"{}").unwrap();

        assert_eq!(first_save_dir_in(&[empty.clone(), first.clone(), second]), Some(first));
        assert_eq!(first_save_dir_in(&[]), None, "nothing to pick from");
        assert_eq!(first_save_dir_in(&[empty.clone()]), None, "a root with no saves is skipped");

        // A root whose saves sit directly in it is used as-is.
        assert_eq!(first_save_dir_in(&[root.join("first")]), Some(root.join("first/st_1")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_save_count_ignores_settings_and_metadata_files() {
        let dir = std::env::temp_dir().join(format!("nms-edit-count-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["save.hg", "save7.hg", "accountdata.hg", "usersettings.hg", "mf_save.hg", "notes.txt"] {
            std::fs::write(dir.join(name), b"{}").unwrap();
        }
        assert_eq!(player_save_count(&dir), 2);
        assert_eq!(player_save_count(&dir.join("missing")), 0);
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
