//! A slice's `[declutter]` and `[tone]` tables, mapped onto the slicer's
//! `map_slicer::DeclutterProfile`.
//!
//! Every slice written before these tables existed has neither, and the
//! per-game default profile is `off`: such a run hands the slicer no
//! profile and the builder no interactive set, so its bytes are what they
//! were. `profile = "readable"` (or `--declutter readable`) starts from
//! the game's readable preset ([`readable_for`]); every other key
//! overrides one setting of it. With `profile = "off"` the other keys are
//! refused, since they would do nothing.

use std::collections::BTreeSet;

use mir::GameVariant;
use serde::{Deserialize, Serialize};

/// `[declutter]` in a slice file. Every key is optional.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclutterSpec {
    /// `"off"` or `"readable"`. Default: the game's default ([`default_profile`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Keep carried items (bottles, glasses, cans, ammunition, weapons,
    /// junk) safe from pruning, as interactive things are. `false` takes
    /// the item kinds off `protected_kinds`, so the clutter rules may drop
    /// the small ones. Default `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protect_items: Option<bool>,
    /// Replaces the protected base kinds outright.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protected_kinds: Option<Vec<String>>,
    /// Protect the references quests, scripts, conditions, links, enable
    /// parents and packages point at (`mir::Records::referenced_refs`).
    /// Default `true`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protect_referenced: Option<bool>,
    /// The micro-clutter rule on or off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clutter: Option<bool>,
    /// Its thresholds: placed volume (cubic units), footprint (hexes) and
    /// height (units) under all of which a prop is clutter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clutter_max_volume: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clutter_max_hexes: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clutter_max_height: Option<f32>,
    /// Model path patterns added to the clutter rule's own (dropped
    /// whatever their size).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clutter_patterns: Option<Vec<String>>,
    /// Thinning of dense piles on or off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub density: Option<bool>,
    /// `"keep"`, `"drop"` or `"fade"` (at `decal_fade` opacity).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decals: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decal_fade: Option<f32>,
    /// Overhang pruning (beams, ducts, fixtures) on or off, indoors.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overhang: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overhang_clearance: Option<f32>,
    /// The lower cut of room shells on or off, indoors, and its height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_cut: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_cut_height: Option<f32>,
    /// The foreground cutaway on or off; its kerb height; whether it also
    /// cuts exterior facades.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cutaway: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kerb_height: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cutaway_exteriors: Option<bool>,
    /// The outline of interactive sprites on or off, and its darkening.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline_factor: Option<f32>,
    /// Also measure the window without the pass (one more render).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measure_baseline: Option<bool>,
    /// Prune decorative tabletop items ([`MiscPrune`]): unreferenced
    /// miscellaneous items (`MISC`) worth at most `prune_misc_max_value`
    /// caps and weighing at most `prune_misc_max_weight`. Default on in
    /// the readable profile, at 1 and 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prune_misc: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prune_misc_max_value: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prune_misc_max_weight: Option<f32>,
}

/// The decorative-item rule of the readable profile (owner decision,
/// 2026-11-23): a placed miscellaneous item (`MISC`) is pruned with the
/// micro-clutter when its base is worth at most `max_value` caps and
/// weighs at most `max_weight`, and nothing marks it as part of the game:
/// it is not in the referenced set (`mir::Records::referenced_refs`:
/// quests, scripts, conditions, links, enable parents, packages), it is
/// not persistent, has no editor id and no linked reference, and its base
/// has no script (quest items carry one). A placed reference is never
/// inside a container, so container contents are never touched. Weapons,
/// ammunition, aid, keys, armour, notes, books and every `MISC` above the
/// thresholds stay protected by kind.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MiscPrune {
    pub max_value: i64,
    pub max_weight: f32,
}

impl Default for MiscPrune {
    fn default() -> Self {
        MiscPrune {
            max_value: 1,
            max_weight: 1.0,
        }
    }
}

