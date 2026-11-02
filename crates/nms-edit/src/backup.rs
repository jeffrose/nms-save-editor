//! Timestamped zip backups of save directories.
//!
//! Every write path takes a backup first. Backups are named
//! `{label}_{yyyyMMdd_HHmmss}.zip` (UTC) so they sort chronologically, and the
//! store keeps a bounded number per label. An archive can be pinned, which
//! renames it to `{label}_{yyyyMMdd_HHmmss}.keep.zip` and takes it out of the
//! retention count — a restore point kept on purpose, not by luck.
//!
//! Reading an archive's [`ArchiveSummary`] does not decompress the save
//! payload: the metadata file inside says which save it is, when the game last
//! wrote it and how long it had been played, which is what choosing between
//! restore points needs.

use std::path::{Path, PathBuf};

use nms_format::metadata::{Metadata, Slot};

use crate::EditError;

/// Backup failures.
#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    /// The archive could not be written.
    #[error("writing backup {path}: {message}")]
    Write {
        /// Archive path.
        path: String,
        /// Underlying reason.
        message: String,
    },
    /// The backup directory could not be created or listed.
    #[error("backup directory {path}: {source}")]
    Directory {
        /// Directory path.
        path: String,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
}

impl From<BackupError> for EditError {
    fn from(error: BackupError) -> Self {
        EditError::Backup(Box::new(error))
    }
}

/// A backup archive on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupEntry {
    /// Full path of the archive.
    pub path: PathBuf,
    /// Archive size in bytes.
    pub size: u64,
    /// Whether the archive is pinned against pruning.
    pub kept: bool,
}

/// Suffix that marks an archive as pinned.
const KEEP_SUFFIX: &str = ".keep.zip";

impl BackupEntry {
    /// The file name of the archive.
    pub fn name(&self) -> String {
        self.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// The label the file name carries (the save's file stem).
    pub fn label(&self) -> Option<String> {
        Some(self.name().split('_').next()?.to_string())
    }
}

/// Creates and prunes timestamped backups.
#[derive(Debug, Clone)]
pub struct BackupStore {
    root: PathBuf,
    keep: usize,
}

impl BackupStore {
    /// A store keeping the newest `keep` archives per label (minimum 1).
    pub fn new(root: impl Into<PathBuf>, keep: usize) -> BackupStore {
        BackupStore { root: root.into(), keep: keep.max(1) }
    }

    /// The directory backups are written to.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// How many archives are kept per label.
    pub fn keep(&self) -> usize {
        self.keep
    }

    /// Archive `files` as `{label}_{timestamp}.zip`, then prune old archives.
    ///
    /// `files` maps an archive entry name to the file to read. Missing files
    /// are skipped rather than failing the backup.
    pub fn create(
        &self,
        label: &str,
        files: &[(String, PathBuf)],
        timestamp: u64,
    ) -> Result<Option<BackupEntry>, BackupError> {
        let present: Vec<&(String, PathBuf)> = files.iter().filter(|(_, path)| path.is_file()).collect();
        if present.is_empty() {
            return Ok(None);
        }

        std::fs::create_dir_all(&self.root).map_err(|source| BackupError::Directory {
            path: self.root.display().to_string(),
            source,
        })?;

        let archive = self.root.join(format!("{label}_{}.zip", format_timestamp(timestamp)));
        // Two saves inside the same second would otherwise share a name, and
        // the second would overwrite the first restore point.
        let mut archive = archive;
        let mut suffix = 2u32;
        while archive.exists() {
            archive = self
                .root
                .join(format!("{label}_{}_{suffix}.zip", format_timestamp(timestamp)));
            suffix += 1;
        }
        let file = std::fs::File::create(&archive).map_err(|source| BackupError::Directory {
            path: archive.display().to_string(),
            source,
        })?;
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        for (name, path) in present {
            let bytes = match std::fs::read(path) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            writer.start_file(name.clone(), options).map_err(|source| BackupError::Write {
                path: archive.display().to_string(),
                message: source.to_string(),
            })?;
            std::io::Write::write_all(&mut writer, &bytes).map_err(|source| BackupError::Write {
                path: archive.display().to_string(),
                message: source.to_string(),
            })?;
        }
        writer.finish().map_err(|source| BackupError::Write {
            path: archive.display().to_string(),
            message: source.to_string(),
        })?;

        let size = std::fs::metadata(&archive).map(|m| m.len()).unwrap_or(0);
        self.prune(label)?;
        Ok(Some(BackupEntry { path: archive, size, kept: false }))
    }

