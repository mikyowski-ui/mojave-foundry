//! The isometric declutter and readability pass.
//!
//! A first-person room packs far more decoration than a fixed oblique view
//! can show: seen from above, cans, papers, beams and lamps read as noise,
//! and the walls nearest the camera hide whoever stands behind them. With
//! [`crate::SlicerConfig::declutter`] set, the slicer applies these rules,
//! each configurable and each off unless its option is set
//! ([`DeclutterProfile::readable`] is the documented preset that turns them
//! on):
//!
//! 1. **Protection** ([`Protection`]). A reference whose base kind is in
//!    `protected_kinds` (containers, doors, terminals, activators, every
//!    carried item, notes, books, keys), whose form key is in
//!    `protected_refs` (quest or script references the caller knows of), a
//!    teleport door, a reference with a lock, a linked reference, an editor
//!    id or the persistent flag (each switchable), a landmark, or a
//!    structural piece (sliced, a landmark or relief, at least
//!    `structural_min_hexes` of floor) is never pruned by this pass and
//!    never demoted to the ground by the budget ladder.
//! 2. **Clutter** ([`ClutterRules`], [`DensityRule`], [`DecalMode`]). A
//!    reference of a clutter kind whose placed bounding box is under every
//!    one of `max_volume`, `max_hexes` and `max_height` is dropped
//!    ([`crate::SkipReason::Clutter`]); one whose model matches `patterns`
//!    is dropped whatever its size ([`crate::SkipReason::ClutterPattern`]);
//!    of more than `max` small props within `radius` hex steps of each
//!    other the largest `keep` stay ([`crate::SkipReason::ClutterDensity`]).
//!    Flat decals (`decal_patterns`) are dropped
//!    ([`crate::SkipReason::Decal`]) or baked at reduced opacity.
//! 3. **Overhangs and shells** ([`OverhangRule`], [`ShellCut`]), indoors
//!    only. A reference whose lowest point is more than `clearance` above
//!    the floor, that is thin (at most `max_thickness` tall and
//!    `min_aspect` times as long) or named like a beam, duct, pipe or
//!    fixture, with at least `min_hexes` walkable hexes within `reach` of
//!    its footprint, is dropped ([`crate::SkipReason::Overhang`]). Room
//!    shells (large, or thin long tall walls) are cut at `cut_height`,
//!    lower than the interior's own cut that free-standing props keep, so
//!    partitions open the room while a jukebox stays whole; a shell holding
//!    wall art that reaches above the lower cut keeps the interior's cut
//!    (`keep_wall_art`), so paintings never float. The cut height is part
//!    of the render key. Outdoors ([`RoofCut`]), roof triangles high over
//!    walkable ground leave the render, so a figure under a canopy or a
//!    hut roof is drawn on the ground in plain view, not on top of the
//!    roof's art.
//! 4. **Foreground cutaway** ([`Cutaway`]). Only walls take part: sliced,
//!    at least `min_height` tall, a shell or thin and long, and protected
//!    for no reason but being structural; of a base kind in `kinds`, not
//!    matching `exclude_patterns` nor listed in `exclude_refs`, indoors
//!    reaching the ceiling cut (`reach_cut`), and at least
//!    `room_height_ratio` of the room's wall height, so a long grocery
//!    shelf or a display case stays whole. After such a reference is
//!    rendered and cut into strips, each strip is checked against every
//!    walkable hex whose walker box (`SlicerConfig::walker_box`) it covers:
//!    a strip anchored at hex `o` is drawn after a walker on `h < o`, so
//!    more than `min_pixels` of it inside the box of a walker at least
//!    `min_distance` steps away hide them. A strip hiding more than
//!    `max_walkers` walkers is cut, and so is a strip of the same wall
//!    within `spread` steps of it that hides nobody and backs nobody. A
//!    cut strip loses, column by column, every pixel from the lowest one
//!    whose surface stands higher than `kerb_height` above the wall's foot
//!    upwards. It is cropped after rendering, so render keys and art
//!    deduplication work as before, its blocking is unchanged (a strip
//!    left with no pixel blocks its hex as an invisible blocker), and since
//!    pixels only leave, the draw order guarantees still hold. Interiors
//!    and exterior facades are switched separately. Doors ([`DoorStub`]):
//!    a door or door-like static within `reach` of a cut strip that hides
//!    a walker is cropped the same way to a stub taller than the kerb
//!    (outlined as before; a teleport door keeps enough pixels to click);
//!    doors are placed after every wall so the cut strips are known.
//!    Strips no rule changes are byte-identical to a run without the door
//!    rule.
//! 5. **Figure-ground** ([`foundry_render::ClassTones`],
//!    [`quantize::RimOptions`]). The floor tone is applied to the baked
//!    floor and the flat overlays, the props tone to every upright render
//!    and to relief; the figures tone is the caller's to apply to critters.
//!    Interactive sprites (items, containers, doors, and the base kinds in
//!    `outline_kinds`) get the rim outline, by kind alone: protection only
//!    decides what the clutter rules may prune, and an item they leave is
//!    outlined whether or not it was protected
//!    ([`crate::DeclutterStats::outlined_kinds`]).
//! 6. **Metrics** ([`crate::DeclutterStats`]): references kept and pruned
//!    by rule and by kind, protected references by reason, strips cut,
//!    pixels removed, walkers covered before and after the cutaway, and
//!    [`crate::FloorStats`] for the run (and, with `measure_baseline`, for
//!    the same window without the pass): the open-floor ratio and the share
//!    of floor hexes holding an object.
//!
//! The walkers are `walkers` when given, else `SlicerConfig::open_hexes`
//! and `SlicerConfig::landmark_walkers` together. Without any, the
//! overhang rule counts every hex under a reference, and the cutaway and
//! the floor metrics have nobody to look for.

use std::collections::{BTreeMap, BTreeSet};

use foundry_render::{ClassTones, Projection};
use mir::{MapWindow, Reference};
use quantize::RimOptions;

