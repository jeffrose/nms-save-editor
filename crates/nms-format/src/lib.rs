//! Format codec for No Man's Sky save files.
//!
//! This crate owns the byte level: LZ4 block containers, Latin-1 JSON
//! payloads, storage-slot identification, and the XXTEA-encrypted `mf_*.hg`
//! metadata files. It has no game knowledge beyond the container layout, which
//! keeps it testable against real saves with no interpretation involved.
//!
//! The integrity contract this crate exists to support:
//!
//! * reading a save and writing it back **without edits is byte-identical**;
//! * metadata is *patched*, never regenerated, so unknown fields survive;
//! * every write goes through an atomic temp-file-and-rename.
//!
//! ```no_run
//! use std::path::Path;
//! use nms_format::{SaveFile, metadata::{Metadata, MetadataPatch, Slot}};
//!
//! let dir = Path::new("/path/to/save/dir");
//! let mut save = SaveFile::read(&dir.join("save.hg"))?;
//! let slot = Slot::from_file_name("save.hg").unwrap();
//!
//! // ... edit save.payload ...
//!
//! let meta_bytes = std::fs::read(dir.join(slot.metadata_file_name()))?;
//! let meta = Metadata::read(&meta_bytes, slot)?;
//! let patched = meta.patch_to_bytes(&MetadataPatch {
//!     decompressed_size: Some(save.stored_payload().len() as u32),
//!     compressed_size: Some(save.to_bytes().len() as u32),
//!     ..Default::default()
//! })?;
//!
//! save.write(&dir.join("save.hg"))?;
//! std::fs::write(dir.join(slot.metadata_file_name()), patched)?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod blocks;
pub mod error;
pub mod latin1;
pub mod metadata;
pub mod save;
pub mod xxtea;

pub use error::FormatError;
pub use metadata::{Metadata, MetadataPatch, Slot};
pub use save::{Container, SaveFile, WriteOutcome, write_atomic};
