//! No Man's Sky storage slots and `mf_*.hg` metadata files.
//!
//! Metadata files are XXTEA-encrypted with a key derived from the storage
//! slot, and hold the sizes, play time, and version of their companion data
//! file. The game reads them to describe save slots, so editing a save without
//! refreshing its metadata leaves the game with stale information — a known
//! source of confusing in-game slot lists, and the exact failure mode reported
//! as "metadata size and cloud sync state differ from what the game syncs".
//!
//! Observed layouts (little-endian `u32` words after decryption):
//!
//! | word | meaning |
//! |------|---------|
//! | 0 | sentinel `0xEEEEEEBE` |
//! | 1 | format version (`0x7D0` vanilla, `0x7D1` foundation, `0x7D2`..`0x7D4` later) |
//! | 2..9 | digest blob (only seen non-zero in `mf_accountdata.hg`) |
//! | 14 | decompressed size, including the trailing NUL byte |
//! | 15 | compressed size (raw data-file size); `0` when the file is stored uncompressed |
//! | 16 | profile hash |
//! | 17 | save version offset: JSON `Version` minus 512 |
//! | 18 | constant `1` in every observed file |
//! | 19 | play time, low word (matches JSON `CommonStateData.TotalPlayTime`) |
//! | 20 | play time, high word |
//!
//! Unknown words are preserved verbatim when patching: the 48-byte blob in
//! `mf_accountdata.hg` is not yet identified, and the cipher is deterministic,
//! so keeping words we do not understand is both safe and byte-exact.

use crate::error::FormatError;
use crate::xxtea;

/// Sentinel that starts every decrypted metadata file.
pub const METADATA_MAGIC: u32 = 0xEEEE_EEBE;
/// Metadata format used by the original game release; not supported.
pub const FORMAT_VANILLA: u32 = 0x7D0;
/// Metadata format introduced with Foundation; carries SpookyHash + SHA-256.
pub const FORMAT_FOUNDATION: u32 = 0x7D1;

/// Known metadata file lengths and their round counts.
const LAYOUTS: [(usize, usize); 4] = [
    (0x68, 8),   // 104 bytes, vanilla
    (0x168, 6),  // 360 bytes
    (0x180, 6),  // 384 bytes, Worlds Part I
    (0x1B0, 6),  // 432 bytes, Worlds Part II and later
];

/// Round count for a metadata length, if the length is known.
pub fn rounds_for_length(length: usize) -> Option<usize> {
    LAYOUTS.iter().find(|(len, _)| *len == length).map(|(_, rounds)| *rounds)
}

/// A storage-file index. Slots 0 and 1 are account-level; 2..=31 are the 30
/// player-state files behind the 15 user-visible paired save slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Slot(u8);

impl Slot {
    /// `usersettings.hg` / `mf_usersettings.hg`.
    pub const USER_SETTINGS: Slot = Slot(0);
    /// `accountdata.hg` / `mf_accountdata.hg`.
    pub const ACCOUNT_DATA: Slot = Slot(1);

    /// The lowest player-state index.
    pub const FIRST_PLAYER: u8 = 1;
    /// The highest player-state index.
    pub const LAST_PLAYER: u8 = 30;

    /// Build a slot from a raw storage index (0..=31).
    pub const fn from_index(index: u8) -> Option<Slot> {
        if index <= 31 {
            Some(Slot(index))
        } else {
            None
        }
    }

    /// Player state `n`, where `n` is 1..=30 (`save.hg` is 1, `save2.hg` is 2...).
    ///
    /// # Panics
    /// Panics when `n` is outside 1..=30; use [`Slot::player_state_checked`] for
    /// untrusted input.
    pub const fn player_state(n: u8) -> Slot {
        assert!(n >= Self::FIRST_PLAYER && n <= Self::LAST_PLAYER, "player state index out of range");
        Slot(n + 1)
    }