    /// Existing archives for `label`, newest first.
    pub fn list(&self, label: &str) -> Result<Vec<BackupEntry>, BackupError> {
        let prefix = format!("{label}_");
        Ok(self.list_all()?.into_iter().filter(|entry| entry.name().starts_with(&prefix)).collect())
    }

    /// Every archive in the backup folder, newest first.
    pub fn list_all(&self) -> Result<Vec<BackupEntry>, BackupError> {
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(BackupError::Directory { path: self.root.display().to_string(), source });
            }
        };

        let mut backups: Vec<BackupEntry> = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.ends_with(".zip")
            })
            .map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                BackupEntry {
                    size: entry.metadata().map(|m| m.len()).unwrap_or(0),
                    path: entry.path(),
                    kept: name.ends_with(KEEP_SUFFIX),
                }
            })
            .collect();
        backups.sort_by(|a, b| b.path.cmp(&a.path));
        Ok(backups)
    }

    /// Read what an archive holds.
    ///
    /// The save payload is not decompressed: the files are listed by size and
    /// the metadata file is decrypted, which is enough to tell one restore
    /// point from another (save name, game mode, play time, when the game last
    /// wrote it).
    pub fn inspect(&self, archive: &Path) -> Result<ArchiveSummary, BackupError> {
        let files = list_archive(archive)?;
        let reader = |name: &str| -> Option<Vec<u8>> { read_archive_entry(archive, name).ok().flatten() };

        // The data file names the slot; the metadata file says what state it
        // was in. They are two different name shapes, so each has its own
        // parser — `save7.hg` and `mf_save7.hg` both mean PlayerState7.
        let mut slot = None;
        let mut metadata = None;
        for file in &files {
            let Ok(name) = std::str::from_utf8(file.name.as_bytes()) else { continue };
            if let Some(candidate) = Slot::from_file_name(name) {
                if candidate.player_number().is_some() {
                    slot = Some(candidate);
                }
            } else if let Some(candidate) = Slot::from_metadata_file_name(name) {
                if candidate.player_number().is_some() {
                    slot.get_or_insert(candidate);
                    if let Some(bytes) = reader(name) {
                        metadata = Metadata::read(&bytes, candidate).ok();
                    }
                }
            }
        }
        Ok(ArchiveSummary {
            path: archive.to_path_buf(),
            size: std::fs::metadata(archive).map(|meta| meta.len()).unwrap_or(0),
            kept: is_kept(archive),
            files,
            slot,
            metadata,
        })
    }

    /// Delete one archive.
    pub fn delete(&self, archive: &Path) -> Result<(), BackupError> {
        std::fs::remove_file(archive).map_err(|source| BackupError::Directory {
            path: archive.display().to_string(),
            source,
        })
    }

    /// Pin an archive against pruning, or unpin it.
    ///
    /// Pinning renames the archive rather than writing a sidecar file, so the
    /// state survives copying the folder around and is visible in any file
    /// manager.
    pub fn set_kept(&self, archive: &Path, kept: bool) -> Result<PathBuf, BackupError> {
        let name = archive
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| BackupError::Write {
                path: archive.display().to_string(),
                message: "not an archive file name".to_string(),
            })?
            .to_string();
        let renamed = match (kept, name.ends_with(KEEP_SUFFIX)) {
            (true, true) | (false, false) => return Ok(archive.to_path_buf()),
            (true, false) => {
                let stem = name.strip_suffix(".zip").unwrap_or(&name);
                format!("{stem}{KEEP_SUFFIX}")
            }
            (false, true) => name.trim_end_matches(KEEP_SUFFIX).to_string() + ".zip",
        };
        let target = archive.with_file_name(renamed);
        std::fs::rename(archive, &target).map_err(|source| BackupError::Directory {
            path: archive.display().to_string(),
            source,
        })?;
        Ok(target)
    }

    /// Delete archives beyond the retention limit for `label`.
    ///
    /// Pinned archives are never removed and do not count against the limit.
    pub fn prune(&self, label: &str) -> Result<Vec<PathBuf>, BackupError> {
        let backups = self.list(label)?;
        let mut removed = Vec::new();
        let mut kept = 0usize;
        for entry in backups {
            if entry.kept {
                continue;
            }
            kept += 1;
            if kept > self.keep && std::fs::remove_file(&entry.path).is_ok() {
                removed.push(entry.path);
            }
        }
        Ok(removed)
    }
}