use crate::classify::{Footprint, MeshFacts};
use crate::hex::{self, Plane};
use crate::report::{DeclutterStats, FloorStats};
use crate::{Class, Role, SkipReason, Sliced, SlicerConfig};

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

/// What protects a reference from this pass and from the ladder's
/// demotion (module documentation, rule 1).
#[derive(Debug, Clone, PartialEq)]
pub struct Protection {
    /// Teleport doors.
    pub teleport: bool,
    /// References with a lock.
    pub lock: bool,
    /// References with a linked reference.
    pub linked_ref: bool,
    /// References with an editor id of their own.
    pub editor_id: bool,
    /// Persistent references (always loaded, addressable by scripts).
    pub persistent: bool,
    /// Owned references (an NPC's or a faction's).
    pub owner: bool,
    /// Landmarks.
    pub landmarks: bool,
    /// Structural pieces: sliced, landmarks or relief, at least
    /// `structural_min_hexes` of floor.
    pub structural: bool,
    pub structural_min_hexes: f64,
}

impl Default for Protection {
    fn default() -> Self {
        Protection {
            teleport: true,
            lock: true,
            linked_ref: true,
            editor_id: true,
            persistent: true,
            owner: false,
            landmarks: true,
            structural: true,
            structural_min_hexes: 6.0,
        }
    }
}

/// Micro-clutter (module documentation, rule 2).
#[derive(Debug, Clone, PartialEq)]
pub struct ClutterRules {
    /// Base kinds this rule may drop. Protection still applies, so the
    /// item kinds listed here are dropped only when `protected_kinds` no
    /// longer names them.
    pub kinds: Vec<String>,
    /// Placed bounding box volume, cubic source units.
    pub max_volume: f64,
    /// Placed footprint, hexes.
    pub max_hexes: f64,
    /// Placed height, source units.
    pub max_height: f32,
    /// Model path patterns (the crate's pattern rules) of decoration never
    /// worth drawing from above, dropped whatever their size and kind.
    pub patterns: Vec<String>,
    /// Wall plates (signs, small posters, mirrors: at most 8 units deep,
    /// at least 12 tall and three times as long as deep) are wall
    /// decoration, not clutter: kept.
    pub keep_wall_plates: bool,
}

impl Default for ClutterRules {
    /// About a can, a plate, a sheet of paper, a tool, a small heap: under
    /// 15,000 cubic units, half a hex and 36 units in every respect.
    fn default() -> Self {
        ClutterRules {
            kinds: strings(&[
                "STAT", "MSTT", "SCOL", "ALCH", "AMMO", "ARMO", "CCRD", "CHIP", "CMNY", "IMOD",
                "MISC", "WEAP",
            ]),
            max_volume: 15_000.0,
            max_hexes: 0.5,
            max_height: 36.0,
            patterns: strings(&[
                "*assortedpapers",
                "*paperpile",
                "*papers0",
                "*debris",
                "*litter",
                "*trashpile",
                "*cigarettebutt",
            ]),
            keep_wall_plates: true,
        }
    }
}

/// Thinning dense piles (module documentation, rule 2).
#[derive(Debug, Clone, PartialEq)]
pub struct DensityRule {
    /// Neighbourhood radius, hex steps.
    pub radius: i32,
    /// More small props than this within the radius of one is a pile...
    pub max: usize,
    /// ...of which the largest `keep` within the radius stay.
    pub keep: usize,
    /// A prop is small under this volume (cubic units)...
    pub max_volume: f64,
    /// ...and this footprint (hexes).
    pub max_hexes: f64,
    /// Wall plates (as for [`ClutterRules::keep_wall_plates`]) are not
    /// props: a row of posters stays.
    pub keep_wall_plates: bool,
}

impl Default for DensityRule {
    fn default() -> Self {
        DensityRule {
            radius: 1,
            max: 4,
            keep: 2,
            max_volume: 60_000.0,
            max_hexes: 1.0,
            keep_wall_plates: true,
        }
    }
}

/// What happens to flat decals (stains, cracks).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum DecalMode {
    /// Baked into the floor as before.
    #[default]
    Keep,
    /// Dropped ([`crate::SkipReason::Decal`]).
    Drop,
    /// Baked with their alpha scaled by this, in `0..=1`.
    Fade(f32),
}

/// Ceiling and overhang pruning, indoors (module documentation, rule 3).
#[derive(Debug, Clone, PartialEq)]
pub struct OverhangRule {
    /// The reference's lowest point is more than this above the floor.
    pub clearance: f32,
    /// And it is at most this tall, or its model matches `patterns`.
    pub max_thickness: f32,
    pub patterns: Vec<String>,
    /// Thin means at most `max_thickness` tall and at least `min_aspect`
    /// times as long as tall: a beam or a plank, not a sign on a wall.
    pub min_aspect: f64,
    /// And at least this many walkable hexes lie within `reach` floor
    /// squares of its footprint.
    pub min_hexes: usize,
    pub reach: f64,
}

impl Default for OverhangRule {
    /// Beams, planks, ducts and fixtures more than 80 units up (above a
    /// counter, below the cut).
    fn default() -> Self {
        OverhangRule {
            clearance: 80.0,
            max_thickness: 40.0,
            patterns: strings(&[
                "*beam",
                "*rafter",
                "*joist",
                "*duct",
                "*pipe",
                "*hanginglight",
                "*hanginglamp",
                "*chandelier",
                "*ceilingfan",
                "*ceilinglight",
            ]),
            min_aspect: 3.0,
            min_hexes: 1,
            reach: 1.0,
        }
    }
}

