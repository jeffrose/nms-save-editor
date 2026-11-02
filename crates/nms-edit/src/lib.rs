//! Editing operations for No Man's Sky saves.
//!
//! This crate sits on top of [`nms_format`] (bytes) and [`nms_model`]
//! (documents) and owns the operations that touch a player's files:
//!
//! * [`SaveBundle`] loads a data file together with its companion metadata;
//! * [`SaveBundle::plan`] computes exactly what a write would change;
//! * [`SaveBundle::commit`] backs up, writes atomically, and refreshes the
//!   metadata so the game's slot list matches the save again.
//!
//! Metadata is *mirrored* rather than regenerated: the writer derives the
//! fields it understands (sizes, version, play time, save name, location, game
//! mode, timestamp) and copies every other word verbatim. If a mirrored string
//! cannot be written safely it is dropped with a warning instead of clobbering
//! unknown data.

pub mod backup;
pub mod bundle;
pub mod discover;
pub mod player;
pub mod safety;

pub use discover::{
    FoundSave, SavePointKind, default_save_dir, default_save_roots, first_save_dir_in, platform_save_roots,
    player_save_count, save_dirs, scan,
};
pub use backup::{
    ArchiveSummary, ArchivedFile, BackupEntry, BackupError, BackupStore, format_timestamp, format_utc, is_kept,
    list_archive, restore_archive,
};
pub use bundle::{ChangeSummary, CommitOptions, CommitReport, MetadataPlan, SaveBundle, WritePlan, now_unix};
pub use player::{
    Cell, Currencies, Currency, Inventory, InventoryPath, InventoryRole, ItemKind, ItemSlot, KnownList, NewItem,
    PlayerError, StackSize, StackSizeSource, SuitStat, SuitStats, normalize_id,
};
pub use safety::{LiveLocation, SANDBOX_MARKER, is_sandboxed, live_notice, live_save_location, mark_sandbox};

/// Anything that can go wrong while editing a save.
#[derive(Debug, thiserror::Error)]
pub enum EditError {
    /// Container or metadata codec failure.
    #[error(transparent)]
    Format(#[from] nms_format::FormatError),

    /// JSON document failure.
    #[error(transparent)]
    Model(#[from] nms_model::ModelError),

    /// Path lookup failure.
    #[error(transparent)]
    Path(#[from] nms_model::PathError),

    /// The file name did not identify a storage slot.
    #[error("cannot determine storage slot from file name {0:?}")]
    UnknownSlot(String),

    /// The companion metadata file could not be decrypted.
    #[error("metadata {path}: {source}")]
    Metadata {
        /// Metadata path.
        path: String,
        /// Underlying failure.
        #[source]
        source: nms_format::FormatError,
    },

    /// Backups.
    #[error(transparent)]
    Backup(Box<BackupError>),

    /// A player-state edit that cannot be carried out (unknown inventory, full
    /// grid, illegal amount, …).
    #[error(transparent)]
    Player(#[from] PlayerError),

    /// The planned bytes did not read back, so nothing was written.
    #[error("refusing to write: the planned save does not read back ({0})")]
    Verification(String),

    /// Filesystem failure.
    #[error("{context}: {source}")]
    Io {
        /// What was being done.
        context: &'static str,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
}

impl EditError {
    pub(crate) fn io(context: &'static str, source: std::io::Error) -> EditError {
        EditError::Io { context, source }
    }
}
