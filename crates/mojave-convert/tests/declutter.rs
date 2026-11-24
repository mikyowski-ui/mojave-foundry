//! The declutter wiring on the synthetic town: off by default (the layer
//! is what it was), `readable` from the slice or the command line turns
//! the slicer's pass on and the report says so, and tables without the
//! pass are refused.

mod common;

use mojave_convert::declutter::DeclutterSpec;
use mojave_convert::{map, pipeline};

#[test]
fn readable_runs_and_off_keeps_the_layer() {
    let fixture = common::fixture();
    let off = fixture.inputs("out-off", 2);
    pipeline::all(&off).expect("off");
    let report = std::fs::read_to_string(off.out.join("map-report.md")).expect("report");
    assert!(report.contains("## Declutter"), "{report}");
    assert!(report.contains("the pass is off"), "{report}");
    assert!(report.contains("## Readability"), "{report}");

    // A slice saying `profile = "off"` builds the same layer.
    let mut said_off = fixture.inputs("out-said-off", 2);
    said_off.slice.declutter = Some(DeclutterSpec {
        profile: Some("off".to_owned()),
        ..DeclutterSpec::default()
    });
    pipeline::all(&said_off).expect("said off");
    assert_eq!(
        common::tree(&off.mod_dir()),
        common::tree(&said_off.mod_dir())
    );

    // --declutter readable: the pass runs and is reported.
    let mut readable = fixture.inputs("out-readable", 2);
    readable.declutter = Some("readable".to_owned());
    assert_eq!(readable.declutter_profile(), "readable");
    pipeline::all(&readable).expect("readable");
    // A second run with the same profile reuses the layer.
    assert!(map::map(&readable).expect("again").reused);
    let report = std::fs::read_to_string(readable.out.join("map-report.md")).expect("report");
    assert!(
        report.contains("- profile: `readable`, from --declutter"),
        "{report}"
    );
    assert!(report.contains("protected by reason"), "{report}");

    // Keys with the pass off are an error naming the way out.
    let mut stray = fixture.inputs("out-stray", 2);
    stray.slice.declutter = Some(DeclutterSpec {
        kerb_height: Some(20.0),
        ..DeclutterSpec::default()
    });
    let error = pipeline::all(&stray).expect_err("keys without the pass");
    assert!(
        error.to_string().contains("--declutter readable"),
        "{error}"
    );
}

#[test]
fn a_slice_file_carries_its_tables() {
    let text = r#"
name = "room"
worldspace = "W"
plugins = ["T.esm"]
x = [0, 0]
y = [0, 0]

[declutter]
profile = "readable"
protect_items = false
kerb_height = 30.0

[tone.floor]
gain = 0.8
"#;
    let spec = mojave_convert::SliceSpec::from_toml(text).expect("parses");
    let declutter = spec.declutter.as_ref().expect("a table");
    assert_eq!(declutter.profile.as_deref(), Some("readable"));
    assert_eq!(declutter.protect_items, Some(false));
    assert_eq!(
        spec.tone.and_then(|t| t.floor).and_then(|f| f.gain),
        Some(0.8)
    );
    assert_eq!(
        mojave_convert::SliceSpec::from_toml(&spec.to_toml()),
        Ok(spec.clone())
    );
    let bad = text.replace("kerb_height", "kerb_hieght");
    assert!(mojave_convert::SliceSpec::from_toml(&bad).is_err());
    let off = text.replace("\"readable\"", "\"off\"");
    let error = mojave_convert::SliceSpec::from_toml(&off).expect_err("off with keys");
    assert!(error.contains("would do nothing"), "{error}");
}
