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
//!          +0x00  id, NUL-terminated, fits in 0x20 bytes
//!          +0x20  value slots: one per language, 16 bytes each, in the game's
//!                 language order (alphabetical by file name):
//!                   +0x00  u64  value offset, relative to the slot's own
//!                              position in the file
//!                   +0x08  u32  value length including the NUL (0 = no value)
//!                   +0x0C  u32  marker (0xAAAAAA01)
//!       then   the value blob: each non-empty value's text plus its NUL,
//!              in entry order, packed back to back with no padding
//! ```
//!
//! **A file fills only its own language's slot.** English happens to be the
//! first slot a base table fills (+0x40), which is why a parser that assumed a
//! fixed offset read English correctly and every other language as empty
//! strings — the bug this layout was worked out to fix. The slot is therefore
//! detected per file: the one that holds a value in the most records.
//!
//! Three invariants make this parser self-checking, and all three hold on
//! every language file in the game: **the blob is consumed exactly to the end
//! of the file**, **every length field matches the text it points at**, and
//! **the stored offset equals the value's position minus the slot's own
//! position**. A wrong stride or count cannot satisfy them, so a successful
//! parse is evidence the layout is right rather than an assumption.
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
/// Bytes reserved for the id within an entry (the longest observed id is 31).
const ENTRY_ID_BYTES: usize = 0x20;
/// The per-language value slots start right after the id.
const SLOT_FIRST: usize = ENTRY_ID_BYTES;
/// Size of one value slot.
const SLOT_LEN: usize = 16;
/// How many languages the record has room for: the rest of the record.
const SLOT_COUNT: usize = (ENTRY_LEN - SLOT_FIRST) / SLOT_LEN;
/// Offset of the value length within a slot.
const SLOT_LENGTH: usize = 8;

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
    /// Which value slot the file filled, when any record held a value.
    pub slot: Option<usize>,
    /// Records whose stored offset did not point at the value the lengths
    /// walked to.
    pub pointer_mismatches: usize,
}

impl Localisation {
    /// True when the blob was consumed exactly to the end of the file, every
    /// entry parsed, and every stored offset matched the walked position —
    /// the strongest available evidence that the layout was understood.
    pub fn is_exact(&self) -> bool {
        self.stats.consumed == self.stats.file_len
            && self.stats.parsed == self.stats.declared
            && self.stats.pointer_mismatches == 0
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

    let slot = value_slot(data, declared);
    let slot_start = slot.map(|slot| SLOT_FIRST + slot * SLOT_LEN);

    let mut entries = Vec::with_capacity(declared);
    let mut cursor = entries_end;
    let mut empty = 0usize;
    let mut pointer_mismatches = 0usize;

    for index in 0..declared {
        let base = ENTRIES_OFFSET + index * ENTRY_LEN;
        let id_bytes = &data[base..base + ENTRY_ID_BYTES];
        let id_end = id_bytes.iter().position(|b| *b == 0).ok_or_else(|| DataError::Localisation {
            message: format!("entry {index}: id is not NUL-terminated within {ENTRY_ID_BYTES} bytes"),
        })?;
        let id = String::from_utf8_lossy(&id_bytes[..id_end]).into_owned();

        let length = match slot_start {
            Some(slot_start) => {
                let at = base + slot_start + SLOT_LENGTH;
                u32::from_le_bytes(data[at..at + 4].try_into().expect("checked length")) as usize
            }
            None => 0,
        };

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

        // The stored offset is the value's position minus the slot's own
        // position. Count a mismatch rather than refusing the file: it is
        // corroborating evidence, and the lengths and the blob are the parts
        // the walk actually needs.
        if let Some(slot_start) = slot_start {
            let slot_position = (base + slot_start) as u64;
            let stored = u64::from_le_bytes(
                data[base + slot_start..base + slot_start + 8].try_into().expect("checked length"),
            );
            if stored != (cursor as u64).wrapping_sub(slot_position) {
                pointer_mismatches += 1;
            }
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
            slot,
            pointer_mismatches,
        },
    })
}

