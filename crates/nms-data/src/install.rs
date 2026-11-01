//! Locating a No Man's Sky installation on disk.
//!
//! The editor's data pack is regenerated from the player's own copy of the game,
//! so the pipeline has to find it. This module is deliberately conservative: an
//! explicit path always wins, and automatic discovery only looks at the standard
//! Steam locations. It never writes to the installation.

use std::path::{Path, PathBuf};

use crate::DataError;

/// Where the game's data archives live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    /// The `No Man's Sky` directory.
    pub root: PathBuf,
    /// `GAMEDATA/PCBANKS`, holding the `.pak` archives.
    pub pak_dir: PathBuf,
}

impl Install {
    /// Open an installation given a path to it, to `GAMEDATA`, or to `PCBANKS`.
    pub fn discover(explicit: Option<&Path>) -> Result<Install, DataError> {
        let candidates: Vec<PathBuf> = match explicit {
            Some(path) => {
                let path = path.to_path_buf();
                vec![
                    path.clone(),
                    path.join("GAMEDATA/PCBANKS"),
                    path.join("PCBANKS"),
                    path.join("GAMEDATA"),
                ]
            }
            None => steam_candidates(),
        };

        for candidate in &candidates {
            if let Some(install) = normalise(candidate) {
                return Ok(install);
            }
        }

        Err(DataError::InstallNotFound {
            searched: candidates.iter().map(|p| p.display().to_string()).collect(),
        })
    }

    /// The `.pak` archives, sorted by name so the merge order is stable.
    pub fn paks(&self) -> Result<Vec<PathBuf>, DataError> {
        let mut paks: Vec<PathBuf> = std::fs::read_dir(&self.pak_dir)
            .map_err(|source| DataError::Io { context: "listing PCBANKS", source })?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| path.extension().map(|e| e == "pak").unwrap_or(false))
            .collect();
        paks.sort();
        if paks.is_empty() {
            return Err(DataError::NoArchives { dir: self.pak_dir.display().to_string() });
        }
        Ok(paks)
    }
}

/// Accept a directory that contains (or is) `GAMEDATA/PCBANKS`.
fn normalise(path: &Path) -> Option<Install> {
    let pak_dir = if path.ends_with("PCBANKS") {
        path.to_path_buf()
    } else if path.join("PCBANKS").is_dir() {
        path.join("PCBANKS")
    } else if path.join("GAMEDATA/PCBANKS").is_dir() {
        path.join("GAMEDATA/PCBANKS")
    } else {
        return None;
    };
    if !pak_dir.is_dir() {
        return None;
    }
    let root = pak_dir
        .parent()
        .and_then(|gamedata| gamedata.parent())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| pak_dir.clone());
    Some(Install { root, pak_dir })
}

/// Standard Steam installation roots for this machine.
fn steam_candidates() -> Vec<PathBuf> {
    let mut libraries = Vec::new();
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        let home = PathBuf::from(home);
        roots.push(home.join(".steam/debian-installation"));
        roots.push(home.join(".steam/steam"));
        roots.push(home.join(".local/share/Steam"));
        roots.push(home.join(".var/app/com.valvesoftware.Steam/data/Steam"));
    }
    if let Ok(value) = std::env::var("STEAM_ROOT") {
        roots.push(PathBuf::from(value));
    }

    for root in &roots {
        libraries.push(root.join("steamapps/common/No Man's Sky"));
        // libraryfolders.vdf lists extra libraries, one path per entry.
        let vdf = root.join("steamapps/libraryfolders.vdf");
        if let Ok(text) = std::fs::read_to_string(&vdf) {
            for line in text.lines() {
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix("\"path\"") {
                    let value = rest.trim().trim_matches('"').replace("\\\\", "/");
                    if !value.is_empty() {
                        libraries.push(PathBuf::from(value).join("steamapps/common/No Man's Sky"));
                    }
                }
            }
        }
    }
    libraries
}
