//! Reader for HGPAK archives, the container No Man's Sky ships its data in.
//!
//! Game data — item tables, localisation, and the field definitions the key
//! mapping is derived from — lives inside `GAMEDATA/PCBANKS/NMSARC.*.pak`. To
//! regenerate an editor's data pack from a player's own install, the first step
//! is reading that container, which is all this crate does: no game knowledge,
//! no MBIN parsing, just bytes.
//!
//! Layout (HGPAK version 2), verified against the game's own files:
//!
//! ```text
//! 0x00  "HGPAK" + 3 bytes padding
//! 0x08  u64  version             2
//! 0x10  u64  file_count          entries in the file index, including the name table
//! 0x18  u64  chunk_count
//! 0x20  u8   is_compressed + 7 bytes padding
//! 0x28  u64  data_offset         where the first compressed chunk starts
//! 0x30  file index   file_count × 32 bytes:
//!          16 bytes  hash (md5 of the lower-cased path, big-endian)
//!          u64       start offset
//!          u64       decompressed size
//!       chunk index  chunk_count × u64 compressed size
//!       padding to data_offset
//! data_offset  chunk table: chunk_count zstd frames, each padded to a
//!              16-byte boundary (the declared size is the frame length)
//! ```
//!
//! The chunks decompress into one virtual buffer. Entry 0 of the file index is
//! the **name table**: a CRLF-separated, CRLF-terminated list of the remaining
//! entries' paths. Each later entry's virtual offset is its recorded start
//! offset minus `data_offset`, and its payload is the decompressed bytes at that
//! offset for `decompressed_size` bytes.
//!
//! References for the format facts (both MIT): `monkeyman192/HGPAKtool`'s
//! `hgpaktool/api.py` and the Go port in `ushineko/nmsbonker`. This is an
//! independent implementation; the game's files are the test data.

use std::io::Read;
use std::path::{Path, PathBuf};

/// Size of the fixed header before the file index.
const HEADER_LEN: usize = 0x30;
/// Size of one file-index entry.
const FILE_ENTRY_LEN: usize = 0x20;
/// Decompressed size of one chunk. Chunks are fixed-size, zero-padded at the end.
const CHUNK_DECOMPRESSED: usize = 0x10000;
/// Zstd frame magic, little-endian: every compressed chunk starts with it.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];
/// Compressed chunks are laid out on 16-byte boundaries: the declared size is
/// the frame length, and the next frame starts after padding to this alignment.
/// Verified against the game's own archives (`NMSARC.MetadataEtc.pak`: declared
/// sizes 0x16DA, 0xC88, 0xB2A, 0xD4F …; observed frame starts 0x0, 0x16E0,
/// 0x2370, 0x2EA0 …).
const CHUNK_ALIGNMENT: u64 = 16;

fn align_up(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment) * alignment
}

/// Errors from reading an archive.
#[derive(Debug, thiserror::Error)]
pub enum PakError {
    /// The file does not start with the HGPAK magic or has an unknown version.
    #[error("{path}: not an HGPAK version 2 archive")]
    NotHgpak {
        /// Archive path.
        path: String,
    },

    /// A structure ran past the end of the file.
    #[error("{path}: truncated archive ({what})")]
    Truncated {
        /// Archive path.
        path: String,
        /// Which structure was being read.
        what: &'static str,
    },

    /// A chunk's decompressed length contradicts the layout model, which means
    /// a mixed-storage archive defeated the single-probe assumption.
    #[error("{path}: chunk {index} decompressed to {found} bytes, expected {expected}")]
    ChunkLength {
        /// Archive path.
        path: String,
        /// Chunk index.
        index: usize,
        /// Length the layout implies.
        expected: usize,
        /// Length the decoder produced.
        found: usize,
    },

    /// A chunk failed to decompress.
    #[error("{path}: chunk {index} failed to decompress: {message}")]
    Chunk {
        /// Archive path.
        path: String,
        /// Chunk index.
        index: usize,
        /// Decompressor message.
        message: String,
    },

