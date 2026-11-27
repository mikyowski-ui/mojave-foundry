//! Per-class tone controls for figure-ground separation.
//!
//! One [`crate::Calibration`] lights every class alike, so a busy floor,
//! the props on it and the people among them all compete at the same
//! contrast. A [`Tone`] is a small, deterministic colour curve that a
//! calling stage applies to the RGBA8 a render resolved to, right after the
//! resolve and before quantizing: the same stage the calibration's
//! contrast acts in, so it composes with it. [`ClassTones`] holds one per
//! class: the floor (ground overlays, an interior's floor bake), props and
//! walls, and figures (critters). Every default is the identity, which
//! leaves every byte as it was.
//!
//! The curve, per opaque pixel, on display values `c` in `0..=1`:
//!
//! 1. `c · gain`;
//! 2. `c^gamma` (above 1 darkens the mid-tones, below 1 lifts them);
//! 3. saturation around the pixel's luma `y = 0.2126 r + 0.7152 g +
//!    0.0722 b`: `y + (c - y) · saturation`;
//! 4. contrast around mid-grey: `(c - 0.5) · contrast + 0.5`;
//!
//! clamped to `0..=1` and rounded. Alpha never changes; transparent pixels
//! are left alone. The power is computed with basic IEEE operations only,
//! so the bytes are the same on every machine.
//!
//! **Ground calm** ([`ClassTones::calmed`], [`Tone::calmed`]). A dial `k`
//! in `0..=1` that sets walkable ground further back than the class tones
//! alone do, so walls and props read in front of it. It scales the floor
//! tone, whatever it is (the preset's, or a slice's `[tone.floor]`):
//! `gain · (1 - 0.20 k)`, `contrast · (1 - 0.05 k)`, `saturation ·
//! (1 - 0.35 k)`, gamma unchanged. Props (walls and interactive things
//! alike, one tone) and figures are left as they are, so against the
//! calmer ground they gain relative contrast. On the readable preset, 0.5
//! takes a mid-grey floor pixel (0.4) about 8 % darker than the preset
//! does and its spread (the slope of the curve) from 0.81 to 0.71; 1.0
//! about 15 % darker, slope 0.62, saturation 0.55. The darkest pixels (under
//! about 0.12) are lifted a few levels, as the preset's own contrast does,
//! so the ground never sinks into black. `k = 0` returns the tone
//! unchanged, bit for bit.

use serde::{Deserialize, Serialize};

use crate::{RenderError, RenderOutput};

/// One colour curve: see the module documentation. [`Tone::IDENTITY`]
/// (every field 1) changes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Tone {
    /// Multiplies every channel (0 to 4).
    pub gain: f32,
    /// Exponent applied to every channel after the gain (0.2 to 5).
    pub gamma: f32,
    /// 0 is grey, 1 unchanged, above 1 more vivid (0 to 4).
    pub saturation: f32,
    /// Around mid-grey; 1 unchanged (0 to 4).
    pub contrast: f32,
}

impl Tone {
    /// The curve that changes nothing.
    pub const IDENTITY: Tone = Tone {
        gain: 1.0,
        gamma: 1.0,
        saturation: 1.0,
        contrast: 1.0,
    };

    /// Whether this curve changes nothing (every field exactly 1).
    pub fn is_identity(&self) -> bool {
        *self == Tone::IDENTITY
    }