    /// Checked variant of [`Slot::player_state`].
    pub const fn player_state_checked(n: u8) -> Option<Slot> {
        if n >= Self::FIRST_PLAYER && n <= Self::LAST_PLAYER {
            Some(Slot(n + 1))
        } else {
            None
        }
    }

    /// The raw storage index.
    pub const fn index(self) -> u8 {
        self.0
    }

    /// True for the two account-level files.
    pub const fn is_account(self) -> bool {
        self.0 <= 1
    }

    /// Player-state number (`saveN.hg` uses N), if this is a player slot.
    pub const fn player_number(self) -> Option<u8> {
        if self.0 >= 2 {
            Some(self.0 - 1)
        } else {
            None
        }
    }

    /// Map a data file name (`save.hg`, `save7.hg`, `accountdata.hg`,
    /// `usersettings.hg`) to its slot.
    pub fn from_file_name(name: &str) -> Option<Slot> {
        let stem = name.strip_suffix(".hg")?;
        match stem {
            "accountdata" => return Some(Slot::ACCOUNT_DATA),
            "usersettings" => return Some(Slot::USER_SETTINGS),
            _ => {}
        }
        let number = stem.strip_prefix("save")?;
        if number.is_empty() {
            return Some(Slot::player_state(1));
        }
        number.parse::<u8>().ok().and_then(Slot::player_state_checked)
    }

    /// Map a metadata file name (`mf_save.hg`, `mf_save7.hg`,
    /// `mf_accountdata.hg`) to its slot.
    pub fn from_metadata_file_name(name: &str) -> Option<Slot> {
        Slot::from_file_name(name.strip_prefix("mf_")?)
    }

    /// The metadata file name for this slot.
    pub fn metadata_file_name(self) -> String {
        match self {
            Slot::USER_SETTINGS => "mf_usersettings.hg".to_string(),
            Slot::ACCOUNT_DATA => "mf_accountdata.hg".to_string(),
            _ => match self.player_number() {
                Some(1) => "mf_save.hg".to_string(),
                Some(n) => format!("mf_save{n}.hg"),
                None => unreachable!("non-account slots always have a player number"),
            },
        }
    }

    /// The data file name for this slot.
    pub fn data_file_name(self) -> String {
        match self {
            Slot::USER_SETTINGS => "usersettings.hg".to_string(),
            Slot::ACCOUNT_DATA => "accountdata.hg".to_string(),
            _ => match self.player_number() {
                Some(1) => "save.hg".to_string(),
                Some(n) => format!("save{n}.hg"),
                None => unreachable!("non-account slots always have a player number"),
            },
        }
    }
}

impl std::fmt::Display for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Slot::USER_SETTINGS => write!(f, "UserSettings"),
            Slot::ACCOUNT_DATA => write!(f, "AccountData"),
            Slot(s) => write!(f, "PlayerState{}", s - 1),
        }
    }
}

/// Fields a writer may update. `None` leaves the existing value untouched.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetadataPatch {
    /// Decompressed payload size in bytes, including the trailing NUL.
    pub decompressed_size: Option<u32>,
    /// Raw data-file size in bytes; `Some(0)` marks an uncompressed file.
    pub compressed_size: Option<u32>,
    /// Save version word (JSON `Version` minus 512).
    pub version: Option<u32>,
    /// Total play time in seconds.
    pub play_time: Option<u64>,
    /// Player-chosen save name, mirrored from the save JSON.
    pub save_name: Option<String>,
    /// Location summary, mirrored from the save JSON.
    pub location: Option<String>,
    /// Game mode, mirrored from the save JSON.
    pub game_mode: Option<String>,
    /// Unix timestamp; the game stores the data file's modification time.
    pub timestamp: Option<u32>,
}

/// Word holding the Unix timestamp: the data file's modification time when the
/// save was written. The editor refreshes it on every write, so it reads as
/// "the state from then".
pub const TIMESTAMP_WORD: usize = 89;