/// A lower ceiling cut for room shells, indoors (module documentation,
/// rule 3). A piece is a shell when it is sliced, a landmark or relief and
/// covers at least `min_hexes`, or is a wall: at most `max_width` across,
/// at least `min_span` long and `min_height` tall; or its model matches
/// `patterns`.
#[derive(Debug, Clone, PartialEq)]
pub struct ShellCut {
    /// Above the floor; used only when lower than the interior's cut.
    pub cut_height: f32,
    pub min_hexes: f64,
    pub max_width: f64,
    pub min_span: f64,
    pub min_height: f32,
    pub patterns: Vec<String>,
    /// A shell with wall art (a wall plate still drawn, reaching above the
    /// lower cut, within 0.3 floor squares of its footprint) keeps the
    /// interior's cut, so that paintings never float.
    pub keep_wall_art: bool,
}

impl Default for ShellCut {
    fn default() -> Self {
        ShellCut {
            cut_height: 120.0,
            min_hexes: 6.0,
            max_width: 40.0,
            min_span: 192.0,
            min_height: 150.0,
            patterns: Vec::new(),
            keep_wall_art: true,
        }
    }
}

/// The foreground cutaway (module documentation, rule 4).
#[derive(Debug, Clone, PartialEq)]
pub struct Cutaway {
    /// In interior mode.
    pub interiors: bool,
    /// Outdoors (building facades); off unless asked for.
    pub exteriors: bool,
    /// Pixels whose surface stands higher than this above the wall's foot
    /// (the lowest point of its placed bounds, not below the floor band
    /// indoors or the terrain outdoors) leave a cut strip: about knee
    /// height by default.
    pub kerb_height: f32,
    /// A strip hiding more walkers than this is cut.
    pub max_walkers: usize,
    /// A strip hides a walker with more than this many pixels in its box...
    pub min_pixels: usize,
    /// ...standing at least this many hex steps from the strip's hex: a
    /// walker against a far wall's foot (on navmesh reaching under the
    /// wall) is not behind it.
    pub min_distance: i32,
    /// Only walls are cut: sliced references at least `min_height` tall
    /// that cover at least `min_hexes` (a room shell) or are at most
    /// `max_width` across and at least `min_span` long (a wall, a
    /// partition, a door frame, a railing). Furniture (a bar, a pool
    /// table, a booth) stays whole.
    pub min_height: f32,
    pub min_hexes: f64,
    pub max_width: f64,
    pub min_span: f64,
    /// A strip of the same wall within this many hex steps of a cut strip,
    /// hiding nobody and standing behind nobody, is cut too (and so on
    /// along the wall); 0 cuts only the strips that hide someone.
    pub spread: i32,
    /// Base kinds a wall may have; empty takes any. Furniture records
    /// (`FURN`) and activators are not walls.
    pub kinds: Vec<String>,
    /// Model patterns (the crate's pattern rules) that are never walls:
    /// shelves, counters, display cases, freezers, booths, tables.
    pub exclude_patterns: Vec<String>,
    /// Form keys that are never walls: a per-reference opt-out.
    pub exclude_refs: BTreeSet<String>,
    /// Indoors, a wall reaches the ceiling cut: its placed top stands at
    /// least the interior's `cut_height` less `cut_tolerance` above the
    /// floor. A grocery shelf or a display case stops well under it.
    pub reach_cut: bool,
    pub cut_tolerance: f32,
    /// A wall is at least this share of the room's wall height: the median
    /// height of every wall-shaped piece (indoors clipped at the
    /// interior's cut). 0 turns the test off.
    pub room_height_ratio: f32,
    /// Doors standing in a cut wall ([`DoorStub`]); `None` leaves them
    /// whole.
    pub doors: Option<DoorStub>,
}

impl Default for Cutaway {
    fn default() -> Self {
        Cutaway {
            interiors: true,
            exteriors: false,
            kerb_height: 35.0,
            max_walkers: 0,
            min_pixels: 120,
            min_distance: 2,
            min_height: 100.0,
            min_hexes: 6.0,
            max_width: 40.0,
            min_span: 120.0,
            spread: 2,
            kinds: strings(&["STAT", "SCOL"]),
            exclude_patterns: strings(&[
                "furniture\\",
                "grocery\\",
                "billiards\\",
                "*shelf",
                "*shelves",
                "*shelving",
                "*counter",
                "*displaycase",
                "*freezer",
                "*fridge",
                "*cabinet",
                "*booth",
                "*pooltable",
                "*desk",
                "*table",
            ]),
            exclude_refs: BTreeSet::new(),
            reach_cut: true,
            cut_tolerance: 16.0,
            room_height_ratio: 0.75,
            doors: Some(DoorStub::default()),
        }
    }
}

/// Doors in a cut wall (module documentation, rule 4): a door, or a static
/// standing in a doorway (`patterns`), whose hex lies within `reach` hex
/// steps of a strip the cutaway cut and that hides a walker itself is
/// cropped like a cut strip, to `stub_height` above its foot: taller than
/// the kerb, so it still reads as a door, and outlined as before. A
/// teleport door keeps at least `min_click_pixels` opaque pixels (its stub
/// grows until it does), so it can still be clicked.
#[derive(Debug, Clone, PartialEq)]
pub struct DoorStub {
    pub stub_height: f32,
    pub reach: i32,
    /// Model patterns of door-like statics: static doors, door leaves.
    pub patterns: Vec<String>,
    pub min_click_pixels: usize,
}

impl Default for DoorStub {
    fn default() -> Self {
        DoorStub {
            stub_height: 70.0,
            reach: 2,
            patterns: strings(&["*door", "!outdoor", "!doormat", "!doorbell"]),
            min_click_pixels: 400,
        }
    }
}

/// Outdoors: roofs over walkable ground (module documentation, rule 3).
/// A triangle of a sliced object or a landmark whose every corner stands
/// more than `clearance` above the terrain, that faces up or down (the
/// vertical part of its normal at least `min_tilt_cos`: a roof, a canopy, a
/// soffit) and whose ground outline comes within `reach` floor squares of
/// a walkable hex's centre, or that is steeper (a fascia, a wall) and comes
/// within `wall_reach`, is left out of the render,
/// like an interior ceiling: a figure under a canopy, a hut
/// roof or a hangar roof is drawn on the ground in plain view instead of
/// over or under the roof's art. Posts, walls and anything lower stay. The
/// triangles left out are part of the render key, so references with the
/// same mesh but other ground under their roof render apart.
#[derive(Debug, Clone, PartialEq)]
pub struct RoofCut {
    pub clearance: f32,
    pub reach: f64,
    pub min_tilt_cos: f64,
    pub wall_reach: f64,
}