/// The decorative-item rule of a run: `None` with the pass off or
/// `prune_misc = false`; the readable default with the slice's thresholds
/// over it.
pub fn misc_prune(profile: &str, spec: Option<&DeclutterSpec>) -> Option<MiscPrune> {
    if profile != "readable" {
        return None;
    }
    let default = DeclutterSpec::default();
    let spec = spec.unwrap_or(&default);
    if spec.prune_misc == Some(false) {
        return None;
    }
    let mut rule = MiscPrune::default();
    if let Some(v) = spec.prune_misc_max_value {
        rule.max_value = v;
    }
    if let Some(w) = spec.prune_misc_max_weight {
        rule.max_weight = w;
    }
    Some(rule)
}

/// What a base record says about an item, for [`MiscPrune::judge`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemFacts {
    pub value: Option<i64>,
    pub weight: Option<f32>,
    pub scripted: bool,
}

impl MiscPrune {
    /// Whether `reference` is a decorative item under this rule; `facts`
    /// is its base's record (`None`: not in the plugin list, kept).
    pub fn judge(
        &self,
        reference: &mir::Reference,
        facts: Option<ItemFacts>,
        referenced: &BTreeSet<String>,
    ) -> bool {
        if reference.base_kind.as_deref() != Some("MISC")
            || referenced.contains(&reference.form.to_string())
            || reference.flags.persistent
            || reference.editor_id.is_some()
            || reference.linked_ref.is_some()
        {
            return false;
        }
        let Some(facts) = facts else {
            return false;
        };
        !facts.scripted
            && facts.value.is_some_and(|v| v <= self.max_value)
            && facts.weight.is_some_and(|w| w <= self.max_weight)
    }
}

/// The decorative items among `references` (form keys), judged by `rule`
/// with each base's record from `facts`.
pub fn decorative_items(
    rule: &MiscPrune,
    references: &[mir::Reference],
    referenced: &BTreeSet<String>,
    mut facts: impl FnMut(&mir::FormKey) -> Option<ItemFacts>,
) -> BTreeSet<String> {
    references
        .iter()
        .filter(|r| r.base_kind.as_deref() == Some("MISC"))
        .filter(|r| rule.judge(r, facts(&r.base), referenced))
        .map(|r| r.form.to_string())
        .collect()
}

/// One class's tone curve in `[tone]`; unset fields keep the profile's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToneCurve {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gamma: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturation: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contrast: Option<f32>,
}

/// `[tone]` in a slice file: `[tone.floor]`, `[tone.props]` and
/// `[tone.figures]`. It needs the declutter pass on (the slicer applies
/// the tones as part of it).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToneSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floor: Option<ToneCurve>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub props: Option<ToneCurve>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub figures: Option<ToneCurve>,
}

/// The profile names a slice or `--declutter` may give.
pub const PROFILES: &[&str] = &["off", "readable"];

/// The base kinds of carried items, which `protect_items = false` takes
/// off the protected list.
pub const ITEM_KINDS: &[&str] = &[
    "ALCH", "AMMO", "ARMO", "BOOK", "CCRD", "CHIP", "CMNY", "IMOD", "KEYM", "MISC", "NOTE", "WEAP",
];

/// The profile a run of `game` uses when neither the slice nor the
/// command line names one: `off` for both games, so that every slice
/// keeps its bytes.
pub fn default_profile(game: GameVariant) -> &'static str {
    match game {
        GameVariant::Fallout3 | GameVariant::FalloutNewVegas => "off",
    }
}

/// The readable preset for a game. New Vegas takes
/// `DeclutterProfile::readable` as it is (tuned on its saloon and casino);
/// Fallout 3 adds the Capital Wasteland's paper and debris heaps to the
/// clutter patterns, and shopping carts: wire meshes that render as brown
/// scribbles from above (judged on the Super-Duper Mart's engine frames).
pub fn readable_for(game: GameVariant) -> map_slicer::DeclutterProfile {
    let mut profile = map_slicer::DeclutterProfile::readable();
    if game == GameVariant::Fallout3
        && let Some(clutter) = &mut profile.clutter
    {
        for pattern in [
            "*paperdebris",
            "*groundlitter",
            "*streetlitter",
            "*paperscatter",
            "*shoppingcart",
        ] {
            if !clutter.patterns.iter().any(|p| p == pattern) {
                clutter.patterns.push(pattern.to_owned());
            }
        }
    }
    profile
}

