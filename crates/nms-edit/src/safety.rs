//! Recognising the game's live save folders.
//!
//! On PC, saves live inside a Steam Cloud-synced folder: a change there is
//! uploaded and lands on every machine signed into the account, which is why
//! the editor used to refuse those paths outright. It no longer does — the
//! owner of this machine asked for live editing once every write was backed up
//! (timestamped zip, restorable, pinnable) and verified before it touched the
//! disk. What remains here is the classification, so each face can say out loud
//! that a write will sync.
//!
//! A directory tree that declares itself a scratch copy with a marker file is
//! never reported as live, which is how `tools/sandbox-test.sh` and the test
//! suite work inside a copied prefix without a false warning.

use std::path::{Path, PathBuf};

/// Marker file that marks a directory tree as a scratch copy.
pub const SANDBOX_MARKER: &str = ".nmsedit-sandbox";

/// A location whose contents are managed by the game or by Steam Cloud.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveLocation {
    /// A Proton prefix managed by Steam (`steamapps/compatdata/<appid>/...`).
    SteamProtonPrefix,
    /// A game save directory (`.../HelloGames/NMS/...`).
    GameSaveDirectory,
}

impl LiveLocation {
    /// A short description used in warnings and errors.
    pub fn describe(self) -> &'static str {
        match self {
            LiveLocation::SteamProtonPrefix => "a Steam-managed Proton prefix",
            LiveLocation::GameSaveDirectory => "a No Man's Sky save folder",
        }
    }

    /// Why writing here is risky.
    pub fn hazard(self) -> &'static str {
        match self {
            LiveLocation::SteamProtonPrefix => {
                "Steam Cloud syncs this prefix, so an edit is uploaded to every machine on the account"
            }
            LiveLocation::GameSaveDirectory => {
                "the game and Steam Cloud both read this folder, so an edit can be synced to every machine on the account"
            }
        }
    }
}

/// True when `path` or an ancestor contains the sandbox marker.
pub fn is_sandboxed(path: &Path) -> bool {
    let mut current: Option<&Path> = Some(path);
    while let Some(dir) = current {
        if dir.join(SANDBOX_MARKER).exists() {
            return true;
        }
        current = dir.parent();
    }
    false
}

/// Create the sandbox marker in `root`.
pub fn mark_sandbox(root: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    let marker = root.join(SANDBOX_MARKER);
    std::fs::write(&marker, b"scratch copy: live-folder guards do not apply inside this tree\n")?;
    Ok(marker)
}

/// Classify `path`, returning `None` for ordinary locations or scratch copies.
pub fn live_save_location(path: &Path) -> Option<LiveLocation> {
    if is_sandboxed(path) {
        return None;
    }
    let parts: Vec<String> = path
        .components()
        .filter_map(|component| component.as_os_str().to_str().map(|text| text.to_ascii_lowercase()))
        .collect();

    // Steam Proton prefix: .../steamapps/compatdata/<appid>/pfx/...
    for window in parts.windows(3) {
        if window[0] == "steamapps" && window[1] == "compatdata" && window[2].chars().all(|c| c.is_ascii_digit()) {
            return Some(LiveLocation::SteamProtonPrefix);
        }
    }

    // Save directory: .../HelloGames/NMS/... (Windows, Proton or Lutris layouts).
    for window in parts.windows(2) {
        if window[0] == "hellogames" && window[1] == "nms" {
            return Some(LiveLocation::GameSaveDirectory);
        }
    }

    None
}

/// The caution to show when a write lands in a live save tree.
///
/// `None` for ordinary paths and for marked scratch copies. Callers print or
/// display this rather than deciding for themselves what to say.
pub fn live_notice(path: &Path) -> Option<String> {
    live_save_location(path).map(|kind| {
        format!(
            "{} is {}: {}. Every write here is backed up first, and Steam Cloud will sync it.",
            path.display(),
            kind.describe(),
            kind.hazard()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steam_save() -> PathBuf {
        PathBuf::from(
            "/home/player/.steam/debian-installation/steamapps/compatdata/275850/pfx/drive_c/users/steamuser/AppData/Roaming/HelloGames/NMS/st_1/save.hg",
        )
    }

    fn windows_save() -> PathBuf {
        PathBuf::from("C:/Users/player/AppData/Roaming/HelloGames/NMS/st_1/save.hg")
    }

    #[test]
    fn detects_live_save_locations() {
        assert_eq!(live_save_location(&steam_save()), Some(LiveLocation::SteamProtonPrefix));
        assert_eq!(live_save_location(&windows_save()), Some(LiveLocation::GameSaveDirectory));
        // The Steam rule wins for a Proton prefix even outside a HelloGames dir.
        let prefix = Path::new("/home/p/.steam/steamapps/compatdata/275850/pfx/drive_c/windows");
        assert_eq!(live_save_location(prefix), Some(LiveLocation::SteamProtonPrefix));
    }

    #[test]
    fn leaves_ordinary_paths_alone() {
        assert_eq!(live_save_location(Path::new("/home/player/Downloads/save.hg")), None);
        assert_eq!(live_save_location(Path::new("/tmp/scratch/save13.hg")), None);
        assert_eq!(live_save_location(Path::new("./fixtures/steam/st_1/save.hg")), None);
    }

    #[test]
    fn a_marker_exempts_a_scratch_copy() {
        let root = std::env::temp_dir().join(format!("nms-edit-safety-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let nested = root.join("compatdata/pfx/drive_c/users/steamuser/AppData/Roaming/HelloGames/NMS/st_1");
        std::fs::create_dir_all(&nested).unwrap();
        let save = nested.join("save.hg");
        std::fs::write(&save, b"{}").unwrap();

        // Without the marker this is a live save folder...
        assert!(live_save_location(&save).is_some());
        // ...with it, the tree is a declared scratch copy.
        mark_sandbox(&root).unwrap();
        assert!(is_sandboxed(&save));
        assert_eq!(live_save_location(&save), None);
        assert_eq!(live_notice(&save), None, "a scratch copy is not worth a warning");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_live_location_gets_a_notice_instead_of_a_refusal() {
        let notice = live_notice(&steam_save()).expect("a live path is reported");
        assert!(notice.contains("Steam"), "{notice}");
        assert!(notice.contains("backed up"), "{notice}");
        assert!(notice.contains("sync"), "{notice}");
        assert_eq!(live_notice(Path::new("/tmp/scratch/save13.hg")), None);
    }
}
