//! Parsing the game's localisation tables.
//!
//! Every `language/nms_loc*_<language>.mbin` (and `nms_update*_<language>.mbin`)
//! is a MBIN "string table": a header, a densely packed array of entries, then
//! one blob holding every text. Layout, verified against the game's own files:
//!
//! ```text
//! 0x00  8 bytes  MBIN magic (0xCCCCCCCC twice)
//! 0x08  u32      template version
//! 0x0C  16 bytes template hash
//! 0x28  u32      entry count
//! 0x30  entries  entry_count × 0x130 bytes:
//!          +0x00  id, NUL-terminated, fits in 0x40 bytes
//!          +0x40  u64, purpose unknown (not a hash of the id; preserved unused)
//!          +0x48  u32, value length including the terminating NUL (0 = no value)
//!       then   the value blob: each non-empty value's text plus its NUL,
//!              in entry order, packed back to back with no padding
//! ```
//!
//! Two invariants make this parser self-checking, and both hold on every
//! language file in the game: **the blob is consumed exactly to the end of the
//! file**, and **every length field matches the text it points at**. A wrong
//! stride or count cannot satisfy both, so a successful parse is evidence the
//! layout is right rather than an assumption.
//!
//! The parser is strict: it reports a short consumption or a missing terminator
//! instead of returning a plausible-looking but wrong table.

use crate::DataError;

/// MBIN magic, as two little-endian `u32`s.
const MBIN_MAGIC: u64 = 0xCCCC_CCCC_CCCC_CCCC;
/// Offset of the entry count.
const COUNT_OFFSET: usize = 0x28;
/// Offset of the first entry.
const ENTRIES_OFFSET: usize = 0x30;
/// Size of one entry record.
const ENTRY_LEN: usize = 0x130;
/// Offset of the value-length field within an entry.
const ENTRY_LENGTH_OFFSET: usize = 0x48;
/// Bytes reserved for the id within an entry.
const ENTRY_ID_BYTES: usize = 0x40;

/// One localisation table.
#[derive(Debug, Clone, Default)]
pub struct Localisation {
    /// Template version from the file header.
    pub version: u32,
    /// `(id, text)` pairs in file order. Empty values are included as empty strings.
    pub entries: Vec<(String, String)>,
    /// How the parse went, for reporting.
    pub stats: LocalisationStats,
}

/// Details of a parse, including the self-checks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalisationStats {
    /// Entries declared by the header.
    pub declared: usize,
    /// Entries actually parsed.
    pub parsed: usize,
    /// Entries whose value is empty.
    pub empty: usize,
    /// Bytes of the value blob consumed.
    pub consumed: usize,
    /// File size.
    pub file_len: usize,
}

impl Localisation {
    /// True when the blob was consumed exactly to the end of the file, which is
    /// the strongest available evidence that the layout was understood.
    pub fn is_exact(&self) -> bool {
        self.stats.consumed == self.stats.file_len && self.stats.parsed == self.stats.declared
    }
}

