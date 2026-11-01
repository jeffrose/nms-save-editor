//! Regenerating the editor's game data from a player's own installation.
//!
//! The point of this crate is update resilience. The older editor shipped one
//! hand-maintained list of names, so every game update produced months of
//! "name mapping not found" reports until someone rebuilt it by hand. Here the
//! data is derived from the archives the game itself ships:
//!
//! ```text
//! GAMEDATA/PCBANKS/NMSARC.*.pak                    (nms-pak)
//!   language/nms_loc*_<language>.mbin              (language)
//!     -> DataPack { id -> text }                   (pack)
//! ```
//!
//! Nothing from the game is redistributed: a pack is generated on the player's
//! machine, from the player's copy, and stays there.
//!
//! ```no_run
//! use nms_data::{Install, DataPack};
//!
//! let install = Install::discover(None)?;
//! let (pack, report) = DataPack::build(&install, "english", 0)?;
//! println!("{} strings from {} archives", pack.len(), report.sources.len());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod install;
pub mod language;
pub mod pack;

pub use install::Install;
pub use language::Localisation;
pub use pack::{BuildReport, DataPack, PACK_FORMAT, SourceReport};

/// Anything that can go wrong while building or reading game data.
#[derive(Debug, thiserror::Error)]
pub enum DataError {
    /// No installation could be found.
    #[error("no No Man's Sky installation found; searched: {}", searched.join(", "))]
    InstallNotFound {
        /// Paths that were tried.
        searched: Vec<String>,
    },

    /// The archive directory exists but holds no archives.
    #[error("no .pak archives in {dir}")]
    NoArchives {
        /// Directory that was searched.
        dir: String,
    },

    /// An archive could not be read.
    #[error("{path}: {message}")]
    Archive {
        /// Archive path.
        path: String,
        /// Underlying failure.
        message: String,
    },

    /// No localisation tables for the requested language.
    #[error("no localisation tables for language {language:?} in this installation")]
    NoTables {
        /// Requested language code.
        language: String,
    },

    /// A localisation table did not parse.
    #[error("localisation table: {message}")]
    Localisation {
        /// What was wrong.
        message: String,
    },

    /// JSON failure.
    #[error("data pack: {message}")]
    Json {
        /// Underlying failure.
        message: String,
    },

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