/// The value slot this file fills: the one holding a value in the most records.
///
/// Each record carries a slot per language and a table fills only its own, so
/// the filled slot stands out by orders of magnitude. Only the offset and the
/// length count — every slot, filled or not, carries the marker `0xAAAAAA01`
/// in its last four bytes, so "any non-zero byte" would find all seventeen
/// slots equally filled. `None` means no record held a value at all.
fn value_slot(data: &[u8], declared: usize) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for slot in 0..SLOT_COUNT {
        let offset = SLOT_FIRST + slot * SLOT_LEN;
        let filled = (0..declared)
            .filter(|index| {
                let base = ENTRIES_OFFSET + index * ENTRY_LEN + offset;
                data[base..base + SLOT_LENGTH + 4].iter().any(|byte| *byte != 0)
            })
            .count();
        if filled > 0 && best.map_or(true, |(_, best_filled)| filled > best_filled) {
            best = Some((slot, filled));
        }
    }
    best.map(|(slot, _)| slot)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The slot the English files fill. The game's languages are ordered
    /// alphabetically by file name (brazilianportuguese, dutch, english, …), so
    /// English is the third; the tests use it because it is the one the game's
    /// own English files use.
    const ENGLISH_SLOT: usize = 2;

    /// Build a language table in the real layout, filling one language's slot.
    ///
    /// Every slot gets the real `0xAAAAAA01` marker — present whether the slot
    /// is filled or not — so the detection logic is exercised as it is on the
    /// game's files, not on all-zero padding.
    fn synthetic_in_slot(entries: &[(&str, &str)], slot: usize) -> Vec<u8> {
        let table_len = ENTRIES_OFFSET + entries.len() * ENTRY_LEN;
        let blob_len: usize = entries.iter().map(|(_, text)| if text.is_empty() { 0 } else { text.len() + 1 }).sum();
        let mut data = vec![0u8; table_len + blob_len];

        data[0..8].copy_from_slice(&MBIN_MAGIC.to_le_bytes());
        data[0x08..0x0C].copy_from_slice(&0x0CE4u32.to_le_bytes());
        data[COUNT_OFFSET..COUNT_OFFSET + 4].copy_from_slice(&(entries.len() as u32).to_le_bytes());
        for index in 0..entries.len() {
            for slot in 0..SLOT_COUNT {
                let marker = ENTRIES_OFFSET + index * ENTRY_LEN + SLOT_FIRST + slot * SLOT_LEN + SLOT_LENGTH + 4;
                data[marker..marker + 4].copy_from_slice(&0xAAAA_AA01u32.to_le_bytes());
            }
        }

        let slot_start = SLOT_FIRST + slot * SLOT_LEN;
        let mut cursor = table_len;
        for (index, (id, text)) in entries.iter().enumerate() {
            let base = ENTRIES_OFFSET + index * ENTRY_LEN;
            assert!(id.len() < ENTRY_ID_BYTES);
            data[base..base + id.len()].copy_from_slice(id.as_bytes());
            let length = if text.is_empty() { 0 } else { text.len() + 1 };
            data[base + slot_start + SLOT_LENGTH..base + slot_start + SLOT_LENGTH + 4]
                .copy_from_slice(&(length as u32).to_le_bytes());
            if length > 0 {
                // The stored offset is relative to the slot's own position.
                let relative = (cursor - (base + slot_start)) as u64;
                data[base + slot_start..base + slot_start + 8].copy_from_slice(&relative.to_le_bytes());
                data[cursor..cursor + text.len()].copy_from_slice(text.as_bytes());
                cursor += text.len() + 1;
            }
        }
        data
    }

    fn synthetic(entries: &[(&str, &str)]) -> Vec<u8> {
        synthetic_in_slot(entries, ENGLISH_SLOT)
    }

    /// The length field of one entry's value slot, for tests that corrupt it.
    fn length_at(index: usize, slot: usize) -> usize {
        ENTRIES_OFFSET + index * ENTRY_LEN + SLOT_FIRST + slot * SLOT_LEN + SLOT_LENGTH
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
        assert_eq!(table.stats.slot, Some(ENGLISH_SLOT));
        assert_eq!(table.stats.pointer_mismatches, 0);
    }

    /// Every record carries a slot per language and a table fills only its own;
    /// the parser must find whichever slot holds the values, not assume
    /// English's. This is the bug that read every non-English table as empty.
    #[test]
    fn reads_whichever_language_slot_the_file_fills() {
        for slot in [0usize, 1, 5, 16] {
            let data = synthetic_in_slot(&[("UI_GREETING", "Hallo"), ("UI_OTHER", "Welt")], slot);
            let table = parse(&data).unwrap();
            assert_eq!(table.stats.slot, Some(slot), "slot {slot}");
            assert_eq!(table.entries[0], ("UI_GREETING".into(), "Hallo".into()));
            assert!(table.is_exact(), "slot {slot}: {:?}", table.stats);
        }
    }

    #[test]
    fn a_table_with_no_filled_slot_reads_as_empty() {
        let mut data = synthetic(&[("UI_ONE", "One")]);
        for slot in 0..SLOT_COUNT {
            let at = ENTRIES_OFFSET + SLOT_FIRST + slot * SLOT_LEN;
            data[at..at + SLOT_LEN].fill(0);
        }
        let table = parse(&data).unwrap();
        assert_eq!(table.stats.slot, None);
        assert_eq!(table.entries[0], ("UI_ONE".into(), String::new()));
        assert_eq!(table.stats.empty, 1);
    }

    /// A stored offset that does not point where the lengths walk is counted,
    /// not fatal: the lengths and the blob are what the walk needs.
    #[test]
    fn a_wrong_stored_offset_is_counted_not_fatal() {
        let mut data = synthetic(&[("UI_ONE", "One"), ("UI_TWO", "Two")]);
        let at = ENTRIES_OFFSET + SLOT_FIRST + ENGLISH_SLOT * SLOT_LEN;
        data[at..at + 8].copy_from_slice(&1234u64.to_le_bytes());
        let table = parse(&data).unwrap();
        assert_eq!(table.stats.pointer_mismatches, 1);
        assert!(!table.is_exact(), "a mismatched offset must show in the report");
        assert_eq!(table.entries[0].1, "One");
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
        let base = length_at(0, ENGLISH_SLOT);
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
