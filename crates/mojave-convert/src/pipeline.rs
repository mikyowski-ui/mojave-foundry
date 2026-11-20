//! The stages, in pipeline order. Each takes validated [`Inputs`] and
//! writes under its own subdirectory of `--out`.

use crate::config::{ConvertError, Inputs};

/// Extracts the slice to the MIR directory and writes `report.md`.
pub fn extract(inputs: &Inputs) -> Result<mir::ExtractSummary, ConvertError> {
    // Checked here, not only where it is first used: a run without a usable
    // Fallout 2 install should stop before minutes of extraction.
    inputs.palette()?;
    let options = mir::ExtractOptions {
        data_dir: inputs.fnv_data.clone(),
        plugins: inputs.slice.plugins.clone(),
        out: inputs.mir_dir(),
        slice: inputs.slice.mir_slice(),
        jobs: inputs.jobs,
        raw_store: mir::RawStore::All,
    };
    mir::extract(&options).map_err(|e| ConvertError::stage("extract", e))
}

/// What [`all`] did, stage by stage.
#[derive(Debug, Clone)]
pub struct AllSummary {
    pub extract: mir::ExtractSummary,
    pub sprites: crate::sprites::SpritesSummary,
    pub critters: crate::critters::CrittersSummary,
    pub map: crate::map::MapSummary,
    pub gallery: crate::gallery::GallerySummary,
}

/// Every stage in order: extract, sprites, critters, map, gallery. Each stage skips
/// the work its content-hash cache already holds.
pub fn all(inputs: &Inputs) -> Result<AllSummary, ConvertError> {
    let extract = extract(inputs)?;
    let sprites = crate::sprites::sprites(inputs)?;
    let critters = crate::critters::critters(inputs)?;
    let map = crate::map::map(inputs)?;
    let gallery = crate::gallery::gallery(inputs, "")?;
    Ok(AllSummary {
        extract,
        sprites,
        critters,
        map,
        gallery,
    })
}