/// A NUL-terminated text field inside a metadata file.
///
/// Boundaries are the observed extents: `SaveName` and `Location` were seen
/// spanning 24 and 36 bytes respectively, `GameMode` 8. The declared capacity is
/// slightly larger, and a write refuses to extend a string into a word that is
/// not empty, so an inaccurate guess can never clobber unknown data — it only
/// makes the writer more conservative about long values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StringField {
    /// Index of the first word of the field.
    pub start_word: usize,
    /// Field capacity in bytes, including the terminating NUL.
    pub max_bytes: usize,
    /// Name used in error messages.
    pub name: &'static str,
}

impl StringField {
    /// Player-chosen save name (JSON `CommonStateData.SaveName`).
    pub const SAVE_NAME: StringField = StringField { start_word: 22, max_bytes: 32, name: "SaveName" };
    /// Location summary (JSON `PlayerStateData.SaveSummary`).
    pub const LOCATION: StringField = StringField { start_word: 54, max_bytes: 40, name: "Location" };
    /// Game mode (JSON `DifficultyState.*.DifficultyPresetType`).
    pub const GAME_MODE: StringField = StringField { start_word: 91, max_bytes: 16, name: "GameMode" };

    /// Words the field occupies.
    fn word_count(&self) -> usize {
        self.max_bytes / 4
    }
}

