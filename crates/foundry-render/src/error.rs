//! The crate's one error type.

/// Everything that can go wrong while configuring or running a renderer.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RenderError {
    /// The calibration text is not valid TOML for [`crate::Calibration`].
    #[error("calibration TOML: {0}")]
    Toml(String),
    /// A calibration value is outside its allowed range.
    #[error("calibration key `{key}`: {reason}")]
    InvalidCalibration {
        /// The offending key.
        key: &'static str,
        /// What is wrong with it.
        reason: String,
    },
    /// A requested window has no pixels.
    #[error("window {width} x {height} has no pixels")]
    EmptyWindow {
        /// Requested width in pixels.
        width: u32,
        /// Requested height in pixels.
        height: u32,
    },
    /// A window is larger than the renderer accepts.
    #[error(
        "window {width} x {height} at supersample {supersample} exceeds the limit \
         ({max_side} px per side, {max_samples} sub-samples)"
    )]
    WindowTooLarge {
        /// Window width in pixels.
        width: u64,
        /// Window height in pixels.
        height: u64,
        /// Supersampling factor per axis.
        supersample: u32,
        /// Largest accepted side in pixels.
        max_side: u32,
        /// Largest accepted number of sub-samples.
        max_samples: u64,
    },
    /// A window lies too far from the anchor.
    #[error("window at ({x}, {y}) lies more than {limit} px from the anchor")]
    WindowOutOfRange {
        /// Left edge.
        x: i64,
        /// Top edge.
        y: i64,
        /// Largest accepted distance in pixels.
        limit: i64,
    },
    /// No GPU adapter (or no device on it) is available.
    #[error("no GPU adapter: {0}")]
    NoAdapter(String),
    /// The GPU reported an error while rendering.
    #[error("GPU: {0}")]
    Gpu(String),
    /// The projection could not be derived from the hex grid.
    #[error("projection: {0}")]
    Projection(String),
}