impl Default for RoofCut {
    /// Higher than a head (110 units); roofs tilted at most about 75
    /// degrees within half a floor square (about a hex), steeper triangles
    /// within 0.15.
    fn default() -> Self {
        RoofCut {
            clearance: 120.0,
            reach: 0.5,
            min_tilt_cos: 0.25,
            wall_reach: 0.15,
        }
    }
}

/// The declutter and readability pass (module documentation). The
/// default protects as documented and changes nothing else; see
/// [`DeclutterProfile::readable`] for the preset.
#[derive(Debug, Clone, PartialEq)]
pub struct DeclutterProfile {
    /// Base kinds never pruned.
    pub protected_kinds: Vec<String>,
    /// Form keys (`plugin:xxxxxx`, as `mir::FormKey` prints them) never
    /// pruned: quest and script references, supplied by the caller.
    pub protected_refs: BTreeSet<String>,
    /// Form keys of references the caller judged decorative (tabletop
    /// junk: in the converter, unreferenced miscellaneous items of little
    /// value and weight). Each is pruned whatever its kind's protection
    /// ([`crate::SkipReason::DecorativeItem`]), unless it is also in
    /// `protected_refs`. Default empty: nothing changes.
    pub decorative_items: BTreeSet<String>,
    pub protect: Protection,
    pub clutter: Option<ClutterRules>,
    pub density: Option<DensityRule>,
    pub decals: DecalMode,
    /// Model path patterns of flat decals.
    pub decal_patterns: Vec<String>,
    pub overhang: Option<OverhangRule>,
    pub shell_cut: Option<ShellCut>,
    pub cutaway: Option<Cutaway>,
    /// Outdoors only: roofs over walkable ground leave the render
    /// ([`RoofCut`]).
    pub roof_cut: Option<RoofCut>,
    /// Floor, props and figures tones; the figures tone is the caller's.
    pub tones: ClassTones,
    /// The outline of interactive sprites; `None` draws none.
    pub outline: Option<RimOptions>,
    /// Base kinds outlined besides items, containers and doors.
    pub outline_kinds: Vec<String>,
    /// The walkable hexes the cutaway and the metrics look for; `None`
    /// takes `open_hexes` and `landmark_walkers`.
    pub walkers: Option<BTreeSet<i32>>,
    /// A walker counts as hidden behind upright art with more than this
    /// many pixels of it in their box (floor metrics). The default, 960,
    /// is a quarter of the default 48 x 80 walker box.
    pub cover_min_pixels: usize,
    /// Also slice the window once without the pass, for the "before" floor
    /// metrics. Costs one more render of the window.
    pub measure_baseline: bool,
}

impl Default for DeclutterProfile {
    fn default() -> Self {
        DeclutterProfile {
            protected_kinds: strings(&[
                "CONT", "DOOR", "TERM", "ACTI", "TACT", "ALCH", "AMMO", "ARMO", "BOOK", "CCRD",
                "CHIP", "CMNY", "IMOD", "KEYM", "MISC", "NOTE", "WEAP",
            ]),
            protected_refs: BTreeSet::new(),
            decorative_items: BTreeSet::new(),
            protect: Protection::default(),
            clutter: None,
            density: None,
            decals: DecalMode::Keep,
            decal_patterns: strings(&["decals\\", "*decal", "*stain", "*blood", "*splatter"]),
            overhang: None,
            shell_cut: None,
            cutaway: None,
            roof_cut: None,
            tones: ClassTones::IDENTITY,
            outline: None,
            outline_kinds: strings(&["TERM", "ACTI", "TACT"]),
            walkers: None,
            cover_min_pixels: 960,
            measure_baseline: false,
        }
    }
}

impl DeclutterProfile {
    /// The readability preset (tuned on two real interiors, a saloon and a
    /// casino, and checked on a store and a gas station): every rule on
    /// with its default, decals at 25 % opacity, shells cut at 120, the
    /// interior cutaway at 35 units with door stubs at 70, roofs over
    /// walkable ground cut outdoors at 120, the readable tones, a dark
    /// outer outline on interactive sprites, and the baseline measured.
    pub fn readable() -> Self {
        DeclutterProfile {
            clutter: Some(ClutterRules::default()),
            density: Some(DensityRule::default()),
            decals: DecalMode::Fade(0.25),
            overhang: Some(OverhangRule::default()),
            shell_cut: Some(ShellCut::default()),
            cutaway: Some(Cutaway::default()),
            roof_cut: Some(RoofCut::default()),
            tones: ClassTones::readable(),
            outline: Some(RimOptions::outer(0.45)),
            measure_baseline: true,
            ..DeclutterProfile::default()
        }
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        let finite = |v: f64| v.is_finite() && v >= 0.0;
        if let Some(c) = &self.clutter
            && !(finite(c.max_volume) && finite(c.max_hexes) && finite(f64::from(c.max_height)))
        {
            return Err("declutter clutter thresholds must be finite and non-negative".into());
        }
        if let Some(d) = &self.density
            && !(d.radius >= 0 && d.keep >= 1 && finite(d.max_volume) && finite(d.max_hexes))
        {
            return Err(
                "declutter density needs a radius of at least 0, keep of at least 1 and finite \
                 thresholds"
                    .into(),
            );
        }
        if let DecalMode::Fade(a) = self.decals
            && !(a.is_finite() && (0.0..=1.0).contains(&a))
        {
            return Err("declutter decal fade must be in 0..=1".into());
        }
        if let Some(o) = &self.overhang
            && !(o.clearance.is_finite()
                && o.max_thickness.is_finite()
                && finite(o.min_aspect)
                && o.reach.is_finite()
                && o.reach >= 0.0)
        {
            return Err("declutter overhang thresholds must be finite".into());
        }
        if let Some(s) = &self.shell_cut
            && !(s.cut_height.is_finite()
                && finite(s.min_hexes)
                && finite(s.max_width)
                && finite(s.min_span)
                && s.min_height.is_finite())
        {
            return Err("declutter shell cut values must be finite".into());
        }
        if let Some(c) = &self.cutaway
            && !(c.kerb_height.is_finite()
                && c.kerb_height >= 0.0
                && c.min_height.is_finite()
                && finite(c.min_hexes)
                && finite(c.max_width)
                && finite(c.min_span)
                && c.cut_tolerance.is_finite()
                && finite(f64::from(c.room_height_ratio))
                && c.doors.as_ref().is_none_or(|d| {
                    d.stub_height.is_finite() && d.stub_height >= c.kerb_height && d.reach >= 0
                }))
        {
            return Err(
                "declutter cutaway values must be finite and non-negative, and a door stub at \
                 least the kerb"
                    .into(),
            );
        }
        if let Some(r) = &self.roof_cut
            && !(r.clearance.is_finite()
                && finite(r.reach)
                && finite(r.min_tilt_cos)
                && finite(r.wall_reach))
        {
            return Err("declutter roof cut values must be finite".into());
        }
        if !self.protect.structural_min_hexes.is_finite() {
            return Err("declutter structural_min_hexes must be finite".into());
        }
        self.tones.validate().map_err(|e| e.to_string())?;
        Ok(())
    }

