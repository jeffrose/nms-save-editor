//! The editor's data pack: game text regenerated from a local installation.
//!
//! The older editor's most common complaint was that a game update left it with
//! thousands of "name mapping not found" warnings until someone hand-edited a
//! data file. Nothing here is hand-edited: `refresh` walks the game's own
//! archives, parses every localisation table for the chosen language, and writes
//! a pack that ships with the version it came from.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::install::Install;
use crate::items::{ItemIndex, TableLayout, extract_table};
use crate::{DataError, language};

/// Pack format version. Bump when the layout changes incompatibly.
pub const PACK_FORMAT: u32 = 2;

/// What a build did, for reporting.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuildReport {
    /// Archives that contained tables, with counts.
    pub sources: Vec<SourceReport>,
    /// Archives that could not be read, with the reason. A texture or mesh
    /// archive has no text in it, so failing to read one is not fatal: the pack
    /// is built from what could be read.
    pub skipped: Vec<String>,
    /// How the item index was built, one entry per table.
    pub items: crate::items::ItemIndexReport,
}

/// Per-archive parse result, for reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceReport {
    /// Archive the tables came from.
    pub pak: String,
    /// Table files read from it.
    pub tables: usize,
    /// Entries parsed.
    pub entries: usize,
    /// True when every table in the archive was consumed exactly to its end.
    pub exact: bool,
}

/// The regenerated game text.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DataPack {
    /// Pack format.
    pub format: u32,
    /// Language the tables were read for (`english`, `french`, ...).
    pub language: String,
    /// Build identifier from the installation, when known.
    pub game_version: Option<String>,
    /// When the pack was generated (Unix seconds).
    pub generated_at: u64,
    /// Documentation of where the data came from.
    pub notes: String,
    /// `id -> text`, merged across every table read.
    pub strings: BTreeMap<String, String>,
    /// `item id -> display name`, joined from the item tables and the text.
    #[serde(default)]
    pub items: BTreeMap<String, String>,
    /// `item id -> category` (`Product`, `Substance`, `Technology`), for the ids
    /// that came from an item table. The save records the same distinction in
    /// its inventory slots, so this is cross-checkable.
    #[serde(default)]
    pub categories: BTreeMap<String, String>,
    /// `item id -> description`, where the game names one. Absent for the
    /// items it does not describe, and empty in packs generated before this
    /// field existed (run `data refresh` to fill it).
    #[serde(default)]
    pub descriptions: BTreeMap<String, String>,
}

impl DataPack {
    /// Build a pack by reading every localisation table for `language` out of
    /// the installation's archives.
    pub fn build(
        install: &Install,
        language_code: &str,
        timestamp: u64,
    ) -> Result<(DataPack, BuildReport), DataError> {
        let needle = format!("language/");
        let suffix = format!("_{language_code}.mbin");
        let mut strings: BTreeMap<String, String> = BTreeMap::new();
        let mut report = BuildReport::default();
        let mut tables_read = 0usize;

        for pak_path in install.paks()? {
            let pak = match nms_pak::Pak::open(&pak_path) {
                Ok(pak) => pak,
                Err(error) => {
                    report.skipped.push(format!("{}: {error}", pak_path.display()));
                    continue;
                }
            };

            let mut exact = true;
            let mut count = 0usize;

            // Text tables are the ones named `language/..._<code>.mbin`; the same
            // language is also present as `nms_update*` tables, and they all merge.
            let mut names: Vec<String> = pak
                .entries()
                .iter()
                .filter(|entry| entry.name.starts_with(&needle) && entry.name.ends_with(&suffix))
                .map(|entry| entry.name.clone())
                .collect();
            names.sort();

            for name in &names {
                let bytes = match pak.extract_name(name) {
                    Ok(bytes) => bytes,
                    Err(error) => {
                        report.skipped.push(format!("{name}: {error}"));
                        continue;
                    }
                };
                let table = language::parse(&bytes)?;
                exact &= table.is_exact();
                count += table.stats.parsed;
                tables_read += 1;
                // Later tables override earlier ones, matching how the game
                // applies its update tables on top of the base ones.
                for (id, text) in table.entries {
                    strings.insert(id, text);
                }
            }

            if count > 0 {
                report.sources.push(SourceReport {
                    pak: pak_path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    tables: names.len(),
                    entries: count,
                    exact,
                });
            }
        }

        if tables_read == 0 {
            return Err(DataError::NoTables { language: language_code.to_string() });
        }

        // Item names: each item table's own keys, joined with the text, plus
        // every `<id>_NAME` key for the ids no table holds.
        let extractions: Vec<_> = TableLayout::ALL
            .iter()
            .filter_map(|layout| {
                let table = read_item_table(install, layout.name, &mut report)?;
                Some(extract_table(&table, layout, &strings))
            })
            .collect();
        let index = ItemIndex::build(&extractions, &strings);
        report.items = index.report.clone();

        let pack = DataPack {
            format: PACK_FORMAT,
            language: language_code.to_string(),
            game_version: None,
            generated_at: timestamp,
            notes: "Regenerated from a local No Man's Sky installation. No game text is distributed with this repository."
                .to_string(),
            strings,
            items: index.iter().map(|(id, text)| (id.clone(), text.clone())).collect(),
            categories: index
                .iter()
                .filter_map(|(id, _)| index.category(id).map(|category| (id.clone(), category.label().to_string())))
                .collect(),
            descriptions: index.descriptions().clone(),
        };
        Ok((pack, report))
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.strings.len()
    }

