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

/// Errors from resolving or creating a path inside a document.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// A path segment did not exist.
    #[error("no such field: {0}")]
    NotFound(String),
    /// A path segment was applied to something that is not an object.
    #[error("{0} is not an object")]
    NotAnObject(String),
    /// A path segment was applied to something that is not an array.
    #[error("{0} is not an array")]
    NotAnArray(String),
    /// An array index was out of bounds.
    #[error("index {index} is out of range for {path} (length {len})")]
    IndexOutOfRange {
        /// The path being resolved.
        path: String,
        /// The requested index.
        index: usize,
        /// Array length.
        len: usize,
    },
    /// A path segment was empty.
    #[error("empty path segment in {0:?}")]
    EmptySegment(String),
}

/// One segment of a dotted path: an object key with an optional array index
/// (`Inventory[3]`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Segment<'a> {
    key: &'a str,
    index: Option<usize>,
}

fn parse_path(path: &str) -> Result<Vec<Segment<'_>>, PathError> {
    let mut segments = Vec::new();
    for raw in path.split('.') {
        if raw.is_empty() {
            return Err(PathError::EmptySegment(path.to_string()));
        }
        match raw.split_once('[') {
            Some((key, rest)) => {
                let index = rest
                    .strip_suffix(']')
                    .and_then(|digits| digits.parse::<usize>().ok())
                    .ok_or_else(|| PathError::EmptySegment(raw.to_string()))?;
                if key.is_empty() {
                    return Err(PathError::EmptySegment(raw.to_string()));
                }
                segments.push(Segment { key, index: Some(index) });
            }
            None => segments.push(Segment { key: raw, index: None }),
        }
    }
    Ok(segments)
}

/// Resolve an object key by readable name, following the key map.
///
/// The literal name is preferred so that documents already using readable keys
/// resolve directly, then the mapped on-disk spelling.
fn resolve_key(object: &serde_json::Map<String, Value>, keys: &KeyMap, name: &str) -> Option<String> {
    keys.candidates(name)
        .find(|candidate| object.contains_key(*candidate))
        .map(str::to_string)
}

/// Candidate key to use when a field has to be created: the on-disk spelling
/// when it is known, otherwise the readable name.
fn creation_key(keys: &KeyMap, name: &str) -> String {
    keys.candidates(name).last().unwrap_or(name).to_string()
}

/// Rename every object key through the map, producing a readable copy.
///
/// Unknown keys are kept verbatim, so nothing is lost in an export.
pub fn readable_tree(value: &Value, keys: &KeyMap) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (key, child) in map {
                let name = keys.readable_for(key).unwrap_or(key);
                out.insert(name.to_string(), readable_tree(child, keys));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(|item| readable_tree(item, keys)).collect()),
        other => other.clone(),
    }
}

impl Document {
    /// Look up a value by dotted, readable path (`CommonStateData.SaveName`,
    /// `Inventory[3].Name`).
    pub fn get_path(&self, keys: &KeyMap, path: &str) -> Result<&Value, PathError> {
        let segments = parse_path(path)?;
        let mut current = &self.tree;
        for (position, segment) in segments.iter().enumerate() {
            let object = current
                .as_object()
                .ok_or_else(|| PathError::NotAnObject(join_segments(&segments[..position])))?;
            let key = resolve_key(object, keys, segment.key).ok_or_else(|| PathError::NotFound(path.to_string()))?;
            current = &object[&key];
            if let Some(index) = segment.index {
                let array = current.as_array().ok_or_else(|| PathError::NotAnArray(segment.key.to_string()))?;
                current = array.get(index).ok_or_else(|| PathError::IndexOutOfRange {
                    path: path.to_string(),
                    index,
                    len: array.len(),
                })?;
            }
        }
        Ok(current)
    }