    /// Checks every value against its range.
    pub fn validate(&self) -> Result<(), RenderError> {
        let check = |key: &'static str, value: f32, low: f32, high: f32| {
            if value.is_finite() && (low..=high).contains(&value) {
                Ok(())
            } else {
                Err(RenderError::InvalidCalibration {
                    key,
                    reason: format!("tone {key} {value} is outside [{low}, {high}]"),
                })
            }
        };
        check("gain", self.gain, 0.0, 4.0)?;
        check("gamma", self.gamma, 0.2, 5.0)?;
        check("saturation", self.saturation, 0.0, 4.0)?;
        check("contrast", self.contrast, 0.0, 4.0)
    }

    /// The per-channel part (gain and gamma) as a table of display values.
    fn channel_table(&self) -> [f64; 256] {
        let gain = f64::from(self.gain);
        let gamma = f64::from(self.gamma);
        std::array::from_fn(|i| {
            let c = (i as f64 / 255.0 * gain).clamp(0.0, 1.0);
            if gamma == 1.0 { c } else { pow(c, gamma) }
        })
    }

    /// The curve applied to one colour.
    pub fn apply_rgb(&self, rgb: [u8; 3]) -> [u8; 3] {
        if self.is_identity() {
            return rgb;
        }
        self.curve()(rgb)
    }

    fn curve(&self) -> impl Fn([u8; 3]) -> [u8; 3] {
        let table = self.channel_table();
        let saturation = f64::from(self.saturation);
        let contrast = f64::from(self.contrast);
        move |rgb: [u8; 3]| {
            let c = rgb.map(|v| table[usize::from(v)]);
            let y = 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
            c.map(|v| {
                let v = y + (v - y) * saturation;
                let v = (v - 0.5) * contrast + 0.5;
                (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
            })
        }
    }

    /// Applies the curve to every opaque pixel (alpha not 0) of a straight
    /// RGBA8 buffer, in place. A buffer whose length is not a multiple of 4
    /// leaves its last bytes alone.
    pub fn apply(&self, rgba: &mut [u8]) {
        if self.is_identity() {
            return;
        }
        let curve = self.curve();
        for px in rgba.as_chunks_mut::<4>().0 {
            if px[3] != 0 {
                let [r, g, b] = curve([px[0], px[1], px[2]]);
                px[0] = r;
                px[1] = g;
                px[2] = b;
            }
        }
    }

    /// [`Tone::apply`] to a render's colour.
    pub fn apply_output(&self, output: &mut RenderOutput) {
        self.apply(&mut output.rgba);
    }

    /// This curve with the ground calm `amount` (module documentation)
    /// laid over it. `amount` is clamped to `0..=1`; 0 (or a value that is
    /// not finite) returns the curve unchanged.
    pub fn calmed(self, amount: f32) -> Tone {
        if !(amount.is_finite() && amount > 0.0) {
            return self;
        }
        let k = amount.min(1.0);
        Tone {
            gain: self.gain * (1.0 - CALM_GAIN * k),
            gamma: self.gamma,
            saturation: self.saturation * (1.0 - CALM_SATURATION * k),
            contrast: self.contrast * (1.0 - CALM_CONTRAST * k),
        }
    }
}

/// How much a full ground calm (1.0) takes off the floor's gain,
/// saturation and contrast ([`Tone::calmed`]).
const CALM_GAIN: f32 = 0.20;
const CALM_SATURATION: f32 = 0.35;
const CALM_CONTRAST: f32 = 0.05;

impl Default for Tone {
    fn default() -> Self {
        Tone::IDENTITY
    }
}

/// One [`Tone`] per class of what a map draws (module documentation).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClassTones {
    /// Ground overlays and an interior's baked floor.
    pub floor: Tone,
    /// Upright scenery: walls, props, doors, containers, items.
    pub props: Tone,
    /// Critters. The critter stage applies it to its own renders.
    pub figures: Tone,
}

impl ClassTones {
    /// Every class unchanged.
    pub const IDENTITY: ClassTones = ClassTones {
        floor: Tone::IDENTITY,
        props: Tone::IDENTITY,
        figures: Tone::IDENTITY,
    };

    /// The figure-ground preset: the floor a little darker, flatter and
    /// less saturated, so it reads as ground; props unchanged; figures
    /// lifted, so people stand out of a dark room.
    pub fn readable() -> Self {
        ClassTones {
            floor: Tone {
                gain: 0.9,
                gamma: 1.0,
                saturation: 0.85,
                contrast: 0.9,
            },
            props: Tone::IDENTITY,
            figures: Tone {
                gain: 1.1,
                gamma: 0.85,
                saturation: 1.1,
                contrast: 1.05,
            },
        }
    }

    /// These tones with the ground calm `ground_calm` (module
    /// documentation): the floor tone calmed, props and figures as they
    /// are. 0 returns them unchanged.
    pub fn calmed(self, ground_calm: f32) -> ClassTones {
        ClassTones {
            floor: self.floor.calmed(ground_calm),
            ..self
        }
    }

    /// Checks every tone.
    pub fn validate(&self) -> Result<(), RenderError> {
        self.floor.validate()?;
        self.props.validate()?;
        self.figures.validate()
    }
}

/// `x^y` for `x` in `0..=1` and positive `y`, from basic IEEE operations
/// only (so the result does not depend on the platform's maths library).
fn pow(x: f64, y: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    if x >= 1.0 {
        return 1.0;
    }
    exp(y * ln(x))
}

