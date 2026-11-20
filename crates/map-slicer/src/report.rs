//! The budget accounting a run returns, on success and on overflow.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};

use crate::{ArtCategory, Budget, ClassKind};

/// References and art of one class.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClassCount {
    /// References classified so.
    pub references: usize,
    /// Distinct renders (mesh, rotation, scale) they needed.
    pub renders: usize,
    /// Art entries they introduced, after deduplication.
    pub entries: usize,
}

/// One art list's demand against its free entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CategoryUse {
    pub category: ArtCategory,
    /// Distinct entries this map needs.
    pub used: usize,
    /// Entries the list has free.
    pub free: usize,
}

impl CategoryUse {
    /// Free minus used; negative on overflow.
    pub fn headroom(&self) -> i64 {
        self.free as i64 - self.used as i64
    }
}

/// A model and the art entries it introduced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contributor {
    pub category: ArtCategory,
    /// The normalised model path.
    pub model: String,
    pub class: ClassKind,
    pub references: usize,
    pub entries: usize,
}

/// Minimum, median, 90th percentile and maximum of a list of sizes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Spread {
    pub min: u32,
    pub median: u32,
    pub p90: u32,
    pub max: u32,
}

impl Spread {
    pub(crate) fn of(values: &mut [u32]) -> Self {
        if values.is_empty() {
            return Spread::default();
        }
        values.sort_unstable();
        let at = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize];
        Spread {
            min: values[0],
            median: at(0.5),
            p90: at(0.9),
            max: values[values.len() - 1],
        }
    }
}

impl fmt::Display for Spread {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} / {} / {} / {}",
            self.min, self.median, self.p90, self.max
        )
    }
}

/// What the strips of a run look like.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StripStats {
    /// Strip placements.
    pub placed: usize,
    /// Distinct strip art entries.
    pub distinct: usize,
    /// Strips that are merged runs (interior or perimeter).
    pub runs: usize,
    /// Hexes whose few pixels went to a neighbour.
    pub merged_tiny: usize,
    /// Hexes folded into another hex's run strip.
    pub folded: usize,
    /// Opaque pixels whose ground point fell off the map.
    pub dropped_pixels: u64,
    /// Floor pieces placed (scenery list): walking surfaces taken out of
    /// the strips.
    pub floor_pieces: usize,
    /// Widths and heights of the distinct single-hex strips (runs excluded).
    pub width: Spread,
    pub height: Spread,
}

/// The accounting of one run: per class, per art list, the largest
/// contributors and how the strips came out. Filled on success and carried
/// by [`crate::SliceError::FidBudget`] on overflow.
#[derive(Debug, Clone, PartialEq)]
pub struct BudgetReport {
    /// From [`crate::estimate`]: footprints counted, nothing rendered.
    pub estimated: bool,
    pub classes: BTreeMap<ClassKind, ClassCount>,
    /// Walls, scenery, items.
    pub categories: [CategoryUse; 3],
    /// The largest contributors of each list, most entries first.
    pub contributors: Vec<Contributor>,
    pub strips: StripStats,
    /// Skipped references by reason.
    pub skips: BTreeMap<String, usize>,
    pub overlays: usize,
    pub blocked_hexes: usize,
}

impl BudgetReport {
    pub(crate) fn new(estimated: bool, budget: &Budget) -> Self {
        let category = |category| CategoryUse {
            category,
            used: 0,
            free: budget.free(category),
        };
        BudgetReport {
            estimated,
            classes: ClassKind::ALL
                .iter()
                .map(|k| (*k, ClassCount::default()))
                .collect(),
            categories: [
                category(ArtCategory::Walls),
                category(ArtCategory::Scenery),
                category(ArtCategory::Items),
            ],
            contributors: Vec::new(),
            strips: StripStats::default(),
            skips: BTreeMap::new(),
            overlays: 0,
            blocked_hexes: 0,
        }
    }

    /// The use of one list.
    pub fn category(&self, category: ArtCategory) -> &CategoryUse {
        &self.categories[category.index()]
    }

    pub(crate) fn category_mut(&mut self, category: ArtCategory) -> &mut CategoryUse {
        &mut self.categories[category.index()]
    }

    pub(crate) fn class_mut(&mut self, kind: ClassKind) -> &mut ClassCount {
        self.classes.entry(kind).or_default()
    }

    /// The first list whose demand exceeds its free entries.
    pub fn overflow(&self) -> Option<&CategoryUse> {
        self.categories.iter().find(|c| c.used > c.free)
    }