/// Read a NUL-terminated text field from decrypted words.
pub fn read_string_field(words: &[u32], field: StringField) -> String {
    let mut bytes = Vec::new();
    for index in field.start_word..(field.start_word + field.word_count()).min(words.len()) {
        for byte in words[index].to_le_bytes() {
            if byte == 0 {
                return String::from_utf8_lossy(&bytes).into_owned();
            }
            bytes.push(byte);
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Write a NUL-terminated text field, refusing to overwrite unknown data.
///
/// Words that the *current* string does not occupy are only written when they
/// are zero, so extending a string into free space is allowed while extending
/// it into a word that holds something else fails with
/// [`FormatError::MetadataFieldOccupied`].
pub fn write_string_field(words: &mut [u32], field: StringField, value: &str) -> Result<(), FormatError> {
    let bytes = value.as_bytes();
    if bytes.len() + 1 > field.max_bytes {
        return Err(FormatError::MetadataStringTooLong {
            field: field.name,
            len: bytes.len(),
            limit: field.max_bytes,
        });
    }

    let current = read_string_field(words, field);
    let current_padded_words = if current.is_empty() { 0 } else { (current.len() + 1).div_ceil(4) };
    let needed_words = (bytes.len() + 1).div_ceil(4);
    let write_words = current_padded_words.max(needed_words);

    // Only the region beyond the current string is "new" territory: it must be
    // empty before we touch it.
    for offset in current_padded_words..write_words {
        let index = field.start_word + offset;
        if words[index] != 0 {
            return Err(FormatError::MetadataFieldOccupied {
                field: field.name,
                index,
                word: words[index],
            });
        }
    }

    let mut padded = vec![0u8; field.max_bytes];
    padded[..bytes.len()].copy_from_slice(bytes);
    for (offset, chunk) in padded.chunks_exact(4).enumerate() {
        let index = field.start_word + offset;
        if offset < write_words {
            words[index] = u32::from_le_bytes(chunk.try_into().unwrap());
        }
    }
    Ok(())
}

/// Decrypted metadata for one storage slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Metadata {
    /// Format word (see the module table).
    pub format_version: u32,
    /// Decompressed payload size including trailing NUL.
    pub decompressed_size: u32,
    /// Raw data-file size, or 0 when stored uncompressed.
    pub compressed_size: u32,
    /// Profile hash word.
    pub profile_hash: u32,
    /// Save version word.
    pub version: u32,
    /// Total play time in seconds.
    pub play_time: u64,
    /// Slot that successfully decrypted the file.
    pub slot: Slot,
    /// All decrypted words, preserved for round-tripping unknown fields.
    pub words: Vec<u32>,
}

impl Metadata {
    /// Player-chosen save name as stored in the metadata file.
    pub fn save_name(&self) -> String {
        read_string_field(&self.words, StringField::SAVE_NAME)
    }

    /// Location summary as stored in the metadata file.
    pub fn location(&self) -> String {
        read_string_field(&self.words, StringField::LOCATION)
    }

    /// Game mode as stored in the metadata file.
    pub fn game_mode(&self) -> String {
        read_string_field(&self.words, StringField::GAME_MODE)
    }

    /// The Unix timestamp the metadata carries, when the word holds one.
    pub fn timestamp(&self) -> Option<u64> {
        self.words.get(TIMESTAMP_WORD).copied().filter(|word| *word != 0).map(u64::from)
    }

    /// Read and decrypt metadata bytes, trying `expected` first and then other
    /// slots of the same kind (account or player).
    pub fn read(bytes: &[u8], expected: Slot) -> Result<Metadata, FormatError> {
        let rounds = rounds_for_length(bytes.len()).ok_or(FormatError::UnknownMetadataLength { length: bytes.len() })?;
        let cipher_words = xxtea::words_from_bytes(bytes);

        let mut candidates = vec![expected];
        for index in 0..=31u8 {
            let slot = Slot(index);
            if slot != expected && slot.is_account() == expected.is_account() {
                candidates.push(slot);
            }
        }

        for slot in candidates {
            let key = xxtea::derive_key(slot);
            let mut words = cipher_words.clone();
            xxtea::decrypt(&mut words, &key, rounds);
            if words[0] == METADATA_MAGIC {
                return Ok(Metadata::from_words(words, slot));
            }
        }
        Err(FormatError::MetadataDecrypt)
    }

    fn from_words(words: Vec<u32>, slot: Slot) -> Metadata {
        let get = |i: usize| words.get(i).copied().unwrap_or(0);
        Metadata {
            format_version: get(1),
            decompressed_size: get(14),
            compressed_size: get(15),
            profile_hash: get(16),
            version: get(17),
            play_time: u64::from(get(19)) | (u64::from(get(20)) << 32),
            slot,
            words,
        }
    }

    /// Re-encrypt with updated fields, preserving every other word verbatim.
    pub fn patch_to_bytes(&self, patch: &MetadataPatch) -> Result<Vec<u8>, FormatError> {
        let rounds = rounds_for_length(self.words.len() * 4)
            .ok_or(FormatError::UnknownMetadataLength { length: self.words.len() * 4 })?;
        if self.format_version == FORMAT_VANILLA {
            return Err(FormatError::UnsupportedMetadataFormat { version: self.format_version });
        }

        let mut words = self.words.clone();
        if let Some(v) = patch.decompressed_size {
            words[14] = v;
        }
        if let Some(v) = patch.compressed_size {
            words[15] = v;
        }
        if let Some(v) = patch.version {
            words[17] = v;
        }
        if let Some(v) = patch.play_time {
            words[19] = v as u32;
            words[20] = (v >> 32) as u32;
        }
        if let Some(v) = patch.timestamp {
            words[TIMESTAMP_WORD] = v;
        }
        if let Some(v) = &patch.save_name {
            write_string_field(&mut words, StringField::SAVE_NAME, v)?;
        }
        if let Some(v) = &patch.location {
            write_string_field(&mut words, StringField::LOCATION, v)?;
        }
        if let Some(v) = &patch.game_mode {
            write_string_field(&mut words, StringField::GAME_MODE, v)?;
        }

        let key = xxtea::derive_key(self.slot);
        xxtea::encrypt(&mut words, &key, rounds);
        Ok(xxtea::words_to_bytes(&words))
    }

    /// Encrypt these exact words, reproducing the source file byte for byte.
    pub fn to_bytes(&self) -> Result<Vec<u8>, FormatError> {
        self.patch_to_bytes(&MetadataPatch::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_metadata(slot: Slot, len: usize) -> Vec<u8> {
        let rounds = rounds_for_length(len).unwrap();
        let mut words = vec![0u32; len / 4];
        words[0] = METADATA_MAGIC;
        words[1] = 0x7D4;
        words[14] = 1234;
        words[15] = 5678;
        words[17] = 4231;
        words[19] = 3156876;
        words[100] = 0xDEAD_BEEF; // free space: must survive patching
        let key = xxtea::derive_key(slot);
        xxtea::encrypt(&mut words, &key, rounds);
        xxtea::words_to_bytes(&words)
    }

    #[test]
    fn reads_and_reencrypts_byte_exactly() {
        for slot in [Slot::ACCOUNT_DATA, Slot::player_state(1), Slot::player_state(30)] {
            let bytes = synthetic_metadata(slot, 432);
            let meta = Metadata::read(&bytes, slot).unwrap();
            assert_eq!(meta.slot, slot);
            assert_eq!(meta.decompressed_size, 1234);
            assert_eq!(meta.compressed_size, 5678);
            assert_eq!(meta.version, 4231);
            assert_eq!(meta.play_time, 3_156_876);
            assert_eq!(meta.to_bytes().unwrap(), bytes, "unchanged metadata must re-encrypt byte-exactly");
        }
    }

    #[test]
    fn patch_preserves_unknown_words() {
        let slot = Slot::player_state(7);
        let bytes = synthetic_metadata(slot, 432);
        let meta = Metadata::read(&bytes, slot).unwrap();
        let patched = meta
            .patch_to_bytes(&MetadataPatch {
                decompressed_size: Some(99),
                compressed_size: Some(42),
                version: Some(4200),
                play_time: Some(7_000_000),
                ..Default::default()
            })
            .unwrap();
        let after = Metadata::read(&patched, slot).unwrap();
        assert_eq!(after.decompressed_size, 99);
        assert_eq!(after.compressed_size, 42);
        assert_eq!(after.version, 4200);
        assert_eq!(after.play_time, 7_000_000);
        assert_eq!(after.words[100], 0xDEAD_BEEF, "unknown fields must be preserved");
    }

    #[test]
    fn mirrors_summary_strings_and_timestamp() {
        let slot = Slot::player_state(4);
        let bytes = synthetic_metadata(slot, 432);
        let meta = Metadata::read(&bytes, slot).unwrap();
        assert_eq!(meta.save_name(), "", "synthetic metadata starts with empty strings");

        let patched = meta
            .patch_to_bytes(&MetadataPatch {
                save_name: Some("Utopia".into()),
                location: Some("Aboard Dalaking Exchange".into()),
                game_mode: Some("Normal".into()),
                timestamp: Some(1_793_416_365),
                ..Default::default()
            })
            .unwrap();
        let after = Metadata::read(&patched, slot).unwrap();
        assert_eq!(after.save_name(), "Utopia");
        assert_eq!(after.location(), "Aboard Dalaking Exchange");
        assert_eq!(after.game_mode(), "Normal");
        assert_eq!(after.words[89], 1_793_416_365);
        // Untouched words survive.
        assert_eq!(after.words[100], 0xDEAD_BEEF);
    }

    #[test]
    fn shortening_a_string_clears_the_tail() {
        let slot = Slot::player_state(4);
        let meta = Metadata::read(&synthetic_metadata(slot, 432), slot).unwrap();
        let long = meta
            .patch_to_bytes(&MetadataPatch { save_name: Some("Our Journey Continues".into()), ..Default::default() })
            .unwrap();
        let mixed = Metadata::read(&long, slot).unwrap();
        assert_eq!(mixed.save_name(), "Our Journey Continues");

        let short = mixed
            .patch_to_bytes(&MetadataPatch { save_name: Some("Short".into()), ..Default::default() })
            .unwrap();
        let after = Metadata::read(&short, slot).unwrap();
        assert_eq!(after.save_name(), "Short", "the previous, longer name must not leak through");
        let field_words = &after.words[StringField::SAVE_NAME.start_word..StringField::SAVE_NAME.start_word + 8];
        assert!(
            field_words[2..].iter().all(|word| *word == 0),
            "words beyond the new value must be cleared, got {field_words:?}"
        );
    }

    #[test]
    fn refuses_overlong_strings() {
        let slot = Slot::player_state(4);
        let meta = Metadata::read(&synthetic_metadata(slot, 432), slot).unwrap();
        let too_long = "x".repeat(StringField::SAVE_NAME.max_bytes);
        let err = meta
            .patch_to_bytes(&MetadataPatch { save_name: Some(too_long), ..Default::default() })
            .unwrap_err();
        assert!(matches!(err, FormatError::MetadataStringTooLong { field: "SaveName", .. }), "got {err:?}");
    }

    #[test]
    fn refuses_to_extend_a_string_into_occupied_words() {
        let slot = Slot::player_state(4);
        // Words 22.. are the SaveName field; word 23 holds something unknown.
        let mut words: Vec<u32> = vec![0; 108];
        words[0] = METADATA_MAGIC;
        words[1] = 0x7D4;
        words[23] = 0xCAFE_0000;
        // Pad into ciphertext form via a rebuild that keeps those words.
        let mut cipher = words.clone();
        let key = xxtea::derive_key(slot);
        xxtea::encrypt(&mut cipher, &key, 6);
        let raw = xxtea::words_to_bytes(&cipher);
        let meta = Metadata::read(&raw, slot).unwrap();

        // A short name only writes word 22, so it stays inside free space...
        let small = meta.patch_to_bytes(&MetadataPatch { save_name: Some("Ab".into()), ..Default::default() });
        assert!(small.is_ok(), "a value that fits before the occupied word must succeed");

        // ...but one that needs word 23 must refuse.
        let err = meta
            .patch_to_bytes(&MetadataPatch { save_name: Some("Longer name".into()), ..Default::default() })
            .unwrap_err();
        assert!(matches!(err, FormatError::MetadataFieldOccupied { field: "SaveName", index: 23, .. }), "got {err:?}");
    }

    #[test]
    fn rejects_unknown_length() {
        let err = Metadata::read(&vec![0u8; 50], Slot::ACCOUNT_DATA).unwrap_err();
        assert!(matches!(err, FormatError::UnknownMetadataLength { length: 50 }));
    }

    #[test]
    fn rejects_wrong_kind_slot() {
        // A player metadata file must not decrypt as an account-level slot.
        let bytes = synthetic_metadata(Slot::player_state(3), 432);
        assert!(Metadata::read(&bytes, Slot::ACCOUNT_DATA).is_err());
    }

    #[test]
    fn slot_names_round_trip() {
        assert_eq!(Slot::from_file_name("save.hg"), Some(Slot::player_state(1)));
        assert_eq!(Slot::from_file_name("save30.hg"), Some(Slot::player_state(30)));
        assert_eq!(Slot::from_file_name("accountdata.hg"), Some(Slot::ACCOUNT_DATA));
        assert_eq!(Slot::from_file_name("usersettings.hg"), Some(Slot::USER_SETTINGS));
        assert_eq!(Slot::from_file_name("save31.hg"), None);
        assert_eq!(Slot::from_metadata_file_name("mf_save7.hg"), Some(Slot::player_state(7)));
        assert_eq!(Slot::from_metadata_file_name("mf_accountdata.hg"), Some(Slot::ACCOUNT_DATA));
        for n in 1..=30u8 {
            let slot = Slot::player_state(n);
            assert_eq!(Slot::from_file_name(&slot.data_file_name()), Some(slot));
            assert_eq!(Slot::from_metadata_file_name(&slot.metadata_file_name()), Some(slot));
        }
        assert_eq!(Slot::ACCOUNT_DATA.metadata_file_name(), "mf_accountdata.hg");
        assert_eq!(Slot::USER_SETTINGS.data_file_name(), "usersettings.hg");
    }
}
