//! Errors produced by the save-format codec.

use thiserror::Error;

/// Anything that can go wrong reading or writing an NMS save container or its
/// metadata.
#[derive(Debug, Error)]
pub enum FormatError {
    /// The payload was empty or too short to contain a block header.
    #[error("truncated block header at offset {offset}: {len} bytes remain, need {needed}")]
    TruncatedHeader {
        /// Byte offset of the incomplete header.
        offset: usize,
        /// Bytes remaining at that offset.
        len: usize,
        /// Bytes needed for a full header.
        needed: usize,
    },

    /// A block header did not start with [`crate::blocks::BLOCK_MAGIC`].
    #[error("bad block magic 0x{found:08X} at offset {offset}")]
    BadBlockMagic {
        /// Byte offset of the offending header.
        offset: usize,
        /// The magic value found.
        found: u32,
    },

    /// A block declared more decompressed bytes than the format allows.
    #[error("block at offset {offset} declares {declared} bytes, limit is {limit}")]
    BlockTooLarge {
        /// Byte offset of the offending header.
        offset: usize,
        /// The declared decompressed size.
        declared: u32,
        /// The format limit.
        limit: usize,
    },

    /// A block's declared compressed size runs past the end of the file.
    #[error("block at offset {offset} declares {declared} compressed bytes past end of file")]
    TruncatedPayload {
        /// Byte offset of the offending header.
        offset: usize,
        /// The declared compressed size.
        declared: u32,
    },

    /// LZ4 block decompression failed.
    #[error("LZ4 decompression failed for block at offset {offset}: {message}")]
    Decompress {
        /// Byte offset of the offending header.
        offset: usize,
        /// Underlying decoder message.
        message: String,
    },

    /// The metadata file length is not one of the known format sizes.
    #[error("metadata length {length} is not a known NMS metadata size")]
    UnknownMetadataLength {
        /// Observed file length in bytes.
        length: usize,
    },

    /// No storage-slot key decrypted the metadata (wrong length or corrupt file).
    #[error("metadata did not decrypt with any known slot key")]
    MetadataDecrypt,

    /// The decrypted metadata did not start with the expected sentinel.
    #[error("decrypted metadata magic 0x{found:08X} is not the expected 0xEEEEEEBE")]
    BadMetadataMagic {
        /// The value found after decryption.
        found: u32,
    },

    /// The metadata format version is known but not supported for writing.
    #[error("metadata format 0x{version:04X} is not supported")]
    UnsupportedMetadataFormat {
        /// The decrypted format word.
        version: u32,
    },

    /// A save file name did not map to a storage slot.
    #[error("cannot determine storage slot from file name {name:?}")]
    UnknownSlot {
        /// The file name that was not recognised.
        name: String,
    },

    /// I/O failure.
    #[error("{context}: {source}")]
    Io {
        /// What the codec was doing.
        context: &'static str,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
}

impl FormatError {
    pub(crate) fn io(context: &'static str, source: std::io::Error) -> Self {
        FormatError::Io { context, source }
    }
}
