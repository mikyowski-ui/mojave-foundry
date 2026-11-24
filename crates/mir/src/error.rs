//! The crate's one error type.

use std::path::Path;

use thiserror::Error;

/// Everything `mir` reports as a failure.
///
/// A bad record, a missing mesh or an undecodable texture is not one of
/// these: an extract records it, lists it in `report.md` and carries on.
/// These are the failures that stop a call.
#[derive(Debug, Error)]
pub enum MirError {
    /// A file or directory could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The path, as given by the caller.
        path: String,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },

    /// A plugin's framing is broken before its first group.
    #[error("plugin {plugin}: {source}")]
    Plugin {
        /// The plugin's file name.
        plugin: String,
        /// The parser's error, which carries the byte offset.
        #[source]
        source: fnv_esm::EsmError,
    },

    /// An archive entry was found but could not be read.
    #[error("archive {archive}: {source}")]
    Archive {
        /// The archive's file name.
        archive: String,
        /// The reader's error, which carries the byte offset.
        #[source]
        source: fnv_bsa::BsaError,
    },

    /// `records.sqlite` could not be written or queried.
    #[error("records.sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// A JSON file of the directory does not parse.
    #[error("{path}: {source}")]
    Json {
        /// The file, relative to the MIR directory.
        path: String,
        /// The parser's error, which carries line and column.
        #[source]
        source: serde_json::Error,
    },

    /// A stored `.glb` does not read back.
    #[error("mesh {hash}: {source}")]
    Mesh {
        /// The content hash that names the file.
        hash: String,
        /// The reader's error, which carries the byte offset.
        #[source]
        source: nif::NifError,
    },

    /// A stored `.png` does not read back as RGBA8.
    #[error("texture {hash}: {reason}")]
    Texture {
        /// The content hash that names the file.
        hash: String,
        /// What is wrong with it.
        reason: String,
    },

    /// A skeleton or animation document was asked for by a string that is
    /// not a content hash.
    #[error("{store} {hash:?}: {reason}")]
    Document {
        /// The store: `skeletons` or `animations`.
        store: &'static str,
        /// The hash as given.
        hash: String,
        /// What is wrong with it.
        reason: String,
    },

    /// The slice names a worldspace no plugin of the list defines.
    #[error("worldspace {0:?} is not defined by any plugin of the list")]
    WorldspaceNotFound(String),

    /// A string is not a form key (`"plugin.esm:0daebb"`).
    #[error("{0:?} is not a form key: expected \"<plugin>:<six hex digits>\"")]
    BadFormKey(String),

    /// A file was written by another schema version.
    #[error("{path}: schema version {found}, but this build reads version {expected}")]
    SchemaVersion {
        /// The file, relative to the MIR directory.
        path: String,
        /// The version the file declares.
        found: u32,
        /// The version this build reads and writes.
        expected: u32,
    },

    /// The extract options cannot be used.
    #[error("invalid extract options: {0}")]
    Options(String),

    /// `records.sqlite` was written before a table was added
    /// ([`crate::RECORDS_REVISION`]); extracting again rebuilds it.
    #[error(
        "records.sqlite has no table `{0}`: it was written by an older build; extract again \
         to rebuild it"
    )]
    MissingTable(&'static str),
}

impl MirError {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.display().to_string(),
            source,
        }
    }
}
