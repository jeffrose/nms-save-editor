//! Deriving the save-key mapping from the player's own game files.
//!
//! Saves store field names obfuscated: the platform word is `"8>q"`, the save
//! name `"Pk4"`, the unit balance `"wGS"`. The readable names are *not* in the
//! save at all — they are in the game's executable, and the obfuscated key is a
//! pure function of the name:
//!
//! ```text
//! key(name) = three 21-bit slices of SpookyHash128(name, seed, seed),
//!             each reduced modulo 68 into the alphabet
//!             "0123456789:;<=>?@A-Za-z" (0x30..0x79 with the gap 0x5B..0x60 skipped)
//! ```
//!
//! with `seed = 8268756125562466087`. This was worked out from the community
//! generator's behaviour and then verified: **all 1,505 entries of the
//! published MBINCompiler mapping reproduce exactly** (see
//! `docs/findings-mapping.md`).
//!
//! So a mapping can be built from the installation the player already has:
//! read the field-name tables out of `NMS.exe`, hash each name, and write the
//! pairs. No game text is redistributed and no third-party file is needed.
//!
//! ## Never guess a name
//!
//! The executable contains far more identifiers than the save structures use,
//! and two different names can hash to the same key (the published mapping has
//! nine such pairs). A name is therefore only kept when its key is **unique**
//! among everything extracted; colliding keys are dropped and reported. An
//! unnamed field shows its id, which is worse than a name but better than a
//! wrong one.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::DataError;

/// The seed the game's save-key hash uses, both halves.
pub const KEY_SEED: u64 = 8_268_756_125_562_466_087;

/// The 68 characters a key is built from: `0`-`9`, `:`-`@`, `A`-`Z`, `a`-`y`.
///
/// Values 0..=67 are written as `'0' + value`, and anything above `'Z'` is
/// shifted up by six to skip the `[`..=`` ` `` gap. This is the same table,
/// spelled out.
const KEY_ALPHABET: &[u8; 68] = b"0123456789:;<=>?@ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxy";

/// The obfuscated key the game uses for a field name.
pub fn key_for_name(name: &str) -> String {
    let (hash, _) = spooky128(name.as_bytes(), KEY_SEED, KEY_SEED);
    let mut key = String::with_capacity(3);
    for shift in [0, 21, 42] {
        let value = ((hash >> shift) % 68) as usize;
        key.push(KEY_ALPHABET[value] as char);
    }
    key
}

/// What one generated mapping found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MappingReport {
    /// Identifiers read from the executable.
    pub names: usize,
    /// Keys that exactly one name mapped to.
    pub unique_keys: usize,
    /// Keys two or more names mapped to; both names are dropped.
    pub ambiguous_keys: usize,
    /// Keys added on top of an existing mapping.
    pub added: usize,
    /// Entries of an existing mapping the extraction reproduced with the same
    /// name.
    pub agreed: usize,
    /// Entries of an existing mapping the extraction would have named
    /// differently (the existing name wins).
    pub differed: usize,
}

/// A generated mapping: key to readable name, sorted by key.
#[derive(Debug, Clone, Default)]
pub struct Mapping {
    /// The pairs, sorted by key.
    pub pairs: BTreeMap<String, String>,
    /// What the extraction found.
    pub report: MappingReport,
}

impl Mapping {
    /// The pairs as JSON, in the shape [`nms_model::KeyMap`] reads.
    ///
    /// The `libMBIN_version` field is kept because that is the shape the
    /// community file has; `generator` says where this one came from.
    pub fn to_json(&self, game_root: &Path, generated_at: u64) -> serde_json::Value {
        self.json(game_root.display().to_string(), Some(generated_at))
    }

    /// The pairs as JSON without a timestamp or a machine path.
    ///
    /// This is the shape of the baseline committed to the repository: the same
    /// installation must produce the same bytes, so nothing machine-specific
    /// goes into the file.
    pub fn to_stable_json(&self, source: &str) -> serde_json::Value {
        self.json(source.to_string(), None)
    }

    fn json(&self, source: String, generated_at: Option<u64>) -> serde_json::Value {
        let mapping: Vec<serde_json::Value> = self
            .pairs
            .iter()
            .map(|(key, name)| serde_json::json!({ "Key": key, "Value": name }))
            .collect();
        let mut document = serde_json::json!({
            "libMBIN_version": "nmsedit",
            "generator": "nmsedit",
            "source": source,
            "names_read": self.report.names,
            "ambiguous_keys_dropped": self.report.ambiguous_keys,
            "Mapping": mapping,
        });
        if let Some(generated_at) = generated_at {
            document["generated_at"] = serde_json::json!(generated_at);
        }
        document
    }
}