    /// True when the pack has no entries.
    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }

    /// Look up a string by id.
    pub fn lookup(&self, id: &str) -> Option<&str> {
        self.strings.get(id).map(String::as_str)
    }

    /// Look up an item's display name by id, accepting the save's `^` and `!`
    /// prefixes.
    pub fn item_name(&self, id: &str) -> Option<String> {
        self.item_name_with_source(id).map(|(text, _)| text)
    }

    /// Look up an item's description by id, where the game names one.
    pub fn item_description(&self, id: &str) -> Option<&str> {
        self.descriptions.get(id.trim_start_matches(['^', '!'])).map(String::as_str)
    }

    /// Look up an item's display name together with where it came from.
    pub fn item_name_with_source(&self, id: &str) -> Option<(String, crate::items::NameSource)> {
        let bare = id.trim_start_matches(['^', '!']);
        if let Some(text) = self.items.get(bare) {
            return Some((text.clone(), crate::items::NameSource::Table));
        }
        crate::items::ItemIndex::default().resolve(id, &self.strings)
    }

    /// Number of named items.
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Every item id and its display name.
    pub fn items(&self) -> impl Iterator<Item = (&str, &str)> {
        self.items.iter().map(|(id, name)| (id.as_str(), name.as_str()))
    }

    /// Items whose name or id matches `query`, best first, at most `limit`.
    ///
    /// Names are how people and agents describe an item ("nanites"), ids the
    /// precise way ("NANITE"); the ranking prefers an exact name, then a name
    /// prefix, then a name that contains the text, then an id that does.
    pub fn search_items(&self, query: &str, limit: usize) -> Vec<(&str, &str)> {
        let needle = query.trim().to_ascii_lowercase();
        if needle.is_empty() || limit == 0 {
            return Vec::new();
        }
        let mut hits: Vec<(u8, &str, &str)> = Vec::new();
        for (id, name) in self.items() {
            let lower_name = name.to_ascii_lowercase();
            let rank = if lower_name == needle {
                0
            } else if lower_name.starts_with(&needle) {
                1
            } else if lower_name.contains(&needle) {
                2
            } else if id.to_ascii_lowercase().contains(&needle) {
                3
            } else {
                continue;
            };
            hits.push((rank, id, name));
        }
        hits.sort_by(|a, b| (a.0, a.2).cmp(&(b.0, b.2)));
        hits.truncate(limit);
        hits.into_iter().map(|(_, id, name)| (id, name)).collect()
    }

    /// The category an item belongs to, when it came from an item table.
    pub fn item_category(&self, id: &str) -> Option<&str> {
        self.categories.get(id.trim_start_matches(['^', '!'])).map(String::as_str)
    }

    /// Write the pack as JSON.
    pub fn save(&self, path: &Path) -> Result<(), DataError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|source| DataError::Io { context: "creating the data directory", source })?;
        }
        let text = serde_json::to_string(self).map_err(|source| DataError::Json { message: source.to_string() })?;
        nms_editor_atomic_write(path, text.as_bytes())
    }

    /// Read a pack from JSON.
    pub fn load(path: &Path) -> Result<DataPack, DataError> {
        let text = std::fs::read_to_string(path)
            .map_err(|source| DataError::Io { context: "reading the data pack", source })?;
        let pack: DataPack =
            serde_json::from_str(&text).map_err(|source| DataError::Json { message: source.to_string() })?;
        if pack.format != PACK_FORMAT {
            return Err(DataError::Json {
                message: format!("data pack format {} is not supported (expected {PACK_FORMAT})", pack.format),
            });
        }
        Ok(pack)
    }

    /// The default location for a generated pack.
    pub fn default_path() -> Option<PathBuf> {
        let base = match std::env::var("XDG_CONFIG_HOME") {
            Ok(value) if !value.is_empty() => PathBuf::from(value),
            _ => PathBuf::from(std::env::var("HOME").ok()?).join(".config"),
        };
        Some(base.join("nmsedit").join("strings.json"))
    }
}

