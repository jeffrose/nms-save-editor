//! Guards against editing the wrong copy of a save.
//!
//! On PC, saves live inside a Steam Cloud-synced folder: a change there is
//! uploaded and lands on every machine signed into the account. That is
//! exactly the failure mode reported over and over in the older editor's issue
//! tracker ("it corrupted my save on all my machines"), and it is not
//! recoverable from inside this process.
//!
//! So the edit layer refuses to write to a location that looks like a live save
//! folder unless the caller passes [`CommitOptions::confirm_live`]. A directory
//! tree that declares itself a sandbox with a marker file is exempt, which is
//! how `tools/sandbox-test.sh` and the test suite work on saved copies without
//! tripping the guard.

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

/// A path that must not be written to without explicit confirmation.
#[derive(Debug, Clone)]
pub struct LiveWrite {
    /// The offending path.
    pub path: PathBuf,
    /// What kind of live location it is.
    pub kind: LiveLocation,
}

impl std::fmt::Display for LiveWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} looks like {}. {}",
            self.path.display(),
            self.kind.describe(),
            self.kind.hazard()
        )
    }
}

impl LiveWrite {
    /// The hint shown to the user.
    pub fn hint(&self) -> String {
        format!(
            "{} looks like {}. {}.\n\
             Take a backup first, then re-run with --confirm-live to accept that Steam Cloud will sync the change.\n\
             To experiment without touching it, use tools/sandbox-test.sh (an isolated prefix).",
            self.path.display(),
            self.kind.describe(),
            self.kind.hazard()
        )
    }
}

/// Check a path before writing, honouring an explicit confirmation.
pub fn check_write(path: &Path, confirmed: bool) -> Result<(), LiveWrite> {
    if confirmed {
        return Ok(());
    }
    match live_save_location(path) {
        Some(kind) => Err(LiveWrite { path: path.to_path_buf(), kind }),
        None => Ok(()),
    }
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
        assert!(check_write(&save, false).is_ok());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unconfirmed_writes_are_refused_with_a_useful_hint() {
        let refusal = check_write(&steam_save(), false).unwrap_err();
        assert_eq!(refusal.kind, LiveLocation::SteamProtonPrefix);
        let hint = refusal.hint();
        assert!(hint.contains("--confirm-live"), "{hint}");
        assert!(hint.contains("sandbox-test.sh"), "{hint}");
        assert!(check_write(&steam_save(), true).is_ok());
    }
}
