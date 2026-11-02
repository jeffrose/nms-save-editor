//! Loading a save together with its metadata, planning a write, and committing
//! it with a backup and a refreshed metadata file.

use std::path::{Path, PathBuf};

use nms_format::{Container, FormatError, Metadata, MetadataPatch, SaveFile, Slot, WriteOutcome, write_atomic};
use nms_model::{ByteDiff, Document, KeyMap, ModelError, PathError, SaveInfo, TreeChange, tree_diff};
use serde_json::Value;

use crate::backup::{BackupEntry, BackupStore};
use crate::safety;
use crate::EditError;

/// Metadata version word offset for player saves (`JSON Version - 512`).
const VERSION_WORD_OFFSET: u64 = 512;

/// Summary of what a write changes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChangeSummary {
    /// Payload size before the edit.
    pub payload_before: usize,
    /// Payload size after the edit.
    pub payload_after: usize,
    /// Offset of the first differing payload byte, if any.
    pub first_diff: Option<usize>,
    /// Size of the changed region (not the position-wise difference count,
    /// which is inflated by any change in payload length).
    pub changed_bytes: usize,
    /// Metadata words that changed, with their indices.
    pub metadata_words_changed: Vec<usize>,
    /// Fields whose values differ, named and readable.
    pub fields_changed: Vec<TreeChange>,
}

impl ChangeSummary {
    /// True when the payload is unchanged.
    pub fn payload_unchanged(&self) -> bool {
        self.first_diff.is_none() && self.payload_before == self.payload_after
    }

    /// A multi-line description of the changed fields.
    pub fn field_report(&self) -> String {
        if self.fields_changed.is_empty() {
            return "no fields changed".to_string();
        }
        self.fields_changed.iter().map(|change| format!("  {}", change.summary())).collect::<Vec<_>>().join("\n")
    }

    /// A one-line description.
    pub fn summary(&self) -> String {
        let payload = if self.payload_unchanged() {
            format!("payload unchanged ({} bytes)", self.payload_before)
        } else {
            format!(
                "payload {} -> {} bytes, {} byte(s) changed at offset {}",
                self.payload_before,
                self.payload_after,
                self.changed_bytes,
                self.first_diff.map(|o| o.to_string()).unwrap_or_else(|| "-".into())
            )
        };
        let fields = match self.fields_changed.len() {
            0 => "no fields changed".to_string(),
            1 => self.fields_changed[0].summary(),
            n => format!("{n} fields changed"),
        };
        let metadata = match self.metadata_words_changed.len() {
            0 => "metadata unchanged",
            n => &format!("{n} metadata words refreshed"),
        };
        format!("{fields} ({payload}; {metadata})")
    }
}

/// The refreshed metadata file a write will produce.
#[derive(Debug, Clone)]
pub struct MetadataPlan {
    /// Encoded metadata bytes.
    pub bytes: Vec<u8>,
    /// The patch that was applied.
    pub patch: MetadataPatch,
    /// Words that differ from the current metadata.
    pub changed_words: Vec<usize>,
    /// Fields that could not be refreshed, with the reason.
    pub warnings: Vec<String>,
}

/// Everything a commit would write, computed without touching the disk.
#[derive(Debug, Clone)]
pub struct WritePlan {
    /// New payload (Latin-1 bytes, without the trailing NUL).
    pub payload: Vec<u8>,
    /// New data file bytes.
    pub data: Vec<u8>,
    /// Refreshed metadata, when the slot has a metadata file.
    pub metadata: Option<MetadataPlan>,
    /// What changed.
    pub changes: ChangeSummary,
}

impl WritePlan {
    /// True when nothing would change on disk.
    ///
    /// Metadata is only ever written alongside a new data file, so an unchanged
    /// payload means an unchanged save: no write, no backup, no timestamp churn.
    pub fn is_noop(&self) -> bool {
        self.changes.payload_unchanged()
    }
}

/// Options for [`SaveBundle::commit`].
#[derive(Debug, Clone)]
pub struct CommitOptions {
    /// Directory for timestamped zip backups. `None` skips the backup.
    pub backup_root: Option<PathBuf>,
    /// How many backups to keep per save.
    pub keep_backups: usize,
    /// Timestamp to use (Unix seconds). Defaults to the system clock.
    pub timestamp: Option<u64>,
    /// Compute the plan but do not write anything.
    pub dry_run: bool,
    /// Accept writing to a live, cloud-synced save location.
    ///
    /// See [`crate::safety`]: without this the commit refuses a path that looks
    /// like the game's own save folder, because Steam Cloud would propagate the
    /// change to every machine on the account.
    pub confirm_live: bool,
}

impl Default for CommitOptions {
    fn default() -> Self {
        CommitOptions { backup_root: None, keep_backups: 10, timestamp: None, dry_run: false, confirm_live: false }
    }
}

