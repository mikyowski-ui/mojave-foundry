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

/// What the landmarks of a run came to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LandmarkStats {
    /// References placed as landmarks.
    pub references: usize,
    /// Piece placements (scenery list).
    pub pieces: usize,
    /// Pieces the visibility cut alone made (each at most
    /// `landmark_piece_max` across).
    pub tiles: usize,
    /// Pieces halved because no hex was both drawn whenever the piece is in
    /// view and late enough to draw it over every walker behind it.
    pub visibility_splits: usize,
    /// Pieces cut because they hid a walker standing in front of some of
    /// their pixels.
    pub sorting_splits: usize,
    /// Walkable hexes whose walker box overlaps a landmark.
    pub walkers: usize,
    /// Of those, walkers still hidden by pixels that stand behind them
    /// (`landmark_max_pieces` reached).
    pub hidden_walkers: usize,
    /// Hexes outside the footprint blocked because a piece would draw under
    /// a walker there pixels that stand in front of it.
    pub sealed_hexes: usize,
    /// Pieces left without an anchor late enough for all their walkers.
    pub unresolved_pieces: usize,
    /// Landmarks that could not be rendered upright (too large for
    /// `max_canvas_pixels`, a piece beyond the FRM limits, a render error)
    /// and were baked into the ground as relief instead. Sorted.
    pub fallbacks: Vec<LandmarkFallback>,
}

impl LandmarkStats {
    pub(crate) fn add(&mut self, stats: &crate::landmark::PieceStats) {
        self.tiles += stats.tiles;
        self.visibility_splits += stats.visibility_splits;
        self.sorting_splits += stats.sorting_splits;
        self.walkers += stats.walkers;
        self.hidden_walkers += stats.hidden;
        self.sealed_hexes += stats.sealed;
        self.unresolved_pieces += stats.unresolved;
    }
}

/// What interior mode ([`crate::SlicerConfig::interior`]) did in a run.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InteriorStats {
    /// The settings the run used.
    pub settings: crate::InteriorSlice,
    /// Placed references that gave floor triangles to the bake.
    pub floor_references: usize,
    /// Of those, references that were nothing but floor: no strip, sprite
    /// or piece of their own.
    pub floor_only: usize,
    /// Floor triangles baked (over every placed reference).
    pub floor_triangles: usize,
    /// Baked floor area on the ground plane, square source units
    /// (overlapping pieces count twice).
    pub floor_area: f64,
    /// Hexes whose centre lies under a baked floor triangle, sorted.
    pub floor_hexes: Vec<i32>,
    /// Ground overlays of the floor bake (the first of
    /// [`crate::Sliced::overlays`]).
    pub floor_overlays: usize,
    /// Placed references with geometry clipped at the cut.
    pub clipped_references: usize,
    /// Triangles wholly or partly above the cut (per distinct render).
    pub cut_triangles: usize,
    /// One-sided down-facing triangles above the floor band dropped (per
    /// distinct render).
    pub ceiling_triangles: usize,
    /// References skipped as wholly above the cut.
    pub above_cut: usize,
    /// References skipped as hanging from the ceiling.
    pub hanging: usize,
    /// References skipped as wholly below the floor band.
    pub below_floor: usize,
}

impl InteriorStats {
    pub(crate) fn new(settings: &crate::InteriorSlice) -> Self {
        InteriorStats {
            settings: settings.clone(),
            ..InteriorStats::default()
        }
    }

    /// Counts the interior skips of `skips` (by reason label).
    pub(crate) fn count_skips(&mut self, skips: &BTreeMap<String, usize>) {
        let n = |reason: crate::SkipReason| skips.get(reason.label()).copied().unwrap_or(0);
        self.above_cut = n(crate::SkipReason::AboveCut);
        self.hanging = n(crate::SkipReason::Hanging);
        self.below_floor = n(crate::SkipReason::BelowFloor);
    }
}

/// References of one base kind the declutter pass kept and pruned.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KindTally {
    /// Placed (not skipped by classification or by the pass).
    pub kept: usize,
    /// Skipped by a declutter rule.
    pub pruned: usize,
}

/// What the foreground cutaway did ([`crate::Cutaway`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CutawayStats {
    /// Strips checked against the walkers.
    pub strips_considered: usize,
    /// Strips cropped to the kerb.
    pub strips_cut: usize,
    /// Opaque pixels they lost.
    pub pixels_removed: u64,
    /// Cut strips left with no pixel (their hexes block without art).
    pub strips_emptied: usize,
    /// Walkers some strip hid, before and after the cut.
    pub walkers_covered_before: usize,
    pub walkers_covered_after: usize,
}