    /// The walkers of a run under `config`.
    pub(crate) fn walkers_of(&self, config: &SlicerConfig) -> BTreeSet<i32> {
        match &self.walkers {
            Some(w) => w.clone(),
            None => {
                let mut w = config.open_hexes.clone();
                if let Some(l) = &config.landmark_walkers {
                    w.extend(l.iter().copied());
                }
                w
            }
        }
    }
}

/// Why a reference is protected, for counting.
fn protection(
    r: &Reference,
    class: &Class,
    hexes: Option<f64>,
    profile: &DeclutterProfile,
) -> Option<&'static str> {
    let p = &profile.protect;
    let kind = r.base_kind.as_deref();
    if kind.is_some_and(|k| profile.protected_kinds.iter().any(|l| l == k)) {
        return Some("kind");
    }
    if profile.protected_refs.contains(&r.form.to_string()) {
        return Some("caller");
    }
    if p.teleport && r.teleport.is_some() {
        return Some("teleport");
    }
    if p.lock && r.lock.is_some() {
        return Some("lock");
    }
    if p.linked_ref && r.linked_ref.is_some() {
        return Some("linked ref");
    }
    if p.editor_id && r.editor_id.is_some() {
        return Some("editor id");
    }
    if p.persistent && r.flags.persistent {
        return Some("persistent");
    }
    if p.owner && r.owner.is_some() {
        return Some("owner");
    }
    if p.landmarks && *class == Class::Landmark {
        return Some("landmark");
    }
    if p.structural
        && matches!(class, Class::Sliced | Class::Landmark | Class::GroundRelief)
        && hexes.is_some_and(|h| h >= p.structural_min_hexes)
    {
        return Some("structural");
    }
    None
}

/// What the pass decided per reference, index-aligned with the prepared
/// references.
#[derive(Debug, Clone, Default)]
pub(crate) struct Decision {
    pub protected: Vec<bool>,
    /// Walls the foreground cutaway may crop.
    pub cutaway: Vec<bool>,
    /// A lower ceiling cut (shells), above the floor.
    pub cuts: Vec<Option<f32>>,
    /// Decals baked at reduced opacity.
    pub faded: Vec<bool>,
    /// Interactive sprites that get the outline.
    pub outlined: Vec<bool>,
    /// Doors and door-like statics the cutaway's door rule may crop.
    pub door_like: Vec<bool>,
    /// The pruning and protection tallies (no run numbers yet).
    pub stats: DeclutterStats,
}

/// One reference's placed facts.
struct Facts {
    footprint: Footprint,
    volume: f64,
    hexes: f64,
    hex: Option<i32>,
    model: String,
}

fn facts_of(
    r: &Reference,
    facts: Option<&MeshFacts>,
    tile_units: f32,
    projection: &Projection,
    window: &MapWindow,
) -> Option<Facts> {
    let bounds = facts?.bounds?;
    if !r.transform.is_finite() {
        return None;
    }
    let footprint = Footprint::of(&r.transform, bounds);
    let s = f64::from(r.transform.scale.abs());
    let volume = (0..3)
        .map(|k| f64::from(bounds[1][k] - bounds[0][k]).max(0.0) * s)
        .product();
    let hexes = footprint.hexes(tile_units);
    let [x, y, z] = r.transform.position;
    let anchor = [window.x_west, window.y_north, z];
    let g = projection.screen(projection.tile_from_source(anchor, [x, y, z]));
    let hex = hex::pick([f64::from(g[0]), f64::from(g[1])]);
    let model = r
        .model
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .replace('/', "\\");
    Some(Facts {
        footprint,
        volume,
        hexes,
        hex,
        model,
    })
}

/// A thin vertical plate: a sign, a poster, a mirror, a painting.
fn wall_plate(f: &Facts) -> bool {
    let (width, long) = (f.footprint.width(), f.footprint.longest_side());
    let height = f64::from(f.footprint.height());
    width <= 8.0 && height >= 12.0 && long >= 20.0_f64.max(3.0 * width)
}

