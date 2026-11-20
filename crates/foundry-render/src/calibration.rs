//! Renderer calibration: the section 4 keys of `docs/contracts.md` plus the
//! sprite look (supersampling, alpha threshold, contrast, lighting).

use serde::{Deserialize, Serialize};

use crate::RenderError;

/// Every tunable of the projection and the renderer.
///
/// All keys are optional in TOML; a missing key takes its default. Unknown
/// keys are rejected so that a typo does not silently fall back to a
/// default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Calibration {
    /// Source units per metre (default 70).
    pub units_per_metre: f32,
    /// Metres per floor square (default 2).
    pub tile_metres: f32,
    /// Screen pixels per vertical metre (default 32, so one 96 px storey is
    /// 3 m).
    pub pixels_per_metre_vertical: f32,
    /// Rotates the window about Z in 90° steps (any integer, taken modulo
    /// 4). Source offsets from the anchor are turned counter-clockwise seen
    /// from above by this many quarter turns before mapping to tile space.
    pub yaw_quarter_turns: i32,
    /// Sub-samples per pixel along each axis (1 to 8, default 4).
    pub supersample: u32,
    /// Fraction of covered sub-samples at which a pixel becomes opaque
    /// (greater than 0, at most 1; default 0.5).
    pub alpha_threshold: f32,
    /// Contrast around mid-grey applied after the resolve: `c' = (c - 0.5)
    /// · contrast + 0.5`, clamped. 1 is off (0.25 to 4, default 1).
    pub contrast: f32,
    /// Direction *towards* the key light in map axes `(s, t, e)`: `+s` map
    /// east, `+t` map south, `+e` up, metric. Normalised on use. The
    /// default `(-1, -1, 1.5)` lights from the upper left of the screen and
    /// from above, whatever the yaw.
    pub key_light_direction: [f32; 3],
    /// Key light intensity (0 to 4, default 0.75).
    pub key_light_intensity: f32,
    /// Ambient fill added to every lit surface (0 to 1, default 0.4).
    pub ambient: f32,
}

impl Default for Calibration {
    fn default() -> Self {
        Calibration {
            units_per_metre: 70.0,
            tile_metres: 2.0,
            pixels_per_metre_vertical: 32.0,
            yaw_quarter_turns: 0,
            supersample: 4,
            alpha_threshold: 0.5,
            contrast: 1.0,
            key_light_direction: [-1.0, -1.0, 1.5],
            key_light_intensity: 0.75,
            ambient: 0.4,
        }
    }
}

fn range(
    key: &'static str,
    value: f32,
    low: f32,
    high: f32,
    low_open: bool,
) -> Result<(), RenderError> {
    let ok =
        value.is_finite() && value <= high && if low_open { value > low } else { value >= low };
    if ok {
        Ok(())
    } else {
        let open = if low_open { "(" } else { "[" };
        Err(RenderError::InvalidCalibration {
            key,
            reason: format!("{value} is outside {open}{low}, {high}]"),
        })
    }
}

impl Calibration {
    /// Parses a calibration table. Missing keys take their defaults; the
    /// result is validated.
    pub fn from_toml(text: &str) -> Result<Self, RenderError> {
        let calibration: Calibration =
            toml::from_str(text).map_err(|e| RenderError::Toml(e.to_string()))?;
        calibration.validate()?;
        Ok(calibration)
    }

    /// Writes the calibration as a TOML table that [`Calibration::from_toml`]
    /// reads back unchanged.
    pub fn to_toml(&self) -> Result<String, RenderError> {
        toml::to_string(self).map_err(|e| RenderError::Toml(e.to_string()))
    }

    /// Checks every value against its range.
    pub fn validate(&self) -> Result<(), RenderError> {
        range("units_per_metre", self.units_per_metre, 0.0, 1.0e6, true)?;
        range("tile_metres", self.tile_metres, 0.0, 1.0e4, true)?;
        range(
            "pixels_per_metre_vertical",
            self.pixels_per_metre_vertical,
            0.0,
            1.0e4,
            true,
        )?;
        if !(1..=8).contains(&self.supersample) {
            return Err(RenderError::InvalidCalibration {
                key: "supersample",
                reason: format!("{} is outside [1, 8]", self.supersample),
            });
        }
        range("alpha_threshold", self.alpha_threshold, 0.0, 1.0, true)?;
        range("contrast", self.contrast, 0.25, 4.0, false)?;
        range(
            "key_light_intensity",
            self.key_light_intensity,
            0.0,
            4.0,
            false,
        )?;
        range("ambient", self.ambient, 0.0, 1.0, false)?;
        let [x, y, z] = self.key_light_direction;
        let length = (x * x + y * y + z * z).sqrt();
        if !(length.is_finite() && length > 1.0e-6) {
            return Err(RenderError::InvalidCalibration {
                key: "key_light_direction",
                reason: "must be a finite, non-zero vector".to_owned(),
            });
        }
        Ok(())
    }

    /// Source units per floor square.
    pub fn tile_units(&self) -> f32 {
        self.units_per_metre * self.tile_metres
    }

    /// The key light direction, unit length, in map axes `(s, t, e)`.
    pub fn light(&self) -> [f32; 3] {
        let [x, y, z] = self.key_light_direction;
        let length = (x * x + y * y + z * z).sqrt();
        if length > 0.0 && length.is_finite() {
            [x / length, y / length, z / length]
        } else {
            [0.0, 0.0, 1.0]
        }
    }
}
