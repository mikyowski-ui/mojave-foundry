//! The objects of a written map whose first frame covers a map screen
//! point: art path, size, prototype and the most common palette indices
//! of the frame. Prints numbers and file names only.
//!
//! ```text
//! cargo run --release -p map-builder --example art_at -- <layer dir> <map file> <elevation> <x> <y>
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use map_builder::read_layer;
use vault_formats::map::MapFile;
use vault_formats::{
    ArtCatalog, ByteSource, Fid, FrmArchive, GameProfile, HexGrid, LocalPrototypeCatalog, MemoryDir,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let layer = read_layer(Path::new(&args[1])).expect("layer");
    let files = layer.files();
    let tree = MemoryDir::new(
        files
            .iter()
            .map(|(p, b)| (p.clone(), ByteSource::new(b.clone()).unwrap())),
    )
    .unwrap();
    let catalog = LocalPrototypeCatalog::for_game(&tree, GameProfile::Fallout2);
    let art = ArtCatalog::load(&tree).expect("art lists");
    let map = MapFile::parse_for(
        &files[&format!("maps/{}", args[2])],
        &catalog,
        GameProfile::Fallout2,
    )
    .expect("map");
    let e: usize = args[3].parse().unwrap();
    let point: [i64; 2] = [args[4].parse().unwrap(), args[5].parse().unwrap()];
    let grid = HexGrid::default();
    for object in &map.objects.elevations[e] {
        let Some(hex) = object.hex else { continue };
        let Ok(centre) = grid.hex_center(hex) else {
            continue;
        };
        let Ok(path) = art.path(Fid(object.art_id)) else {
            continue;
        };
        let Some(frm) = files.get(&path).and_then(|b| FrmArchive::parse(b).ok()) else {
            continue;
        };
        let Some(f) = frm.frame(0, 0) else { continue };
        let rect = f.world_rect(
            [centre[0] as i32, centre[1] as i32],
            frm.shifts[0],
            object.displacement,
        );
        if point[0] < rect.left
            || point[0] >= rect.right
            || point[1] < rect.top
            || point[1] >= rect.bottom
        {
            continue;
        }
        let x = (point[0] - rect.left) as usize;
        let y = (point[1] - rect.top) as usize;
        let here = f.pixels()[y * usize::from(f.width) + x];
        let mut h: BTreeMap<u8, usize> = BTreeMap::new();
        for p in f.pixels() {
            if *p != 0 {
                *h.entry(*p).or_default() += 1;
            }
        }
        let mut v: Vec<(usize, u8)> = h.into_iter().map(|(k, n)| (n, k)).collect();
        v.sort_by(|a, b| b.cmp(a));
        v.truncate(5);
        println!(
            "hex {hex} pid {:#x} {path} {}x{} pixel here {here} top {v:?}",
            object.pid, f.width, f.height
        );
    }
}