/// How a run's floor reads ([`crate::floor_stats`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FloorStats {
    /// Floor hexes: the walkers and, indoors, the baked floor's hexes.
    pub floor_hexes: usize,
    /// Walker hexes given.
    pub walkers: usize,
    /// Of those, not blocked by anything placed.
    pub walkable: usize,
    /// Of those, hidden behind upright art drawn after them.
    pub covered: usize,
    /// Floor hexes holding a scenery, door, container or item placement.
    pub occupied: usize,
    /// Walkable hexes neither covered nor occupied.
    pub open: usize,
}

impl FloorStats {
    /// Open walkable hexes per floor hex (0 without floor).
    pub fn open_ratio(&self) -> f64 {
        if self.floor_hexes == 0 {
            0.0
        } else {
            self.open as f64 / self.floor_hexes as f64
        }
    }

    /// Floor hexes holding an object, per floor hex (0 without floor).
    pub fn occupied_share(&self) -> f64 {
        if self.floor_hexes == 0 {
            0.0
        } else {
            self.occupied as f64 / self.floor_hexes as f64
        }
    }

    /// Walkable hexes hidden behind upright art, per walkable hex.
    pub fn covered_share(&self) -> f64 {
        if self.walkable == 0 {
            0.0
        } else {
            self.covered as f64 / self.walkable as f64
        }
    }
}

impl fmt::Display for FloorStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "open floor {:.1} % ({} of {} floor hexes), {} of {} walkable hexes hidden \
             ({:.1} %), objects on {:.1} % of the floor",
            100.0 * self.open_ratio(),
            self.open,
            self.floor_hexes,
            self.covered,
            self.walkable,
            100.0 * self.covered_share(),
            100.0 * self.occupied_share()
        )
    }
}

/// What the declutter pass did ([`crate::DeclutterProfile`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeclutterStats {
    /// References pruned, by rule (skip reason label).
    pub pruned: BTreeMap<String, usize>,
    /// References kept and pruned, by base kind (`?` without one).
    pub by_kind: BTreeMap<String, KindTally>,
    /// Protected references, by the first reason that applies.
    pub protected: BTreeMap<String, usize>,
    pub protected_references: usize,
    /// Decals baked at reduced opacity.
    pub decals_faded: usize,
    /// References given the lower shell cut.
    pub shell_cut_references: usize,
    /// Shells that kept the interior's cut for the wall art on them.
    pub shells_kept_for_art: usize,
    /// Sprite placements drawn with the outline.
    pub outlined: usize,
    pub cutaway: CutawayStats,
    /// The floor of the same window without the pass
    /// (`measure_baseline`).
    pub floor_before: Option<FloorStats>,
    /// The floor of this run.
    pub floor_after: Option<FloorStats>,
}

impl DeclutterStats {
    /// References pruned by every rule together.
    pub fn pruned_total(&self) -> usize {
        self.pruned.values().sum()
    }
}

/// A landmark baked into the ground instead, and why.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct LandmarkFallback {
    pub reference: String,
    pub model: String,
    pub reason: String,
}

/// One step of the budget ladder ([`crate::BudgetLadder`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LadderKind {
    /// Perimeter runs merged over more hexes.
    LongerRuns,
    /// More small strips joined to a neighbour.
    CoarserStrips,
    /// The largest sliced models placed as landmarks.
    Landmarks,
    /// The smallest contributors baked into the ground.
    Demoted,
}

impl LadderKind {
    pub fn name(self) -> &'static str {
        match self {
            LadderKind::LongerRuns => "longer perimeter runs",
            LadderKind::CoarserStrips => "coarser strips",
            LadderKind::Landmarks => "largest sliced models as landmarks",
            LadderKind::Demoted => "smallest contributors to the ground",
        }
    }
}

/// A ladder step taken and what it saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LadderStep {
    pub kind: LadderKind,
    /// The threshold it changed, or the number of models it moved.
    pub change: String,
    /// Models it reclassified, sorted; empty for a threshold step.
    pub models: Vec<String>,
    /// References it reclassified.
    pub references: usize,
    /// Entries used in walls, scenery and items before and after it.
    pub before: [usize; 3],
    pub after: [usize; 3],
}

impl LadderStep {
    /// Entries saved per list (negative: the list grew).
    pub fn saved(&self) -> [i64; 3] {
        std::array::from_fn(|i| self.before[i] as i64 - self.after[i] as i64)
    }
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
    pub landmarks: LandmarkStats,
    /// Budget ladder steps taken, in order; empty when the run fit at once.
    pub ladder: Vec<LadderStep>,
    /// Interior mode's tallies; `None` when it is off. An estimate fills
    /// the settings and skips only.
    pub interior: Option<InteriorStats>,
    /// The declutter pass's tallies; `None` when it is off.
    pub declutter: Option<DeclutterStats>,
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
            landmarks: LandmarkStats::default(),
            ladder: Vec::new(),
            interior: None,
            declutter: None,
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

