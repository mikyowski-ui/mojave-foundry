//! Pins the `vault-formats` surface this pipeline builds on, at the revision
//! in the root `Cargo.toml`. If the pin moves and one of these stops
//! compiling or changes value, the contracts in `docs/contracts.md` need a
//! second look before anything else.

use std::path::Path;

use vault_formats::ParseError;
use vault_formats::fid::Fid;
use vault_formats::frm::FrmArchive;
use vault_formats::gam::{GamFile, GamSection};
use vault_formats::geometry::HexGrid;
use vault_formats::lst::{LstKind, LstTable};
use vault_formats::map::MapFile;
use vault_formats::mod_manifest::{ModManifest, write_mod_folder};
use vault_formats::msg::MsgCatalog;
use vault_formats::palette::{CYCLE_RANGES, Palette, TRANSPARENT_INDEX};
use vault_formats::pro::Prototype;

type WriteModFolder = fn(&Path, &ModManifest, &[(String, Vec<u8>)]) -> std::io::Result<()>;

#[test]
fn writers_this_pipeline_wraps_exist() {
    let _: fn(&Prototype) -> Vec<u8> = Prototype::write;
    let _: fn(&MapFile) -> Result<Vec<u8>, ParseError> = MapFile::write;
    let _: WriteModFolder = write_mod_folder;
}

#[test]
fn parsers_used_for_round_trips_exist() {
    let _: fn(&[u8]) -> Result<FrmArchive, ParseError> = FrmArchive::parse;
    let _: fn(&[u8]) -> Result<Palette, ParseError> = Palette::parse;
    let _: fn(&[u8]) -> Result<Prototype, ParseError> = Prototype::parse;
    let _: fn(&[u8], LstKind) -> Result<LstTable, ParseError> = LstTable::parse;
    let _: fn(&[u8]) -> Result<MsgCatalog, ParseError> = MsgCatalog::parse_strict;
    let _: fn(&[u8], GamSection) -> Result<GamFile, ParseError> = GamFile::parse;
}

#[test]
fn palette_reservations_are_exported() {
    assert_eq!(TRANSPARENT_INDEX, 0);
    assert!(!CYCLE_RANGES.is_empty());
    for range in CYCLE_RANGES {
        assert!(range.first <= range.last);
    }
}

#[test]
fn fid_packs_its_five_fields() {
    let fid = Fid::new(2, 123, 0, 0, 0);
    assert_eq!(fid.category(), 2);
    assert_eq!(fid.base(), 123);
}

/// The three screen vectors of `docs/contracts.md` section 4.
#[test]
fn projection_vectors_match_the_contract() {
    let grid = HexGrid::default();
    let corner = |square: i32, roof: bool| grid.square_corner(square, roof).expect("in range");
    let sub = |a: [i64; 2], b: [i64; 2]| [a[0] - b[0], a[1] - b[1]];
    // s = 99 - square % 100, so one step in +s is one square index lower.
    let square = 5050;
    assert_eq!(
        sub(corner(square - 1, false), corner(square, false)),
        [48, -12]
    );
    assert_eq!(
        sub(corner(square + 100, false), corner(square, false)),
        [32, 24]
    );
    assert_eq!(sub(corner(square, true), corner(square, false)), [0, -96]);
    // The square with s = 0, t = 0 sits at the documented offset.
    assert_eq!(corner(99, false), [-16, -2]);
}

/// A hex reference is the projection of `(h/2, r/2 + (h mod 2)/4, 0)`.
#[test]
fn hex_references_lie_on_the_tile_plane() {
    let grid = HexGrid::default();
    for hex in [0, 199, 200, 20100, 39999, 12345] {
        let [column, row] = HexGrid::coordinates(hex).expect("in range");
        let h = i64::from(199 - column);
        let r = i64::from(row);
        // Quarter-tile units keep the arithmetic exact.
        let s4 = 2 * h;
        let t4 = 2 * r + h % 2;
        let expected = [(48 * s4 + 32 * t4) / 4, (-12 * s4 + 24 * t4) / 4];
        assert_eq!(grid.hex_reference(hex).expect("in range"), expected);
    }
}