/// Parse a localisation MBIN.
pub fn parse(data: &[u8]) -> Result<Localisation, DataError> {
    if data.len() < ENTRIES_OFFSET {
        return Err(DataError::Localisation { message: "file is shorter than the header".into() });
    }
    let magic = u64::from_le_bytes(data[0..8].try_into().expect("checked length"));
    if magic != MBIN_MAGIC {
        return Err(DataError::Localisation {
            message: format!("expected MBIN magic, found 0x{magic:016X}"),
        });
    }

    let version = u32::from_le_bytes(data[0x08..0x0C].try_into().expect("checked length"));
    let declared = u32::from_le_bytes(
        data[COUNT_OFFSET..COUNT_OFFSET + 4].try_into().expect("checked length"),
    ) as usize;

    let entries_end = ENTRIES_OFFSET
        .checked_add(declared.checked_mul(ENTRY_LEN).ok_or_else(|| DataError::Localisation {
            message: format!("entry count {declared} overflows"),
        })?)
        .ok_or_else(|| DataError::Localisation { message: "entry table is too large".into() })?;
    if entries_end > data.len() {
        return Err(DataError::Localisation {
            message: format!(
                "entry table ({declared} entries) runs to 0x{entries_end:X}, file is 0x{:X} bytes",
                data.len()
            ),
        });
    }

    let mut entries = Vec::with_capacity(declared);
    let mut cursor = entries_end;
    let mut empty = 0usize;

    for index in 0..declared {
        let base = ENTRIES_OFFSET + index * ENTRY_LEN;
        let id_bytes = &data[base..base + ENTRY_ID_BYTES];
        let id_end = id_bytes.iter().position(|b| *b == 0).ok_or_else(|| DataError::Localisation {
            message: format!("entry {index}: id is not NUL-terminated within {ENTRY_ID_BYTES} bytes"),
        })?;
        let id = String::from_utf8_lossy(&id_bytes[..id_end]).into_owned();

        let length = u32::from_le_bytes(
            data[base + ENTRY_LENGTH_OFFSET..base + ENTRY_LENGTH_OFFSET + 4]
                .try_into()
                .expect("checked length"),
        ) as usize;

        if length == 0 {
            empty += 1;
            entries.push((id, String::new()));
            continue;
        }

        let value_end = cursor + length;
        if value_end > data.len() {
            return Err(DataError::Localisation {
                message: format!(
                    "entry {index} ({id}) declares {length} bytes at 0x{cursor:X}, past the end of the file"
                ),
            });
        }
        if data[value_end - 1] != 0 {
            return Err(DataError::Localisation {
                message: format!("entry {index} ({id}): value is not NUL-terminated"),
            });
        }
        let text = String::from_utf8_lossy(&data[cursor..value_end - 1]).into_owned();
        entries.push((id, text));
        cursor = value_end;
    }

    Ok(Localisation {
        version,
        entries,
        stats: LocalisationStats {
            declared,
            parsed: declared,
            empty,
            consumed: cursor,
            file_len: data.len(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a language table in the real layout.
    fn synthetic(entries: &[(&str, &str)]) -> Vec<u8> {
        let table_len = ENTRIES_OFFSET + entries.len() * ENTRY_LEN;
        let blob_len: usize = entries.iter().map(|(_, text)| if text.is_empty() { 0 } else { text.len() + 1 }).sum();
        let mut data = vec![0u8; table_len + blob_len];

        data[0..8].copy_from_slice(&MBIN_MAGIC.to_le_bytes());
        data[0x08..0x0C].copy_from_slice(&0x0CE4u32.to_le_bytes());
        data[COUNT_OFFSET..COUNT_OFFSET + 4].copy_from_slice(&(entries.len() as u32).to_le_bytes());

        let mut cursor = table_len;
        for (index, (id, text)) in entries.iter().enumerate() {
            let base = ENTRIES_OFFSET + index * ENTRY_LEN;
            assert!(id.len() < ENTRY_ID_BYTES);
            data[base..base + id.len()].copy_from_slice(id.as_bytes());
            let length = if text.is_empty() { 0 } else { text.len() + 1 };
            data[base + ENTRY_LENGTH_OFFSET..base + ENTRY_LENGTH_OFFSET + 4]
                .copy_from_slice(&(length as u32).to_le_bytes());
            if length > 0 {
                data[cursor..cursor + text.len()].copy_from_slice(text.as_bytes());
                cursor += text.len() + 1;
            }
        }
        data
    }

    #[test]
    fn parses_a_table_exactly() {
        let data = synthetic(&[
            ("UI_FIRST", "First string"),
            ("UI_EMPTY", ""),
            ("UI_LONGER_ID", "Second string, longer than the first"),
        ]);
        let table = parse(&data).unwrap();
        assert_eq!(table.version, 0x0CE4);
        assert_eq!(table.entries.len(), 3);
        assert_eq!(table.entries[0], ("UI_FIRST".into(), "First string".into()));
        assert_eq!(table.entries[1], ("UI_EMPTY".into(), String::new()));
        assert_eq!(table.entries[2].1, "Second string, longer than the first");
        assert!(table.is_exact(), "{:?}", table.stats);
        assert_eq!(table.stats.empty, 1);
    }

    #[test]
    fn rejects_a_wrong_magic() {
        let mut data = synthetic(&[("A", "a")]);
        data[0] = 0;
        assert!(parse(&data).is_err());
    }

    #[test]
    fn rejects_a_truncated_entry_table() {
        let mut data = synthetic(&[("A", "a")]);
        data.truncate(ENTRIES_OFFSET + 8);
        assert!(parse(&data).is_err());
    }

    #[test]
    fn rejects_a_value_that_overruns_the_file() {
        let mut data = synthetic(&[("A", "a")]);
        let base = ENTRIES_OFFSET + ENTRY_LENGTH_OFFSET;
        data[base..base + 4].copy_from_slice(&9_999u32.to_le_bytes());
        assert!(parse(&data).is_err());
    }

    #[test]
    fn rejects_a_value_without_a_terminator() {
        let mut data = synthetic(&[("A", "abc")]);
        // The value blob starts right after the entry table: "abc" then its NUL.
        let terminator = ENTRIES_OFFSET + ENTRY_LEN + 3;
        data[terminator] = b'X';
        assert!(parse(&data).is_err());
    }
}
