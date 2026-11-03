//! Mapping between on-disk object keys and readable game field names.
//!
//! No Man's Sky stores its save JSON with obfuscated keys: the save version
//! lives under `"F2P"`, the platform under `"8>q"`, and so on. The key is a
//! hash of the name, so the pairs can be derived from the game's executable —
//! and the ones real saves use are committed with this crate
//! (`data/baseline-mapping.json`), which is what [`KeyMap::builtin`] loads. A
//! community mapping file (distributed with MBINCompiler releases) is merged
//! on top when one is installed, and `nmsedit data refresh` regenerates the
//! full list from the player's own game.
//!
//! Design rule for this project: **the on-disk keys are the source of truth.**
//! Documents keep whatever key text the file used, and mapping is applied only
//! when looking a field up by name. Nothing is ever re-keyed on write, which
//! removes any chance of a lossy reverse mapping corrupting a save.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

/// A bidirectional key map.
#[derive(Debug, Clone, Default)]
pub struct KeyMap {
    /// On-disk key -> readable name.
    obfuscated_to_readable: HashMap<String, String>,
    /// Readable name -> on-disk key.
    readable_to_obfuscated: HashMap<String, String>,
}

/// Field pairs written by hand.
///
/// The generated baseline covers what the derivation can resolve; these cover
/// what it cannot. `Version` (`F2P`), `ExpeditionContext` (`2YS`),
/// `UsesThirdPersonCharacterCam` (`kPF`), `UsesThirdPersonShipCam` (`Wh<`),
/// `Preset` (`brJ`) and `EasiestUsedPreset` (`qAf`) collide with another
/// identifier in the executable, so the derivation drops them — but a save
/// must still show its version and summary fields on a machine with no game
/// installed.
///
/// The same is true of structural keys every real save needs read: `X` (`>Qh`)
/// and `MaxAmount` (`F9q`), without which every inventory comes out empty, the
/// nanite balance (`7QL`), and the learned lists (`eZ<`, `4kj`, `24<`). Each
/// pair is the community mapping's own, and each key is checked against the
/// values the saves hold. These stand alone if the embedded baseline is ever
/// unreadable.
const HAND_WRITTEN_PAIRS: &[(&str, &str)] = &[
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
    // Structural keys the derivation drops as ambiguous but that reading a
    // real save needs: slot cells and stack sizes, one balance, learned lists.
    (">Qh", "X"),
    ("F9q", "MaxAmount"),
    ("7QL", "Nanites"),
    ("eZ<", "KnownProducts"),
    ("4kj", "KnownTech"),
    ("24<", "KnownSpecials"),
];

/// The generated baseline, parsed once on first use.
///
/// The file is written by `tools/build-baseline-mapping.sh` from the game's own
/// field-name tables, scoped to the keys real saves use. It is a build-time
/// input, so a parse failure can only mean a broken build — degrade to the
/// hand-written pairs rather than panic in a user's session.
fn baseline() -> &'static KeyMap {
    static BASELINE: OnceLock<KeyMap> = OnceLock::new();
    BASELINE.get_or_init(|| KeyMap::from_json(include_str!("../data/baseline-mapping.json")).unwrap_or_default())
}

impl KeyMap {
    /// An empty map (only exact key matches will resolve).
    pub fn empty() -> KeyMap {
        KeyMap::default()
    }

    /// The mapping compiled into the editor.
    ///
    /// The committed baseline names the fields real saves use, so a machine
    /// with no game installed still shows names; the game installation only
    /// matters for *refreshing* the mapping, never for reading one. The
    /// hand-written pairs fill the gaps the derivation cannot resolve.
    pub fn builtin() -> KeyMap {
        let mut map = baseline().clone();
        for (obfuscated, readable) in HAND_WRITTEN_PAIRS {
            if map.readable_for(obfuscated).is_none() {
                map.insert(obfuscated, readable);
            }
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

    /// The committed baseline is what makes `builtin()` useful with no game
    /// installed; if it stops loading, users silently lose field names.
    #[test]
    fn the_embedded_baseline_is_loaded() {
        let map = KeyMap::builtin();
        assert!(map.len() >= 900, "the embedded baseline did not load: {} entries", map.len());
        assert_eq!(map.readable_for("Pk4"), Some("SaveName"), "a key the derivation resolves");
        assert_eq!(map.readable_for("F2P"), Some("Version"), "a key only the hand-written pairs resolve");
        // Structural keys the derivation drops as ambiguous: a real save cannot
        // be read without them (slots, cells, one balance, learned lists).
        assert_eq!(map.readable_for(">Qh"), Some("X"));
        assert_eq!(map.readable_for("F9q"), Some("MaxAmount"));
        assert_eq!(map.readable_for("7QL"), Some("Nanites"));
        assert_eq!(map.candidates("KnownProducts").last(), Some("eZ<"));
        assert_eq!(map.candidates("KnownTech").last(), Some("4kj"));
        assert_eq!(map.candidates("KnownSpecials").last(), Some("24<"));
    }

    #[test]
    fn the_hand_written_pairs_agree_with_the_baseline() {
        let baseline = KeyMap::from_json(include_str!("../data/baseline-mapping.json"))
            .expect("the committed baseline must parse");
        for (key, name) in HAND_WRITTEN_PAIRS {
            if let Some(generated) = baseline.readable_for(key) {
                assert_eq!(generated, *name, "the hand-written pair {key} disagrees with the baseline");
            }
        }
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
