//! Latin-1 (ISO-8859-1) byte-to-text conversion for NMS save payloads.
//!
//! No Man's Sky writes its save JSON as raw bytes where every byte is one
//! character code. Payloads routinely contain bytes above `0x7F` (custom ship,
//! base, and player names), which makes them invalid UTF-8. A decoder that
//! assumes UTF-8 either fails outright or silently mangles those names, so all
//! payload handling goes through this module.
//!
//! Layout of the conversion, both directions:
//!
//! ```text
//! file bytes (0x00..=0xFF)  <->  Rust String (U+0000..=U+00FF)
//! ```
//!
//! Characters above `U+00FF` cannot be written back as single bytes. On write
//! they are emitted as JSON `\uXXXX` escapes (see [`encode_escaped`]), which
//! the game's JSON reader understands, so user-entered Unicode survives.

/// Errors produced when a string cannot be written as Latin-1 bytes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Latin1Error {
    /// The character has no single-byte representation in a Latin-1 payload.
    #[error("character U+{codepoint:04X} ({ch:?}) is not representable in a Latin-1 payload")]
    Unrepresentable {
        /// The offending character.
        ch: char,
        /// Its Unicode scalar value.
        codepoint: u32,
    },
}

/// Decode bytes as Latin-1. Infallible: one byte always maps to one code point.
pub fn decode(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

/// Encode a string as Latin-1 bytes, rejecting characters above `U+00FF`.
pub fn encode(text: &str) -> Result<Vec<u8>, Latin1Error> {
    let mut out = Vec::with_capacity(text.len());
    for ch in text.chars() {
        let cp = ch as u32;
        if cp > 0xFF {
            return Err(Latin1Error::Unrepresentable { ch, codepoint: cp });
        }
        out.push(cp as u8);
    }
    Ok(out)
}

/// Encode a string as a Latin-1 payload, escaping characters above `U+00FF`
/// as uppercase `\uXXXX` escapes.
///
/// This is the write path used for edited documents: it never fails, and the
/// resulting bytes are valid JSON provided the input text was valid JSON. Only
/// characters inside string literals can exceed `U+00FF` in valid JSON, so the
/// escapes always land inside a string.
pub fn encode_escaped(text: &str) -> Vec<u8> {
    fn push_escape(out: &mut Vec<u8>, unit: u16) {
        out.extend_from_slice(b"\\u");
        for shift in (0..4).rev() {
            let nibble = ((unit >> (shift * 4)) & 0xF) as usize;
            out.push(b"0123456789ABCDEF"[nibble]);
        }
    }

    let mut out = Vec::with_capacity(text.len());
    for ch in text.chars() {
        let cp = ch as u32;
        if cp <= 0xFF {
            out.push(cp as u8);
        } else if cp <= 0xFFFF {
            push_escape(&mut out, cp as u16);
        } else {
            // Above the BMP: emit the UTF-16 surrogate pair, as JSON requires.
            let v = cp - 0x1_0000;
            push_escape(&mut out, 0xD800 | ((v >> 10) as u16));
            push_escape(&mut out, 0xDC00 | ((v & 0x3FF) as u16));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_every_byte_round_trip() {
        let bytes: Vec<u8> = (0u8..=255).collect();
        let text = decode(&bytes);
        assert_eq!(text.chars().count(), 256);
        assert_eq!(encode(&text).unwrap(), bytes);
    }

    #[test]
    fn decodes_latin1_not_utf8() {
        // 0xE9 is 'é' in Latin-1 but an invalid UTF-8 lead byte on its own.
        let text = decode(&[0xE9, 0x80]);
        assert_eq!(text, "\u{E9}\u{80}");
        assert_eq!(encode(&text).unwrap(), vec![0xE9, 0x80]);
    }

    #[test]
    fn rejects_unicode_above_latin1() {
        let err = encode("emoji \u{1F680}").unwrap_err();
        assert_eq!(err, Latin1Error::Unrepresentable { ch: '\u{1F680}', codepoint: 0x1F680 });
    }

    #[test]
    fn escapes_unicode_above_latin1() {
        let bytes = encode_escaped("a\u{00E9}b\u{1F680}");
        assert_eq!(bytes, b"a\xE9b\\uD83D\\uDE80");
    }
}