/// What an archive holds, for choosing between restore points.
#[derive(Debug, Clone)]
pub struct ArchiveSummary {
    /// Archive path.
    pub path: PathBuf,
    /// Size of the archive on disk.
    pub size: u64,
    /// Whether the archive is pinned against pruning.
    pub kept: bool,
    /// The files inside, in archive order.
    pub files: Vec<ArchivedFile>,
    /// The storage slot the archive's data file names.
    pub slot: Option<Slot>,
    /// The decrypted metadata file, when the archive holds one that reads.
    pub metadata: Option<Metadata>,
}

impl ArchiveSummary {
    /// When the game last wrote the state the archive holds, in Unix seconds.
    ///
    /// This is the metadata's timestamp word, which the editor sets to the
    /// time of the write, so it reads as "the state from then".
    pub fn state_time(&self) -> Option<u64> {
        self.metadata.as_ref().and_then(Metadata::timestamp)
    }

    /// The save's name in that state, when the metadata says.
    pub fn save_name(&self) -> Option<String> {
        self.metadata
            .as_ref()
            .map(Metadata::save_name)
            .filter(|name| !name.is_empty())
    }

    /// Play time in that state, in seconds.
    pub fn play_time(&self) -> Option<u64> {
        self.metadata.as_ref().map(|metadata| metadata.play_time)
    }
}

/// Whether an archive file name is pinned.
pub fn is_kept(archive: &Path) -> bool {
    archive.file_name().is_some_and(|name| name.to_string_lossy().ends_with(KEEP_SUFFIX))
}

/// Read one entry of an archive into memory.
fn read_archive_entry(archive: &Path, name: &str) -> Result<Option<Vec<u8>>, BackupError> {
    let file = std::fs::File::open(archive).map_err(|source| BackupError::Directory {
        path: archive.display().to_string(),
        source,
    })?;
    let mut zip = zip::ZipArchive::new(file).map_err(|source| BackupError::Write {
        path: archive.display().to_string(),
        message: source.to_string(),
    })?;
    let mut entry = match zip.by_name(name) {
        Ok(entry) => entry,
        Err(_) => return Ok(None),
    };
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    std::io::Read::read_to_end(&mut entry, &mut bytes).map_err(|source| BackupError::Write {
        path: archive.display().to_string(),
        message: source.to_string(),
    })?;
    Ok(Some(bytes))
}

/// One file inside a backup archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchivedFile {
    /// Entry name inside the archive.
    pub name: String,
    /// Uncompressed size in bytes.
    pub size: u64,
}

