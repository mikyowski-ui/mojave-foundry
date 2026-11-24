//! How a standing figure is drawn on a written map: for given hexes (or
//! every hex no object blocks), the any-pixel `Sight`, the shares of the
//! figure box that art drawn after and before it covers
//! (`cover::Cover::shares`), and the placements that cover it in front.
//!
//! ```text
//! cargo run --release -p map-builder --example sight -- <layer dir> <map file> [hex,hex,...]
//! ```
//!
//! Placements are rebuilt from the map's objects and their FRMs (wall and
//! scenery art; `FLAT` scenery as floor pieces, which never cover), as
//! the `islands` example does. Counts and hex numbers only.

use std::collections::BTreeMap;
use std::path::Path;

use map_builder::cover::{Cover, SightRule};
use map_builder::islands;
use map_builder::walk::HEXES;
use map_builder::{ArtCategory, ArtImage, ENTRANCE_BOX, Placement, Role, read_layer};
use vault_formats::map::{MapFile, ObjectDetail, ObjectState};
use vault_formats::pro::BasicFlags;
use vault_formats::{
    ArtCatalog, ByteSource, Fid, FrmArchive, GameProfile, HexGrid, LocalPrototypeCatalog, MemoryDir,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: sight <layer> <map> [hex,hex,...]");
        std::process::exit(2);
    }
    let layer = read_layer(Path::new(&args[1])).expect("layer");
    let files = layer.files();
    let tree = MemoryDir::new(
        files
            .iter()
            .map(|(p, b)| (p.clone(), ByteSource::new(b.clone()).unwrap())),
    )
    .unwrap();
    let catalog = LocalPrototypeCatalog::for_game(&tree, GameProfile::Fallout2);
    let art_catalog = ArtCatalog::load(&tree).expect("art lists");
    let map_path = format!("maps/{}", args[2]);
    let map = MapFile::parse_for(&files[&map_path], &catalog, GameProfile::Fallout2).expect("map");
    let objects = &map.objects.elevations[0];
    // Art and placements back from the objects.
    let mut art: Vec<ArtImage> = Vec::new();
    let mut art_index: BTreeMap<u32, usize> = BTreeMap::new();
    let mut placements = Vec::new();
    for object in objects {
        let Some(hex) = object.hex else { continue };
        let fid = Fid(object.art_id);
        if matches!(
            object.state,
            ObjectState::Critter(_)
                | ObjectState::Other {
                    detail: ObjectDetail::ExitGrid { .. },
                    ..
                }
        ) {
            continue;
        }
        if !matches!(fid.category(), 2 | 3) {
            continue;
        }
        let index = *art_index.entry(object.art_id).or_insert_with(|| {
            let image = art_catalog
                .path(Fid(object.art_id))
                .ok()
                .and_then(|path| files.get(&path))
                .and_then(|bytes| FrmArchive::parse(bytes).ok())
                .and_then(|frm| {
                    let f = frm.frame(0, 0)?;
                    Some(ArtImage {
                        category: if fid.category() == 3 {
                            ArtCategory::Walls
                        } else {
                            ArtCategory::Scenery
                        },
                        width: u32::from(f.width),
                        height: u32::from(f.height),
                        pixels: f.pixels().to_vec(),
                        shift: frm.shifts[0],
                        source: String::new(),
                    })
                })
                .unwrap_or(ArtImage {
                    category: ArtCategory::Scenery,
                    width: 0,
                    height: 0,
                    pixels: Vec::new(),
                    shift: [0, 0],
                    source: String::new(),
                });
            art.push(image);
            art.len() - 1
        });
        let door = matches!(
            object.state,
            ObjectState::Other {
                detail: ObjectDetail::Door(_),
                ..
            }
        );
        let flat = object.flags.contains(BasicFlags::FLAT);
        let role = if fid.category() == 3 {
            Role::WallStrip
        } else if door {
            Role::Door
        } else if flat {
            Role::FloorPiece
        } else {
            Role::Scenery
        };
        let stops = islands::stops_movement(object.flags, object.art_id);
        placements.push(Placement {
            art: index,
            hex,
            displacement: object.displacement,
            role,
            reference: format!("object {}", object.id),
            blocking: stops,
        });
    }
    let cover = Cover::new(&art, &placements);
    let blocked = islands::engine_blocked(objects);
    let hexes: Vec<i32> = match args.get(3) {
        Some(list) if list.starts_with("mir=") => {
            // mir=<dir>,<interior editor id>,<floor z>,<centre x>,<centre y>:
            // the walkable hexes (the band's navmesh, nothing that stops
            // movement on them).
            let parts: Vec<&str> = list[4..].split(',').collect();
            let mir = mir::Mir::open(Path::new(parts[0])).expect("mir");
            let cell = mir.interior(parts[1]).expect("mir").expect("interior");
            let floor_z: f32 = parts[2].parse().unwrap();
            let centre = [parts[3].parse().unwrap(), parts[4].parse().unwrap()];
            let projection =
                foundry_render::Projection::new(&foundry_render::Calibration::default()).unwrap();
            let window = mir::MapWindow::from_center(centre, projection.tile_units());
            let frame = map_builder::walk::Frame::new(&window, &projection).unwrap();
            let options = map_builder::BuildOptions {
                interior: Some(map_builder::InteriorBuild::new(floor_z)),
                ..map_builder::BuildOptions::default()
            };
            let (verdicts, _) = map_builder::walk::verdicts(
                &frame,
                &mir::GroundSampler::default(),
                &[cell],
                &blocked,
                &options,
            );
            (0..HEXES as i32)
                .filter(|h| verdicts[*h as usize] == map_builder::walk::Verdict::Walkable)
                .collect()
        }
        Some(list) => list.split(',').map(|h| h.parse().unwrap()).collect(),
        None => (0..HEXES as i32)
            .filter(|h| {
                !blocked[*h as usize] && placements.iter().any(|p| (p.hex - h).abs() < 2000)
            })
            .collect(),
    };
    let figure = ENTRANCE_BOX;
    let mut tally: BTreeMap<String, usize> = BTreeMap::new();
    let mut histogram = [0usize; 11];
    for hex in &hexes {
        let any = cover.sight(*hex, figure);
        let shares = cover.shares(*hex, figure).unwrap_or_default();
        let visible = cover.sight_by(*hex, figure, SightRule::VISIBLE);
        *tally.entry(format!("{any:?} -> {visible:?}")).or_default() += 1;
        histogram[((shares.front * 10.0).floor() as usize).min(10)] += 1;
        if args.get(3).is_none_or(|a| a.starts_with("mir=")) {
            continue;
        }
        let centre = HexGrid::default().hex_center(*hex).unwrap_or([0, 0]);
        println!(
            "hex {hex} (screen {:?}): any pixel {any:?}; front {:.0} %, behind {:.0} %; by share \
             {visible:?}",
            centre,
            100.0 * shares.front,
            100.0 * shares.behind
        );
        // The placements in front that cover it, each alone.
        let [cx, cy] = HexGrid::default().hex_center(*hex).unwrap();
        let (w, h) = (i64::from(figure[0]), i64::from(figure[1]));
        let (x0, y0) = (cx - w / 2, cy - h + 1);
        for p in &placements {
            if p.hex < *hex || matches!(p.role, Role::FloorPiece | Role::Item) {
                continue;
            }
            let image = &art[p.art];
            if image.width == 0 {
                continue;
            }
            let Ok(c) = HexGrid::default().hex_center(p.hex) else {
                continue;
            };
            let left = c[0] + i64::from(image.shift[0]) + i64::from(p.displacement[0])
                - i64::from(image.width / 2);
            let top = c[1] + i64::from(image.shift[1]) + i64::from(p.displacement[1])
                - i64::from(image.height)
                + 1;
            let mut count = 0;
            let mut rows = [i64::MAX, i64::MIN];
            for y in 0..h {
                for x in 0..w {
                    let (sx, sy) = (x0 + x - left, y0 + y - top);
                    if sx >= 0
                        && sy >= 0
                        && sx < i64::from(image.width)
                        && sy < i64::from(image.height)
                        && image.pixels[(sy * i64::from(image.width) + sx) as usize] != 0
                    {
                        count += 1;
                        rows[0] = rows[0].min(y);
                        rows[1] = rows[1].max(y);
                    }
                }
            }
            if count > 0 {
                println!(
                    "  {:?} on hex {} ({} rows south, {} columns over), {}x{} art: {count} box \
                     pixels ({:.0} %), box rows {} to {} of {h}",
                    p.role,
                    p.hex,
                    p.hex / 200 - hex / 200,
                    p.hex % 200 - hex % 200,
                    image.width,
                    image.height,
                    100.0 * count as f64 / (w * h) as f64,
                    rows[0],
                    rows[1]
                );
            }
        }
    }
    println!("{} hexes: {tally:?}", hexes.len());
    println!("front share by tenths: {histogram:?}");
}