/// What a commit did.
#[derive(Debug, Clone)]
pub struct CommitReport {
    /// What changed.
    pub changes: ChangeSummary,
    /// Outcome of writing the data file.
    pub data: WriteOutcome,
    /// Outcome of writing the metadata file.
    pub metadata: WriteOutcome,
    /// Backup created before the write, if any.
    pub backup: Option<BackupEntry>,
    /// Fields that could not be refreshed.
    pub warnings: Vec<String>,
}

/// A save file, its JSON document, and its metadata.
#[derive(Debug, Clone)]
pub struct SaveBundle {
    /// Storage slot, derived from the data file name.
    pub slot: Slot,
    /// Path of the data file.
    pub data_path: PathBuf,
    /// Path of the companion metadata file.
    pub metadata_path: PathBuf,
    /// The data file.
    pub save: SaveFile,
    /// The parsed document.
    pub document: Document,
    /// The metadata file, when it exists and decrypts.
    pub metadata: Option<Metadata>,
    /// Key map used to resolve readable field names.
    pub keys: KeyMap,
}

impl SaveBundle {
    /// Load a data file and its companion metadata.
    pub fn load(data_path: &Path, keys: KeyMap) -> Result<SaveBundle, EditError> {
        let file_name = data_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let slot = Slot::from_file_name(&file_name).ok_or(EditError::UnknownSlot(file_name))?;

        let save = SaveFile::read(data_path)?;
        let document = Document::parse(&save.payload)?;
        let metadata_path = data_path.with_file_name(slot.metadata_file_name());
        let metadata = if metadata_path.is_file() {
            let bytes = std::fs::read(&metadata_path)
                .map_err(|e| EditError::io("reading metadata", e))?;
            Some(Metadata::read(&bytes, slot).map_err(|source| EditError::Metadata {
                path: metadata_path.display().to_string(),
                source,
            })?)
        } else {
            None
        };

        Ok(SaveBundle { slot, data_path: data_path.to_path_buf(), metadata_path, save, document, metadata, keys })
    }

    /// The standard summary fields.
    pub fn info(&self) -> SaveInfo {
        self.document.info(&self.keys)
    }

    /// Set a value by readable, dotted path.
    pub fn set(&mut self, path: &str, value: Value) -> Result<(), PathError> {
        self.document.set_path(&self.keys, path, value)
    }

    /// Read a value by readable, dotted path.
    pub fn get(&self, path: &str) -> Result<&Value, PathError> {
        self.document.get_path(&self.keys, path)
    }

    /// The current payload the document would serialise to.
    pub fn payload(&self) -> Result<Vec<u8>, ModelError> {
        self.document.payload()
    }

    /// Compute what a commit would write, without touching the disk.
    pub fn plan(&self, timestamp: u64) -> Result<WritePlan, EditError> {
        let payload = self.document.payload()?;

        let mut save = self.save.clone();
        save.set_payload(payload.clone());
        let data = save.to_bytes();

        let before = self.save.payload.clone();
        let diff = ByteDiff::compute(&before, &payload);

        let fields_changed = if payload == before {
            Vec::new()
        } else {
            let mut document = self.document.clone();
            document.mark_dirty();
            let before_tree = Document::parse(&before)?.tree().clone();
            tree_diff(&before_tree, document.tree(), &self.keys, 32)
        };

        let mut metadata_plan = None;
        let mut metadata_words_changed = Vec::new();
        if let Some(meta) = &self.metadata {
            let patch = self.derive_metadata_patch(&save, &data, timestamp);
            let (bytes, patch, warnings) = apply_patch(meta, patch)?;
            let refreshed = Metadata::read(&bytes, meta.slot)?;
            metadata_words_changed = differing_words(&meta.words, &refreshed.words);
            metadata_plan = Some(MetadataPlan { bytes, patch, changed_words: metadata_words_changed.clone(), warnings });
        }

        let changes = ChangeSummary {
            payload_before: before.len(),
            payload_after: payload.len(),
            first_diff: diff.first_diff,
            changed_bytes: diff.changed_bytes(),
            metadata_words_changed,
            fields_changed,
        };

        Ok(WritePlan { payload, data, metadata: metadata_plan, changes })
    }