/// List the files inside a backup archive.
pub fn list_archive(archive: &Path) -> Result<Vec<ArchivedFile>, BackupError> {
    let file = std::fs::File::open(archive).map_err(|source| BackupError::Directory {
        path: archive.display().to_string(),
        source,
    })?;
    let mut zip = zip::ZipArchive::new(file).map_err(|source| BackupError::Write {
        path: archive.display().to_string(),
        message: source.to_string(),
    })?;

    let mut files = Vec::new();
    for index in 0..zip.len() {
        let entry = zip.by_index(index).map_err(|source| BackupError::Write {
            path: archive.display().to_string(),
            message: source.to_string(),
        })?;
        // Only regular files, and never anything with a path component: a
        // crafted archive must not be able to write outside the target.
        let name = entry
            .name()
            .map_err(|source| BackupError::Write {
                path: archive.display().to_string(),
                message: source.to_string(),
            })?
            .to_string();
        if entry.is_file() && !name.contains(['/', '\\']) && name != ".." {
            files.push(ArchivedFile { name, size: entry.size() });
        }
    }
    Ok(files)
}

/// Restore a backup archive into `destination`.
///
/// Files already present are first archived under `{label}_pre-restore`, so a
/// restore is itself undoable. Returns the restored entry names.
pub fn restore_archive(
    archive: &Path,
    destination: &Path,
    store: &BackupStore,
    timestamp: u64,
) -> Result<Vec<String>, BackupError> {
    let files = list_archive(archive)?;
    if files.is_empty() {
        return Err(BackupError::Write {
            path: archive.display().to_string(),
            message: "archive contains no files".to_string(),
        });
    }

    std::fs::create_dir_all(destination).map_err(|source| BackupError::Directory {
        path: destination.display().to_string(),
        source,
    })?;

    // Preserve what is about to be replaced.
    let existing: Vec<(String, PathBuf)> = files
        .iter()
        .map(|file| (file.name.clone(), destination.join(&file.name)))
        .filter(|(_, path)| path.is_file())
        .collect();
    if !existing.is_empty() {
        store.create("pre-restore", &existing, timestamp)?;
    }

    let file = std::fs::File::open(archive).map_err(|source| BackupError::Directory {
        path: archive.display().to_string(),
        source,
    })?;
    let mut zip = zip::ZipArchive::new(file).map_err(|source| BackupError::Write {
        path: archive.display().to_string(),
        message: source.to_string(),
    })?;

    let mut restored = Vec::new();
    for file in &files {
        let mut entry = zip.by_name(&file.name).map_err(|source| BackupError::Write {
            path: archive.display().to_string(),
            message: source.to_string(),
        })?;
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        std::io::Read::read_to_end(&mut entry, &mut bytes).map_err(|source| BackupError::Write {
            path: archive.display().to_string(),
            message: source.to_string(),
        })?;
        let target = destination.join(&file.name);
        // Write through the same atomic path used for saves.
        nms_format::write_atomic(&target, &bytes).map_err(|source| BackupError::Write {
            path: target.display().to_string(),
            message: source.to_string(),
        })?;
        restored.push(file.name.clone());
    }
    Ok(restored)
}

/// Format a Unix timestamp as `yyyy-mm-dd hh:mm:ss` in UTC, for listings.
pub fn format_utc(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let time = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}",
        hour = time / 3_600,
        minute = (time % 3_600) / 60,
        second = time % 60
    )
}

/// Format a Unix timestamp as `yyyyMMdd_HHmmss` in UTC.
pub fn format_timestamp(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let time = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}{month:02}{day:02}_{hour:02}{minute:02}{second:02}",
        hour = time / 3_600,
        minute = (time % 3_600) / 60,
        second = time % 60
    )
}

/// Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = (z - era * 146_097) as u64; // 0..=146096
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 { month_prime + 3 } else { month_prime - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store with one save file to back up.
    fn scratch_store(name: &str) -> (PathBuf, PathBuf, BackupStore) {
        let dir = std::env::temp_dir().join(format!("nms-edit-backup-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let data = dir.join("save.hg");
        std::fs::write(&data, b"payload").unwrap();
        let store = BackupStore::new(dir.join("backups"), 2);
        (dir, data, store)
    }

    #[test]
    fn pinning_keeps_an_archive_out_of_pruning() {
        let (dir, data, store) = scratch_store("pin");
        let files = vec![("save.hg".to_string(), data.clone())];
        let first = store.create("save", &files, 1_793_488_365).unwrap().unwrap();
        store.create("save", &files, 1_793_488_400).unwrap().unwrap();

        // Pin the oldest, then write enough to push everything else out.
        let pinned = store.set_kept(&first.path, true).unwrap();
        assert!(pinned.to_string_lossy().ends_with(".keep.zip"), "{}", pinned.display());
        assert!(is_kept(&pinned));
        store.create("save", &files, 1_793_488_500).unwrap().unwrap();
        store.create("save", &files, 1_793_488_600).unwrap().unwrap();

        let listed = store.list("save").unwrap();
        assert!(listed.iter().any(|entry| entry.path == pinned), "the pinned archive survives: {listed:?}");
        assert_eq!(listed.iter().filter(|entry| !entry.kept).count(), 2, "retention counts only unpinned: {listed:?}");
        assert!(listed.iter().filter(|entry| entry.kept).all(|entry| entry.kept));

        // Unpinning puts it back in the count and the next prune takes it.
        let unpinned = store.set_kept(&pinned, false).unwrap();
        assert_eq!(unpinned.file_name().unwrap().to_string_lossy().trim_end_matches(".zip"), "save_20261031_231245");
        assert!(!is_kept(&unpinned));
        let removed = store.prune("save").unwrap();
        assert!(removed.iter().any(|path| path == &unpinned), "{removed:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inspect_reads_the_files_and_survives_a_missing_metadata_file() {
        let (dir, data, store) = scratch_store("inspect");
        let files = vec![("save.hg".to_string(), data.clone())];
        let entry = store.create("save", &files, 1_793_488_365).unwrap().unwrap();

        let summary = store.inspect(&entry.path).unwrap();
        assert_eq!(summary.files.len(), 1);
        assert_eq!(summary.files[0].name, "save.hg");
        assert_eq!(summary.files[0].size, b"payload".len() as u64);
        assert_eq!(summary.slot.map(|slot| slot.to_string()), Some("PlayerState1".to_string()));
        assert!(summary.metadata.is_none(), "no metadata file in this archive");
        assert_eq!(summary.state_time(), None);
        assert!(!summary.kept);

        // A file that is not an archive is reported, not panicked on.
        let junk = dir.join("junk.zip");
        std::fs::write(&junk, b"not a zip").unwrap();
        assert!(store.inspect(&junk).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_removes_exactly_one_archive() {
        let (dir, data, store) = scratch_store("delete");
        let files = vec![("save.hg".to_string(), data.clone())];
        let first = store.create("save", &files, 1_793_488_365).unwrap().unwrap();
        let second = store.create("save", &files, 1_793_488_400).unwrap().unwrap();

        store.delete(&first.path).unwrap();
        let listed = store.list("save").unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].path, second.path);
        assert!(store.delete(&first.path).is_err(), "deleting it twice is an error, not a panic");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn listing_every_archive_covers_all_labels() {
        let (dir, data, store) = scratch_store("list-all");
        let files = vec![("save.hg".to_string(), data.clone())];
        store.create("save", &files, 1_793_488_365).unwrap().unwrap();
        store.create("save2", &files, 1_793_488_400).unwrap().unwrap();
        store.create("pre-restore", &files, 1_793_488_500).unwrap().unwrap();

        // Grouped by label (by name), newest first inside each group, which is
        // what makes the CLI listing readable.
        let all = store.list_all().unwrap();
        assert_eq!(all.len(), 3);
        let labels: Vec<String> = all.iter().filter_map(|entry| entry.label()).collect();
        assert_eq!(labels, ["save", "save2", "pre-restore"], "{labels:?}");
        assert_eq!(store.list("save").unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn utc_dates_read_like_dates() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00");
        assert_eq!(format_utc(1_793_488_365), "2026-10-31 23:12:45");
    }

    #[test]
    fn two_saves_in_one_second_keep_two_archives() {
        let dir = std::env::temp_dir().join(format!("nms-edit-backup-collision-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let data = dir.join("save.hg");
        std::fs::write(&data, b"payload").unwrap();

        let store = BackupStore::new(dir.join("backups"), 10);
        let files = vec![("save.hg".to_string(), data.clone())];
        let first = store.create("save", &files, 1_793_488_365).unwrap().unwrap();
        std::fs::write(&data, b"changed").unwrap();
        let second = store.create("save", &files, 1_793_488_365).unwrap().unwrap();

        assert_ne!(first.path, second.path, "the second save must not overwrite the first archive");
        let listed = store.list("save").unwrap();
        assert_eq!(listed.len(), 2, "both archives stay listed: {listed:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn formats_known_timestamps() {
        assert_eq!(format_timestamp(0), "19700101_000000");
        assert_eq!(format_timestamp(1), "19700101_000001");
        // 2026-10-31 23:12:45 UTC
        assert_eq!(format_timestamp(1_793_488_365), "20261031_231245");
        // Leap day.
        assert_eq!(format_timestamp(1_709_164_800), "20240229_000000");
    }

    #[test]
    fn backup_round_trip_and_retention() {
        let dir = std::env::temp_dir().join(format!("nms-edit-backup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let saves = dir.join("saves");
        std::fs::create_dir_all(&saves).unwrap();
        let data = saves.join("save.hg");
        std::fs::write(&data, b"payload").unwrap();
        let meta = saves.join("mf_save.hg");
        std::fs::write(&meta, b"metadata").unwrap();

        let store = BackupStore::new(dir.join("backups"), 2);
        let files = vec![("save.hg".to_string(), data.clone()), ("mf_save.hg".to_string(), meta.clone())];

        let first = store.create("slot1", &files, 1_793_488_365).unwrap().unwrap();
        assert!(first.path.exists());
        assert_eq!(first.path.file_name().unwrap(), "slot1_20261031_231245.zip");

        // Contents are readable and match.
        let archive = std::fs::File::open(&first.path).unwrap();
        let mut zip = zip::ZipArchive::new(archive).unwrap();
        assert_eq!(zip.len(), 2);
        let mut text = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("save.hg").unwrap(), &mut text).unwrap();
        assert_eq!(text, "payload");

        // Retention keeps the newest two.
        store.create("slot1", &files, 1_793_488_400).unwrap().unwrap();
        store.create("slot1", &files, 1_793_488_500).unwrap().unwrap();
        let kept = store.list("slot1").unwrap();
        assert_eq!(kept.len(), 2, "retention limit must apply: {kept:?}");
        assert!(kept[0].path > kept[1].path, "newest first");

        // A label with no files produces no archive.
        assert!(store.create("empty", &[], 1).unwrap().is_none());

        // Restore: list the archive, then put the files back, keeping a copy of
        // what is replaced. A separate label keeps retention out of the way.
        let source = store.create("restore-src", &files, 1_793_488_365).unwrap().unwrap();
        let listed = list_archive(&source.path).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].name, "save.hg");
        std::fs::write(&data, b"edited").unwrap();
        let restored = restore_archive(&source.path, &saves, &store, 1_793_488_600).unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(std::fs::read(&data).unwrap(), b"payload", "restore must bring the old contents back");
        let pre = store.list("pre-restore").unwrap();
        assert_eq!(pre.len(), 1, "the replaced file must be preserved: {pre:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