    /// The requested entry is not in the archive.
    #[error("{name:?} is not in {path}")]
    NoSuchEntry {
        /// Archive path.
        path: String,
        /// Requested entry name.
        name: String,
    },

    /// I/O failure.
    #[error("{context}: {source}")]
    Io {
        /// What was being done.
        context: &'static str,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
}

/// One file inside an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PakEntry {
    /// Path within the archive, lower-cased, with `/` separators.
    pub name: String,
    /// The 16-byte hash from the file index (md5 of the lower-cased path).
    pub hash: [u8; 16],
    /// Offset within the concatenated decompressed chunks.
    pub offset: u64,
    /// Uncompressed size in bytes.
    pub size: u64,
}

/// An opened archive.
///
/// Only the header and both indexes are read up front: `PCBANKS` holds archives
/// up to several gigabytes, so chunk payloads are seeked and decompressed on
/// demand, and the decompressed chunks a request touches are cached. Reading a
/// single file therefore never loads an archive into memory.
pub struct Pak {
    path: PathBuf,
    /// Compressed (stored) chunk sizes, in order.
    chunk_sizes: Vec<u64>,
    /// Absolute file offset of each chunk.
    chunk_offsets: Vec<u64>,
    /// Cumulative decompressed size *before* each chunk.
    chunk_starts: Vec<u64>,
    /// Whether each chunk is a zstd frame (false = stored verbatim).
    compressed: Vec<bool>,
    /// True for archives whose payloads are stored as-is (no chunks at all).
    uncompressed: bool,
    /// Decompressed chunks, indexed by chunk number.
    cache: std::cell::RefCell<std::collections::HashMap<usize, Vec<u8>>>,
    /// Every entry except the name table.
    entries: Vec<PakEntry>,
}

