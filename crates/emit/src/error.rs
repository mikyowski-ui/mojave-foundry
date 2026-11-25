//! The crate's one error type.

use std::path::PathBuf;

/// Everything `emit` can refuse. Every writer validates its input against
/// what the pinned `vault-formats` parser reads, and refuses rather than
/// emit bytes that would parse differently.
#[derive(Debug, thiserror::Error)]
pub enum EmitError {
    /// An FRM specification the format cannot carry.
    #[error("FRM: {0}")]
    Frm(String),
    /// An image buffer whose size does not match its dimensions.
    #[error("image: {0}")]
    Image(String),
    /// An LST line the format cannot carry; `line` is one-based.
    #[error("LST line {line}: {message}")]
    Lst { line: usize, message: String },
    /// A MSG entry the format cannot carry.
    #[error("MSG entry {id}: {message}")]
    Msg { id: i32, message: String },
    /// A GAM variable the format cannot carry; `index` is zero-based.
    #[error("GAM variable {index}: {message}")]
    Gam { index: usize, message: String },
    /// An art list would exceed the 12-bit FID index space.
    #[error("art list {category} would need {requested} entries; a FID addresses at most {cap}")]
    FidBudget {
        /// The art directory, such as `scenery`.
        category: &'static str,
        /// The largest number of entries a FID can address.
        cap: usize,
        /// The number of entries the list would need.
        requested: usize,
    },
    /// A critter stem, art row, prototype figure or map placement the
    /// engine cannot use.
    #[error("critter: {0}")]
    Critter(String),
    /// A prototype, prototype list or PID the format cannot carry.
    #[error("prototype: {0}")]
    Prototype(String),
    /// A `vault-formats` parser refused bytes, or a `vault-formats` writer
    /// refused a value.
    #[error(transparent)]
    Format(#[from] vault_formats::ParseError),
    /// Assembler source the `.int` format cannot carry.
    #[error(transparent)]
    Asm(#[from] crate::int::AsmError),
    /// A map edit the map cannot take (`map::attach_scripts`).
    #[error("map: {0}")]
    Map(String),
    /// A row or base file an appender (`append`) cannot extend.
    #[error("append: {0}")]
    Append(String),
    /// A mod manifest that cannot be written or registered.
    #[error("mod manifest: {0}")]
    Manifest(String),
    /// A VFS path that the engine would not accept.
    #[error("path {path:?}: {message}")]
    Path { path: String, message: String },
    /// The output directory holds something this layer did not write.
    #[error("refusing to write into {}: {reason}", dir.display())]
    ForeignOutput { dir: PathBuf, reason: String },
    /// A filesystem operation failed.
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl EmitError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
