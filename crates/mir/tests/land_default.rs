//! Land layers that give no usable texture are drawn with the engine's
//! default land texture: a synthetic install whose one cell has every
//! such case, and a settings file naming the default.

use std::fs;
use std::path::Path;

use fnv_bsa::fixture::BsaBuilder;
use fnv_esm::fixture::{CellSpec, PluginBuilder, WorldSpec, flat_heights, up_normals};
use fnv_esm::{
    AlphaLayer, AlphaPoint, BaseLayer, Cell, FormId, Land, LandTexture, Static, TextureSet, World,
};
use mir::{
    DEFAULT_LAND_TEXTURE, ExtractOptions, LandDefault, LandLayer, Mir, RawStore, Slice, extract,
};

const PLUGIN: &str = "LandDefault.esm";
const WORLDSPACE: &str = "LDWorld";
const WORLD: u32 = 0x800;
const CELL: u32 = 0x801;
const LAND: u32 = 0x802;
const TXST_SAND: u32 = 0x810;
const TXST_EMPTY: u32 = 0x811;
const LTEX_SAND: u32 = 0x820;
const LTEX_NO_SET: u32 = 0x821;
const LTEX_EMPTY_SET: u32 = 0x822;
const NOT_LTEX: u32 = 0x830;
/// A form id no record of the plugin has.
const DANGLING: u32 = 0x8ff;
/// What the synthetic settings file names.
const SETTINGS_DEFAULT: &str = "textures\\landscape\\ldfallback.dds";

fn bc1(rgb: [u8; 3]) -> Vec<u8> {
    let rgba: Vec<u8> = std::iter::repeat_n([rgb[0], rgb[1], rgb[2], 255], 16 * 16)
        .flatten()
        .collect();
    dds::fixture::encode_bc1(16, 16, &rgba)
}

fn alpha(texture: u32, quadrant: u8, layer: u16) -> AlphaLayer {
    AlphaLayer {
        texture: FormId(texture),
        quadrant,
        layer,
        opacity: vec![AlphaPoint {
            position: 144,
            opacity: 1.0,
        }],
    }
}

/// Writes the install under `root` (`root/Data`); `settings` is the text
/// of `Fallout_default.ini`, or no file.
fn install(root: &Path, settings: Option<&str>) -> ExtractOptions {
    let data = root.join("Data");
    fs::create_dir_all(&data).unwrap();
    let mut plugin = PluginBuilder::new();
    plugin.master_file(true);
    plugin.add(TextureSet::new(
        TXST_SAND,
        "LDSandSet",
        "landscape\\ldsand.dds",
    ));
    plugin.add(TextureSet {
        common: fnv_esm::Common::new(TXST_EMPTY).with_editor_id("LDEmptySet"),
        ..TextureSet::default()
    });
    plugin.add(LandTexture::new(LTEX_SAND, "LDSand", TXST_SAND));
    plugin.add(LandTexture {
        common: fnv_esm::Common::new(LTEX_NO_SET).with_editor_id("LDNoSet"),
        ..LandTexture::default()
    });
    plugin.add(LandTexture::new(LTEX_EMPTY_SET, "LDEmpty", TXST_EMPTY));
    plugin.add(Static::new(NOT_LTEX, "LDRock", "ld\\rock.nif"));

    let mut land = Land::new(LAND).with_heights(&flat_heights(0.0));
    land.normals = Some(up_normals());
    // Quadrants 0 and 1 have a base; 0's is null. 2 and 3 store none.
    land.base_layers = vec![
        BaseLayer {
            texture: FormId::NULL,
            quadrant: 0,
            layer: -1,
        },
        BaseLayer {
            texture: FormId(LTEX_SAND),
            quadrant: 1,
            layer: -1,
        },
    ];
    land.alpha_layers = vec![
        alpha(LTEX_SAND, 0, 0),
        alpha(0, 1, 0),
        alpha(DANGLING, 1, 1),
        alpha(NOT_LTEX, 2, 0),
        alpha(LTEX_NO_SET, 3, 0),
        alpha(LTEX_EMPTY_SET, 3, 1),
    ];
    let world = WorldSpec::new(World::new(WORLD, WORLDSPACE))
        .with_cell(CellSpec::new(Cell::exterior(CELL, 0, 0)).with_land(land));
    plugin.world(world);
    fs::write(data.join(PLUGIN), plugin.build()).unwrap();

    let mut textures = BsaBuilder::new();
    textures.add("textures\\landscape\\ldsand.dds", &bc1([200, 180, 120]));
    textures.add(SETTINGS_DEFAULT, &bc1([90, 70, 50]));
    fs::write(data.join("LandDefault - Textures.bsa"), textures.build()).unwrap();
    if let Some(text) = settings {
        fs::write(root.join("Fallout_default.ini"), text).unwrap();
    }
    ExtractOptions {
        data_dir: data,
        plugins: vec![PLUGIN.to_owned()],
        out: root.join("mir"),
        slice: Slice {
            name: "land default".to_owned(),
            worldspace: WORLDSPACE.to_owned(),
            x: 0..=0,
            y: 0..=0,
            interior_depth: 0,
        },
        jobs: 2,
        raw_store: RawStore::All,
    }
}