impl Pak {
    /// Open and index an archive.
    pub fn open(path: &Path) -> Result<Pak, PakError> {
        let path_string = path.display().to_string();
        let mut file = std::fs::File::open(path).map_err(|source| PakError::Io { context: "opening archive", source })?;
        let file_len = file
            .metadata()
            .map_err(|source| PakError::Io { context: "reading archive size", source })?
            .len();

        // Header, then both indexes in one read: together they are well under a
        // megabyte even for the largest archive.
        let mut header = [0u8; HEADER_LEN];
        read_exact_at(&mut file, 0, &mut header, &path_string)?;
        let raw = &header[..];

        if &raw[0..5] != b"HGPAK" {
            return Err(PakError::NotHgpak { path: path_string });
        }
        let le_u64 = |offset: usize| -> u64 {
            u64::from_le_bytes(raw[offset..offset + 8].try_into().expect("checked length"))
        };
        let version = le_u64(0x08);
        if version != 2 {
            return Err(PakError::NotHgpak { path: path_string });
        }
        let file_count = le_u64(0x10) as usize;
        let chunk_count = le_u64(0x18) as usize;
        let is_compressed = raw[0x20] != 0;
        let data_offset = le_u64(0x28);
        if !is_compressed {
            // Uncompressed archives store the name table and every file at
            // absolute offsets in the archive; there is no chunk table.
            return Pak::open_uncompressed(path, &path_string, &mut file, data_offset, file_count);
        }

        // File index: hash, start offset, decompressed size per entry.
        let index_len = file_count.checked_mul(FILE_ENTRY_LEN).ok_or(PakError::Truncated {
            path: path_string.clone(),
            what: "file index",
        })?;
        let index_end = HEADER_LEN + index_len;
        let chunk_end = index_end + chunk_count * 8;
        if chunk_end as u64 > file_len {
            return Err(PakError::Truncated { path: path_string, what: "file or chunk index" });
        }

        let mut indexes = vec![0u8; index_len + chunk_count * 8];
        read_exact_at(&mut file, HEADER_LEN as u64, &mut indexes, &path_string)?;

        let mut raw_entries: Vec<([u8; 16], u64, u64)> = Vec::with_capacity(file_count);
        for index in 0..file_count {
            let base = index * FILE_ENTRY_LEN;
            let mut hash = [0u8; 16];
            hash.copy_from_slice(&indexes[base..base + 16]);
            let start = u64::from_le_bytes(indexes[base + 16..base + 24].try_into().expect("checked"));
            let size = u64::from_le_bytes(indexes[base + 24..base + 32].try_into().expect("checked"));
            raw_entries.push((hash, start, size));
        }
        let chunk_sizes: Vec<u64> = (0..chunk_count)
            .map(|index| {
                let base = index_len + index * 8;
                u64::from_le_bytes(indexes[base..base + 8].try_into().expect("checked"))
            })
            .collect();

        let mut chunk_offsets = Vec::with_capacity(chunk_count);
        let mut offset = data_offset;
        for size in &chunk_sizes {
            chunk_offsets.push(offset);
            offset += align_up(*size, CHUNK_ALIGNMENT);
        }

        // Compressed chunks decompress to a fixed 0x10000 bytes; chunks stored
        // verbatim (the texture archives do that, sharing the same header)
        // contribute exactly their stored length. Probing the four-byte magic of
        // every chunk up front is what makes a virtual offset mappable without
        // decompressing, and costs one seek per chunk on a warm cache.
        let magic_of = |file: &mut std::fs::File, index: usize| -> Option<[u8; 4]> {
            if chunk_sizes[index] < 4 {
                return None;
            }
            let mut magic = [0u8; 4];
            read_exact_at(file, chunk_offsets[index], &mut magic, "").ok()?;
            Some(magic)
        };

        // The data archives that matter here are uniformly frame-based, so one
        // probe answers for the whole archive; the texture archives mix raw and
        // frame chunks, and their first chunk is raw. Probing every chunk of a
        // multi-gigabyte mesh archive would be thousands of seeks for no gain.
        let first_is_frame = matches!(magic_of(&mut file, 0), Some(magic) if magic == ZSTD_MAGIC);
        let mut compressed = Vec::with_capacity(chunk_count);
        let mut chunk_starts = Vec::with_capacity(chunk_count);
        let mut virtual_offset = 0u64;
        for index in 0..chunk_count {
            chunk_starts.push(virtual_offset);
            let is_frame = if first_is_frame {
                true
            } else {
                matches!(magic_of(&mut file, index), Some(magic) if magic == ZSTD_MAGIC)
            };
            compressed.push(is_frame);
            virtual_offset += if is_frame { CHUNK_DECOMPRESSED as u64 } else { chunk_sizes[index] };
        }
        if offset > file_len {
            return Err(PakError::Truncated { path: path_string, what: "chunk data" });
        }

        let mut pak = Pak {
            path: path.to_path_buf(),
            chunk_sizes,
            chunk_offsets,
            chunk_starts,
            compressed,
            uncompressed: false,

            cache: std::cell::RefCell::new(std::collections::HashMap::new()),
            entries: Vec::new(),
        };

        // Entry 0 is the name table; the rest are files, in the same order.
        let Some(&(_, table_offset, table_size)) = raw_entries.first() else {
            return Ok(pak);
        };
        let table = pak.read_virtual(table_offset.saturating_sub(data_offset), table_size as usize)?;
        let text = String::from_utf8_lossy(&table);
        let names: Vec<&str> = text.trim_end_matches(['\r', '\n']).split("\r\n").collect();

        pak.entries = names
            .iter()
            .zip(raw_entries.iter().skip(1))
            .filter(|(name, _)| !name.is_empty())
            .map(|(name, &(hash, start, size))| PakEntry {
                name: name.to_string(),
                hash,
                offset: start.saturating_sub(data_offset),
                size,
            })
            .collect();

        Ok(pak)
    }

