# HGPAK archives

The game ships its data in `GAMEDATA/PCBANKS/NMSARC.*.pak`, about 32 GB across
24 archives. [`crates/nms-pak`](../../crates/nms-pak) reads them; everything here
was confirmed against the installed game.

## Layout (version 2)

```text
0x00  "HGPAK" + 3 bytes padding
0x08  u64  version              2
0x10  u64  file_count           entries in the file index, including the name table
0x18  u64  chunk_count
0x20  u8   is_compressed + 7 bytes padding
0x28  u64  data_offset          where the first chunk starts
0x30  file index    file_count × 32 bytes:
          16 bytes  hash (md5 of the lower-cased path)
          u64       start offset
          u64       decompressed size
      chunk index   chunk_count × u64 stored size
      padding to data_offset
data_offset  chunk table
```

The file index has one extra entry at the front: **entry 0 is the name table**, a
CRLF-separated list of the remaining entries' paths, in order.

## Three rules that are easy to get wrong

1. **Chunks are 16-byte aligned.** The declared size is the frame length, and the
   next chunk starts after padding to a multiple of 16. Without this, chunk 1
   starts in the wrong place — visible immediately as "bad magic" on a large
   archive. Verified on `NMSARC.MetadataEtc.pak`: declared sizes `0x16DA, 0xC88,
   0xB2A, 0xD4F …`, observed frame starts `0x0, 0x16E0, 0x2370, 0x2EA0 …`.
2. **Chunks are addressable through one virtual space.** Compressed chunks
   decompress to exactly `0x10000` bytes, so a file's offset is resolved by
   dividing by that size. File offsets in the index are relative to
   `data_offset`.
3. **Some archives store payloads verbatim.** The texture archives share the
   header but hold raw bytes (a chunk starts with `textures/...` instead of a
   zstd frame; `NMSARC.TexBiomesALPINE.pak` has 114 such chunks), and the audio
   archive has `is_compressed = 0` with no chunks at all. A chunk that does not
   begin with the zstd magic is its own content, and a verbatim chunk contributes
   its stored length rather than `0x10000`.

## Behaviour worth keeping

* **Open is cheap and lazy.** Only the header and both indexes are read up front;
  payloads are seeked and decompressed on demand, with the chunks a request
  touches cached. Indexing the 743 MB `NMSARC.MeshCommon.pak` (36,911 chunks)
  takes 2 ms, and no archive is ever loaded into memory.
* **A wrong assumption fails loudly.** If a chunk's decompressed length
  contradicts the layout model, the reader returns an error instead of handing
  back bytes at the wrong offset — see `PakError::ChunkLength`.
* **Failure is per archive.** The data pipeline skips an archive it cannot read
  and records why, because a mesh archive has no text in it and its absence does
  not matter.

## Known limitation

Some entries in the metadata archives point past the end of the chunk region
(2,614 of 50,805 in `NMSARC.MetadataEtc.pak`, in a second contiguous region whose
structure has not been worked out). Reads of those entries fail with a clear
error. Nothing needed for the data pack lives there: every localisation table and
every table read so far resolves inside the chunk region.

## References

The layout is documented in the community tools `monkeyman192/HGPAKtool`
(MIT, Python) and its Go port in `ushineko/nmsbonker`. This is an independent
implementation written against the game's own files; the extraction results were
cross-checked byte-for-byte against a separate extraction path.