fn layers(mir: &Mir) -> Vec<LandLayer> {
    mir.cell(&WORLDSPACE.to_ascii_lowercase(), 0, 0)
        .unwrap()
        .expect("the cell")
        .land
        .expect("its land")
        .layers
}

#[test]
fn every_untextured_layer_draws_the_default_named_by_the_settings() {
    let root = tempfile::tempdir().unwrap();
    let options = install(
        root.path(),
        Some("[Landscape]\nsDefaultLandDiffuseTexture=LDFallback.dds\n"),
    );
    let summary = extract(&options).unwrap();
    let mir = Mir::open(&options.out).unwrap();
    assert_eq!(
        mir.manifest().default_land_texture.as_deref(),
        Some(SETTINGS_DEFAULT)
    );
    let sand = mir
        .manifest()
        .texture_hash("textures\\landscape\\ldsand.dds")
        .expect("sand converted")
        .to_owned();
    let fallback = mir
        .manifest()
        .texture_hash(SETTINGS_DEFAULT)
        .expect("the default texture is extracted")
        .to_owned();
    let got: Vec<(u8, i32, Option<LandDefault>, Option<String>)> = layers(&mir)
        .into_iter()
        .map(|l| (l.quadrant, l.layer, l.defaulted, l.texture_hash))
        .collect();
    let s = Some(sand);
    let d = Some(fallback);
    use LandDefault::*;
    assert_eq!(
        got,
        vec![
            (0, -1, Some(NullTexture), d.clone()),
            (0, 0, None, s.clone()),
            (1, -1, None, s.clone()),
            (1, 0, Some(NullTexture), d.clone()),
            (1, 1, Some(UnknownTexture), d.clone()),
            (2, -1, Some(NoBaseLayer), d.clone()),
            (2, 0, Some(UnknownTexture), d.clone()),
            (3, -1, Some(NoBaseLayer), d.clone()),
            (3, 0, Some(NoTextureSet), d.clone()),
            (3, 1, Some(NoTextureSet), d),
        ]
    );
    // A dangling form or a form of another kind (one issue: they read the
    // same for one record), and each land texture with no diffuse texture,
    // are reported; null forms and absent base layers are ordinary
    // authoring and are not.
    let issues = &mir.manifest().issues;
    assert_eq!(issues.len(), 3, "{issues:?}");
    assert_eq!(summary.issues, 3);
    let layers = layers(&mir);
    // The land texture of a defaulted layer is kept when it resolves.
    assert_eq!(layers[8].land_texture_editor_id.as_deref(), Some("LDNoSet"));
    assert_eq!(layers[5].land_texture, None);
    let report = fs::read_to_string(options.out.join("report.md")).unwrap();
    assert!(
        report.contains(
            "- land layers drawn with the default land texture \
             `textures\\landscape\\ldfallback.dds`: 8 of 10 (2 quadrants without a base \
             layer, 2 null land textures, 2 forms that are not land textures, 2 land textures \
             without a diffuse texture)\n"
        ),
        "{report}"
    );
    // A second run writes the same bytes.
    let cell = options.out.join("cells/ldworld/0_0.json");
    let first = fs::read(&cell).unwrap();
    extract(&options).unwrap();
    assert_eq!(fs::read(&cell).unwrap(), first);
    // The field is written only where it is set.
    let text = String::from_utf8(first).unwrap();
    assert_eq!(text.matches("\"defaulted\"").count(), 8);
    assert!(text.contains("\"defaulted\":\"no_base_layer\""));
}

#[test]
fn without_settings_the_stock_default_is_named_and_missing_is_honest() {
    let root = tempfile::tempdir().unwrap();
    let options = install(root.path(), None);
    extract(&options).unwrap();
    let mir = Mir::open(&options.out).unwrap();
    assert_eq!(
        mir.manifest().default_land_texture.as_deref(),
        Some(DEFAULT_LAND_TEXTURE)
    );
    // The synthetic archive does not hold the stock default: the layers
    // say so, defaulted and with no hash, and the manifest lists it as
    // missing.
    for layer in layers(&mir).iter().filter(|l| l.is_defaulted()) {
        assert_eq!(layer.texture.as_deref(), Some(DEFAULT_LAND_TEXTURE));
        assert_eq!(layer.texture_hash, None);
    }
    let entry = &mir.manifest().textures[DEFAULT_LAND_TEXTURE];
    assert_eq!(entry.status, mir::AssetStatus::Missing);
    let report = fs::read_to_string(options.out.join("report.md")).unwrap();
    assert!(
        report.contains("; the texture is missing, so 8 of them have none\n"),
        "{report}"
    );
}

#[test]
fn documents_written_before_the_field_still_read() {
    let old =
        r#"{"quadrant":2,"layer":-1,"land_texture":"a.esm:000001","texture":"textures\\x.dds"}"#;
    let layer: LandLayer = serde_json::from_str(old).unwrap();
    assert_eq!(layer.defaulted, None);
    assert!(!layer.is_defaulted());
    let back = serde_json::to_string(&layer).unwrap();
    assert_eq!(back, old);
}
