//! The whole path from a synthetic install: extract, open, build the ground.

use foundry_render::{Calibration, Projection};
use map_terrain::{SQUARES, TerrainOptions, build_ground};
use mir::{GroundSampler, MapWindow, Mir};
use quantize::PaletteMap;
use vault_formats::Palette;

#[test]
fn the_mini_town_ground_builds_from_a_mir_directory() {
    let root = tempfile::tempdir().expect("temp dir");
    let town = mir::fixture::mini_town(&root.path().join("source"));
    let out = root.path().join("mir");
    mir::extract(&town.options(&out, 2)).expect("extract");
    let mir = Mir::open(&out).expect("open");

    let calibration = Calibration::default();
    let projection = Projection::new(&calibration).expect("projection");
    // The town spans cells −1 to 1: centre the map on it.
    let window = MapWindow::from_center([2048.0, 2048.0], calibration.tile_units());
    let sampler = GroundSampler::load(&mir, mir::fixture::WORLDSPACE, &window).expect("sampler");
    let palette = Palette::parse(&quantize::fixture::synthetic_palette()).expect("palette");
    let map = PaletteMap::new(&palette);
    let options = TerrainOptions {
        library_cap: 300,
        jobs: 4,
        ..TerrainOptions::default()
    };
    let ground = build_ground(
        &mir,
        mir::fixture::WORLDSPACE,
        &window,
        &sampler,
        &projection,
        &calibration,
        &map,
        &[],
        &options,
    )
    .expect("ground");
    let s = &ground.stats;
    assert_eq!(ground.squares.len(), SQUARES);
    assert!(ground.tiles.len() <= 300 && !ground.tiles.is_empty());
    assert_eq!(s.cells_with_land, 9, "{s:?}");
    assert!(s.textures_loaded >= 1, "{s:?}");
    // The map is wider than the town's 3 × 3 cells: its rim has no land.
    assert!(s.fallback_pixels > 0);
    // Textures, relief and the fallback rim give more than a few colours.
    let (pixels, _, _) = ground.compose();
    let mut used = pixels.clone();
    used.sort_unstable();
    used.dedup();
    assert!(used.len() > 4, "{} indices", used.len());
}