    /// Set a value by dotted, readable path, creating intermediate objects.
    ///
    /// New fields are created under their on-disk key when the map knows one,
    /// so a field added by this editor is one the game will read.
    pub fn set_path(&mut self, keys: &KeyMap, path: &str, value: Value) -> Result<(), PathError> {
        let segments = parse_path(path)?;
        let mut current = self.tree_mut();
        for (position, segment) in segments.iter().enumerate() {
            let last = position + 1 == segments.len();
            let object = current
                .as_object_mut()
                .ok_or_else(|| PathError::NotAnObject(join_segments(&segments[..position])))?;

            let key = match resolve_key(object, keys, segment.key) {
                Some(existing) => existing.to_string(),
                None => creation_key(keys, segment.key),
            };
            if !object.contains_key(&key) {
                object.insert(key.clone(), Value::Object(serde_json::Map::new()));
            }
            let slot = object.get_mut(&key).expect("just inserted");

            match segment.index {
                Some(index) => {
                    let array = slot.as_array_mut().ok_or_else(|| PathError::NotAnArray(segment.key.to_string()))?;
                    if index >= array.len() {
                        return Err(PathError::IndexOutOfRange {
                            path: path.to_string(),
                            index,
                            len: array.len(),
                        });
                    }
                    if last {
                        array[index] = value;
                        return Ok(());
                    }
                    current = &mut array[index];
                }
                None => {
                    if last {
                        *slot = value;
                        return Ok(());
                    }
                    current = slot;
                }
            }
        }
        Ok(())
    }

    /// Render the document with readable key names, for export and review.
    pub fn readable(&self, keys: &KeyMap) -> Value {
        readable_tree(&self.tree, keys)
    }
}

fn join_segments(segments: &[Segment<'_>]) -> String {
    segments.iter().map(|s| s.key).collect::<Vec<_>>().join(".")
}

/// One field that differs between two documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeChange {
    /// Readable path, using mapped key names where known (`CommonStateData.SaveName`).
    pub path: String,
    /// Value before, if it existed.
    pub before: Option<String>,
    /// Value after, if it exists.
    pub after: Option<String>,
}

impl TreeChange {
    /// A one-line description.
    pub fn summary(&self) -> String {
        match (&self.before, &self.after) {
            (Some(before), Some(after)) => format!("{}: {before} -> {after}", self.path),
            (Some(before), None) => format!("{}: removed ({before})", self.path),
            (None, Some(after)) => format!("{}: added ({after})", self.path),
            (None, None) => format!("{}: unchanged", self.path),
        }
    }
}

/// Describe the fields that differ between two documents, up to `limit` entries.
///
/// Key names are resolved through the map for readability. This is the change
/// report a user sees; comparing raw bytes cannot tell the difference between
/// "one field was edited" and "the payload got shorter so everything shifted".
pub fn tree_diff(before: &Value, after: &Value, keys: &KeyMap, limit: usize) -> Vec<TreeChange> {
    let mut changes = Vec::new();
    diff_walk("", before, after, keys, limit, &mut changes);
    changes
}

fn diff_walk(
    path: &str,
    before: &Value,
    after: &Value,
    keys: &KeyMap,
    limit: usize,
    out: &mut Vec<TreeChange>,
) {
    if out.len() >= limit {
        return;
    }
    match (before, after) {
        (Value::Object(a), Value::Object(b)) => {
            let mut seen = std::collections::HashSet::new();
            for key in a.keys().chain(b.keys()) {
                if !seen.insert(key.as_str()) {
                    continue;
                }
                let child = join_path(path, keys.readable_for(key).unwrap_or(key));
                match (a.get(key), b.get(key)) {
                    (Some(x), Some(y)) => diff_walk(&child, x, y, keys, limit, out),
                    (Some(x), None) => out.push(TreeChange { path: child, before: Some(brief(x)), after: None }),
                    (None, Some(y)) => out.push(TreeChange { path: child, before: None, after: Some(brief(y)) }),
                    (None, None) => {}
                }
                if out.len() >= limit {
                    return;
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for index in 0..a.len().max(b.len()) {
                let child = format!("{}[{index}]", if path.is_empty() { "" } else { path });
                match (a.get(index), b.get(index)) {
                    (Some(x), Some(y)) => diff_walk(&child, x, y, keys, limit, out),
                    (Some(x), None) => out.push(TreeChange { path: child, before: Some(brief(x)), after: None }),
                    (None, Some(y)) => out.push(TreeChange { path: child, before: None, after: Some(brief(y)) }),
                    (None, None) => {}
                }
                if out.len() >= limit {
                    return;
                }
            }
        }
        _ if before == after => {}
        _ => out.push(TreeChange {
            path: path.to_string(),
            before: Some(brief(before)),
            after: Some(brief(after)),
        }),
    }
}

fn join_path(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_string()
    } else {
        format!("{path}.{key}")
    }
}

/// A short, printable rendering of a value for change reports.
fn brief(value: &Value) -> String {
    let text = match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    const LIMIT: usize = 80;
    if text.chars().count() <= LIMIT {
        text
    } else {
        let head: String = text.chars().take(LIMIT).collect();
        format!("{head}…")
    }
}

/// Byte-level comparison of two payloads.
///
/// `differing_bytes` counts position-wise differences, which is misleading when
/// an edit changes the payload length: everything after the edit shifts by one
/// and counts as "differing". `common_prefix`, `common_suffix` and
/// [`ByteDiff::changed_bytes`] describe the edit as a single region instead,
/// which is what a change report should show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ByteDiff {
    /// Offset of the first differing byte, if any.
    pub first_diff: Option<usize>,
    /// Number of position-wise differing bytes within the common prefix.
    pub differing_bytes: usize,
    /// Length of the first payload.
    pub len_a: usize,
    /// Length of the second payload.
    pub len_b: usize,
    /// Length of the identical prefix.
    pub common_prefix: usize,
    /// Length of the identical suffix, not overlapping the prefix.
    pub common_suffix: usize,
}