/// Read `{"Mapping": [{"Key": ..., "Value": ...}]}`, the shape both the
/// community file and this generator write. A missing or unreadable file reads
/// as empty rather than failing: it is an input to a merge, not a requirement.
pub fn read_mapping_pairs(path: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(path) else { return BTreeMap::new() };
    let Ok(document) = serde_json::from_str::<serde_json::Value>(&text) else {
        return BTreeMap::new();
    };
    let Some(entries) = document.get("Mapping").and_then(|mapping| mapping.as_array()) else {
        return BTreeMap::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let key = entry.get("Key")?.as_str()?.to_string();
            let name = entry.get("Value")?.as_str()?.to_string();
            Some((key, name))
        })
        .collect()
}

/// Build the mapping from the installation and write it to `out`.
///
/// Whatever is already at `out` is read and merged in — its entries win — so
/// refreshing never discards a maintained mapping, only extends it.
pub fn write_mapping(install_root: &Path, out: &Path) -> Result<Mapping, DataError> {
    let existing = read_mapping_pairs(out);
    let mapping = build_mapping(install_root, &existing)?;
    write_json(out, &mapping.to_json(install_root, now_unix()))?;
    Ok(mapping)
}

/// Build a mapping covering only `keys` and write it to `out`.
///
/// This is how the baseline compiled into the editor is produced: every key a
/// set of real saves uses, resolved from the game's executable. Unlike
/// [`write_mapping`], nothing at `out` is read first — the file holds exactly
/// the scope asked for, and the same installation produces the same bytes.
pub fn write_mapping_for_keys(
    install_root: &Path,
    keys: &BTreeSet<String>,
    out: &Path,
) -> Result<Mapping, DataError> {
    let mapping = mapping_for_keys(install_root, keys)?;
    let source = "the field-name tables in the game's executable, limited to keys real saves use";
    write_json(out, &mapping.to_stable_json(source))?;
    Ok(mapping)
}

/// Write a mapping document where it belongs, creating the directory.
fn write_json(out: &Path, json: &serde_json::Value) -> Result<(), DataError> {
    let text = serde_json::to_string_pretty(json).map_err(|error| DataError::Json {
        message: error.to_string(),
    })?;
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|source| DataError::Io {
            context: "creating the mapping directory",
            source,
        })?;
    }
    // The trailing newline is for the committed baseline: it is a tracked text
    // file, and git prefers a newline-terminated one.
    std::fs::write(out, text + "\n").map_err(|source| DataError::Io {
        context: "writing the mapping",
        source,
    })?;
    Ok(())
}

/// Seconds since the Unix epoch, for the `generated_at` field.
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|age| age.as_secs())
        .unwrap_or(0)
}

/// Build a mapping from the field-name tables in the game's executable.
///
/// `existing` is merged in and wins: what a maintained mapping already says is
/// never overridden by an extraction, only extended with keys it lacks.
pub fn build_mapping(install_root: &Path, existing: &BTreeMap<String, String>) -> Result<Mapping, DataError> {
    build(install_root, existing, None)
}

/// Build a mapping covering only `keys`.
///
/// This is how the baseline that ships inside the editor is produced: every key
/// a set of real saves uses, resolved from the game's executable, so the file
/// stays around a thousand entries rather than sixty thousand.
pub fn mapping_for_keys(
    install_root: &Path,
    keys: &BTreeSet<String>,
) -> Result<Mapping, DataError> {
    build(install_root, &BTreeMap::new(), Some(keys))
}