    /// Entries used in walls, scenery and items.
    pub fn used(&self) -> [usize; 3] {
        self.categories.map(|c| c.used)
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
            // A class that did not occur and did not exist in the first
            // pass leaves the table as it was.
            if *kind == ClassKind::Landmark && *count == ClassCount::default() {
                continue;
            }
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
        if let Some(i) = &self.interior {
            let s = &i.settings;
            let _ = writeln!(
                out,
                "Interior: floor at z {}, cut {} above it (z {}), floor band {} to {}, \
                 hanging above {}",
                s.floor_z,
                s.cut_height,
                s.cut_z(),
                s.floor_band[0],
                s.floor_band[1],
                s.hang_clearance
            );
            let _ = writeln!(
                out,
                "  floor baked: {} triangles from {} references ({} floor only), {} hexes, \
                 {:.0} square units, {} overlays",
                i.floor_triangles,
                i.floor_references,
                i.floor_only,
                i.floor_hexes.len(),
                i.floor_area,
                i.floor_overlays
            );
            let _ = writeln!(
                out,
                "  cut: {} references clipped, {} triangles over the cut, {} ceiling triangles \
                 dropped; skipped {} above the cut, {} hanging, {} below the floor",
                i.clipped_references,
                i.cut_triangles,
                i.ceiling_triangles,
                i.above_cut,
                i.hanging,
                i.below_floor
            );
        }
        if let Some(d) = &self.declutter {
            let list = |m: &BTreeMap<String, usize>| {
                m.iter()
                    .map(|(k, n)| format!("{n} {k}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let _ = writeln!(
                out,
                "Declutter: {} references pruned ({}); {} protected ({})",
                d.pruned_total(),
                if d.pruned.is_empty() {
                    "none".to_owned()
                } else {
                    list(&d.pruned)
                },
                d.protected_references,
                if d.protected.is_empty() {
                    "none".to_owned()
                } else {
                    list(&d.protected)
                }
            );
            let kinds: Vec<String> = d
                .by_kind
                .iter()
                .map(|(k, t)| format!("{k} {}/{}", t.kept, t.kept + t.pruned))
                .collect();
            let _ = writeln!(out, "  kept by kind: {}", kinds.join(", "));
            let _ = writeln!(
                out,
                "  {} decals faded, {} shells cut lower ({} kept their cut for wall art), {} \
                 sprites outlined",
                d.decals_faded, d.shell_cut_references, d.shells_kept_for_art, d.outlined
            );
            let c = &d.cutaway;
            let _ = writeln!(
                out,
                "  cutaway: {} of {} strips cut to the kerb ({} emptied), {} pixels removed; \
                 walkers hidden by strips {} -> {}",
                c.strips_cut,
                c.strips_considered,
                c.strips_emptied,
                c.pixels_removed,
                c.walkers_covered_before,
                c.walkers_covered_after
            );
            if let Some(before) = &d.floor_before {
                let _ = writeln!(out, "  floor before: {before}");
            }
            if let Some(after) = &d.floor_after {
                let _ = writeln!(out, "  floor after: {after}");
            }
        }
        let l = &self.landmarks;
        if l.references > 0 || !l.fallbacks.is_empty() {
            let _ = writeln!(
                out,
                "Landmarks: {} placed in {} pieces (scenery list), {} baked as relief instead",
                l.references,
                l.pieces,
                l.fallbacks.len()
            );
            if l.references > 0 {
                let _ = writeln!(
                    out,
                    "  {} tiles, {} visibility splits, {} sorting splits, {} unresolved; \
                     {} walkers beside them, {} still hidden behind a piece, {} hexes sealed",
                    l.tiles,
                    l.visibility_splits,
                    l.sorting_splits,
                    l.unresolved_pieces,
                    l.walkers,
                    l.hidden_walkers,
                    l.sealed_hexes
                );
            }
            for fb in &l.fallbacks {
                let _ = writeln!(
                    out,
                    "  relief instead: {} {} ({})",
                    fb.reference, fb.model, fb.reason
                );
            }
        }
        if !self.ladder.is_empty() {
            let _ = writeln!(out, "Budget ladder:");
            for (n, step) in self.ladder.iter().enumerate() {
                let _ = writeln!(
                    out,
                    "  {}. {} ({}): walls {} -> {}, scenery {} -> {}, items {} -> {}{}",
                    n + 1,
                    step.kind.name(),
                    step.change,
                    step.before[0],
                    step.after[0],
                    step.before[1],
                    step.after[1],
                    step.before[2],
                    step.after[2],
                    if step.references > 0 {
                        format!("; {} references", step.references)
                    } else {
                        String::new()
                    }
                );
                for model in step.models.iter().take(8) {
                    let _ = writeln!(out, "       {model}");
                }
                if step.models.len() > 8 {
                    let _ = writeln!(out, "       and {} more", step.models.len() - 8);
                }
            }
        }
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