/// Walkable hexes within `reach` floor squares of a footprint.
fn hexes_near(plane: &Plane, footprint: &Footprint, reach: f64, walkers: &BTreeSet<i32>) -> usize {
    let tile: Vec<[f64; 2]> = footprint
        .polygon
        .iter()
        .map(|p| plane.tile_of_source(*p))
        .collect();
    if tile.is_empty() {
        return 0;
    }
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for p in &tile {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k] - reach);
            hi[k] = hi[k].max(p[k] + reach);
        }
    }
    let around = [
        [lo[0], lo[1]],
        [hi[0], lo[1]],
        [hi[0], hi[1]],
        [lo[0], hi[1]],
    ];
    plane
        .hexes_in(&around)
        .into_iter()
        .filter(|h| walkers.is_empty() || walkers.contains(h))
        .filter(|&h| {
            plane
                .hex_tile(h)
                .is_some_and(|c| crate::slice::distance_to_polygon(&tile, c) <= reach)
        })
        .count()
}

/// Runs rules 1 to 3 over the classified references, changing `classes`
/// in place (module documentation).
#[allow(clippy::too_many_arguments)]
pub(crate) fn decide(
    refs: &[&Reference],
    classes: &mut [Class],
    facts: &dyn Fn(&Reference) -> Option<MeshFacts>,
    tile_units: f32,
    plane: &Plane,
    projection: &Projection,
    window: &MapWindow,
    config: &SlicerConfig,
    profile: &DeclutterProfile,
) -> Decision {
    let n = refs.len();
    let mut d = Decision {
        protected: vec![false; n],
        cutaway: vec![false; n],
        cuts: vec![None; n],
        faded: vec![false; n],
        outlined: vec![false; n],
        door_like: vec![false; n],
        stats: DeclutterStats::default(),
    };
    let walkers = profile.walkers_of(config);
    let interior = config.interior.as_ref();
    let placed: Vec<Option<Facts>> = refs
        .iter()
        .map(|r| facts_of(r, facts(r).as_ref(), tile_units, projection, window))
        .collect();
    let kind_of = |r: &Reference| r.base_kind.clone().unwrap_or_else(|| "?".to_owned());
    let in_list = |list: &[String], r: &Reference| {
        r.base_kind
            .as_deref()
            .is_some_and(|k| list.iter().any(|l| l == k))
    };
    let prune = |d: &mut Decision, classes: &mut [Class], i: usize, reason: SkipReason| {
        *d.stats.pruned.entry(reason.label().to_owned()).or_insert(0) += 1;
        classes[i] = Class::Skip(reason);
    };
    for i in 0..n {
        let r = refs[i];
        if matches!(classes[i], Class::Skip(_)) {
            continue;
        }
        let f = placed[i].as_ref();
        if !profile.decorative_items.is_empty() {
            let key = r.form.to_string();
            if profile.decorative_items.contains(&key) && !profile.protected_refs.contains(&key) {
                prune(&mut d, classes, i, SkipReason::DecorativeItem);
                continue;
            }
        }
        let reason = protection(r, &classes[i], f.map(|f| f.hexes), profile);
        if let Some(reason) = reason {
            d.protected[i] = true;
            *d.stats.protected.entry(reason.to_owned()).or_insert(0) += 1;
        }
        // The cutaway crops walls, shells included (protected from pruning
        // as structural); never what is protected for any other reason.
        // (Shape only here; what is not a wall leaves after the loop.)
        if let (Some(c), Some(f), Class::Sliced) = (&profile.cutaway, f, &classes[i])
            && reason.is_none_or(|r| r == "structural")
        {
            let wall =
                f.footprint.width() <= c.max_width && f.footprint.longest_side() >= c.min_span;
            d.cutaway[i] = f.footprint.height() >= c.min_height && (f.hexes >= c.min_hexes || wall);
        }
        if let Some(doors) = profile.cutaway.as_ref().and_then(|c| c.doors.as_ref())
            && let Class::Sprite(role) = &classes[i]
        {
            d.door_like[i] = *role == Role::Door
                || (*role == Role::Scenery
                    && f.is_some_and(|f| config.matches(&doors.patterns, &f.model).is_some()));
        }
        if matches!(
            classes[i],
            Class::Sprite(Role::Item | Role::Container | Role::Door)
        ) || (matches!(classes[i], Class::Sprite(_)) && in_list(&profile.outline_kinds, r))
        {
            d.outlined[i] = profile.outline.is_some();
        }
        if d.protected[i] {
            continue;
        }
        let Some(f) = f else { continue };
        // Decals.
        if profile.decals != DecalMode::Keep
            && config.matches(&profile.decal_patterns, &f.model).is_some()
        {
            match profile.decals {
                DecalMode::Drop => {
                    prune(&mut d, classes, i, SkipReason::Decal);
                    continue;
                }
                DecalMode::Fade(_) if classes[i] == Class::GroundFlat => {
                    d.faded[i] = true;
                }
                _ => {}
            }
        }
        if let Some(c) = &profile.clutter {
            if let Some(p) = config.matches(&c.patterns, &f.model) {
                let name = p.strip_prefix('*').unwrap_or(&p).to_owned();
                prune(&mut d, classes, i, SkipReason::ClutterPattern(name));
                continue;
            }
            if in_list(&c.kinds, r)
                && !(c.keep_wall_plates && wall_plate(f))
                && f.volume <= c.max_volume
                && f.hexes <= c.max_hexes
                && f.footprint.height() <= c.max_height
            {
                prune(&mut d, classes, i, SkipReason::Clutter);
                continue;
            }
        }
        if let (Some(o), Some(indoors)) = (&profile.overhang, interior) {
            let low = f.footprint.z[0] - indoors.floor_z;
            let height = f.footprint.height();
            let thin = (height <= o.max_thickness
                && f.footprint.longest_side() >= o.min_aspect * f64::from(height))
                || config.matches(&o.patterns, &f.model).is_some();
            if low > o.clearance
                && thin
                && hexes_near(plane, &f.footprint, o.reach, &walkers) >= o.min_hexes.max(1)
            {
                prune(&mut d, classes, i, SkipReason::Overhang);
                continue;
            }
        }
    }
    // The cutaway takes walls only: furniture shaped like a wall (a long
    // shelf, a display case) leaves by kind, pattern, opt-out, the ceiling
    // or the room's wall height.
    if let Some(c) = &profile.cutaway {
        let cut_of = |f: &Facts| match interior {
            Some(indoors) => f
                .footprint
                .height()
                .min(indoors.cut_height + indoors.floor_z - f.footprint.z[0]),
            None => f.footprint.height(),
        };
        let mut shells: Vec<f32> = (0..n)
            .filter(|&i| d.cutaway[i] && classes[i] == Class::Sliced)
            .filter_map(|i| placed[i].as_ref())
            .map(cut_of)
            .collect();
        shells.sort_by(f32::total_cmp);
        let room = shells.get(shells.len() / 2).copied();
        for i in 0..n {
            if !(d.cutaway[i] && classes[i] == Class::Sliced) {
                continue;
            }
            let Some(f) = placed[i].as_ref() else {
                continue;
            };
            let r = refs[i];
            let why = if !c.kinds.is_empty() && !in_list(&c.kinds, r) {
                Some("kind")
            } else if c.exclude_refs.contains(&r.form.to_string()) {
                Some("opted out")
            } else if config.matches(&c.exclude_patterns, &f.model).is_some() {
                Some("pattern")
            } else if let Some(indoors) = interior
                && c.reach_cut
                && f.footprint.z[1] - indoors.floor_z < indoors.cut_height - c.cut_tolerance
            {
                Some("under the ceiling cut")
            } else if c.room_height_ratio > 0.0
                && room.is_some_and(|room| cut_of(f) < c.room_height_ratio * room)
            {
                Some("low for the room")
            } else {
                None
            };
            match why {
                Some(why) => {
                    d.cutaway[i] = false;
                    *d.stats.cutaway_excluded.entry(why.to_owned()).or_insert(0) += 1;
                }
                None => d.stats.cutaway_walls += 1,
            }
        }
        d.stats.room_wall_height = room;
    }
    // Dense piles: largest first, a prop stays while fewer than `keep`
    // kept props stand within the radius of it.
    if let Some(rule) = &profile.density {
        let kinds = profile.clutter.as_ref().map(|c| c.kinds.clone());
        let mut small: Vec<(usize, i32)> = (0..n)
            .filter(|&i| !d.protected[i] && !matches!(classes[i], Class::Skip(_)))
            .filter(|&i| !matches!(classes[i], Class::Landmark | Class::GroundRelief))
            .filter(|&i| kinds.as_ref().is_none_or(|k| in_list(k, refs[i])))
            .filter_map(|i| {
                let f = placed[i].as_ref()?;
                let plate = rule.keep_wall_plates && wall_plate(f);
                (!plate && f.volume <= rule.max_volume && f.hexes <= rule.max_hexes)
                    .then_some((i, f.hex?))
            })
            .collect();
        small.sort_by(|a, b| {
            let (fa, fb) = (placed[a.0].as_ref(), placed[b.0].as_ref());
            let va = fa.map_or(0.0, |f| f.volume);
            let vb = fb.map_or(0.0, |f| f.volume);
            vb.total_cmp(&va)
                .then_with(|| refs[a.0].form.to_string().cmp(&refs[b.0].form.to_string()))
        });
        let near = |a: i32, b: i32| hex::distance(a, b) <= rule.radius;
        let mut kept: Vec<i32> = Vec::new();
        let mut dropped = Vec::new();
        for &(i, h) in &small {
            let crowd = small.iter().filter(|(_, o)| near(h, *o)).count();
            let kept_near = kept.iter().filter(|&&o| near(h, o)).count();
            if crowd > rule.max && kept_near >= rule.keep {
                dropped.push(i);
            } else {
                kept.push(h);
            }
        }
        for i in dropped {
            prune(&mut d, classes, i, SkipReason::ClutterDensity);
        }
    }
    // Shells get the lower cut indoors.
    if let (Some(s), Some(indoors)) = (&profile.shell_cut, interior)
        && s.cut_height < indoors.cut_height
    {
        // Wall plates still drawn whose top stands above the lower cut.
        let art: Vec<usize> = (0..n)
            .filter(|&j| !matches!(classes[j], Class::Skip(_)))
            .filter(|&j| {
                placed[j].as_ref().is_some_and(|a| {
                    wall_plate(a) && a.footprint.z[1] - indoors.floor_z > s.cut_height
                })
            })
            .collect();
        for i in 0..n {
            let Some(f) = placed[i].as_ref() else {
                continue;
            };
            if !matches!(
                classes[i],
                Class::Sliced | Class::Landmark | Class::GroundRelief
            ) {
                continue;
            }
            let wall = f.footprint.width() <= s.max_width
                && f.footprint.longest_side() >= s.min_span
                && f.footprint.height() >= s.min_height;
            if !(f.hexes >= s.min_hexes || wall || config.matches(&s.patterns, &f.model).is_some())
            {
                continue;
            }
            // Wall art hung above the lower cut would float: its shell
            // keeps the interior's cut.
            let tile: Vec<[f64; 2]> = f
                .footprint
                .polygon
                .iter()
                .map(|p| plane.tile_of_source(*p))
                .collect();
            let holds_art = s.keep_wall_art
                && art.iter().any(|&j| {
                    placed[j].as_ref().is_some_and(|a| {
                        let c = a
                            .footprint
                            .polygon
                            .iter()
                            .fold([0.0, 0.0], |c, p| [c[0] + p[0], c[1] + p[1]]);
                        let k = a.footprint.polygon.len().max(1) as f64;
                        let centre = plane.tile_of_source([c[0] / k, c[1] / k]);
                        crate::slice::distance_to_polygon(&tile, centre) <= 0.3
                    })
                });
            if holds_art {
                d.stats.shells_kept_for_art += 1;
                continue;
            }
            d.cuts[i] = Some(s.cut_height);
            d.stats.shell_cut_references += 1;
        }
    }
    d.stats.decals_faded = d.faded.iter().filter(|v| **v).count();
    d.stats.protected_references = d.protected.iter().filter(|v| **v).count();
    // Kept and pruned per kind, over what classification placed at all.
    for i in 0..n {
        let tally = d.stats.by_kind.entry(kind_of(refs[i])).or_default();
        match &classes[i] {
            Class::Skip(reason) if reason.is_declutter() => tally.pruned += 1,
            Class::Skip(_) => {}
            _ => tally.kept += 1,
        }
    }
    d.stats.by_kind.retain(|_, t| t.kept + t.pruned > 0);
    d
}

