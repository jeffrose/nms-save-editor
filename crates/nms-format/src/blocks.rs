//! LZ4 block container used by No Man's Sky save files.
//!
//! A container is a sequence of blocks, each with a 16-byte header:
//!
//! ```text
//! offset 0  u32 LE  magic            0xFEEDA1E5
//! offset 4  u32 LE  compressed_size  bytes of LZ4 payload that follow the header
//! offset 8  u32 LE  decompressed_size (<= 0x80000, i.e. 512 KiB)
//! offset 12 u32 LE  reserved         zero in every observed file
//! offset 16 ...     LZ4 block payload (raw block format, not a frame)
//! ```
//!
//! The concatenated decompressed payload is the save JSON, Latin-1 encoded.
//! Repeated block boundaries are not significant to the game.

use crate::error::FormatError;

/// Little-endian magic (`0xFEEDA1E5`) that starts every block header.
pub const BLOCK_MAGIC: u32 = 0xFEED_A1E5;
/// Size of one block header in bytes.
pub const HEADER_LEN: usize = 0x10;
/// Largest decompressed payload allowed in a single block (512 KiB).
pub const MAX_CHUNK: usize = 0x80000;

/// One block header, as read from a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockLayout {
    /// Number of compressed payload bytes following the header.
    pub compressed: u32,
    /// Number of bytes this block expands to.
    pub decompressed: u32,
    /// The reserved header word; zero in every observed save.
    pub reserved: u32,
}

/// Returns true when `bytes` starts with the block magic.
pub fn is_block_container(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && u32::from_le_bytes(bytes[..4].try_into().unwrap()) == BLOCK_MAGIC
}

/// Decompress a block container, returning the payload and the original block
/// layout (useful for diagnostics and for reporting without rewriting).
pub fn read(bytes: &[u8]) -> Result<(Vec<u8>, Vec<BlockLayout>), FormatError> {
    let mut payload = Vec::new();
    let mut layout = Vec::new();
    let mut offset = 0usize;

    while offset < bytes.len() {
        if offset + HEADER_LEN > bytes.len() {
            return Err(FormatError::TruncatedHeader {
                offset,
                len: bytes.len() - offset,
                needed: HEADER_LEN,
            });
        }
        let header = &bytes[offset..offset + HEADER_LEN];
        let magic = u32::from_le_bytes(header[0..4].try_into().unwrap());
        if magic != BLOCK_MAGIC {
            return Err(FormatError::BadBlockMagic { offset, found: magic });
        }
        let compressed = u32::from_le_bytes(header[4..8].try_into().unwrap());
        let decompressed = u32::from_le_bytes(header[8..12].try_into().unwrap());
        let reserved = u32::from_le_bytes(header[12..16].try_into().unwrap());

        if decompressed as usize > MAX_CHUNK {
            return Err(FormatError::BlockTooLarge {
                offset,
                declared: decompressed,
                limit: MAX_CHUNK,
            });
        }

        let payload_start = offset + HEADER_LEN;
        let payload_end = payload_start
            .checked_add(compressed as usize)
            .ok_or(FormatError::TruncatedPayload { offset, declared: compressed })?;
        if payload_end > bytes.len() {
            return Err(FormatError::TruncatedPayload { offset, declared: compressed });
        }

        let block = lz4_flex::block::decompress(&bytes[payload_start..payload_end], decompressed as usize)
            .map_err(|e| FormatError::Decompress { offset, message: e.to_string() })?;

        payload.extend_from_slice(&block);
        layout.push(BlockLayout { compressed, decompressed, reserved });
        offset = payload_end;
    }

    Ok((payload, layout))
}

/// Compress a payload into a block container.
///
/// The game's own writer uses 512 KiB chunks; matching that keeps output
/// structurally familiar and bounds per-block memory use.
pub fn write(payload: &[u8]) -> Vec<u8> {
    fn push_block(out: &mut Vec<u8>, chunk: &[u8]) {
        let compressed = lz4_flex::block::compress(chunk);
        out.extend_from_slice(&BLOCK_MAGIC.to_le_bytes());
        out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        out.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&compressed);
    }

    let mut out = Vec::with_capacity(payload.len() / 3 + 64);
    if payload.is_empty() {
        // A zero-byte file would be rejected by the game; emit one empty block.
        push_block(&mut out, &[]);
        return out;
    }
    for chunk in payload.chunks(MAX_CHUNK) {
        push_block(&mut out, chunk);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_small_and_multi_block_payloads() {
        for size in [0usize, 1, 100, MAX_CHUNK - 1, MAX_CHUNK, MAX_CHUNK + 1, MAX_CHUNK * 3 + 7] {
            let payload: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            let container = write(&payload);
            let (back, layout) = read(&container).unwrap();
            assert_eq!(back, payload, "payload mismatch at size {size}");
            let expected_blocks = if size == 0 { 1 } else { size.div_ceil(MAX_CHUNK) };
            assert_eq!(layout.len(), expected_blocks, "block count at size {size}");
            assert!(layout.iter().all(|b| b.reserved == 0));
        }
    }

    #[test]
    fn rejects_bad_magic() {
        let mut container = write(b"hello");
        container[0] ^= 0xFF;
        assert!(matches!(read(&container), Err(FormatError::BadBlockMagic { .. })));
    }

    #[test]
    fn rejects_truncated_payload() {
        let mut container = write(b"hello world");
        container.truncate(container.len() - 3);
        assert!(matches!(read(&container), Err(FormatError::TruncatedPayload { .. })));
    }

    #[test]
    fn detects_container_by_magic() {
        assert!(is_block_container(&write(b"x")));
        assert!(!is_block_container(b"{\"F2P\":1}"));
    }
}
