//! Which floor and roof tiles a written map draws around a hex, and what
//! their pixels hold (palette index histogram).
//!
//! ```text
//! cargo run --release -p map-builder --example tile_at -- <layer dir> <map file> <elevation> <hex,hex,...>
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use map_builder::read_layer;
use map_builder::walk::Frame;
use vault_formats::map::MapFile;
use vault_formats::{ByteSource, FrmArchive, GameProfile, LocalPrototypeCatalog, MemoryDir};

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
    let map = MapFile::parse_for(
        &files[&format!("maps/{}", args[2])],
        &catalog,
        GameProfile::Fallout2,
    )
    .expect("map");
    let e: usize = args[3].parse().unwrap();
    let list = String::from_utf8_lossy(&files["art/tiles/tiles.lst"]).to_string();
    let names: Vec<&str> = list.lines().collect();
    let tiles = map.tiles[e].as_ref().expect("elevation");
    for hex in args[4].split(',').map(|s| s.parse::<i32>().unwrap()) {
        let square = Frame::square_of_hex(hex).unwrap();
        let pair = tiles[square as usize];
        let floor = pair.floor();
        let name = names.get(usize::from(floor)).copied().unwrap_or("?");
        let path = format!("art/tiles/{}", name.trim().to_ascii_lowercase());
        let hist = files.get(&path).map(|bytes| {
            let frm = FrmArchive::parse(bytes).expect("frm");
            let f = frm.frame(0, 0).unwrap();
            let mut h: BTreeMap<u8, usize> = BTreeMap::new();
            for p in f.pixels() {
                *h.entry(*p).or_default() += 1;
            }
            let mut v: Vec<(usize, u8)> = h.into_iter().map(|(k, n)| (n, k)).collect();
            v.sort_by(|a, b| b.cmp(a));
            v.truncate(6);
            v
        });
        println!(
            "hex {hex} square {square}: floor {floor} ({name}), roof {}, hist {hist:?}",
            pair.roof()
        );
    }
}
