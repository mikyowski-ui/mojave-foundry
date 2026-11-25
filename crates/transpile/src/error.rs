//! The crate's one error type (contracts section 7). Malformed source is
//! never an error: it is a [`crate::diag::Diag`]. An error stops the build.

use thiserror::Error;

/// What stops a build.
#[derive(Debug, Error)]
pub enum TranspileError {
    /// The MIR could not be read.
    #[error("reading the MIR: {0}")]
    Mir(#[from] mir::MirError),
    /// A query of `records.sqlite` failed (a table or column is missing,
    /// or a row is malformed).
    #[error("querying records.sqlite ({query}): {message}")]
    Query {
        /// What was being read.
        query: &'static str,
        /// The database's message.
        message: String,
    },
    /// The slice is inconsistent: a reference placed twice on one map, a
    /// placement of another game.
    #[error("the slice: {0}")]
    Slice(String),
}

impl TranspileError {
    /// A failed query.
    pub fn query(query: &'static str, error: impl std::fmt::Display) -> Self {
        Self::Query {
            query,
            message: error.to_string(),
        }
    }
}