/// The scan behind both: names out of the executable, one candidate per key.
fn build(
    install_root: &Path,
    existing: &BTreeMap<String, String>,
    scope: Option<&BTreeSet<String>>,
) -> Result<Mapping, DataError> {
    let exe = find_game_binary(install_root)?;
    let bytes = std::fs::read(&exe).map_err(|source| DataError::Io {
        context: "reading the game executable",
        source,
    })?;

    let names = extract_field_names(&bytes);
    let report_names = names.len();

    // Group by key: a key two names share cannot be resolved, so both are
    // dropped rather than one being picked.
    let mut by_key: HashMap<String, Option<&str>> = HashMap::new();
    for name in &names {
        let key = key_for_name(name);
        if scope.is_some_and(|scope| !scope.contains(&key)) {
            continue;
        }
        match by_key.get_mut(&key) {
            None => {
                by_key.insert(key, Some(name.as_str()));
            }
            Some(slot) => {
                if slot.as_deref() != Some(name.as_str()) {
                    *slot = None;
                }
            }
        }
    }

    let ambiguous = by_key.values().filter(|slot| slot.is_none()).count();
    let mut pairs: BTreeMap<String, String> = BTreeMap::new();
    for (key, name) in by_key {
        if let Some(name) = name {
            pairs.insert(key, name.to_string());
        }
    }
    let unique_keys = pairs.len();

    // How the extraction compares with what the file already says, before the
    // file's entries are merged over it.
    let mut agreed = 0usize;
    let mut differed = 0usize;
    for (key, name) in existing {
        match pairs.get(key) {
            Some(ours) if ours == name => agreed += 1,
            Some(_) => differed += 1,
            None => {}
        }
    }

    let mut added = 0usize;
    for (key, name) in existing {
        if !pairs.contains_key(key) {
            added += 1;
        }
        pairs.insert(key.clone(), name.clone());
    }

    Ok(Mapping {
        pairs,
        report: MappingReport {
            names: report_names,
            unique_keys,
            ambiguous_keys: ambiguous,
            added,
            agreed,
            differed,
        },
    })
}

/// Read the field-name tables out of a game executable.
///
/// The game stores its serialization field names as NUL-terminated identifiers,
/// so this is a scan for identifier-shaped strings: 1..=64 characters of
/// `[A-Za-z0-9_]`, starting with a letter or underscore, terminated by NUL.
/// One-character names are kept (`X`, `Y`, `Z`, `W`, `C`, `F` are all real).
/// That is deliberately loose — the cost of a name that is not a field name is
/// one extra mapping entry for a key no save uses, and the cost of a missing
/// one is a field shown as its id.
pub fn extract_field_names(bytes: &[u8]) -> Vec<String> {
    let mut names = Vec::new();
    let mut current = String::new();
    let mut overlong = false;
    for &byte in bytes {
        let identifier = byte.is_ascii_alphanumeric() || byte == b'_';
        if identifier {
            if current.len() < 64 {
                current.push(byte as char);
            } else {
                // A longer run is not a field name; do not truncate it into one.
                overlong = true;
            }
            continue;
        }
        // One-character names are real: `X`, `Y`, `Z` and `W` are the vector
        // components, `C` and `F` are stats. Dropping them would let a longer
        // name that happens to share their key win by default.
        if byte == 0 && !overlong && (1..=64).contains(&current.len()) {
            let starts_like_a_name = current.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_');
            // `__`-prefixed and all-digit runs are compiler noise, not fields.
            let looks_like_a_name = starts_like_a_name && current.chars().any(|c| c.is_ascii_alphabetic());
            if looks_like_a_name {
                names.push(current.clone());
            }
        }
        current.clear();
        overlong = false;
    }
    names.sort();
    names.dedup();
    names
}