/// Read one of the item tables out of the archives, if present.
///
/// A missing table is not fatal: the pack falls back to derived names.
fn read_item_table(install: &Install, file_name: &str, report: &mut BuildReport) -> Option<Vec<u8>> {
    let Ok(paks) = install.paks() else { return None };
    for pak_path in paks {
        let Ok(pak) = nms_pak::Pak::open(&pak_path) else { continue };
        let Some(entry) = pak
            .entries()
            .iter()
            .find(|entry| entry.name.ends_with(file_name))
        else {
            continue;
        };
        match pak.extract(entry) {
            Ok(bytes) => return Some(bytes),
            Err(error) => report.skipped.push(format!("{file_name}: {error}")),
        }
    }
    None
}

/// Round-trip JSON through the same atomic replace used for saves.
fn nms_editor_atomic_write(path: &Path, bytes: &[u8]) -> Result<(), DataError> {
    nms_format_write(path, bytes)
}

fn nms_format_write(path: &Path, bytes: &[u8]) -> Result<(), DataError> {
    // `nms-format` owns atomic writes; re-exporting it here keeps one
    // implementation of "write a file without ever leaving it half-written".
    nms_format::write_atomic(path, bytes).map_err(|source| DataError::Io {
        context: "writing the data pack",
        source: std::io::Error::other(source.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_round_trips_through_json() {
        let dir = std::env::temp_dir().join(format!("nms-data-pack-{}", std::process::id()));
        let path = dir.join("strings.json");
        let _ = std::fs::remove_dir_all(&dir);

        let mut pack = DataPack {
            format: PACK_FORMAT,
            language: "english".into(),
            game_version: Some("7.11".into()),
            generated_at: 1_793_488_365,
            notes: "test".into(),
            strings: BTreeMap::new(),
            items: BTreeMap::new(),
            categories: BTreeMap::new(),
            descriptions: BTreeMap::new(),
        };
        pack.strings.insert("UI_VR_RECENTRE".into(), "Recentre VR View".into());
        pack.strings.insert("UI_EMPTY".into(), String::new());

        pack.save(&path).unwrap();
        let reloaded = DataPack::load(&path).unwrap();
        assert_eq!(reloaded.len(), 2);
        assert_eq!(reloaded.lookup("UI_VR_RECENTRE"), Some("Recentre VR View"));
        assert_eq!(reloaded.lookup("missing"), None);
        assert_eq!(reloaded.game_version.as_deref(), Some("7.11"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn item_search_ranks_names_over_ids() {
        let mut pack = DataPack {
            format: PACK_FORMAT,
            language: "english".into(),
            game_version: None,
            generated_at: 0,
            notes: "test".into(),
            strings: BTreeMap::new(),
            items: BTreeMap::new(),
            categories: BTreeMap::new(),
            descriptions: BTreeMap::new(),
        };
        pack.items.insert("NANITE".into(), "Nanites".into());
        pack.items.insert("NANITE_CRYSTAL".into(), "Nanite Crystal".into());
        pack.items.insert("STELLAR_NANITE".into(), "Stellar Nanites".into());
        pack.items.insert("CARBON".into(), "Carbon".into());

        let hits = pack.search_items("nanite", 10);
        assert_eq!(
            hits.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            ["NANITE_CRYSTAL", "NANITE", "STELLAR_NANITE"],
            "prefix matches come first, then the shortest name, then contains"
        );
        assert_eq!(pack.search_items("CARBON", 10), [("CARBON", "Carbon")]);
        assert!(pack.search_items("nothing here", 10).is_empty());
        assert!(pack.search_items("nanite", 0).is_empty(), "a zero limit returns nothing");
        assert_eq!(pack.search_items("nanite", 1).len(), 1);
    }

    #[test]
    fn rejects_a_pack_from_the_future() {
        let dir = std::env::temp_dir().join(format!("nms-data-format-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("strings.json");
        std::fs::write(&path, br#"{"format":99,"language":"english","generated_at":0,"notes":"","strings":{}}"#).unwrap();
        assert!(DataPack::load(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
