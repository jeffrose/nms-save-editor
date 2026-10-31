//! The editable save document: an exact-text JSON tree plus the bytes it came
//! from.
//!
//! Fidelity strategy, in three parts:
//!
//! 1. **Numbers are text.** The tree is parsed with `serde_json`'s
//!    `arbitrary_precision` feature, so a literal like `0.30000001192092898`
//!    is preserved verbatim instead of being reformatted to `0.30000001192092896`
//!    (about 6% of the numbers in a real save change under a naive `f64` round
//!    trip, and some are rewritten into scientific notation the game never
//!    writes).
//! 2. **Nothing is re-keyed.** Keys keep the exact spelling the file used.
//! 3. **Unmodified documents never re-serialise.** [`Document::payload`]
//!    returns the original bytes when no edit happened, which makes a
//!    read/write cycle byte-identical by construction.

use std::path::Path;

use nms_format::latin1;
use serde_json::Value;

use crate::keymap::KeyMap;

/// Errors from parsing or serialising a save document.
#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    /// The payload is not valid JSON after Latin-1 decoding.
    #[error("save payload is not valid JSON: {message}")]
    Json {
        /// Underlying parser message.
        message: String,
    },
    /// The payload contains a character that cannot be written back.
    #[error(transparent)]
    Latin1(#[from] latin1::Latin1Error),
    /// Serialising the tree failed.
    #[error("serialising the save document failed: {message}")]
    Serialize {
        /// Underlying serialiser message.
        message: String,
    },
}

/// A summary of the fields every save has, read through a [`KeyMap`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SaveInfo {
    /// Save version word (`Version`, e.g. 4743).
    pub version: Option<u64>,
    /// Platform string (e.g. `Win|Final`).
    pub platform: Option<String>,
    /// Active context (e.g. `Main`, `Season`).
    pub active_context: Option<String>,
    /// Player-chosen save name, if set.
    pub save_name: Option<String>,
    /// Total play time in seconds.
    pub play_time: Option<u64>,
}

/// A parsed save document.
#[derive(Debug, Clone)]
pub struct Document {
    tree: Value,
    original_payload: Vec<u8>,
    dirty: bool,
}

impl Document {
    /// Parse a Latin-1 payload (as returned by the format layer) into a document.
    pub fn parse(payload: &[u8]) -> Result<Document, ModelError> {
        let text = latin1::decode(payload);
        let tree: Value = serde_json::from_str(&text).map_err(|e| ModelError::Json { message: e.to_string() })?;
        Ok(Document { tree, original_payload: payload.to_vec(), dirty: false })
    }

    /// Read a payload from disk and parse it.
    pub fn read(path: &Path) -> Result<Document, Box<dyn std::error::Error>> {
        let text = std::fs::read_to_string(path)?;
        Ok(Document::parse(text.as_bytes())?)
    }

    /// The JSON tree.
    pub fn tree(&self) -> &Value {
        &self.tree
    }

    /// Mutable access to the tree; marks the document as edited.
    pub fn tree_mut(&mut self) -> &mut Value {
        self.dirty = true;
        &mut self.tree
    }

    /// Record an edit performed through other means (for example a bulk
    /// operation that already holds a mutable reference).
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Whether anything has been edited since parsing.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// The original payload bytes, exactly as parsed.
    pub fn original_payload(&self) -> &[u8] {
        &self.original_payload
    }

    /// Serialise the document to Latin-1 payload bytes.
    ///
    /// An unedited document returns its original bytes; an edited one is
    /// re-serialised, with any character above `U+00FF` written as a JSON
    /// escape so it survives a Latin-1 payload.
    pub fn payload(&self) -> Result<Vec<u8>, ModelError> {
        if !self.dirty {
            return Ok(self.original_payload.clone());
        }
        let text = serde_json::to_string(&self.tree).map_err(|e| ModelError::Serialize { message: e.to_string() })?;
        Ok(latin1::encode_escaped(&text))
    }

