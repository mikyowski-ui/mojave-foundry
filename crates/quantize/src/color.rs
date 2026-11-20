//! Colour conversions: sRGB to linear light, linear light to Oklab and back.
//!
//! Oklab (Björn Ottosson, 2020) is a perceptual space in which Euclidean
//! distance approximates perceived colour difference; `L` runs from 0
//! (black) to 1 (white). Every function here uses only `+ - * /`,
//! comparisons and bit casts on `f64`, which IEEE 754 rounds identically on
//! every machine, so the lookup tables and every quantized byte are the same
//! everywhere. The two roots the conversion needs (the sRGB 2.4 power and the
//! Oklab cube root) are computed by fixed Newton iterations rather than by
//! the platform's `powf` / `cbrt`, whose last bit may differ between C
//! libraries.

use std::sync::OnceLock;

/// Lookup tables shared by every map: the linear value of each 8-bit sRGB
/// level, and the linear values at which 6-bit bins change.
struct Tables {
    linear: [f64; 256],
    /// `thresholds[k]` is the linear value of the 8-bit level `4k + 2`, the
    /// rounding point between the display levels `4k` and `4k + 4`.
    thresholds: [f64; 63],
}

fn tables() -> &'static Tables {
    static TABLES: OnceLock<Tables> = OnceLock::new();
    TABLES.get_or_init(|| Tables {
        linear: std::array::from_fn(|v| srgb_decode(v as f64 / 255.0)),
        thresholds: std::array::from_fn(|k| srgb_decode((4 * k + 2) as f64 / 255.0)),
    })
}

/// The sRGB transfer function, inverted: an encoded value in `0..=1` to
/// linear light.
pub(crate) fn srgb_decode(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        let x = (c + 0.055) / 1.055;
        let square = x * x;
        square * fifth_root(square)
    }
}

/// `a^(1/5)` for `a` in `(0, 1]` by Newton's method from 1 (fixed
/// iteration count, so the result is reproducible).
fn fifth_root(a: f64) -> f64 {
    let mut y = 1.0_f64;
    for _ in 0..64 {
        let y2 = y * y;
        y = (4.0 * y + a / (y2 * y2)) / 5.0;
    }
    y
}

/// Real cube root by a bit-level first guess and five Newton steps.
pub(crate) fn cbrt(x: f64) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return if x.is_nan() { 0.0 } else { x };
    }
    let a = x.abs();
    let high = (a.to_bits() >> 32) as u32;
    let mut y = f64::from_bits(u64::from(high / 3 + 715_094_163) << 32);
    for _ in 0..5 {
        y -= (y * y * y - a) / (3.0 * y * y);
    }
    if x < 0.0 { -y } else { y }
}

/// Linear light of one 8-bit sRGB channel.
pub(crate) fn linear8(v: u8) -> f64 {
    tables().linear[usize::from(v)]
}

/// The 6-bit bin (0..=63) whose display level `4k` is nearest in sRGB to
/// a linear value; values outside `0..=1` clamp to the end bins.
pub(crate) fn bin_of_linear(x: f64) -> usize {
    tables().thresholds.partition_point(|t| *t <= x)
}

/// The 6-bit bin of an 8-bit level: the nearest display level `4k`, ties
/// upward, at most 63. Agrees with `bin_of_linear(linear8(v))`.
pub(crate) fn bin_of_u8(v: u8) -> usize {
    ((usize::from(v) + 2) / 4).min(63)
}

/// Linear sRGB to the cube roots of the Oklab LMS response.
pub(crate) fn lms_prime(rgb: [f64; 3]) -> [f64; 3] {
    let [r, g, b] = rgb;
    let l = 0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b;
    let m = 0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b;
    let s = 0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b;
    [cbrt(l), cbrt(m), cbrt(s)]
}

/// Cube-rooted LMS to Oklab `[L, a, b]`.
pub(crate) fn lab_from_lms_prime(p: [f64; 3]) -> [f64; 3] {
    let [l, m, s] = p;
    [
        0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
        1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
        0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
    ]
}

/// Cube-rooted LMS back to linear sRGB (not clamped).
pub(crate) fn linear_from_lms_prime(p: [f64; 3]) -> [f64; 3] {
    let [l, m, s] = p.map(|c| c * c * c);
    [
        4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s,
        -1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s,
        -0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701_0 * s,
    ]
}

/// Linear sRGB to Oklab.
pub(crate) fn oklab_from_linear(rgb: [f64; 3]) -> [f64; 3] {
    lab_from_lms_prime(lms_prime(rgb))
}

/// The Oklab coordinates `[L, a, b]` of an 8-bit sRGB colour.
pub fn oklab_from_rgb8(rgb: [u8; 3]) -> [f64; 3] {
    oklab_from_linear(rgb.map(linear8))
}

/// Squared Euclidean distance in Oklab.
pub(crate) fn distance2(a: [f64; 3], b: [f64; 3]) -> f64 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_match_the_library_functions() {
        for i in 1..=1000 {
            let x = f64::from(i) / 1000.0;
            assert!((cbrt(x) - x.cbrt()).abs() < 1e-14, "cbrt {x}");
            assert!((cbrt(-x) + x.cbrt()).abs() < 1e-14);
            assert!((cbrt(x * 1e-9) - (x * 1e-9).cbrt()).abs() < 1e-14);
            let c = x;
            let expected = if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            };
            assert!((srgb_decode(c) - expected).abs() < 1e-13, "decode {c}");
        }
        assert_eq!(cbrt(0.0), 0.0);
    }

    #[test]
    fn white_and_black_are_the_ends_of_lightness() {
        let white = oklab_from_rgb8([255, 255, 255]);
        let black = oklab_from_rgb8([0, 0, 0]);
        assert!((white[0] - 1.0).abs() < 1e-6);
        assert!(white[1].abs() < 1e-6 && white[2].abs() < 1e-6);
        assert_eq!(black, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn bins_agree_between_the_integer_and_linear_paths() {
        for v in 0..=255u8 {
            assert_eq!(bin_of_u8(v), bin_of_linear(linear8(v)), "level {v}");
        }
        assert_eq!(bin_of_linear(-1.0), 0);
        assert_eq!(bin_of_linear(2.0), 63);
    }

    #[test]
    fn the_inverse_round_trips() {
        for rgb in [[10u8, 200, 30], [255, 0, 128], [17, 17, 17]] {
            let linear = rgb.map(linear8);
            let back = linear_from_lms_prime(lms_prime(linear));
            for c in 0..3 {
                assert!((back[c] - linear[c]).abs() < 1e-9);
            }
        }
    }
}
