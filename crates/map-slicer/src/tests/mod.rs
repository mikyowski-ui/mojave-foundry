//! Synthetic tests: `nif::fixture` meshes, hand-built references and
//! terrain, the synthetic palette, the CPU renderer.

mod classes;
mod overlays;
mod slicing;
mod town;

use std::borrow::Cow;
use std::collections::BTreeMap;

use foundry_render::{Calibration, CpuRenderer, Projection, Texture, TextureSource};
use mir::{
    FormKey, GroundSampler, LAND_GRID, LandDoc, MapWindow, RefFlags, RefGroup, RefKind, Reference,
    Transform,
};
use nif::StaticMesh;
use quantize::PaletteMap;

use crate::render::Assets;
use crate::{ArtImage, Budget, Placement, SliceError, Sliced, SlicerConfig, slice_with};

pub(crate) const WALL: &str = "textures\\test\\wall.dds";
pub(crate) const ROOF: &str = "textures\\test\\roof.dds";

pub(crate) fn mesh(bytes: &[u8]) -> StaticMesh {
    nif::extract_mesh(&nif::parse(bytes).expect("fixture parses")).expect("fixture flattens")
}

/// A 16 × 16 checker in a hue picked from the path.
struct Patterns;

impl TextureSource for Patterns {
    fn texture(&self, path: &str) -> Option<Cow<'_, Texture>> {
        let seed = path
            .bytes()
            .fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
        let base = [
            (60 + seed % 150) as u8,
            (60 + (seed / 7) % 150) as u8,
            (60 + (seed / 49) % 150) as u8,
        ];
        let mut rgba = Vec::with_capacity(16 * 16 * 4);
        for y in 0..16u32 {
            for x in 0..16u32 {
                let k = if (x / 2 + y / 3) % 2 == 0 {
                    255u32
                } else {
                    150
                };
                rgba.extend(base.iter().map(|c| (u32::from(*c) * k / 255) as u8));
                rgba.push(255);
            }
        }
        Some(Cow::Owned(Texture {
            width: 16,
            height: 16,
            rgba,
        }))
    }
}

/// Meshes by hash, patterned textures.
pub(crate) struct TestAssets {
    meshes: BTreeMap<String, StaticMesh>,
    patterns: Patterns,
}

impl TestAssets {
    pub fn new() -> Self {
        TestAssets {
            meshes: BTreeMap::new(),
            patterns: Patterns,
        }
    }

    pub fn with(mut self, hash: &str, mesh: StaticMesh) -> Self {
        self.meshes.insert(hash.to_owned(), mesh);
        self
    }

    /// The standard set: a building, a small box, a large box, a quad and
    /// a tall thin box.
    pub fn standard() -> Self {
        Self::new()
            .with(
                "house",
                mesh(&nif::fixture::building([840.0, 560.0, 280.0], WALL, ROOF)),
            )
            .with(
                "crate",
                mesh(&nif::fixture::box_mesh([50.0, 50.0, 60.0], WALL)),
            )
            .with(
                "slab",
                mesh(&nif::fixture::box_mesh([700.0, 700.0, 200.0], WALL)),
            )
            .with("road", mesh(&nif::fixture::quad([560.0, 280.0], ROOF)))
            .with(
                "fence",
                mesh(&nif::fixture::box_mesh([700.0, 10.0, 120.0], WALL)),
            )
            .with(
                "huge",
                mesh(&nif::fixture::box_mesh([5000.0, 5000.0, 400.0], WALL)),
            )
    }
}

impl Assets for TestAssets {
    fn mesh(&self, hash: &str) -> Result<StaticMesh, String> {
        self.meshes
            .get(hash)
            .cloned()
            .ok_or_else(|| format!("no mesh {hash}"))
    }

    fn textures(&self) -> &dyn TextureSource {
        &self.patterns
    }
}

pub(crate) fn window() -> MapWindow {
    MapWindow::from_center([0.0, 0.0], 140.0)
}

/// Terrain over the window from a height function.
pub(crate) fn sampler(height: impl Fn(f32, f32) -> f32) -> GroundSampler {
    let mut sampler = GroundSampler::default();
    for cy in -3..=2 {
        for cx in -3..=2 {
            let mut heights = Vec::with_capacity(LAND_GRID * LAND_GRID);
            for row in 0..LAND_GRID {
                for column in 0..LAND_GRID {
                    let x = cx as f32 * 4096.0 + column as f32 * 128.0;
                    let y = cy as f32 * 4096.0 + row as f32 * 128.0;
                    heights.push(height(x, y));
                }
            }
            sampler.insert(
                cx,
                cy,
                &LandDoc {
                    form: FormKey::new("test.esm", 1),
                    heights,
                    normals: None,
                    colors: None,
                    layers: Vec::new(),
                },
            );
        }
    }
    sampler
}