    /// Look up a value by readable name (`"Version"`), following the mapping.
    pub fn get_named<'a>(&'a self, keys: &KeyMap, name: &str) -> Option<&'a Value> {
        let root = self.tree.as_object()?;
        for candidate in keys.candidates(name) {
            if let Some(value) = root.get(candidate) {
                return Some(value);
            }
        }
        None
    }

    /// Extract the standard save summary fields.
    pub fn info(&self, keys: &KeyMap) -> SaveInfo {
        let version = self
            .get_named(keys, "Version")
            .and_then(|v| v.as_u64());
        let platform = self.get_named(keys, "Platform").and_then(|v| v.as_str()).map(str::to_string);
        let active_context = self.get_named(keys, "ActiveContext").and_then(|v| v.as_str()).map(str::to_string);
        let (save_name, play_time) = match self.get_named(keys, "CommonStateData").and_then(|v| v.as_object()) {
            Some(common) => {
                let name_key = keys.candidates("SaveName").find(|k| common.contains_key(*k));
                let time_key = keys.candidates("TotalPlayTime").find(|k| common.contains_key(*k));
                (
                    name_key
                        .and_then(|k| common.get(k))
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                    time_key.and_then(|k| common.get(k)).and_then(|v| v.as_u64()),
                )
            }
            None => (None, None),
        };
        SaveInfo { version, platform, active_context, save_name, play_time }
    }

    /// Number of object keys in the document (a cheap structural sanity check).
    pub fn key_count(&self) -> usize {
        fn count(value: &Value) -> usize {
            match value {
                Value::Object(map) => map.len() + map.values().map(count).sum::<usize>(),
                Value::Array(items) => items.iter().map(count).sum(),
                _ => 0,
            }
        }
        count(&self.tree)
    }
}

/// Byte-level comparison of two payloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByteDiff {
    /// Offset of the first differing byte, if any.
    pub first_diff: Option<usize>,
    /// Number of differing byte positions within the common prefix.
    pub differing_bytes: usize,
    /// Length of the first payload.
    pub len_a: usize,
    /// Length of the second payload.
    pub len_b: usize,
}

impl ByteDiff {
    /// True when the payloads are identical.
    pub fn is_identical(&self) -> bool {
        self.first_diff.is_none() && self.len_a == self.len_b
    }

    /// Compare two payloads.
    pub fn compute(a: &[u8], b: &[u8]) -> ByteDiff {
        let common = a.len().min(b.len());
        let first_diff = (0..common).find(|&i| a[i] != b[i]);
        let differing_bytes = (0..common).filter(|&i| a[i] != b[i]).count();
        ByteDiff { first_diff, differing_bytes, len_a: a.len(), len_b: b.len() }
    }

    /// A short human-readable summary.
    pub fn summary(&self) -> String {
        if self.is_identical() {
            format!("identical ({} bytes)", self.len_a)
        } else {
            match self.first_diff {
                Some(offset) => format!(
                    "{} bytes vs {} bytes, first difference at offset {offset}, {} differing bytes",
                    self.len_a, self.len_b, self.differing_bytes
                ),
                None => format!("{} bytes vs {} bytes, one is a prefix of the other", self.len_a, self.len_b),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A payload whose numbers a naive `f64` round trip would rewrite, and whose
    /// strings contain Latin-1 bytes and an uppercase `\u` escape.
    fn tricky_payload() -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"{\"F2P\":4743,\"8>q\":\"Win|Final\",\"<h0\":{\"Pk4\":\"\",\"Lg8\":1234},");
        payload.extend_from_slice(b"\"v\":[0.30000001192092898,0.000031987179681891578,3],");
        payload.extend_from_slice(b"\"s\":\"caf\xE9 \\u001A\"}");
        payload
    }

    #[test]
    fn unedited_document_is_byte_identical() {
        let payload = tricky_payload();
        let doc = Document::parse(&payload).unwrap();
        assert!(!doc.is_dirty());
        assert_eq!(doc.payload().unwrap(), payload);
    }

    #[test]
    fn edited_document_preserves_untouched_number_text() {
        let mut doc = Document::parse(&tricky_payload()).unwrap();
        doc.tree_mut()["v"][2] = serde_json::Value::from(4);
        let out = doc.payload().unwrap();
        let text = latin1::decode(&out);
        assert!(text.contains("0.30000001192092898"), "untouched float text must survive: {text}");
        assert!(text.contains("0.000031987179681891578"), "untouched small float must survive: {text}");
        assert!(text.contains("\"v\":[0.30000001192092898,0.000031987179681891578,4]"), "edit must apply: {text}");
        assert!(out.contains(&0xE9), "latin-1 byte must survive on disk: {text}");
    }

    #[test]
    fn reads_summary_fields() {
        let doc = Document::parse(&tricky_payload()).unwrap();
        let info = doc.info(&KeyMap::builtin());
        assert_eq!(info.version, Some(4743));
        assert_eq!(info.platform.as_deref(), Some("Win|Final"));
        assert_eq!(info.play_time, Some(1234));
        assert_eq!(info.save_name, None);
    }

    #[test]
    fn byte_diff_reports_first_divergence() {
        let diff = ByteDiff::compute(b"abcdef", b"abXdef");
        assert_eq!(diff.first_diff, Some(2));
        assert_eq!(diff.differing_bytes, 1);
        assert!(!diff.is_identical());
        assert!(ByteDiff::compute(b"same", b"same").is_identical());
        assert!(!ByteDiff::compute(b"abc", b"abcd").is_identical());
    }
}