impl DeclutterSpec {
    /// The profile name this table chooses, or `None` to leave it to the
    /// command line and the game.
    pub fn profile_name(&self) -> Option<&str> {
        self.profile.as_deref()
    }

    /// Whether any key besides `profile` is set.
    fn sets_anything(&self) -> bool {
        let bare = DeclutterSpec {
            profile: self.profile.clone(),
            ..DeclutterSpec::default()
        };
        *self != bare
    }

    pub(crate) fn validate(&self, tone: Option<&ToneSpec>) -> Result<(), String> {
        if let Some(name) = &self.profile
            && !PROFILES.contains(&name.as_str())
        {
            return Err(format!(
                "declutter profile {name:?}: expected one of {}",
                PROFILES.join(", ")
            ));
        }
        if let Some(mode) = &self.decals
            && !["keep", "drop", "fade"].contains(&mode.as_str())
        {
            return Err(format!(
                "declutter decals {mode:?}: expected \"keep\", \"drop\" or \"fade\""
            ));
        }
        if self.profile.as_deref() == Some("off") && (self.sets_anything() || tone.is_some()) {
            return Err(
                "declutter profile \"off\" with other declutter or tone keys: they would do \
                 nothing; remove them or choose \"readable\""
                    .to_owned(),
            );
        }
        Ok(())
    }
}

/// The slicer profile of a run, or `None` when the pass is off.
///
/// `profile` is the effective name (the command line's, else the slice's,
/// else [`default_profile`]); `spec` and `tone` are the slice's tables;
/// `referenced` the references quests and scripts point at.
pub fn profile(
    game: GameVariant,
    profile: &str,
    spec: Option<&DeclutterSpec>,
    tone: Option<&ToneSpec>,
    referenced: &BTreeSet<String>,
) -> Result<Option<map_slicer::DeclutterProfile>, String> {
    let mut out = match profile {
        "off" => {
            if tone.is_some() || spec.is_some_and(DeclutterSpec::sets_anything) {
                return Err(
                    "the declutter pass is off, but the slice sets declutter or tone keys; pass \
                     --declutter readable or set profile = \"readable\""
                        .to_owned(),
                );
            }
            return Ok(None);
        }
        "readable" => readable_for(game),
        other => {
            return Err(format!(
                "declutter profile {other:?}: expected one of {}",
                PROFILES.join(", ")
            ));
        }
    };
    let default = DeclutterSpec::default();
    let spec = spec.unwrap_or(&default);
    if let Some(kinds) = &spec.protected_kinds {
        out.protected_kinds = kinds.clone();
    }
    if spec.protect_items == Some(false) {
        out.protected_kinds
            .retain(|k| !ITEM_KINDS.contains(&k.as_str()));
    }
    if spec.protect_referenced != Some(false) {
        out.protected_refs = referenced.clone();
    }
    match spec.clutter {
        Some(false) => out.clutter = None,
        Some(true) if out.clutter.is_none() => out.clutter = Some(Default::default()),
        _ => {}
    }
    if let Some(clutter) = &mut out.clutter {
        if let Some(v) = spec.clutter_max_volume {
            clutter.max_volume = v;
        }
        if let Some(v) = spec.clutter_max_hexes {
            clutter.max_hexes = v;
        }
        if let Some(v) = spec.clutter_max_height {
            clutter.max_height = v;
        }
        if let Some(extra) = &spec.clutter_patterns {
            clutter.patterns.extend(extra.iter().cloned());
        }
    }
    match spec.density {
        Some(false) => out.density = None,
        Some(true) if out.density.is_none() => out.density = Some(Default::default()),
        _ => {}
    }
    match spec.decals.as_deref() {
        Some("keep") => out.decals = map_slicer::DecalMode::Keep,
        Some("drop") => out.decals = map_slicer::DecalMode::Drop,
        Some("fade") => {
            let current = match out.decals {
                map_slicer::DecalMode::Fade(a) => a,
                _ => 0.25,
            };
            out.decals = map_slicer::DecalMode::Fade(spec.decal_fade.unwrap_or(current));
        }
        _ => {
            if let (Some(a), map_slicer::DecalMode::Fade(_)) = (spec.decal_fade, out.decals) {
                out.decals = map_slicer::DecalMode::Fade(a);
            }
        }
    }
    match spec.overhang {
        Some(false) => out.overhang = None,
        Some(true) if out.overhang.is_none() => out.overhang = Some(Default::default()),
        _ => {}
    }
    if let (Some(rule), Some(v)) = (&mut out.overhang, spec.overhang_clearance) {
        rule.clearance = v;
    }
    match spec.shell_cut {
        Some(false) => out.shell_cut = None,
        Some(true) if out.shell_cut.is_none() => out.shell_cut = Some(Default::default()),
        _ => {}
    }
    if let (Some(rule), Some(v)) = (&mut out.shell_cut, spec.shell_cut_height) {
        rule.cut_height = v;
    }
    match spec.cutaway {
        Some(false) => out.cutaway = None,
        Some(true) if out.cutaway.is_none() => out.cutaway = Some(Default::default()),
        _ => {}
    }
    if let Some(rule) = &mut out.cutaway {
        if let Some(v) = spec.kerb_height {
            rule.kerb_height = v;
        }
        if let Some(v) = spec.cutaway_exteriors {
            rule.exteriors = v;
        }
    }
    match spec.outline {
        Some(false) => out.outline = None,
        Some(true) if out.outline.is_none() => {
            out.outline = Some(quantize::RimOptions::outer(0.45))
        }
        _ => {}
    }
    if let (Some(rim), Some(f)) = (&mut out.outline, spec.outline_factor) {
        rim.factor = f;
    }
    if let Some(v) = spec.measure_baseline {
        out.measure_baseline = v;
    }
    if let Some(tone) = tone {
        let apply = |tone: &mut foundry_render::Tone, curve: Option<ToneCurve>| {
            if let Some(c) = curve {
                tone.gain = c.gain.unwrap_or(tone.gain);
                tone.gamma = c.gamma.unwrap_or(tone.gamma);
                tone.saturation = c.saturation.unwrap_or(tone.saturation);
                tone.contrast = c.contrast.unwrap_or(tone.contrast);
            }
        };
        apply(&mut out.tones.floor, tone.floor);
        apply(&mut out.tones.props, tone.props);
        apply(&mut out.tones.figures, tone.figures);
    }
    Ok(Some(out))
}

