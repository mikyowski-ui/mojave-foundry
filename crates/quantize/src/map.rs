//! The allowed index set and the nearest-colour lookup.

use crate::QuantizeError;
use crate::color::{self, bin_of_linear, bin_of_u8, distance2};
use vault_formats::Palette;
use vault_formats::palette::{CYCLE_RANGES, RED_PULSE_INDEX, TRANSPARENT_INDEX};

/// Why an index is never produced for an opaque pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reserved {
    /// Index 0, the transparency key.
    Transparent,
    /// Inside one of the animated colour-cycling ranges.
    ColourCycle,
    /// The pulsing red entry.
    RedPulse,
    /// A palette triple with a channel above 63.
    Unmapped,
}

/// Side of the lookup table: one cell per 6-bit channel value.
const SIDE: usize = 64;

/// The indices an opaque pixel may use, and a nearest-colour lookup over them.
///
/// **Metric.** Colour distance is Euclidean distance in Oklab, with palette
/// entries at their 8-bit display colour (`Palette::rgba8`, each 6-bit
/// channel times 4) read as sRGB.
///
/// **Lookup.** A 64 × 64 × 64 table holds, for every 6-bit RGB triple (the
/// palette's own precision), the allowed index nearest to its display colour
/// `4·(r, g, b)`. An 8-bit query is rounded to the nearest such triple, so
/// palette colours map exactly and other colours are off by at most two
/// 8-bit levels per channel (on the synthetic test palette the chosen entry
/// is at most about 0.016 Oklab units farther than the true nearest, the
/// worst cases near black). Ties go to the lowest index. The table is
/// built once, by a search over the allowed entries sorted by lightness that
/// stops as soon as the lightness gap alone exceeds the best distance; that
/// search is exact and is also available as [`PaletteMap::nearest_oklab`].
#[derive(Clone)]
pub struct PaletteMap {
    allowed: Vec<u8>,
    reserved: Vec<u8>,
    reasons: [Option<Reserved>; 256],
    colours: [[u8; 3]; 256],
    labs: [[f64; 3]; 256],
    /// Allowed entries as `(L, index)`, sorted by `L` then index.
    by_lightness: Vec<(f64, u8)>,
    table: Box<[u8]>,
}

impl std::fmt::Debug for PaletteMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaletteMap")
            .field("allowed", &self.allowed.len())
            .field("reserved", &self.reserved)
            .finish_non_exhaustive()
    }
}

impl PaletteMap {
    /// Builds the map.
    ///
    /// # Panics
    ///
    /// Only when the palette leaves no allowed index (every entry outside
    /// the reserved ranges is unmapped); [`PaletteMap::try_new`] returns
    /// that case as an error instead.
    pub fn new(palette: &Palette) -> Self {
        match Self::try_new(palette) {
            Ok(map) => map,
            Err(error) => panic!("{error}"),
        }
    }

    /// Builds the map, or returns [`QuantizeError::NoAllowedIndex`].
    pub fn try_new(palette: &Palette) -> Result<Self, QuantizeError> {
        let display = palette.rgba8();
        let colours: [[u8; 3]; 256] = std::array::from_fn(|i| {
            let [r, g, b, _] = display[i];
            [r, g, b]
        });
        let labs = colours.map(color::oklab_from_rgb8);
        let reasons: [Option<Reserved>; 256] = std::array::from_fn(|i| {
            let index = i as u8;
            if index == TRANSPARENT_INDEX {
                Some(Reserved::Transparent)
            } else if CYCLE_RANGES
                .iter()
                .any(|range| (range.first..=range.last).contains(&index))
            {
                Some(Reserved::ColourCycle)
            } else if i == RED_PULSE_INDEX {
                Some(Reserved::RedPulse)
            } else if !palette.is_mapped(index) {
                Some(Reserved::Unmapped)
            } else {
                None
            }
        });
        let (allowed, reserved): (Vec<u8>, Vec<u8>) =
            (0..=255u8).partition(|i| reasons[usize::from(*i)].is_none());
        if allowed.is_empty() {
            return Err(QuantizeError::NoAllowedIndex);
        }
        let mut by_lightness: Vec<(f64, u8)> = allowed
            .iter()
            .map(|i| (labs[usize::from(*i)][0], *i))
            .collect();
        by_lightness.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut map = Self {
            allowed,
            reserved,
            reasons,
            colours,
            labs,
            by_lightness,
            table: Box::new([]),
        };
        let mut table = vec![0u8; SIDE * SIDE * SIDE];
        let level: [f64; SIDE] = std::array::from_fn(|k| color::linear8((4 * k) as u8));
        for (cell, slot) in table.iter_mut().enumerate() {
            let rgb = [
                level[cell / (SIDE * SIDE)],
                level[cell / SIDE % SIDE],
                level[cell % SIDE],
            ];
            *slot = map.nearest_oklab(color::oklab_from_linear(rgb));
        }
        map.table = table.into_boxed_slice();
        Ok(map)
    }

