//! Mapping between on-disk object keys and readable game field names.
//!
//! No Man's Sky stores its save JSON with obfuscated keys: the save version
//! lives under `"F2P"`, the platform under `"8>q"`, and so on. Two different
//! key spellings are in the wild, and the community maintains a mapping file
//! (distributed with MBINCompiler releases) that lists each pair.
//!
//! Design rule for this project: **the on-disk keys are the source of truth.**
//! Documents keep whatever key text the file used, and mapping is applied only
//! when looking a field up by name. Nothing is ever re-keyed on write, which
//! removes any chance of a lossy reverse mapping corrupting a save.

use std::collections::HashMap;
use std::path::Path;

/// A bidirectional key map.
#[derive(Debug, Clone, Default)]
pub struct KeyMap {
    /// On-disk key -> readable name.
    obfuscated_to_readable: HashMap<String, String>,
    /// Readable name -> on-disk key.
    readable_to_obfuscated: HashMap<String, String>,
}

impl KeyMap {
    /// An empty map (only exact key matches will resolve).
    pub fn empty() -> KeyMap {
        KeyMap::default()
    }

    /// The small set of keys this crate needs for save summaries.
    ///
    /// This is deliberately tiny: the full ~1300-entry mapping is game data
    /// that belongs in a generated data pack (see the data pipeline milestone),
    /// not hand-copied into source.
    pub fn builtin() -> KeyMap {
        const PAIRS: &[(&str, &str)] = &[
            // Root fields.
            ("F2P", "Version"),
            ("8>q", "Platform"),
            ("XTp", "ActiveContext"),
            ("<h0", "CommonStateData"),
            ("vLc", "BaseContext"),
            ("2YS", "ExpeditionContext"),
            ("fDu", "DiscoveryManagerData"),
            // Common state.
            ("Pk4", "SaveName"),
            ("Lg8", "TotalPlayTime"),
            ("kPF", "UsesThirdPersonCharacterCam"),
            ("wb:", "UsesThirdPersonVehicleCam"),
            ("Wh<", "UsesThirdPersonShipCam"),
            // Player state, reached through a context (BaseContext or ExpeditionContext).
            ("6f=", "PlayerStateData"),
            ("rnc", "SpawnStateData"),
            ("n:R", "SaveSummary"),
            ("LyC", "DifficultyState"),
            ("brJ", "Preset"),
            ("qAf", "EasiestUsedPreset"),
            ("4I:", "HardestUsedPreset"),
            ("7ND", "DifficultyPresetType"),
        ];
        let mut map = KeyMap::default();
        for (obfuscated, readable) in PAIRS {
            map.insert(obfuscated, readable);
        }
        map
    }

    fn insert(&mut self, obfuscated: &str, readable: &str) {
        self.obfuscated_to_readable.insert(obfuscated.to_string(), readable.to_string());
        // First writer wins, matching the community mapping's precedence, so a
        // later duplicate cannot silently change an existing lookup.
        self.readable_to_obfuscated.entry(readable.to_string()).or_insert_with(|| obfuscated.to_string());
    }

    /// Load a community mapping file (`{"libMBIN_version": .., "Mapping": [{"Key": .., "Value": ..}]}`).
    pub fn from_json(text: &str) -> Result<KeyMap, serde_json::Error> {
        #[derive(serde::Deserialize)]
        struct Entry {
            #[serde(rename = "Key")]
            key: String,
            #[serde(rename = "Value")]
            value: String,
        }
        #[derive(serde::Deserialize)]
        struct File {
            #[serde(rename = "Mapping")]
            mapping: Vec<Entry>,
        }

        let file: File = serde_json::from_str(text)?;
        let mut map = KeyMap::default();
        for entry in file.mapping {
            map.insert(&entry.key, &entry.value);
        }
        Ok(map)
    }

    /// Load a mapping file from disk.
    pub fn load(path: &Path) -> Result<KeyMap, Box<dyn std::error::Error>> {
        let text = std::fs::read_to_string(path)?;
        Ok(KeyMap::from_json(&text)?)
    }

    /// Merged view: `builtin` first, then `other` filling in missing entries.
    pub fn extended_with(&self, other: &KeyMap) -> KeyMap {
        let mut merged = self.clone();
        for (key, value) in &other.obfuscated_to_readable {
            merged.insert(key, value);
        }
        merged
    }

    /// Number of mapping entries.
    pub fn len(&self) -> usize {
        self.obfuscated_to_readable.len()
    }

    /// True when there are no entries.
    pub fn is_empty(&self) -> bool {
        self.obfuscated_to_readable.is_empty()
    }

    /// Readable name for an on-disk key, if known.
    pub fn readable_for(&self, obfuscated: &str) -> Option<&str> {
        self.obfuscated_to_readable.get(obfuscated).map(String::as_str)
    }

    /// Candidate on-disk keys for a readable name, in lookup order.
    ///
    /// A document may already use readable keys (some exports and older saves
    /// do), so the readable name itself is always a candidate first.
    pub fn candidates<'a>(&'a self, readable: &'a str) -> impl Iterator<Item = &'a str> {
        std::iter::once(readable).chain(self.readable_to_obfuscated.get(readable).map(String::as_str))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_resolves_core_fields() {
        let map = KeyMap::builtin();
        assert_eq!(map.readable_for("F2P"), Some("Version"));
        assert_eq!(map.readable_for("Lg8"), Some("TotalPlayTime"));
        let candidates: Vec<_> = map.candidates("Version").collect();
        assert_eq!(candidates, vec!["Version", "F2P"]);
    }

    #[test]
    fn parses_community_mapping_file() {
        let json = r#"{"libMBIN_version":"1.0","Mapping":[{"Key":"F2P","Value":"Version"},{"Key":"zzz","Value":"Custom"}]}"#;
        let map = KeyMap::from_json(json).unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map.readable_for("zzz"), Some("Custom"));
        assert_eq!(map.candidates("Custom").last(), Some("zzz"));
    }

    #[test]
    fn first_duplicate_wins() {
        let json = r#"{"Mapping":[{"Key":"A","Value":"Same"},{"Key":"B","Value":"Same"}]}"#;
        let map = KeyMap::from_json(json).unwrap();
        assert_eq!(map.candidates("Same").last(), Some("A"));
    }
}
