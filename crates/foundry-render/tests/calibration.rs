//! `Calibration` parsing and validation.

use foundry_render::{Calibration, RenderError};

#[test]
fn defaults_match_the_contract() {
    let c = Calibration::default();
    assert_eq!(c.units_per_metre, 70.0);
    assert_eq!(c.tile_metres, 2.0);
    assert_eq!(c.pixels_per_metre_vertical, 32.0);
    assert_eq!(c.yaw_quarter_turns, 0);
    assert_eq!(c.supersample, 4);
    assert_eq!(c.alpha_threshold, 0.5);
    assert_eq!(c.contrast, 1.0);
    assert_eq!(c.tile_units(), 140.0);
    assert!(c.validate().is_ok());
    assert_eq!(
        Calibration::from_toml("").expect("empty is all defaults"),
        c
    );
}

#[test]
fn toml_round_trip() {
    let c = Calibration {
        units_per_metre: 64.0,
        tile_metres: 2.5,
        pixels_per_metre_vertical: 30.0,
        yaw_quarter_turns: 3,
        supersample: 2,
        alpha_threshold: 0.375,
        contrast: 1.25,
        key_light_direction: [-2.0, -1.0, 3.0],
        key_light_intensity: 0.5,
        ambient: 0.25,
    };
    let text = c.to_toml().expect("serialise");
    assert_eq!(Calibration::from_toml(&text).expect("parse"), c);
    let partial = Calibration::from_toml("supersample = 8\ncontrast = 1.5\n").expect("parse");
    assert_eq!(partial.supersample, 8);
    assert_eq!(partial.contrast, 1.5);
    assert_eq!(partial.units_per_metre, 70.0);
}

#[test]
fn bad_values_are_rejected() {
    for (text, key) in [
        ("supersample = 0", "supersample"),
        ("supersample = 9", "supersample"),
        ("alpha_threshold = 0.0", "alpha_threshold"),
        ("alpha_threshold = 1.5", "alpha_threshold"),
        ("contrast = 0.1", "contrast"),
        ("contrast = nan", "contrast"),
        ("tile_metres = -2.0", "tile_metres"),
        ("units_per_metre = 0.0", "units_per_metre"),
        (
            "pixels_per_metre_vertical = inf",
            "pixels_per_metre_vertical",
        ),
        ("ambient = 1.5", "ambient"),
        ("key_light_intensity = -1.0", "key_light_intensity"),
        (
            "key_light_direction = [0.0, 0.0, 0.0]",
            "key_light_direction",
        ),
    ] {
        match Calibration::from_toml(text) {
            Err(RenderError::InvalidCalibration { key: k, .. }) => assert_eq!(k, key, "{text}"),
            other => panic!("{text}: {other:?}"),
        }
    }
    for text in [
        "unknown_key = 1",
        "supersample = \"four\"",
        "supersample = -1",
        "key_light_direction = [1.0, 2.0]",
        "not toml at all [",
    ] {
        assert!(
            matches!(Calibration::from_toml(text), Err(RenderError::Toml(_))),
            "{text}"
        );
    }
}