impl ByteDiff {
    /// True when the payloads are identical.
    pub fn is_identical(&self) -> bool {
        self.first_diff.is_none() && self.len_a == self.len_b
    }

    /// Size of the changed region: the larger of the two differing spans.
    pub fn changed_bytes(&self) -> usize {
        let a = self.len_a - self.common_prefix - self.common_suffix;
        let b = self.len_b - self.common_prefix - self.common_suffix;
        a.max(b)
    }

    /// The changed region in the first payload.
    pub fn changed_span_a(&self) -> std::ops::Range<usize> {
        self.common_prefix..(self.len_a - self.common_suffix)
    }

    /// Compare two payloads.
    pub fn compute(a: &[u8], b: &[u8]) -> ByteDiff {
        let common = a.len().min(b.len());
        let first_diff = (0..common).find(|&i| a[i] != b[i]);
        let differing_bytes = (0..common).filter(|&i| a[i] != b[i]).count();

        let common_prefix = (0..common).take_while(|&i| a[i] == b[i]).count();
        let remaining = common - common_prefix;
        let common_suffix = (0..remaining)
            .take_while(|&i| a[a.len() - 1 - i] == b[b.len() - 1 - i])
            .count();

        ByteDiff { first_diff, differing_bytes, len_a: a.len(), len_b: b.len(), common_prefix, common_suffix }
    }