    /// Write the save and its refreshed metadata.
    ///
    /// Order matters for crash safety: back up, write the data file, then write
    /// the metadata. Metadata that describes a payload the game cannot read yet
    /// is worse than metadata that is briefly stale.
    pub fn commit(&mut self, options: &CommitOptions) -> Result<CommitReport, EditError> {
        let timestamp = options.timestamp.unwrap_or_else(now_unix);
        let plan = self.plan(timestamp)?;

        let mut report = CommitReport {
            changes: plan.changes.clone(),
            data: WriteOutcome::Unchanged,
            metadata: WriteOutcome::Unchanged,
            backup: None,
            warnings: plan.metadata.as_ref().map(|m| m.warnings.clone()).unwrap_or_default(),
        };

        if plan.is_noop() || options.dry_run {
            return Ok(report);
        }

        // Refuse a live save folder unless the caller said so explicitly.
        safety::check_write(&self.data_path, options.confirm_live).map_err(EditError::LiveSave)?;

        if let Some(root) = &options.backup_root {
            let store = BackupStore::new(root.clone(), options.keep_backups);
            let label = self
                .data_path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_else(|| "save".into());
            let files = vec![
                (self.slot.data_file_name(), self.data_path.clone()),
                (self.slot.metadata_file_name(), self.metadata_path.clone()),
            ];
            report.backup = store.create(&label, &files, timestamp).map_err(EditError::from)?;
        }

        if !plan.changes.payload_unchanged() {
            write_atomic(&self.data_path, &plan.data)?;
            report.data = WriteOutcome::Written(plan.data.len());

            // Metadata describes the data file, so it is only refreshed when the
            // data file was actually replaced. An unedited save keeps both files
            // exactly as they were.
            if let Some(metadata) = &plan.metadata {
                if !metadata.changed_words.is_empty() {
                    write_atomic(&self.metadata_path, &metadata.bytes)?;
                    report.metadata = WriteOutcome::Written(metadata.bytes.len());
                }
            }
        }

        // Re-read so the bundle reflects what is on disk.
        if report.data != WriteOutcome::Unchanged {
            self.save = SaveFile::read(&self.data_path)?;
            self.document = Document::parse(&self.save.payload)?;
        }
        if let Some(meta) = &self.metadata {
            if report.metadata != WriteOutcome::Unchanged {
                let bytes = std::fs::read(&self.metadata_path).map_err(|e| EditError::io("re-reading metadata", e))?;
                self.metadata = Some(Metadata::read(&bytes, meta.slot)?);
            }
        }

        Ok(report)
    }

    /// Derive the metadata patch that mirrors the current save.
    ///
    /// Player saves mirror the save name, location summary, game mode, play
    /// time and version word; account-level files only carry sizes.
    fn derive_metadata_patch(&self, save: &SaveFile, data: &[u8], timestamp: u64) -> MetadataPatch {
        let info = self.info();
        let mut patch = MetadataPatch {
            decompressed_size: Some(save.stored_payload().len() as u32),
            compressed_size: Some(if save.container == Container::PlainJson { 0 } else { data.len() as u32 }),
            timestamp: Some(timestamp as u32),
            ..Default::default()
        };

        if self.slot.player_number().is_some() {
            if let Some(version) = info.version.and_then(|v| v.checked_sub(VERSION_WORD_OFFSET)) {
                patch.version = Some(version as u32);
            }
            patch.play_time = info.play_time;
            patch.save_name = Some(info.save_name.unwrap_or_default());
            patch.location = self.player_state_string("SaveSummary");
            patch.game_mode = self
                .player_state_string("DifficultyState.Preset.DifficultyPresetType")
                .or_else(|| self.player_state_string("DifficultyState.EasiestUsedPreset.DifficultyPresetType"));
        }
        patch
    }

    /// Resolve a field inside the active player-state context.
    ///
    /// `PlayerStateData` is reached through a context, which is `BaseContext`
    /// for normal saves and `ExpeditionContext` for expedition saves; the same
    /// field therefore has two possible paths. Both are probed, in that order,
    /// and the first that resolves wins.
    pub(crate) fn context_key(&self) -> Option<&'static str> {
        const CONTEXTS: [&str; 2] = ["BaseContext", "ExpeditionContext"];
        CONTEXTS.into_iter().find(|context| self.get(&format!("{context}.PlayerStateData")).is_ok())
    }

    pub(crate) fn player_state_value(&self, path: &str) -> Option<&Value> {
        let context = self.context_key()?;
        self.get(&format!("{context}.PlayerStateData.{path}")).ok()
    }

    fn player_state_string(&self, path: &str) -> Option<String> {
        self.player_state_value(path).and_then(Value::as_str).map(str::to_string)
    }
}

/// Apply a patch, dropping mirror fields that cannot be written and recording
/// why. A metadata file is never left partially updated: the patch is applied
/// to a copy and either succeeds or the field is dropped.
fn apply_patch(meta: &Metadata, mut patch: MetadataPatch) -> Result<(Vec<u8>, MetadataPatch, Vec<String>), EditError> {
    let mut warnings = Vec::new();
    loop {
        match meta.patch_to_bytes(&patch) {
            Ok(bytes) => return Ok((bytes, patch, warnings)),
            Err(error) => {
                let field = match &error {
                    FormatError::MetadataStringTooLong { field, .. }
                    | FormatError::MetadataFieldOccupied { field, .. } => *field,
                    _ => return Err(error.into()),
                };
                let dropped = match field {
                    "SaveName" => patch.save_name.take().is_some(),
                    "Location" => patch.location.take().is_some(),
                    "GameMode" => patch.game_mode.take().is_some(),
                    _ => false,
                };
                if !dropped {
                    return Err(error.into());
                }
                warnings.push(format!("metadata field {field} not refreshed: {error}"));
            }
        }
    }
}

fn differing_words(before: &[u32], after: &[u32]) -> Vec<usize> {
    (0..before.len().max(after.len()))
        .filter(|index| before.get(*index) != after.get(*index))
        .collect()
}

/// Current time in Unix seconds.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