/// Lays two readability dials (the `[readability]` table's
/// `ground_calm`, `foreground_kerb_narrow` and `narrow_width_hexes`) over
/// a run's declutter profile:
///
/// - `ground_calm` (0 to 1) calms the floor tone
///   (`foundry_render::ClassTones::calmed`): the baked floor, the floor
///   overlays and, where the terrain takes the floor tone, the terrain.
///   Props, walls and figures keep their tones. 0 changes nothing.
/// - `foreground_kerb_narrow` (units, 0 to the cutaway's kerb) cuts the
///   cutaway's strips beside a walkable band at most `narrow_width_hexes`
///   (at least 1) hexes across to that kerb
///   (`map_slicer::Cutaway::narrow_kerb_height`). `None` changes nothing.
///   Indoors, and outdoors with `cutaway_exteriors = true`, it lowers the
///   kerb of those cut strips; outdoors without `cutaway_exteriors` the
///   cutaway runs for it alone and cuts only those strips (a catwalk's
///   railing), leaving every other exterior strip whole.
///
/// Both belong to the readable pass: with `profile` `None` (the pass off)
/// a dial away from its default is an error naming the way out. At their
/// defaults the profile is left exactly as it was.
pub fn apply_readability(
    profile: Option<&mut map_slicer::DeclutterProfile>,
    ground_calm: f64,
    foreground_kerb_narrow: Option<f64>,
    narrow_width_hexes: u32,
) -> Result<(), String> {
    if !(ground_calm.is_finite() && (0.0..=1.0).contains(&ground_calm)) {
        return Err(format!(
            "readability ground_calm {ground_calm} is outside [0, 1]"
        ));
    }
    if narrow_width_hexes < 1 {
        return Err("readability narrow_width_hexes must be at least 1".to_owned());
    }
    let calm = ground_calm > 0.0;
    let Some(profile) = profile else {
        if calm || foreground_kerb_narrow.is_some() {
            return Err(
                "readability ground_calm and foreground_kerb_narrow act on the declutter pass, \
                 which is off; pass --declutter readable or set profile = \"readable\""
                    .to_owned(),
            );
        }
        return Ok(());
    };
    if calm {
        profile.tones = profile.tones.calmed(ground_calm as f32);
    }
    if let Some(kerb) = foreground_kerb_narrow {
        let Some(cutaway) = profile.cutaway.as_mut() else {
            return Err(
                "readability foreground_kerb_narrow needs the cutaway, which the slice turns off \
                 (cutaway = false)"
                    .to_owned(),
            );
        };
        if !(kerb.is_finite() && kerb >= 0.0 && kerb <= f64::from(cutaway.kerb_height)) {
            return Err(format!(
                "readability foreground_kerb_narrow {kerb} must be from 0 to the kerb ({})",
                cutaway.kerb_height
            ));
        }
        cutaway.narrow_kerb_height = Some(kerb as f32);
        cutaway.narrow_max_width = narrow_width_hexes;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_readability_dials_at_their_defaults_change_nothing() {
        let none = BTreeSet::new();
        let preset = profile(GameVariant::Fallout3, "readable", None, None, &none)
            .unwrap()
            .unwrap();
        let mut dialled = preset.clone();
        apply_readability(Some(&mut dialled), 0.0, None, 3).unwrap();
        assert_eq!(dialled, preset);
        // Off with the defaults: fine; with a dial set: refused.
        assert_eq!(apply_readability(None, 0.0, None, 3), Ok(()));
        assert!(apply_readability(None, 0.5, None, 3).is_err());
        assert!(apply_readability(None, 0.0, Some(16.0), 3).is_err());
    }

    #[test]
    fn the_readability_dials_calm_the_floor_and_lower_the_narrow_kerb() {
        let none = BTreeSet::new();
        let preset = profile(GameVariant::Fallout3, "readable", None, None, &none)
            .unwrap()
            .unwrap();
        let mut dialled = preset.clone();
        apply_readability(Some(&mut dialled), 0.5, Some(16.0), 2).unwrap();
        assert_eq!(dialled.tones, preset.tones.calmed(0.5));
        assert_eq!(dialled.tones.props, preset.tones.props);
        assert!(dialled.tones.floor.gain < preset.tones.floor.gain);
        let cutaway = dialled.cutaway.as_ref().unwrap();
        assert_eq!(cutaway.narrow_kerb_height, Some(16.0));
        assert_eq!(cutaway.narrow_max_width, 2);
        // Everything else as the preset.
        let mut back = dialled.clone();
        back.tones = preset.tones;
        if let Some(c) = back.cutaway.as_mut() {
            c.narrow_kerb_height = None;
            c.narrow_max_width = 3;
        }
        assert_eq!(back, preset);
        // Out of range.
        for (calm, kerb, width) in [
            (1.5, None, 3),
            (f64::NAN, None, 3),
            (0.0, Some(40.0), 3),
            (0.0, Some(-1.0), 3),
            (0.0, Some(16.0), 0),
        ] {
            let mut p = preset.clone();
            assert!(
                apply_readability(Some(&mut p), calm, kerb, width).is_err(),
                "{calm} {kerb:?} {width}"
            );
        }
        // The cutaway off: the narrow kerb has nothing to act on.
        let spec = DeclutterSpec {
            cutaway: Some(false),
            ..DeclutterSpec::default()
        };
        let mut uncut = profile(GameVariant::Fallout3, "readable", Some(&spec), None, &none)
            .unwrap()
            .unwrap();
        assert!(apply_readability(Some(&mut uncut), 0.0, Some(16.0), 3).is_err());
        assert!(apply_readability(Some(&mut uncut), 0.5, None, 3).is_ok());
    }

    #[test]
    fn off_is_no_profile_and_refuses_settings() {
        let none = BTreeSet::new();
        for game in GameVariant::ALL {
            assert_eq!(default_profile(game), "off");
            assert_eq!(profile(game, "off", None, None, &none), Ok(None));
        }
        let spec = DeclutterSpec {
            kerb_height: Some(20.0),
            ..DeclutterSpec::default()
        };
        assert!(profile(GameVariant::Fallout3, "off", Some(&spec), None, &none).is_err());
        let tone = ToneSpec::default();
        assert!(profile(GameVariant::Fallout3, "off", None, Some(&tone), &none).is_err());
        assert!(profile(GameVariant::Fallout3, "loud", None, None, &none).is_err());
    }

    #[test]
    fn readable_takes_the_preset_and_the_overrides() {
        let referenced: BTreeSet<String> = ["fallout3.esm:000abc".to_owned()].into();
        let nv = profile(
            GameVariant::FalloutNewVegas,
            "readable",
            None,
            None,
            &referenced,
        )
        .expect("valid")
        .expect("on");
        let mut preset = map_slicer::DeclutterProfile::readable();
        preset.protected_refs = referenced.clone();
        assert_eq!(nv, preset);

        let spec = DeclutterSpec {
            protect_items: Some(false),
            protect_referenced: Some(false),
            clutter_max_height: Some(20.0),
            clutter_patterns: Some(vec!["*bottle".to_owned()]),
            density: Some(false),
            decals: Some("drop".to_owned()),
            overhang_clearance: Some(90.0),
            shell_cut_height: Some(140.0),
            kerb_height: Some(25.0),
            cutaway_exteriors: Some(true),
            outline: Some(false),
            measure_baseline: Some(false),
            ..DeclutterSpec::default()
        };
        let tone = ToneSpec {
            floor: Some(ToneCurve {
                gain: Some(0.8),
                ..ToneCurve::default()
            }),
            figures: Some(ToneCurve {
                contrast: Some(1.2),
                ..ToneCurve::default()
            }),
            ..ToneSpec::default()
        };
        let fo3 = profile(
            GameVariant::Fallout3,
            "readable",
            Some(&spec),
            Some(&tone),
            &referenced,
        )
        .expect("valid")
        .expect("on");
        assert!(
            fo3.protected_kinds
                .iter()
                .all(|k| !ITEM_KINDS.contains(&k.as_str()))
        );
        assert!(fo3.protected_kinds.contains(&"CONT".to_owned()));
        assert!(fo3.protected_refs.is_empty());
        let clutter = fo3.clutter.as_ref().expect("clutter on");
        assert_eq!(clutter.max_height, 20.0);
        assert!(clutter.patterns.iter().any(|p| p == "*bottle"));
        assert!(clutter.patterns.iter().any(|p| p == "*paperdebris"));
        assert_eq!(fo3.density, None);
        assert_eq!(fo3.decals, map_slicer::DecalMode::Drop);
        assert_eq!(fo3.overhang.as_ref().map(|o| o.clearance), Some(90.0));
        assert_eq!(fo3.shell_cut.as_ref().map(|s| s.cut_height), Some(140.0));
        let cutaway = fo3.cutaway.as_ref().expect("cutaway on");
        assert_eq!((cutaway.kerb_height, cutaway.exteriors), (25.0, true));
        assert_eq!(fo3.outline, None);
        assert!(!fo3.measure_baseline);
        assert_eq!(fo3.tones.floor.gain, 0.8);
        assert_eq!(
            fo3.tones.floor.saturation,
            foundry_render::ClassTones::readable().floor.saturation
        );
        assert_eq!(fo3.tones.figures.contrast, 1.2);
    }

    fn misc(id: u32, kind: &str) -> mir::Reference {
        serde_json::from_value(serde_json::json!({
            "form": format!("fallout3.esm:{id:06x}"),
            "kind": "object",
            "group": "temporary",
            "base": "fallout3.esm:000100",
            "base_kind": kind,
            "transform": {"position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": 1.0},
            "flags": {"persistent": false, "initially_disabled": false, "deleted": false}
        }))
        .expect("a reference")
    }

    fn facts(value: i64, weight: f32) -> Option<ItemFacts> {
        Some(ItemFacts {
            value: Some(value),
            weight: Some(weight),
            scripted: false,
        })
    }

    #[test]
    fn decorative_items_by_value_weight_and_reference() {
        let rule = MiscPrune::default();
        assert_eq!((rule.max_value, rule.max_weight), (1, 1.0));
        let none = BTreeSet::new();
        let cup = misc(0x10, "MISC");
        // Both sides of each threshold.
        assert!(rule.judge(&cup, facts(1, 1.0), &none));
        assert!(rule.judge(&cup, facts(0, 0.5), &none));
        assert!(!rule.judge(&cup, facts(2, 1.0), &none), "worth more");
        assert!(!rule.judge(&cup, facts(1, 1.5), &none), "heavier");
        // Unknown figures, a script, or no record keep it.
        assert!(!rule.judge(&cup, None, &none));
        let scripted = ItemFacts {
            scripted: true,
            ..facts(0, 0.0).expect("facts")
        };
        assert!(!rule.judge(&cup, Some(scripted), &none));
        let unvalued = ItemFacts {
            value: None,
            ..facts(0, 0.0).expect("facts")
        };
        assert!(!rule.judge(&cup, Some(unvalued), &none));
        // A referenced item survives; so do persistent, named or linked ones.
        let referenced: BTreeSet<String> = [cup.form.to_string()].into();
        assert!(!rule.judge(&cup, facts(0, 0.0), &referenced));
        let mut persistent = cup.clone();
        persistent.flags.persistent = true;
        assert!(!rule.judge(&persistent, facts(0, 0.0), &none));
        let mut named = cup.clone();
        named.editor_id = Some("QuestCup".to_owned());
        assert!(!rule.judge(&named, facts(0, 0.0), &none));
        // Only MISC: a cheap, light weapon, aid item or key stays.
        for kind in ["WEAP", "ALCH", "KEYM", "AMMO", "ARMO", "BOOK"] {
            assert!(
                !rule.judge(&misc(0x11, kind), facts(0, 0.1), &none),
                "{kind}"
            );
        }
        // The set over a window.
        let refs = vec![cup.clone(), misc(0x12, "MISC"), misc(0x13, "WEAP")];
        let set = decorative_items(&rule, &refs, &referenced, |_| facts(0, 0.1));
        assert_eq!(set, ["fallout3.esm:000012".to_owned()].into());
    }

    #[test]
    fn the_misc_rule_is_on_in_readable_only_and_takes_the_keys() {
        assert_eq!(misc_prune("off", None), None);
        assert_eq!(misc_prune("readable", None), Some(MiscPrune::default()));
        let spec = DeclutterSpec {
            prune_misc_max_value: Some(5),
            prune_misc_max_weight: Some(0.5),
            ..DeclutterSpec::default()
        };
        assert_eq!(
            misc_prune("readable", Some(&spec)),
            Some(MiscPrune {
                max_value: 5,
                max_weight: 0.5
            })
        );
        let off = DeclutterSpec {
            prune_misc: Some(false),
            ..DeclutterSpec::default()
        };
        assert_eq!(misc_prune("readable", Some(&off)), None);
        // With the pass off the keys are refused.
        let none = BTreeSet::new();
        assert!(profile(GameVariant::Fallout3, "off", Some(&spec), None, &none).is_err());
    }

    #[test]
    fn a_table_validates_its_names() {
        let bad = DeclutterSpec {
            decals: Some("smear".to_owned()),
            ..DeclutterSpec::default()
        };
        assert!(bad.validate(None).is_err());
        let off = DeclutterSpec {
            profile: Some("off".to_owned()),
            outline: Some(true),
            ..DeclutterSpec::default()
        };
        assert!(off.validate(None).is_err());
        let plain_off = DeclutterSpec {
            profile: Some("off".to_owned()),
            ..DeclutterSpec::default()
        };
        assert!(plain_off.validate(None).is_ok());
    }
}