    /// Open an archive whose payloads are stored uncompressed.
    fn open_uncompressed(
        path: &Path,
        path_string: &str,
        file: &mut std::fs::File,
        _data_offset: u64,
        file_count: usize,
    ) -> Result<Pak, PakError> {
        let mut indexes = vec![0u8; file_count * FILE_ENTRY_LEN];
        read_exact_at(file, HEADER_LEN as u64, &mut indexes, path_string)?;

        let mut raw_entries: Vec<([u8; 16], u64, u64)> = Vec::with_capacity(file_count);
        for index in 0..file_count {
            let base = index * FILE_ENTRY_LEN;
            let mut hash = [0u8; 16];
            hash.copy_from_slice(&indexes[base..base + 16]);
            let start = u64::from_le_bytes(indexes[base + 16..base + 24].try_into().expect("checked"));
            let size = u64::from_le_bytes(indexes[base + 24..base + 32].try_into().expect("checked"));
            raw_entries.push((hash, start, size));
        }

        let Some(&(_, table_offset, table_size)) = raw_entries.first() else {
            return Ok(Pak {
                path: path.to_path_buf(),
                chunk_sizes: Vec::new(),
                chunk_offsets: Vec::new(),
                chunk_starts: vec![0],
                compressed: Vec::new(),
                uncompressed: true,
                cache: std::cell::RefCell::new(std::collections::HashMap::new()),
                entries: Vec::new(),
            });
        };

        let mut table = vec![0u8; table_size as usize];
        read_exact_at(file, table_offset, &mut table, path_string)?;
        let text = String::from_utf8_lossy(&table);
        let names: Vec<&str> = text.trim_end_matches(['\r', '\n']).split("\r\n").collect();

        let entries = names
            .iter()
            .zip(raw_entries.iter().skip(1))
            .filter(|(name, _)| !name.is_empty())
            .map(|(name, &(hash, start, size))| PakEntry {
                name: name.to_string(),
                hash,
                offset: start,
                size,
            })
            .collect();

        Ok(Pak {
            path: path.to_path_buf(),
            chunk_sizes: Vec::new(),
            chunk_offsets: Vec::new(),
            chunk_starts: vec![0],
            compressed: Vec::new(),
            uncompressed: true,
            cache: std::cell::RefCell::new(std::collections::HashMap::new()),
            entries,
        })
    }

    /// True when payloads are stored as-is instead of in chunks.
    pub fn is_uncompressed(&self) -> bool {
        self.uncompressed
    }

    /// Every entry in the archive, sorted as the archive lists them.
    pub fn entries(&self) -> &[PakEntry] {
        &self.entries
    }

    /// Number of chunks.
    pub fn chunk_count(&self) -> usize {
        self.chunk_sizes.len()
    }

    /// How many chunks are zstd frames (the rest are stored verbatim).
    pub fn compressed_chunk_count(&self) -> usize {
        self.compressed.iter().filter(|flag| **flag).count()
    }

    /// Archive path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Find entries whose name contains `needle` (case-insensitive).
    pub fn find(&self, needle: &str) -> Vec<&PakEntry> {
        let needle = needle.to_lowercase();
        self.entries.iter().filter(|entry| entry.name.contains(&needle)).collect()
    }

    /// Case-insensitive exact lookup.
    pub fn entry(&self, name: &str) -> Option<&PakEntry> {
        let name = name.to_lowercase();
        self.entries.iter().find(|entry| entry.name == name)
    }

    /// Extract one entry's decompressed bytes.
    pub fn extract(&self, entry: &PakEntry) -> Result<Vec<u8>, PakError> {
        if self.uncompressed {
            return self.read_absolute(entry.offset, entry.size as usize);
        }
        self.read_virtual(entry.offset, entry.size as usize)
    }

    /// Read bytes straight out of an uncompressed archive.
    fn read_absolute(&self, offset: u64, len: usize) -> Result<Vec<u8>, PakError> {
        let path = self.path.display().to_string();
        let mut file = std::fs::File::open(&self.path)
            .map_err(|source| PakError::Io { context: "opening archive", source })?;
        let mut out = vec![0u8; len];
        read_exact_at(&mut file, offset, &mut out, &path)?;
        Ok(out)
    }