    /// The allowed indices, ascending.
    pub fn allowed(&self) -> &[u8] {
        &self.allowed
    }

    /// The reserved indices, ascending: index 0, the cycling ranges, the
    /// red pulse and every unmapped entry.
    pub fn reserved(&self) -> &[u8] {
        &self.reserved
    }

    /// Why `index` is reserved, or `None` when an opaque pixel may use it.
    pub fn reservation(&self, index: u8) -> Option<Reserved> {
        self.reasons[usize::from(index)]
    }

    pub fn is_allowed(&self, index: u8) -> bool {
        self.reasons[usize::from(index)].is_none()
    }

    /// The 8-bit display colour of an entry (black for an unmapped one).
    pub fn colour(&self, index: u8) -> [u8; 3] {
        self.colours[usize::from(index)]
    }

    /// The Oklab coordinates of an entry's display colour.
    pub fn oklab(&self, index: u8) -> [f64; 3] {
        self.labs[usize::from(index)]
    }

    /// The nearest allowed index to an 8-bit sRGB colour, through the table.
    pub fn nearest(&self, rgb: [u8; 3]) -> u8 {
        self.table[cell(rgb.map(bin_of_u8))]
    }

    /// The nearest allowed index to the exact 8-bit colour, by search
    /// rather than through the table (slower; for diagnostics and tests).
    pub fn nearest_exact(&self, rgb: [u8; 3]) -> u8 {
        self.nearest_oklab(color::oklab_from_rgb8(rgb))
    }

    /// The allowed index nearest to an Oklab colour, ties to the lowest
    /// index. A non-finite coordinate gives an arbitrary allowed index.
    pub fn nearest_oklab(&self, lab: [f64; 3]) -> u8 {
        let entries = &self.by_lightness;
        let start = entries.partition_point(|entry| entry.0 < lab[0]);
        let mut best = (f64::INFINITY, self.allowed[0]);
        let mut consider = |&(lightness, index): &(f64, u8)| {
            let gap = lightness - lab[0];
            if gap * gap > best.0 {
                return false;
            }
            let d = distance2(lab, self.labs[usize::from(index)]);
            if d < best.0 || (d == best.0 && index < best.1) {
                best = (d, index);
            }
            true
        };
        for entry in &entries[start..] {
            if !consider(entry) {
                break;
            }
        }
        for entry in entries[..start].iter().rev() {
            if !consider(entry) {
                break;
            }
        }
        best.1
    }

    /// The nearest allowed index to a linear-light colour (clamped), through
    /// the table.
    pub(crate) fn nearest_linear(&self, rgb: [f64; 3]) -> u8 {
        self.table[cell(rgb.map(bin_of_linear))]
    }

    /// The nearest allowed index to an 8-bit colour whose Oklab lightness is
    /// raised by `shift`. Adding to `L` adds the same amount to every cube
    /// root of the LMS response, so the shift needs no full Oklab round trip.
    pub(crate) fn nearest_shifted(&self, rgb: [u8; 3], shift: f64) -> u8 {
        let p = color::lms_prime(rgb.map(color::linear8)).map(|c| c + shift);
        self.nearest_linear(color::linear_from_lms_prime(p))
    }
}

fn cell(bins: [usize; 3]) -> usize {
    (bins[0] * SIDE + bins[1]) * SIDE + bins[2]
}