    /// A short human-readable summary.
    pub fn summary(&self) -> String {
        if self.is_identical() {
            return format!("identical ({} bytes)", self.len_a);
        }
        match self.first_diff {
            Some(offset) => format!(
                "{} -> {} bytes, {} changed byte(s) starting at offset {}",
                self.len_a,
                self.len_b,
                self.changed_bytes(),
                offset
            ),
            None => format!("{} bytes vs {} bytes, one is a prefix of the other", self.len_a, self.len_b),
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
    fn tree_diff_names_the_edited_field() {
        let keys = KeyMap::builtin();
        let before = Document::parse(&tricky_payload()).unwrap();
        let mut after = Document::parse(&tricky_payload()).unwrap();
        after.tree_mut()["<h0"]["Lg8"] = Value::from(1);

        let changes = tree_diff(before.tree(), after.tree(), &keys, 10);
        assert_eq!(changes.len(), 1, "one field was edited: {changes:#?}");
        assert_eq!(changes[0].path, "CommonStateData.TotalPlayTime");
        assert_eq!(changes[0].before.as_deref(), Some("1234"));
        assert_eq!(changes[0].after.as_deref(), Some("1"));
        assert_eq!(changes[0].summary(), "CommonStateData.TotalPlayTime: 1234 -> 1");
        assert!(tree_diff(before.tree(), before.tree(), &keys, 10).is_empty());
    }

    #[test]
    fn tree_diff_reports_added_and_removed_fields() {
        let keys = KeyMap::empty();
        let before: Value = serde_json::from_str(r#"{"a":1,"b":{"c":2}}"#).unwrap();
        let after: Value = serde_json::from_str(r#"{"a":1,"d":3}"#).unwrap();
        let mut changes = tree_diff(&before, &after, &keys, 10);
        changes.sort_by(|x, y| x.path.cmp(&y.path));
        assert_eq!(changes.len(), 2, "{changes:#?}");
        // A removed object is reported as one change at the shallowest level,
        // which keeps reports short.
        assert_eq!(changes[0].path, "b");
        assert_eq!(changes[0].before.as_deref(), Some("{\"c\":2}"));
        assert_eq!(changes[0].after, None);
        assert_eq!(changes[1].path, "d");
        assert_eq!(changes[1].before, None);
    }

    #[test]
    fn byte_diff_reports_first_divergence() {
        let diff = ByteDiff::compute(b"abcdef", b"abXdef");
        assert_eq!(diff.first_diff, Some(2));
        assert_eq!(diff.differing_bytes, 1);
        assert_eq!(diff.changed_bytes(), 1);
        assert!(!diff.is_identical());
        assert!(ByteDiff::compute(b"same", b"same").is_identical());
        assert!(!ByteDiff::compute(b"abc", b"abcd").is_identical());
    }

    #[test]
    fn byte_diff_describes_a_shift_as_one_region() {
        // Deleting five bytes shifts everything after the edit: a position-wise
        // comparison calls most of the tail different, the region view does not.
        let before = b"HEAD-AAAAA-TAIL";
        let after = b"HEAD-TAIL";
        let diff = ByteDiff::compute(before, after);
        // The prefix and suffix may not overlap, so the shared tail is 4 bytes
        // and the removed run reads as 6 bytes in the longer payload.
        assert_eq!(diff.common_prefix, 5);
        assert_eq!(diff.common_suffix, 4);
        assert_eq!(diff.changed_bytes(), 6);
        assert_eq!(diff.changed_span_a(), 5..11);
        assert_eq!(diff.differing_bytes, 3, "position-wise comparison is small here only because the payloads are short");
    }

    /// A document with the obfuscated on-disk spelling, plus an array.
    fn obfuscated_document() -> Document {
        let payload = b"{\"F2P\":4743,\"<h0\":{\"Pk4\":\"\"},\"Ps4\":[{\"b2n\":\"A\"},{\"b2n\":\"B\"}]}";
        Document::parse(payload).unwrap()
    }

    #[test]
    fn reads_and_writes_paths_through_the_key_map() {
        let keys = KeyMap::builtin();
        let mut doc = obfuscated_document();

        assert_eq!(doc.get_path(&keys, "CommonStateData.Pk4").unwrap().as_str(), Some(""));
        assert_eq!(doc.get_path(&keys, "CommonStateData.SaveName").unwrap().as_str(), Some(""));

        doc.set_path(&keys, "CommonStateData.SaveName", Value::from("Utopia")).unwrap();
        assert_eq!(doc.get_path(&keys, "CommonStateData.SaveName").unwrap().as_str(), Some("Utopia"));

        // The write must land on the on-disk key the game reads.
        let text = latin1::decode(&doc.payload().unwrap());
        assert!(text.contains("\"Pk4\":\"Utopia\""), "on-disk key must be used: {text}");
        assert!(!text.contains("SaveName"), "readable name must not leak into the payload: {text}");
    }

    #[test]
    fn resolves_array_indices() {
        let keys = KeyMap::builtin();
        let mut doc = obfuscated_document();
        assert_eq!(doc.get_path(&keys, "Ps4[1].b2n").unwrap().as_str(), Some("B"));
        doc.set_path(&keys, "Ps4[0].b2n", Value::from("Z")).unwrap();
        assert_eq!(doc.get_path(&keys, "Ps4[0].b2n").unwrap().as_str(), Some("Z"));
        let err = doc.get_path(&keys, "Ps4[9].b2n").unwrap_err();
        assert!(matches!(err, PathError::IndexOutOfRange { index: 9, len: 2, .. }), "got {err:?}");
    }

    #[test]
    fn creates_missing_fields_under_their_on_disk_key() {
        let keys = KeyMap::builtin();
        let mut doc = obfuscated_document();
        doc.set_path(&keys, "CommonStateData.TotalPlayTime", Value::from(42)).unwrap();
        let text = latin1::decode(&doc.payload().unwrap());
        assert!(text.contains("\"Lg8\":42"), "created field must use the on-disk key: {text}");
    }

    #[test]
    fn reports_missing_paths() {
        let keys = KeyMap::builtin();
        let doc = obfuscated_document();
        assert!(matches!(doc.get_path(&keys, "Nope"), Err(PathError::NotFound(_))));
        assert!(matches!(doc.get_path(&keys, "F2P.Deep"), Err(PathError::NotAnObject(_))));
        assert!(matches!(doc.get_path(&keys, ""), Err(PathError::EmptySegment(_))));
    }

    #[test]
    fn readable_export_renames_known_keys_only() {
        let keys = KeyMap::builtin();
        let doc = obfuscated_document();
        let readable = doc.readable(&keys);
        assert_eq!(readable["Version"], 4743);
        assert_eq!(readable["CommonStateData"]["SaveName"], "");
        // An unmapped key keeps its on-disk spelling so nothing is lost.
        assert!(readable.get("Ps4").is_some(), "unmapped keys must survive the export");
    }
}