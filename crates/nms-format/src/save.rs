//! Reading and writing a single `.hg` data file.
//!
//! Two containers are in use for the same JSON payload:
//!
//! * **block stream** — every `save*.hg` on Steam/GOG and Switch: LZ4 blocks
//!   with 16-byte headers ([`crate::blocks`]);
//! * **plain JSON** — `accountdata.hg` and `usersettings.hg`: no compression
//!   and no headers, detected by a leading `{`.
//!
//! In both cases the payload is Latin-1, ends with a single NUL byte, and is
//! otherwise untouched by this layer. [`SaveFile`] keeps the payload as the
//! exact bytes that were stored so that a read/write cycle with no edits is
//! byte-identical, and records whether the trailing NUL was present.

use std::path::{Path, PathBuf};

use crate::blocks::{self, BlockLayout};
use crate::error::FormatError;

/// How a save's payload is stored inside the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    /// LZ4 blocks with 16-byte headers.
    BlockStream,
    /// Uncompressed JSON.
    PlainJson,
}

impl Container {
    /// Detect the container from the first bytes of a file.
    pub fn detect(bytes: &[u8]) -> Container {
        if blocks::is_block_container(bytes) {
            Container::BlockStream
        } else {
            Container::PlainJson
        }
    }

    /// Human-readable name.
    pub fn name(self) -> &'static str {
        match self {
            Container::BlockStream => "lz4-blocks",
            Container::PlainJson => "plain-json",
        }
    }
}

/// A save data file, decompressed and ready to edit.
///
/// Two different guarantees apply, and it is worth keeping them apart:
///
/// * **Payload** (the JSON): byte-identical for an unedited save, always.
/// * **Container** (the compressed bytes): *not* reproducible byte-for-byte,
///   because the game's LZ4 encoder output cannot be replicated. A re-encoded
///   container is valid and decompresses to the same payload, but its bytes
///   differ. Therefore an unedited save is never rewritten at all
///   ([`SaveFile::is_modified`] is false and [`SaveFile::write`] does nothing).
#[derive(Debug, Clone)]
pub struct SaveFile {
    /// Storage container of the source file.
    pub container: Container,
    /// Current decompressed payload bytes, without the trailing NUL.
    pub payload: Vec<u8>,
    /// Whether the stored payload ended with a NUL byte.
    pub trailing_nul: bool,
    /// Original block layout (empty for plain JSON files).
    pub blocks: Vec<BlockLayout>,
    /// Payload as read from disk, for change detection.
    original_payload: Vec<u8>,
}

/// What a write did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    /// Nothing was modified, so the file was left untouched.
    Unchanged,
    /// The file was replaced; contains the new file size in bytes.
    Written(usize),
}

impl SaveFile {
    /// Parse raw file bytes.
    pub fn from_bytes(raw: &[u8]) -> Result<SaveFile, FormatError> {
        match Container::detect(raw) {
            Container::BlockStream => {
                let (payload, blocks) = blocks::read(raw)?;
                let (payload, trailing_nul) = split_trailing_nul(payload);
                Ok(SaveFile {
                    container: Container::BlockStream,
                    original_payload: payload.clone(),
                    payload,
                    trailing_nul,
                    blocks,
                })
            }
            Container::PlainJson => {
                let (payload, trailing_nul) = split_trailing_nul(raw.to_vec());
                Ok(SaveFile {
                    container: Container::PlainJson,
                    original_payload: payload.clone(),
                    payload,
                    trailing_nul,
                    blocks: Vec::new(),
                })
            }
        }
    }

    /// Read and decompress a save file.
    pub fn read(path: &Path) -> Result<SaveFile, FormatError> {
        let raw = std::fs::read(path).map_err(|e| FormatError::io("reading save file", e))?;
        SaveFile::from_bytes(&raw)
    }

    /// True when the payload differs from what was read from disk.
    pub fn is_modified(&self) -> bool {
        self.payload != self.original_payload
    }

    /// The payload as it was read from disk.
    pub fn original_payload(&self) -> &[u8] {
        &self.original_payload
    }

    /// The payload as it would be stored, including the trailing NUL.
    pub fn stored_payload(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.payload.len() + 1);
        out.extend_from_slice(&self.payload);
        if self.trailing_nul {
            out.push(0);
        }
        out
    }

    /// Encode the save into file bytes using its original container.
    ///
    /// The result is a valid container, but not necessarily byte-identical to
    /// the file it came from (see the type docs).
    pub fn to_bytes(&self) -> Vec<u8> {
        let stored = self.stored_payload();
        match self.container {
            Container::BlockStream => blocks::write(&stored),
            Container::PlainJson => stored,
        }
    }

    /// Replace the payload, keeping the container and NUL convention.
    ///
    /// Callers are responsible for the payload being JSON text; this layer does
    /// not validate it.
    pub fn set_payload(&mut self, payload: Vec<u8>) {
        self.payload = payload;
    }

    /// Write the save atomically.
    ///
    /// An unmodified save is not written at all, so opening and closing a save
    /// cannot touch the file, its timestamp, or cloud-sync state.
    pub fn write(&self, path: &Path) -> Result<WriteOutcome, FormatError> {
        if !self.is_modified() {
            return Ok(WriteOutcome::Unchanged);
        }
        let bytes = self.to_bytes();
        write_atomic(path, &bytes)?;
        Ok(WriteOutcome::Written(bytes.len()))
    }
}