pub(crate) fn flat() -> GroundSampler {
    sampler(|_, _| 100.0)
}

pub(crate) fn sloped() -> GroundSampler {
    sampler(|x, y| 100.0 + 0.05 * x + 0.03 * y)
}

pub(crate) fn calibration() -> Calibration {
    Calibration::default()
}

pub(crate) fn projection() -> Projection {
    Projection::new(&calibration()).expect("projection")
}

pub(crate) fn renderer() -> CpuRenderer {
    CpuRenderer::new(&calibration()).expect("renderer")
}

pub(crate) fn palette_map() -> PaletteMap {
    let palette = vault_formats::Palette::parse(&quantize::fixture::synthetic_palette())
        .expect("synthetic palette");
    PaletteMap::new(&palette)
}

pub(crate) fn big_budget() -> Budget {
    Budget {
        walls: 4000,
        scenery: 4000,
        items: 4000,
    }
}

/// A reference to mesh `hash` of base kind `kind`, at `(x, y)` on the
/// terrain of `ground` (plus `lift`), turned `yaw` radians.
#[allow(clippy::too_many_arguments)]
pub(crate) fn reference(
    id: u32,
    kind: &str,
    model: &str,
    hash: &str,
    ground: &GroundSampler,
    xy: [f32; 2],
    lift: f32,
    yaw: f32,
) -> Reference {
    let z = ground.height(xy[0], xy[1]).unwrap_or(0.0) + lift;
    Reference {
        form: FormKey::new("test.esm", id),
        editor_id: None,
        kind: RefKind::Object,
        group: RefGroup::Temporary,
        base: FormKey::new("test.esm", 0x100 + id),
        base_kind: Some(kind.to_owned()),
        base_editor_id: None,
        model: Some(model.to_owned()),
        mesh: Some(hash.to_owned()),
        transform: Transform {
            position: [xy[0], xy[1], z],
            rotation: [0.0, 0.0, yaw],
            scale: 1.0,
        },
        flags: RefFlags::default(),
        teleport: None,
        enable_parent: None,
        lock: None,
        count: None,
        linked_ref: None,
        owner: None,
    }
}

pub(crate) fn run(
    assets: &TestAssets,
    refs: &[Reference],
    ground: &GroundSampler,
    budget: &Budget,
    config: &SlicerConfig,
) -> Result<Sliced, SliceError> {
    slice_with(
        assets,
        refs,
        &window(),
        ground,
        &projection(),
        &mut renderer(),
        &palette_map(),
        budget,
        config,
    )
}

/// A canvas of map screen space, indexed.
pub(crate) struct Screen {
    pub origin: [i64; 2],
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

impl Screen {
    pub fn new(origin: [i64; 2], width: usize, height: usize) -> Self {
        Screen {
            origin,
            width,
            height,
            pixels: vec![0; width * height],
        }
    }

    pub fn put(&mut self, x: i64, y: i64, value: u8) {
        let (i, j) = (x - self.origin[0], y - self.origin[1]);
        assert!(
            i >= 0 && j >= 0 && (i as usize) < self.width && (j as usize) < self.height,
            "pixel ({x}, {y}) outside the test screen"
        );
        self.pixels[j as usize * self.width + i as usize] = value;
    }

    /// Draws placements by the engine's rule, in their order: the frame's
    /// bottom-centre at hex centre + shift + displacement.
    pub fn draw(&mut self, art: &[ArtImage], placements: &[&Placement]) {
        for p in placements {
            let a = &art[p.art];
            let [left, top] = frame_origin(a, p);
            for y in 0..a.height as usize {
                for x in 0..a.width as usize {
                    let v = a.pixels[y * a.width as usize + x];
                    if v != 0 {
                        self.put(left + x as i64, top + y as i64, v);
                    }
                }
            }
        }
    }
}

/// Map screen position of a placed frame's pixel (0, 0), by the engine's
/// placement rule.
pub(crate) fn frame_origin(a: &ArtImage, p: &Placement) -> [i64; 2] {
    let c = vault_formats::geometry::HexGrid::default()
        .hex_center(p.hex)
        .expect("hex");
    [
        c[0] + i64::from(a.shift[0]) + i64::from(p.displacement[0]) - i64::from(a.width / 2),
        c[1] + i64::from(a.shift[1]) + i64::from(p.displacement[1]) - (i64::from(a.height) - 1),
    ]
}