    /// Extract by name.
    pub fn extract_name(&self, name: &str) -> Result<Vec<u8>, PakError> {
        let entry = self.entry(name).ok_or_else(|| PakError::NoSuchEntry {
            path: self.path.display().to_string(),
            name: name.to_string(),
        })?;
        self.extract(entry)
    }

    /// Read `len` bytes at `offset` in the concatenated decompressed chunks.
    fn read_virtual(&self, offset: u64, len: usize) -> Result<Vec<u8>, PakError> {
        let mut out = Vec::with_capacity(len);
        let path = self.path.display().to_string();
        let mut remaining = len;
        let mut position = offset;

        while remaining > 0 {
            // Last chunk whose start is at or before the position.
            let index = match self.chunk_starts.binary_search(&position) {
                Ok(found) => found,
                Err(next) => next.saturating_sub(1),
            };
            let Some(data) = self.chunk_opt(index)? else {
                return Err(PakError::Truncated { path, what: "virtual offset" });
            };
            let expected = if self.compressed.get(index).copied().unwrap_or(false) {
                CHUNK_DECOMPRESSED
            } else {
                self.chunk_sizes[index] as usize
            };
            if data.len() != expected {
                return Err(PakError::ChunkLength {
                    path,
                    index,
                    expected,
                    found: data.len(),
                });
            }
            let within = (position - self.chunk_starts[index]) as usize;
            if within >= data.len() {
                return Err(PakError::Truncated { path, what: "chunk boundary" });
            }
            let take = (data.len() - within).min(remaining);
            out.extend_from_slice(&data[within..within + take]);
            self.cache.borrow_mut().insert(index, data);
            position += take as u64;
            remaining -= take;
        }
        Ok(out)
    }

    /// Decompress chunk `index`, reusing the cache when present.
    ///
    /// Cloned out of the cache rather than borrowed, because the caller
    /// frequently needs the bytes while also inserting a freshly decompressed
    /// chunk; at most one chunk (64 KiB) is copied per call.
    fn chunk_opt(&self, index: usize) -> Result<Option<Vec<u8>>, PakError> {
        if let Some(cached) = self.cache.borrow().get(&index) {
            return Ok(Some(cached.clone()));
        }
        self.decompress_chunk(index).map(Some)
    }


    fn decompress_chunk(&self, index: usize) -> Result<Vec<u8>, PakError> {
        let path = self.path.display().to_string();
        let (Some(size), Some(start)) =
            (self.chunk_sizes.get(index).copied(), self.chunk_offsets.get(index).copied())
        else {
            return Err(PakError::Truncated { path, what: "chunk index" });
        };

        let mut file = std::fs::File::open(&self.path)
            .map_err(|source| PakError::Io { context: "opening archive", source })?;
        let mut compressed = vec![0u8; size as usize];
        read_exact_at(&mut file, start, &mut compressed, &path)?;

        // Verbatim chunks are their own content.
        if !self.compressed.get(index).copied().unwrap_or(false) {
            return Ok(compressed);
        }

        let mut decoder = ruzstd::decoding::StreamingDecoder::new(compressed.as_slice())
            .map_err(|e| PakError::Chunk { path: path.clone(), index, message: e.to_string() })?;
        let mut out = Vec::with_capacity(CHUNK_DECOMPRESSED);
        match decoder.read_to_end(&mut out) {
            Ok(_) => Ok(out),
            Err(error) => Err(PakError::Chunk { path, index, message: error.to_string() }),
        }
    }

}


/// Read exactly `buffer.len()` bytes at `offset`.
fn read_exact_at(
    file: &mut std::fs::File,
    offset: u64,
    buffer: &mut [u8],
    path: &str,
) -> Result<(), PakError> {
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(offset))
        .map_err(|source| PakError::Io { context: "seeking in archive", source })?;
    file.read_exact(buffer).map_err(|_| PakError::Truncated {
        path: path.to_string(),
        what: "archive data",
    })
}

impl std::fmt::Debug for Pak {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pak")
            .field("path", &self.path)
            .field("entries", &self.entries.len())
            .field("chunks", &self.chunk_sizes.len())
            .finish()
    }
}
