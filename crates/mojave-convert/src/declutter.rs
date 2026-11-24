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

#[cfg(test)]
mod tests {
    use super::*;

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