/// The game's executable, in the layouts the platforms use.
pub fn find_game_binary(install_root: &Path) -> Result<PathBuf, DataError> {
    let candidates = [
        "Binaries/NMS.exe",
        "NMS.exe",
        // The native macOS build ships the binary inside the app bundle.
        "No Man's Sky.app/Contents/MacOS/No Man's Sky",
    ];
    for candidate in candidates {
        let path = install_root.join(candidate);
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(DataError::MissingBinary { root: install_root.display().to_string() })
}

// ---------------------------------------------------------------------------
// SpookyHash V2, 128-bit, short-message path
// ---------------------------------------------------------------------------

/// Bob Jenkins' SpookyHash V2 (public domain), 128-bit, for messages under 192
/// bytes.
///
/// Only the short path is needed — every field name is a few dozen bytes — and
/// only the path itself: the game hashes names with the one-shot function, not
/// the streaming state machine.
pub fn spooky128(message: &[u8], seed1: u64, seed2: u64) -> (u64, u64) {
    const CONST: u64 = 0xdead_beef_dead_beef;

    /// The reference's `ShortMix`, in its order: rotate one state word, add
    /// the next, fold into the third.
    fn short_mix(h: &mut [u64; 4]) {
        const STEPS: [(usize, usize, usize, u32); 12] = [
            (2, 3, 0, 50),
            (3, 0, 1, 52),
            (0, 1, 2, 30),
            (1, 2, 3, 41),
            (2, 3, 0, 54),
            (3, 0, 1, 48),
            (0, 1, 2, 38),
            (1, 2, 3, 37),
            (2, 3, 0, 62),
            (3, 0, 1, 34),
            (0, 1, 2, 5),
            (1, 2, 3, 36),
        ];
        for (rotated, added, target, rotation) in STEPS {
            h[rotated] = h[rotated].rotate_left(rotation);
            h[rotated] = h[rotated].wrapping_add(h[added]);
            h[target] ^= h[rotated];
        }
    }

    /// The reference's `ShortEnd`: eleven steps, each folding one word into
    /// another and rotating it.
    fn short_end(h: &mut [u64; 4]) {
        const STEPS: [(usize, usize, u32); 11] = [
            (3, 2, 15),
            (0, 3, 52),
            (1, 0, 26),
            (2, 1, 51),
            (3, 2, 28),
            (0, 3, 9),
            (1, 0, 47),
            (2, 1, 54),
            (3, 2, 32),
            (0, 3, 25),
            (1, 0, 63),
        ];
        for (target, rotated, rotation) in STEPS {
            h[target] ^= h[rotated];
            h[rotated] = h[rotated].rotate_left(rotation);
            h[target] = h[target].wrapping_add(h[rotated]);
        }
    }

    let length = message.len();
    let mut h = [seed1, seed2, CONST, CONST];
    let mut position = 0usize;
    let mut remainder = length % 32;

    if length > 15 {
        // Whole 32-byte groups: two words into c and d, mix, two more into a and b.
        for _ in 0..length / 32 {
            h[2] = h[2].wrapping_add(word(message, position));
            h[3] = h[3].wrapping_add(word(message, position + 8));
            short_mix(&mut h);
            h[0] = h[0].wrapping_add(word(message, position + 16));
            h[1] = h[1].wrapping_add(word(message, position + 24));
            position += 32;
        }
        // A 16..31-byte remainder gets mixed the same way before the tail.
        if remainder >= 16 {
            h[2] = h[2].wrapping_add(word(message, position));
            h[3] = h[3].wrapping_add(word(message, position + 8));
            short_mix(&mut h);
            position += 16;
            remainder -= 16;
        }
    }

    // The tail: 0..15 bytes, and the message length in the top byte of d. The
    // reference writes this as a switch that falls through; each group is the
    // same thing spelled out.
    let tail = &message[position..];
    h[3] = h[3].wrapping_add((length as u64) << 56);
    match remainder {
        12..=15 => {
            if remainder >= 15 {
                h[3] = h[3].wrapping_add((tail[14] as u64) << 48);
            }
            if remainder >= 14 {
                h[3] = h[3].wrapping_add((tail[13] as u64) << 40);
            }
            if remainder >= 13 {
                h[3] = h[3].wrapping_add((tail[12] as u64) << 32);
            }
            h[3] = h[3].wrapping_add(u32_at(tail, 8) as u64);
            h[2] = h[2].wrapping_add(word(tail, 0));
        }
        8..=11 => {
            if remainder >= 11 {
                h[3] = h[3].wrapping_add((tail[10] as u64) << 16);
            }
            if remainder >= 10 {
                h[3] = h[3].wrapping_add((tail[9] as u64) << 8);
            }
            if remainder >= 9 {
                h[3] = h[3].wrapping_add(tail[8] as u64);
            }
            h[2] = h[2].wrapping_add(word(tail, 0));
        }
        4..=7 => {
            if remainder >= 7 {
                h[2] = h[2].wrapping_add((tail[6] as u64) << 48);
            }
            if remainder >= 6 {
                h[2] = h[2].wrapping_add((tail[5] as u64) << 40);
            }
            if remainder >= 5 {
                h[2] = h[2].wrapping_add((tail[4] as u64) << 32);
            }
            h[2] = h[2].wrapping_add(u32_at(tail, 0) as u64);
        }
        1..=3 => {
            if remainder >= 3 {
                h[2] = h[2].wrapping_add((tail[2] as u64) << 16);
            }
            if remainder >= 2 {
                h[2] = h[2].wrapping_add((tail[1] as u64) << 8);
            }
            h[2] = h[2].wrapping_add(tail[0] as u64);
        }
        _ => {
            h[2] = h[2].wrapping_add(CONST);
            h[3] = h[3].wrapping_add(CONST);
        }
    }

    short_end(&mut h);
    (h[0], h[1])
}

/// Eight bytes little-endian; the caller guarantees `offset + 8 <= len`.
fn word(bytes: &[u8], offset: usize) -> u64 {
    let mut buffer = [0u8; 8];
    buffer.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(buffer)
}

/// Four bytes little-endian; the caller guarantees `offset + 4 <= len`.
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    let mut buffer = [0u8; 4];
    buffer.copy_from_slice(&bytes[offset..offset + 4]);
    u32::from_le_bytes(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Name-to-key pairs taken from the published mapping, which the game's own
    /// saves agree with. These are the format facts the derivation rests on.
    #[test]
    fn keys_match_the_published_mapping() {
        for (name, key) in [
            ("Version", "F2P"),
            ("Platform", "8>q"),
            ("ActiveContext", "XTp"),
            ("BaseContext", "vLc"),
            ("SaveName", "Pk4"),
            ("UsesThirdPersonCharacterCam", "kPF"),
            ("RevealBlackHoles", "tSD"),
            ("LastChangeTimestamp", "6X1"),
            ("GalacticAddresses", "r:j"),
            ("ProductionAccumulationCap", "T=R"),
            ("BuildingPreventionDisallowBuilding", "uLa"),
            ("ReportedAllianceArray", "kku"),
            ("X", ">Qh"),
            ("Y", "XJ>"),
        ] {
            assert_eq!(key_for_name(name), key, "{name}");
        }
    }

    #[test]
    fn the_hash_matches_the_reference_implementation() {
        // Values from the reference implementation (Bob Jenkins' SpookyHash V2,
        // public domain), run with the same seed over "ABC..." strings of each
        // length. The 16-byte case is the one that exercises `ShortMix`; the
        // shorter ones only exercise the tail.
        let message = |length: usize| -> Vec<u8> {
            (0..length).map(|index| b'A' + (index % 26) as u8).collect()
        };
        for (length, expected) in [
            (1usize, 0x1622_7f4b_fc59_8ad5u64),
            (8, 0xb742_5d5c_f243_ebcd),
            (15, 0x8f89_4d5a_1689_3332),
            (16, 0x59b5_cdec_9335_a78e),
            (17, 0x5f76_139f_30da_52ed),
            (32, 0xa3ef_e46e_e4a3_975a),
            (33, 0xff40_42c8_fd05_6ee1),
            (40, 0x0d3f_64ca_5909_ef0e),
        ] {
            let (hash, _) = spooky128(&message(length), KEY_SEED, KEY_SEED);
            assert_eq!(hash, expected, "{length} bytes");
        }
    }

    #[test]
    fn names_are_read_out_of_an_executable_like_buffer() {
        let mut buffer = Vec::new();
        // two padded field names, a type name, and noise that must not match
        buffer.extend_from_slice(b"ForceBroken\0\0\0\0\0");
        buffer.extend_from_slice(b"ForceReplaceStoryPortalSeed\0\0\0\0\0\0");
        buffer.extend_from_slice(b"cGcScanEventData\0");
        buffer.extend_from_slice(b"\x01\x02\xff");
        buffer.extend_from_slice(b"X\0");
        buffer.extend_from_slice(b"9\x00123\0_\0");
        buffer.extend_from_slice(&[b'A'; 80]);
        buffer.push(0);

        let names = extract_field_names(&buffer);
        assert!(names.contains(&"ForceBroken".to_string()));
        assert!(names.contains(&"ForceReplaceStoryPortalSeed".to_string()));
        assert!(names.contains(&"cGcScanEventData".to_string()));
        assert!(!names.contains(&"9".to_string()), "a digit run is not a field name");
        assert!(!names.contains(&"_".to_string()));
        assert!(names.contains(&"X".to_string()), "one-character names are kept");
        assert!(!names.iter().any(|name| name.len() == 80), "over-long runs are not names");
    }

    #[test]
    fn a_key_two_names_share_is_dropped_not_guessed() {
        // "DM" and "Aabb4BytesTree" really do collide on key `q9a`.
        assert_eq!(key_for_name("DM"), "q9a");
        assert_eq!(key_for_name("Aabb4BytesTree"), "q9a");

        let dir = std::env::temp_dir().join(format!("nms-data-mapping-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Binaries")).unwrap();
        let mut fake = Vec::new();
        fake.extend_from_slice(b"DM\0");
        fake.extend_from_slice(b"Aabb4BytesTree\0");
        fake.extend_from_slice(b"SaveName\0Pk4\0Version\0F2P\0X\0");
        std::fs::write(dir.join("Binaries/NMS.exe"), &fake).unwrap();

        let mapping = build_mapping(&dir, &BTreeMap::new()).expect("build");
        assert!(!mapping.pairs.contains_key("q9a"), "an ambiguous key is dropped");
        assert_eq!(mapping.pairs.get("Pk4").map(String::as_str), Some("SaveName"));
        assert_eq!(mapping.pairs.get("F2P").map(String::as_str), Some("Version"));
        assert!(mapping.report.ambiguous_keys >= 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_existing_mapping_is_extended_never_overridden() {
        let dir = std::env::temp_dir().join(format!("nms-data-mapping-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Binaries")).unwrap();
        std::fs::write(dir.join("Binaries/NMS.exe"), b"SaveName\0Pk4\0").unwrap();

        // The maintained file says something different for the same key.
        let existing = BTreeMap::from([("Pk4".to_string(), "TheMaintainedName".to_string())]);
        let mapping = build_mapping(&dir, &existing).expect("build");
        assert_eq!(mapping.pairs.get("Pk4").map(String::as_str), Some("TheMaintainedName"));
        assert_eq!(mapping.report.added, 0, "nothing to add: the key was already there");
        assert_eq!(mapping.report.differed, 1, "the extraction would have said something else");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writing_a_mapping_merges_what_is_already_there() {
        let dir = std::env::temp_dir().join(format!("nms-data-mapping-write-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Binaries")).unwrap();
        std::fs::write(dir.join("Binaries/NMS.exe"), b"SaveName\0Pk4\0Version\0F2P\0").unwrap();
        let out = dir.join("mapping.json");
        std::fs::write(
            &out,
            r#"{"Mapping":[{"Key":"Pk4","Value":"TheMaintainedName"},{"Key":"zzz","Value":"Kept"}]}"#,
        )
        .unwrap();

        let mapping = write_mapping(&dir, &out).expect("write");
        assert_eq!(mapping.pairs.get("Pk4").map(String::as_str), Some("TheMaintainedName"));
        assert_eq!(mapping.pairs.get("F2P").map(String::as_str), Some("Version"));

        // What was written reads back, and still holds both.
        let reread = read_mapping_pairs(&out);
        assert_eq!(reread.get("Pk4").map(String::as_str), Some("TheMaintainedName"));
        assert_eq!(reread.get("F2P").map(String::as_str), Some("Version"));
        assert_eq!(reread.get("zzz").map(String::as_str), Some("Kept"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_scoped_mapping_holds_only_the_scope_and_is_stable() {
        let dir = std::env::temp_dir().join(format!("nms-data-mapping-scope-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Binaries")).unwrap();
        // `Pk4` resolves; `q9a` is shared by two names and must be dropped even
        // though the scope asks for it; `>Qh` is out of scope.
        std::fs::write(
            dir.join("Binaries/NMS.exe"),
            b"SaveName\0Pk4\0DM\0Aabb4BytesTree\0X\0>Qh\0",
        )
        .unwrap();
        let scope = BTreeSet::from(["Pk4".to_string(), "q9a".to_string(), "zzz".to_string()]);
        let out = dir.join("baseline.json");

        let mapping = write_mapping_for_keys(&dir, &scope, &out).expect("write");
        assert_eq!(mapping.pairs.get("Pk4").map(String::as_str), Some("SaveName"));
        assert!(!mapping.pairs.contains_key("q9a"), "an ambiguous in-scope key is dropped");
        assert!(!mapping.pairs.contains_key("zzz"), "a key with no candidate is dropped");
        assert!(!mapping.pairs.contains_key(">Qh"), "out-of-scope keys are not written");
        assert_eq!(mapping.report.unique_keys, 1);
        assert_eq!(mapping.report.ambiguous_keys, 1);

        // Nothing machine-specific goes in the file, so writing it again from
        // the same executable produces identical bytes.
        let first = std::fs::read_to_string(&out).unwrap();
        write_mapping_for_keys(&dir, &scope, &out).expect("rewrite");
        assert_eq!(first, std::fs::read_to_string(&out).unwrap(), "the baseline must be byte-stable");
        assert!(!first.contains("generated_at"), "a committed baseline carries no timestamp");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_binary_is_reported_not_guessed() {
        let dir = std::env::temp_dir().join("nms-data-mapping-missing");
        assert!(find_game_binary(&dir).is_err());
    }
}