fn split_trailing_nul(mut payload: Vec<u8>) -> (Vec<u8>, bool) {
    if payload.last() == Some(&0) {
        payload.pop();
        (payload, true)
    } else {
        (payload, false)
    }
}

/// Write `bytes` to `path` atomically (temp file + rename), flushing to disk.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), FormatError> {
    use std::io::Write;

    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| FormatError::UnknownSlot { name: path.display().to_string() })?;
    let tmp: PathBuf = dir.join(format!(".{file_name}.tmp"));

    {
        let mut file = std::fs::File::create(&tmp).map_err(|e| FormatError::io("creating temporary file", e))?;
        file.write_all(bytes).map_err(|e| FormatError::io("writing temporary file", e))?;
        file.sync_all().map_err(|e| FormatError::io("flushing temporary file", e))?;
    }

    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        FormatError::io("replacing save file", e)
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_block_container_payload_and_skips_unmodified_writes() {
        let payload = br#"{"F2P":4743,"8>q":"Win|Final"}"#.to_vec();
        let mut stored = payload.clone();
        stored.push(0);
        let raw = blocks::write(&stored);

        let save = SaveFile::from_bytes(&raw).unwrap();
        assert_eq!(save.container, Container::BlockStream);
        assert_eq!(save.payload, payload);
        assert!(save.trailing_nul);
        assert!(!save.is_modified(), "a freshly read save must not report modifications");

        // Re-encoding produces a valid container carrying the same payload,
        // though not necessarily the same compressed bytes.
        let re_encoded = save.to_bytes();
        let reparsed = SaveFile::from_bytes(&re_encoded).unwrap();
        assert_eq!(reparsed.payload, payload);
        assert_eq!(reparsed.stored_payload(), stored);

        // An unmodified save is never written.
        let dir = std::env::temp_dir().join(format!("nms-format-unchanged-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("save.hg");
        std::fs::write(&path, &raw).unwrap();
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(save.write(&path).unwrap(), WriteOutcome::Unchanged);
        assert_eq!(std::fs::read(&path).unwrap(), raw, "file must be left byte-identical");
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), before, "mtime must not change");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn modified_save_writes_a_readable_container() {
        let payload = br#"{"F2P":4743,"v":[0.30000001192092898]}"#.to_vec();
        let mut stored = payload.clone();
        stored.push(0);
        let raw = blocks::write(&stored);

        let mut save = SaveFile::from_bytes(&raw).unwrap();
        let edited = String::from_utf8(payload.clone()).unwrap().replace("]", ",1]");
        save.set_payload(edited.into_bytes());
        assert!(save.is_modified());

        let dir = std::env::temp_dir().join(format!("nms-format-modified-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("save.hg");
        std::fs::write(&path, &raw).unwrap();
        assert!(matches!(save.write(&path).unwrap(), WriteOutcome::Written(_)));

        let reread = SaveFile::read(&path).unwrap();
        assert_eq!(reread.payload, save.payload);
        assert!(nms_latin1(&reread.payload).contains("0.30000001192092898"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn nms_latin1(bytes: &[u8]) -> String {
        bytes.iter().map(|&b| b as char).collect()
    }

    #[test]
    fn round_trips_plain_json_byte_exactly() {
        let raw = br#"{"F2P":4098,"B89":{"32m":false}}"#.to_vec();
        let save = SaveFile::from_bytes(&raw).unwrap();
        assert_eq!(save.container, Container::PlainJson);
        assert!(!save.trailing_nul);
        assert_eq!(save.to_bytes(), raw);
    }

    #[test]
    fn plain_json_with_trailing_nul_keeps_it() {
        let mut raw = br#"{"a":1}"#.to_vec();
        raw.push(0);
        let save = SaveFile::from_bytes(&raw).unwrap();
        assert!(save.trailing_nul);
        assert_eq!(save.to_bytes(), raw);
    }

    #[test]
    fn atomic_write_replaces_content() {
        let dir = std::env::temp_dir().join(format!("nms-format-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("save.hg");
        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        // no temp file left behind
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files left behind: {leftovers:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