    /// Whether every list fits.
    pub fn fits(&self) -> bool {
        self.overflow().is_none()
    }

    /// The overflow as an error, or `Ok` when everything fits.
    pub fn check(&self) -> Result<(), crate::SliceError> {
        match self.overflow() {
            None => Ok(()),
            Some(over) => Err(crate::SliceError::FidBudget {
                category: over.category,
                cap: over.free,
                demand: over.used,
                top: self
                    .contributors
                    .iter()
                    .filter(|c| c.category == over.category)
                    .cloned()
                    .collect(),
                report: Box::new(self.clone()),
            }),
        }
    }

    /// Ranks `all` and keeps the `top` largest of each list.
    pub(crate) fn set_contributors(&mut self, mut all: Vec<Contributor>, top: usize) {
        all.sort_by(|a, b| {
            a.category
                .cmp(&b.category)
                .then(b.entries.cmp(&a.entries))
                .then(a.model.cmp(&b.model))
                .then(a.class.cmp(&b.class))
        });
        let mut kept: BTreeMap<ArtCategory, usize> = BTreeMap::new();
        self.contributors = all
            .into_iter()
            .filter(|c| c.entries > 0)
            .filter(|c| {
                let n = kept.entry(c.category).or_insert(0);
                *n += 1;
                *n <= top
            })
            .collect();
    }
}

pub(crate) fn contributor_table(out: &mut String, rows: &[&Contributor]) {
    let _ = writeln!(
        out,
        "  {:>7}  {:>5}  {:<13}  model",
        "entries", "refs", "class"
    );
    for c in rows {
        let _ = writeln!(
            out,
            "  {:>7}  {:>5}  {:<13}  {}",
            c.entries, c.references, c.class, c.model
        );
    }
}

impl fmt::Display for BudgetReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "Art budget{}:",
            if self.estimated {
                " (estimate, nothing rendered)"
            } else {
                ""
            }
        );
        let _ = writeln!(
            out,
            "  {:<13}  {:>6}  {:>7}  {:>7}",
            "class", "refs", "renders", "entries"
        );
        for (kind, count) in &self.classes {
            let _ = writeln!(
                out,
                "  {:<13}  {:>6}  {:>7}  {:>7}",
                kind.name(),
                count.references,
                count.renders,
                count.entries
            );
        }
        let _ = writeln!(
            out,
            "  {:<8}  {:>6}  {:>6}  {:>8}",
            "list", "used", "free", "headroom"
        );
        for c in &self.categories {
            let _ = writeln!(
                out,
                "  {:<8}  {:>6}  {:>6}  {:>8}{}",
                c.category.name(),
                c.used,
                c.free,
                c.headroom(),
                if c.used > c.free { "  OVER" } else { "" }
            );
        }
        let s = &self.strips;
        let _ = writeln!(
            out,
            "Strips: {} placed, {} distinct, {} runs ({} hexes folded into them), {} tiny merged, \
             {} pixels off the map",
            s.placed, s.distinct, s.runs, s.folded, s.merged_tiny, s.dropped_pixels
        );
        let _ = writeln!(
            out,
            "  single-hex strip width min/median/p90/max {}, height {}",
            s.width, s.height
        );
        let _ = writeln!(out, "Floor pieces: {} placed", s.floor_pieces);
        let _ = writeln!(
            out,
            "Overlays: {} pieces; blocked hexes: {}",
            self.overlays, self.blocked_hexes
        );
        if !self.skips.is_empty() {
            let _ = writeln!(out, "Skipped:");
            for (reason, n) in &self.skips {
                let _ = writeln!(out, "  {n:>6}  {reason}");
            }
        }
        for category in ArtCategory::ALL {
            let rows: Vec<&Contributor> = self
                .contributors
                .iter()
                .filter(|c| c.category == category)
                .collect();
            if !rows.is_empty() {
                let _ = writeln!(out, "Largest {} contributors:", category.name());
                contributor_table(&mut out, &rows);
            }
        }
        f.write_str(out.trim_end())
    }
}

pub(crate) fn fid_message(
    category: &ArtCategory,
    cap: &usize,
    demand: &usize,
    top: &[Contributor],
) -> String {
    let mut out = format!(
        "the {} art list has {cap} free entries but this map needs {demand} (over by {})\n\
         largest contributors:\n",
        category.name(),
        demand.saturating_sub(*cap)
    );
    let rows: Vec<&Contributor> = top.iter().collect();
    contributor_table(&mut out, &rows);
    out.trim_end().to_owned()
}
