//! Where each art list starts, and how many entries it leaves free.
//!
//! [`art_budget`] answers that without building a map; the build starts
//! its lists through the same [`start_list`], so the two always agree.

use std::collections::BTreeMap;

use emit::{ArtList, CRITTER_LIST_PATH, CritterList, FID_INDEX_CAP, RESERVED_TILE_ENTRIES};
use vault_formats::fid::ART_DIRECTORIES;

use crate::BuildError;

pub(crate) const ART_ITEMS: u8 = 0;
pub(crate) const ART_CRITTERS: u8 = 1;
pub(crate) const ART_SCENERY: u8 = 2;
pub(crate) const ART_WALLS: u8 = emit::WALL_CATEGORY;
pub(crate) const ART_TILES: u8 = emit::TILE_CATEGORY;
pub(crate) const ART_MISC: u8 = 5;

/// The art lists a build loads and may extend, in category order.
pub(crate) const BUILD_LISTS: [u8; 5] = [ART_ITEMS, ART_SCENERY, ART_WALLS, ART_TILES, ART_MISC];

/// One art list's budget for a given base and mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtListBudget {
    /// The layer's list does not start from the base list: standalone
    /// mode, which applies to the walls and tiles lists only.
    pub replaced: bool,
    /// Entries of the base game's list (0 when none is supplied).
    pub base_entries: usize,
    /// Entries the layer's list starts with: all of the base list, or for
    /// a replaced list only the reserved ones
    /// ([`emit::standalone_reserved`]).
    pub kept: usize,
    /// Entries a FID can still address: 4,096 minus `kept`.
    pub free: usize,
    /// Of those, the most the builder takes for its own art: the
    /// invisible blocker (scenery), the exit-grid marker (misc, only when
    /// a door becomes an exit) and the reserved floor tiles a list
    /// shorter than two entries is padded with (tiles).
    pub builder: usize,
}

impl ArtListBudget {
    /// What the caller's own art (the slicer's sprites, the terrain's
    /// tile library, one row per critter sprite set) may use.
    pub fn available(&self) -> usize {
        self.free.saturating_sub(self.builder)
    }
}

fn base_error(file: &str, error: impl std::fmt::Display) -> BuildError {
    BuildError::Base {
        file: file.into(),
        message: error.to_string(),
    }
}

/// `art/<directory>/<directory>.lst`.
pub(crate) fn list_path(category: u8) -> String {
    let directory = ART_DIRECTORIES[usize::from(category)];
    format!("art/{directory}/{directory}.lst")
}

/// Whether `category` starts standalone in this mode.
pub(crate) fn replaced(category: u8, standalone: bool) -> bool {
    standalone && emit::standalone_reserved(category).is_some()
}

/// The list `category` starts as, and its base list's entry count.
pub(crate) fn start_list(
    category: u8,
    base_files: &BTreeMap<String, Vec<u8>>,
    standalone: bool,
) -> Result<(ArtList, usize), BuildError> {
    let path = list_path(category);
    let Some(bytes) = base_files.get(&path) else {
        let list = ArtList::new(category).map_err(|e| base_error(&path, e))?;
        return Ok((list, 0));
    };
    let base = ArtList::from_base(category, bytes).map_err(|e| base_error(&path, e))?;
    if !replaced(category, standalone) {
        let entries = base.len();
        return Ok((base, entries));
    }
    let list = ArtList::standalone(category, bytes).map_err(|e| base_error(&path, e))?;
    Ok((list, base.len()))
}

/// The most entries the builder adds to `category` for its own art when
/// the list starts with `kept` entries.
pub(crate) fn builder_entries(category: u8, kept: usize) -> usize {
    match category {
        ART_SCENERY | ART_MISC => 1,
        ART_TILES => RESERVED_TILE_ENTRIES.saturating_sub(kept),
        _ => 0,
    }
}

/// Free entries per art list, by art directory (`items`, `critters`,
/// `scenery`, `walls`, `tiles`, `misc`), for the base files a build would
/// take ([`crate::BuildInput::base_files`]) and
/// [`crate::BuildOptions::standalone`]. A list whose base file is missing
/// starts empty. Each `available()` is what that list can take of the
/// caller's art; a build that asks for more fails with
/// [`BuildError::FidBudget`].
pub fn art_budget(
    base_files: &BTreeMap<String, Vec<u8>>,
    standalone: bool,
) -> Result<BTreeMap<String, ArtListBudget>, BuildError> {
    let mut out = BTreeMap::new();
    for category in BUILD_LISTS {
        let (list, base_entries) = start_list(category, base_files, standalone)?;
        let kept = list.len();
        out.insert(
            ART_DIRECTORIES[usize::from(category)].to_owned(),
            ArtListBudget {
                replaced: replaced(category, standalone),
                base_entries,
                kept,
                free: FID_INDEX_CAP - kept,
                builder: builder_entries(category, kept),
            },
        );
    }
    let critters = match base_files.get(CRITTER_LIST_PATH) {
        Some(bytes) => {
            CritterList::from_base(bytes).map_err(|e| base_error(CRITTER_LIST_PATH, e))?
        }
        None => CritterList::new(),
    };
    out.insert(
        ART_DIRECTORIES[usize::from(ART_CRITTERS)].to_owned(),
        ArtListBudget {
            replaced: false,
            base_entries: critters.len(),
            kept: critters.len(),
            free: critters.remaining(),
            builder: 0,
        },
    );
    Ok(out)
}
