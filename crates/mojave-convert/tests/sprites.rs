//! The M2 exit check: synthetic props go through render → quantize → emit,
//! and what comes out round-trips through `vault-formats`' own FRM parser.
//! A contact sheet is written from the result.

mod common;

use mojave_convert::{gallery, pipeline, sprites};
use quantize::PaletteMap;
use vault_formats::frm::FrmArchive;

#[test]
fn synthetic_props_become_frm_files_that_vault_formats_parses() {
    let fixture = common::fixture();
    let inputs = fixture.inputs("out", 2);
    pipeline::extract(&inputs).expect("extract");
    let summary = sprites::sprites(&inputs).expect("sprites");
    assert!(
        summary.meshes >= 4,
        "the town places several props: {summary:?}"
    );
    assert_eq!(summary.reused, 0);
    assert!(summary.rendered >= 4, "{summary:?}");

    let palette = inputs.palette().expect("palette");
    let map = PaletteMap::new(&palette);
    let index = sprites::read_index(&inputs).expect("index");
    assert_eq!(index.len(), summary.meshes);
    let mut checked = 0;
    for entry in index.iter().filter(|e| e.error.is_none()) {
        let (meta, bytes) = sprites::read_sprite(&inputs, &entry.key).expect("sprite");
        let archive = FrmArchive::parse(&bytes).expect("vault-formats parses the FRM");
        assert_eq!(archive.frame_count(), 1);
        let frame = archive.frame(0, 0).expect("frame 0");
        assert_eq!(u32::from(frame.width), meta.width);
        assert_eq!(u32::from(frame.height), meta.height);
        assert_eq!(archive.shifts[0], meta.shift);
        let pixels = frame.pixels();
        assert_eq!(
            pixels.len(),
            usize::from(frame.width) * usize::from(frame.height)
        );
        // Cropped to content: every edge row and column has an opaque pixel.
        let w = usize::from(frame.width);
        assert!(pixels[..w].iter().any(|p| *p != 0));
        assert!(pixels[pixels.len() - w..].iter().any(|p| *p != 0));
        // Only the transparent index and allowed palette indices appear.
        for pixel in pixels {
            assert!(
                *pixel == 0 || map.is_allowed(*pixel),
                "reserved index {pixel}"
            );
        }
        assert_eq!(
            pixels.iter().filter(|p| **p != 0).count() as u64,
            meta.opaque_pixels
        );
        checked += 1;
    }
    assert_eq!(checked, summary.rendered);
    // The fixture's building is several tiles wide; its sprite must be too.
    let widest = index
        .iter()
        .filter(|e| e.error.is_none())
        .map(|e| {
            sprites::read_sprite(&inputs, &e.key)
                .expect("sprite")
                .0
                .width
        })
        .max()
        .expect("at least one sprite");
    assert!(
        widest > 80,
        "the building is wider than one floor tile: {widest}"
    );
}

#[test]
fn a_second_run_reuses_every_sprite_and_job_count_changes_nothing() {
    let fixture = common::fixture();
    let one = fixture.inputs("out-1", 1);
    pipeline::extract(&one).expect("extract");
    let first = sprites::sprites(&one).expect("sprites");
    let before = common::tree(&one.sprites_dir());
    let again = sprites::sprites(&one).expect("sprites again");
    assert_eq!(again.rendered, 0);
    assert_eq!(again.reused, first.rendered);
    assert_eq!(again.failed, first.failed);
    assert_eq!(common::tree(&one.sprites_dir()), before);

    let four = fixture.inputs("out-4", 4);
    pipeline::extract(&four).expect("extract");
    sprites::sprites(&four).expect("sprites");
    assert_eq!(common::tree(&four.sprites_dir()), before);
}

#[test]
fn a_changed_option_is_a_different_cache_entry() {
    let fixture = common::fixture();
    let mut inputs = fixture.inputs("out", 2);
    pipeline::extract(&inputs).expect("extract");
    let plain = sprites::sprites(&inputs).expect("sprites");
    inputs.outline = true;
    let outlined = sprites::sprites(&inputs).expect("sprites with outline");
    assert_eq!(outlined.reused, 0);
    assert_eq!(outlined.rendered, plain.rendered);
}

#[test]
fn the_gallery_writes_a_contact_sheet() {
    let fixture = common::fixture();
    let inputs = fixture.inputs("out", 2);
    pipeline::extract(&inputs).expect("extract");
    let rendered = sprites::sprites(&inputs).expect("sprites").rendered;
    let summary = gallery::gallery(&inputs, "").expect("gallery");
    assert_eq!(summary.sprites, rendered);
    assert_eq!(summary.pages, 1);
    // The synthetic install has no original art to compare against.
    assert!(!summary.with_reference);
    let png = std::fs::read(inputs.gallery_dir().join("sprites-01.png")).expect("sheet");
    let image = foundry_gallery::decode_png(&png).expect("a valid PNG");
    assert!(image.width > 200 && image.height > 100);
    assert_eq!(
        image.rgba.len(),
        image.width as usize * image.height as usize * 4
    );
    // Deterministic bytes.
    gallery::gallery(&inputs, "").expect("gallery again");
    let again = std::fs::read(inputs.gallery_dir().join("sprites-01.png")).expect("sheet");
    assert_eq!(png, again);

    let filtered = gallery::gallery(&inputs, "no-such-model").expect("gallery");
    assert_eq!(filtered.sprites, 0);
    assert_eq!(filtered.pages, 0);
    assert!(!inputs.gallery_dir().join("sprites-01.png").exists());
}

#[test]
fn both_installs_are_required() {
    let fixture = common::fixture();
    let out = fixture.root.path().join("out");
    let missing = fixture.root.path().join("nowhere");
    let error = mojave_convert::Inputs::new(&fixture.fnv, &missing, &out, fixture.slice.clone(), 1)
        .expect_err("no Fallout 2 install");
    assert!(error.to_string().contains("--f2"), "{error}");
    let error = mojave_convert::Inputs::new(&missing, &fixture.f2, &out, fixture.slice.clone(), 1)
        .expect_err("no source install");
    assert!(error.to_string().starts_with("--game-dir "), "{error}");
    // A directory that exists but holds no palette is refused as well.
    let empty = fixture.root.path().join("empty");
    std::fs::create_dir_all(&empty).expect("dir");
    let error = mojave_convert::Inputs::new(&fixture.fnv, &empty, &out, fixture.slice.clone(), 1)
        .expect_err("no palette");
    assert!(error.to_string().contains("--f2"), "{error}");
}
