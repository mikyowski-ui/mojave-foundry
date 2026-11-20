use crate::ArtCategory;
use crate::report::{BudgetReport, Contributor, fid_message};

/// Every failure of this crate. A bad reference is never an error: it is
/// listed in [`crate::Sliced::skipped`] with its reason.
#[derive(Debug, thiserror::Error)]
pub enum SliceError {
    /// The map needs more entries in an art list than it has free.
    #[error("{}", fid_message(.category, .cap, .demand, .top))]
    FidBudget {
        category: ArtCategory,
        /// Free entries in the list.
        cap: usize,
        /// Distinct entries the map needs.
        demand: usize,
        /// Its largest contributors, most entries first.
        top: Vec<Contributor>,
        /// The whole accounting.
        report: Box<BudgetReport>,
    },
    #[error("invalid slicer configuration: {0}")]
    Config(String),
    #[error("the projection cannot be inverted on the ground plane")]
    Projection,
    #[error("rendering a ground overlay failed: {0}")]
    Render(#[from] foundry_render::RenderError),
    #[error("quantizing failed: {0}")]
    Quantize(#[from] quantize::QuantizeError),
}
