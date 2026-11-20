//! The crate's one error type.

use emit::EmitError;

/// Everything [`crate::build_map`] and [`crate::verify_layer`] can refuse.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    /// The caller's input is inconsistent: a bad hex, an art index out of
    /// range, a library tile of the wrong size, a role drawn from the
    /// wrong art category, an unusable map name or number.
    #[error("input: {0}")]
    Input(String),
    /// A base-game file the caller supplied does not load.
    #[error("base file {file}: {message}")]
    Base { file: String, message: String },
    /// An art list would exceed the 4,096 entries a FID addresses. The
    /// message names the list, the cap, the demand and the largest
    /// contributors (by source label) of the entries this build adds.
    #[error(
        "art list {category} would need {requested} entries ({base} from the base game); \
         a FID addresses at most {cap}; largest contributors: {}",
        contributors_text(.largest)
    )]
    FidBudget {
        /// The art directory, such as `scenery`.
        category: &'static str,
        cap: usize,
        /// Entries of the base game's list.
        base: usize,
        /// Entries the list would need.
        requested: usize,
        /// Source label and the number of new entries it asks for,
        /// largest first (at most ten).
        largest: Vec<(String, usize)>,
    },
    /// No hex of the window is walkable, so the map has no entrance.
    #[error("no hex of the window is walkable; the map would have no entrance")]
    NoWalkableHex,
    /// An `emit` writer refused a value while building `file`.
    #[error("{file}: {source}")]
    Emit {
        file: String,
        #[source]
        source: EmitError,
    },
    /// A file of the layer does not parse back, or parses into something
    /// other than what was written.
    #[error("verify {file}: {message}")]
    Verify { file: String, message: String },
    /// Reading a layer back from disk failed.
    #[error("{path}: {message}")]
    Io { path: String, message: String },
}

fn contributors_text(largest: &[(String, usize)]) -> String {
    if largest.is_empty() {
        return "none".into();
    }
    largest
        .iter()
        .map(|(label, count)| format!("{label} ({count})"))
        .collect::<Vec<_>>()
        .join(", ")
}

impl BuildError {
    pub(crate) fn emit(file: impl Into<String>, source: EmitError) -> Self {
        Self::Emit {
            file: file.into(),
            source,
        }
    }
    pub(crate) fn verify(file: impl Into<String>, message: impl std::fmt::Display) -> Self {
        Self::Verify {
            file: file.into(),
            message: message.to_string(),
        }
    }
}