/// Natural logarithm of a positive, finite `x`.
fn ln(x: f64) -> f64 {
    // x = m · 2^k with m in [1, 2), from the bits.
    let bits = x.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i64;
    if exponent == 0 {
        // Subnormal: scale up first.
        return ln(x * 4_503_599_627_370_496.0) - 52.0 * std::f64::consts::LN_2;
    }
    let k = exponent - 1023;
    let m = f64::from_bits((bits & 0x000f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000);
    // ln m = 2 atanh(s), s = (m - 1) / (m + 1) in [0, 1/3).
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    let mut term = s;
    let mut sum = 0.0;
    let mut n = 1.0;
    for _ in 0..30 {
        sum += term / n;
        term *= s2;
        n += 2.0;
    }
    2.0 * sum + k as f64 * std::f64::consts::LN_2
}

/// `e^x` for a finite `x` (results below the smallest normal are 0).
fn exp(x: f64) -> f64 {
    if x < -700.0 {
        return 0.0;
    }
    // x = n ln 2 + r, |r| <= ln 2 / 2.
    let n = (x / std::f64::consts::LN_2 + if x < 0.0 { -0.5 } else { 0.5 }) as i64;
    let r = x - n as f64 * std::f64::consts::LN_2;
    let mut term = 1.0;
    let mut sum = 1.0;
    for i in 1..30 {
        term *= r / f64::from(i);
        sum += term;
    }
    let scale = f64::from_bits(((n + 1023).clamp(1, 2046) as u64) << 52);
    sum * scale
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_changes_nothing() {
        let mut rgba = vec![10, 200, 30, 255, 5, 6, 7, 0, 255, 0, 128, 255];
        let before = rgba.clone();
        Tone::IDENTITY.apply(&mut rgba);
        assert_eq!(rgba, before);
        assert!(Tone::default().is_identity());
        assert!(ClassTones::default() == ClassTones::IDENTITY);
        assert_eq!(Tone::IDENTITY.apply_rgb([1, 2, 3]), [1, 2, 3]);
    }

    #[test]
    fn a_neutral_curve_with_explicit_math_is_exact() {
        // gain 1 and gamma 1 but saturation 1.0 and contrast 1.0 through the
        // full curve: every level maps to itself.
        let tone = Tone {
            gain: 1.0,
            gamma: 1.0,
            saturation: 1.0,
            contrast: 1.0,
        };
        for v in 0..=255u8 {
            assert_eq!(tone.curve()([v, v, v]), [v, v, v]);
        }
    }

    #[test]
    fn transparent_pixels_and_alpha_are_left_alone() {
        let tone = Tone {
            gain: 0.5,
            ..Tone::IDENTITY
        };
        let mut rgba = vec![200, 100, 50, 255, 200, 100, 50, 0];
        tone.apply(&mut rgba);
        assert_eq!(rgba, vec![100, 50, 25, 255, 200, 100, 50, 0]);
    }

    #[test]
    fn saturation_zero_is_grey_and_contrast_flattens() {
        let grey = Tone {
            saturation: 0.0,
            ..Tone::IDENTITY
        };
        let [r, g, b] = grey.apply_rgb([255, 0, 0]);
        assert_eq!((r, g, b), (54, 54, 54));
        let flat = Tone {
            contrast: 0.0,
            ..Tone::IDENTITY
        };
        assert_eq!(flat.apply_rgb([0, 255, 30]), [128, 128, 128]);
    }

    #[test]
    fn gamma_matches_the_power_function() {
        for (x, y) in [(0.5, 2.2), (0.25, 0.5), (0.9, 1.7), (1.0e-3, 0.3)] {
            let ours = pow(x, y);
            let reference = f64::powf(x, y);
            assert!(
                (ours - reference).abs() < 1.0e-12,
                "{x}^{y}: {ours} vs {reference}"
            );
        }
        let tone = Tone {
            gamma: 2.0,
            ..Tone::IDENTITY
        };
        // (128/255)^2 = 0.2520 -> 64.
        assert_eq!(tone.apply_rgb([128, 255, 0]), [64, 255, 0]);
    }

    #[test]
    fn validation_rejects_out_of_range_values() {
        assert!(Tone::IDENTITY.validate().is_ok());
        assert!(ClassTones::readable().validate().is_ok());
        for bad in [
            Tone {
                gain: -1.0,
                ..Tone::IDENTITY
            },
            Tone {
                gamma: 0.0,
                ..Tone::IDENTITY
            },
            Tone {
                saturation: f32::NAN,
                ..Tone::IDENTITY
            },
            Tone {
                contrast: 9.0,
                ..Tone::IDENTITY
            },
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_readable_preset_darkens_the_floor_and_lifts_figures() {
        let tones = ClassTones::readable();
        let mid = [120, 110, 100];
        let floor = tones.floor.apply_rgb(mid);
        let figure = tones.figures.apply_rgb(mid);
        let sum = |c: [u8; 3]| c.iter().map(|v| u32::from(*v)).sum::<u32>();
        assert!(sum(floor) < sum(mid));
        assert!(sum(figure) > sum(mid));
        assert_eq!(tones.props.apply_rgb(mid), mid);
    }

    /// A synthetic floor: a ramp of warm and cool greys, opaque.
    fn floor_ramp() -> Vec<u8> {
        let mut rgba = Vec::new();
        for i in 0..64u32 {
            let v = 30 + i * 3;
            let (r, g, b) = if i % 2 == 0 {
                (v + 12, v, v.saturating_sub(10))
            } else {
                (v.saturating_sub(6), v, v + 8)
            };
            rgba.extend([r as u8, g as u8, b as u8, 255]);
        }
        rgba
    }

    /// Mean and standard deviation of the luma, and the mean chroma (largest
    /// minus smallest channel).
    fn measure(rgba: &[u8]) -> (f64, f64, f64) {
        let px: Vec<[f64; 3]> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])])
            .collect();
        let n = px.len() as f64;
        let luma: Vec<f64> = px
            .iter()
            .map(|c| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2])
            .collect();
        let mean = luma.iter().sum::<f64>() / n;
        let spread = (luma.iter().map(|y| (y - mean).powi(2)).sum::<f64>() / n).sqrt();
        let chroma = px
            .iter()
            .map(|c| {
                c.iter().copied().fold(f64::MIN, f64::max)
                    - c.iter().copied().fold(f64::MAX, f64::min)
            })
            .sum::<f64>()
            / n;
        (mean, spread, chroma)
    }

    #[test]
    fn ground_calm_zero_is_the_preset_bit_for_bit() {
        let tones = ClassTones::readable();
        assert_eq!(tones.calmed(0.0), tones);
        assert_eq!(tones.calmed(-1.0), tones);
        assert_eq!(tones.calmed(f32::NAN), tones);
        assert_eq!(Tone::IDENTITY.calmed(0.0), Tone::IDENTITY);
        // Above 1 is a full calm.
        assert_eq!(tones.calmed(3.0), tones.calmed(1.0));
        assert!(tones.calmed(1.0).validate().is_ok());
    }

    #[test]
    fn ground_calm_lowers_floor_luminance_contrast_and_colour_monotonically() {
        let base = ClassTones::readable();
        let mut last: Option<(f64, f64, f64)> = None;
        for step in 0..=4 {
            let k = step as f32 / 4.0;
            let tones = base.calmed(k);
            let mut floor = floor_ramp();
            tones.floor.apply(&mut floor);
            let now = measure(&floor);
            if let Some(before) = last {
                assert!(now.0 < before.0, "luminance at {k}: {now:?} vs {before:?}");
                assert!(now.1 < before.1, "contrast at {k}: {now:?} vs {before:?}");
                assert!(now.2 <= before.2, "chroma at {k}: {now:?} vs {before:?}");
            }
            last = Some(now);
            // Walls and props, and figures, never change.
            assert_eq!(tones.props, base.props);
            assert_eq!(tones.figures, base.figures);
        }
        // 1.0 is strong but not a black or grey wash: the ramp keeps most
        // of its spread and its mean stays well off black.
        let mut preset = floor_ramp();
        base.floor.apply(&mut preset);
        let mut full = floor_ramp();
        base.calmed(1.0).floor.apply(&mut full);
        let (p, f) = (measure(&preset), measure(&full));
        assert!(f.0 > 0.6 * p.0 && f.0 < 0.95 * p.0, "{f:?} vs {p:?}");
        assert!(f.1 > 0.6 * p.1 && f.1 < 0.9 * p.1, "{f:?} vs {p:?}");
    }

    #[test]
    fn props_stand_out_more_against_a_calmer_floor() {
        // The same colour as floor and as prop: the gap widens with the calm.
        let colour = [150, 130, 110];
        let luma = |c: [u8; 3]| {
            0.2126 * f64::from(c[0]) + 0.7152 * f64::from(c[1]) + 0.0722 * f64::from(c[2])
        };
        let gap = |k: f32| {
            let t = ClassTones::readable().calmed(k);
            luma(t.props.apply_rgb(colour)) - luma(t.floor.apply_rgb(colour))
        };
        assert!(gap(0.5) > gap(0.0));
        assert!(gap(1.0) > gap(0.5));
    }

    #[test]
    fn tones_round_trip_through_toml() {
        let tones = ClassTones::readable();
        let text = toml::to_string(&tones).unwrap();
        let back: ClassTones = toml::from_str(&text).unwrap();
        assert_eq!(back, tones);
        let partial: ClassTones = toml::from_str("[floor]\ngain = 0.5\n").unwrap();
        assert_eq!(partial.floor.gain, 0.5);
        assert!(partial.props.is_identity());
    }
}