/// Which walkers of a run are free to stand and which of those are hidden
/// ([`walker_cover`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WalkerCover {
    /// Walker hexes no blocking placement and no blocked hex holds.
    pub walkable: BTreeSet<i32>,
    /// Of those, the hexes whose walker is hidden behind upright art.
    pub hidden: BTreeSet<i32>,
}

/// The walkers of `walkers` that can stand in `sliced`, and those of them
/// hidden: a walker on hex `h` is hidden when the upright art drawn after
/// it (every placement but floor pieces anchored at a higher hex, by the
/// engine's order) has more than `min_pixels` opaque pixels inside its
/// `walker_box` (bottom centre on the hex centre). Overlapping placements
/// count each pixel once.
pub fn walker_cover(
    sliced: &Sliced,
    walkers: &BTreeSet<i32>,
    walker_box: [u32; 2],
    min_pixels: usize,
) -> WalkerCover {
    let mut blocked: BTreeSet<i32> = sliced.blocked_hexes.iter().copied().collect();
    for p in &sliced.placements {
        if p.blocking {
            blocked.insert(p.hex);
        }
    }
    let [bw, bh] = walker_box.map(i64::from);
    // Placements by 128-pixel buckets of their drawn extent.
    const CELL: i64 = 128;
    let mut rects = Vec::with_capacity(sliced.placements.len());
    let mut buckets: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
    for (k, p) in sliced.placements.iter().enumerate() {
        let a = &sliced.art[p.art];
        let Some(c) = hex::center(p.hex) else {
            rects.push(None);
            continue;
        };
        let left =
            c[0] + i64::from(a.shift[0]) + i64::from(p.displacement[0]) - i64::from(a.width / 2);
        let top =
            c[1] + i64::from(a.shift[1]) + i64::from(p.displacement[1]) - (i64::from(a.height) - 1);
        let rect = [
            left,
            top,
            left + i64::from(a.width) - 1,
            top + i64::from(a.height) - 1,
        ];
        rects.push(Some(rect));
        if p.role == Role::FloorPiece || a.width == 0 || a.height == 0 {
            continue;
        }
        for by in rect[1].div_euclid(CELL)..=rect[3].div_euclid(CELL) {
            for bx in rect[0].div_euclid(CELL)..=rect[2].div_euclid(CELL) {
                buckets.entry((bx, by)).or_default().push(k);
            }
        }
    }
    let mut out = WalkerCover::default();
    let mut mask = vec![false; (bw * bh).max(0) as usize];
    for &h in walkers {
        if blocked.contains(&h) {
            continue;
        }
        out.walkable.insert(h);
        let Some(c) = hex::center(h) else { continue };
        let b = [c[0] - bw / 2, c[1] - bh + 1, c[0] - bw / 2 + bw - 1, c[1]];
        mask.iter_mut().for_each(|m| *m = false);
        let mut seen = BTreeSet::new();
        let mut count = 0usize;
        for by in b[1].div_euclid(CELL)..=b[3].div_euclid(CELL) {
            for bx in b[0].div_euclid(CELL)..=b[2].div_euclid(CELL) {
                for &k in buckets.get(&(bx, by)).map(Vec::as_slice).unwrap_or(&[]) {
                    let p = &sliced.placements[k];
                    if p.hex <= h || !seen.insert(k) {
                        continue;
                    }
                    let Some(r) = rects[k] else { continue };
                    let a = &sliced.art[p.art];
                    let (x0, x1) = (r[0].max(b[0]), r[2].min(b[2]));
                    let (y0, y1) = (r[1].max(b[1]), r[3].min(b[3]));
                    for y in y0..=y1 {
                        for x in x0..=x1 {
                            let v =
                                a.pixels[((y - r[1]) * i64::from(a.width) + (x - r[0])) as usize];
                            let m = &mut mask[((y - b[1]) * bw + (x - b[0])) as usize];
                            if v != 0 && !*m {
                                *m = true;
                                count += 1;
                            }
                        }
                    }
                }
            }
        }
        if count > min_pixels {
            out.hidden.insert(h);
        }
    }
    out
}

/// How the floor of a run reads (module documentation, rule 6), for
/// `walkers` and `floor` hexes: walkable and hidden as [`walker_cover`]
/// says; a floor hex is occupied when a scenery, door, container or item
/// placement is anchored on it; open walkable hexes are neither hidden
/// nor occupied.
pub fn floor_stats(
    sliced: &Sliced,
    walkers: &BTreeSet<i32>,
    floor: &BTreeSet<i32>,
    walker_box: [u32; 2],
    min_pixels: usize,
) -> FloorStats {
    let occupied: BTreeSet<i32> = sliced
        .placements
        .iter()
        .filter(|p| {
            matches!(
                p.role,
                Role::Scenery | Role::Door | Role::Container | Role::Item
            )
        })
        .map(|p| p.hex)
        .filter(|h| floor.contains(h))
        .collect();
    let cover = walker_cover(sliced, walkers, walker_box, min_pixels);
    let open = cover
        .walkable
        .iter()
        .filter(|h| !cover.hidden.contains(h) && !occupied.contains(h))
        .count();
    FloorStats {
        floor_hexes: floor.len(),
        walkers: walkers.len(),
        walkable: cover.walkable.len(),
        covered: cover.hidden.len(),
        occupied: occupied.len(),
        open,
    }
}
