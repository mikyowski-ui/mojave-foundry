//! Art lists and the 4,096-entry FID budget.

use emit::{ArtList, EmitError, FID_INDEX_CAP};
use vault_formats::{ArtCatalog, Fid, LstKind, LstTable};

const SCENERY: u8 = 2;

#[test]
fn pushes_append_and_deduplicate_without_case() {
    let mut list = ArtList::new(SCENERY).unwrap();
    assert_eq!(list.list_path(), "art/scenery/scenery.lst");
    assert_eq!(list.push("rock01.frm").unwrap(), 0);
    assert_eq!(list.push("rock02.frm").unwrap(), 1);
    assert_eq!(list.push("ROCK01.FRM").unwrap(), 0);
    assert_eq!(list.len(), 2);
    assert_eq!(list.remaining(), FID_INDEX_CAP - 2);
    let table = LstTable::parse(&list.to_bytes(), LstKind::Art).unwrap();
    assert_eq!(table.entries().len(), 2);
    let fid = list.fid(1);
    assert_eq!(fid, Fid::new(SCENERY, 1, 0, 0, 0));
    assert_eq!(list.path(fid).unwrap(), "art/scenery/rock02.frm");
    // The engine's own catalog resolves the same path.
    let mut catalog = ArtCatalog::default();
    catalog.insert(SCENERY, table);
    assert_eq!(catalog.path(fid).unwrap(), "art/scenery/rock02.frm");
    assert_eq!(catalog.last_row(SCENERY, b"ROCK02.frm"), Some(1));
}

#[test]
fn a_base_list_is_kept_byte_for_byte() {
    // Blank and comment-like lines keep positions; no final LF.
    let base = b"first.frm\n\n;comment\ndup.frm\nDUP.FRM ; again\r\nlast.frm";
    let mut list = ArtList::from_base(SCENERY, base).unwrap();
    assert_eq!(list.len(), 6);
    assert_eq!(list.push("dup.frm").unwrap(), 4, "the last row wins");
    assert_eq!(list.push("last.FRM").unwrap(), 5);
    assert_eq!(list.push("new.frm").unwrap(), 6);
    let bytes = list.to_bytes();
    assert!(bytes.starts_with(base));
    let table = LstTable::parse(&bytes, LstKind::Art).unwrap();
    assert_eq!(table.entries().len(), 7);
    assert_eq!(
        table.art_id(5).unwrap().name.as_deref(),
        Some(&b"last.frm"[..])
    );
    assert_eq!(
        table.art_id(6).unwrap().name.as_deref(),
        Some(&b"new.frm"[..])
    );
}

#[test]
fn a_full_last_read_is_not_split_by_the_terminator() {
    // A final unterminated line of exactly 199 bytes fills one read; an
    // added LF would become a position of its own.
    let base = vec![b'a'; 199];
    let mut list = ArtList::from_base(SCENERY, &base).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list.push("b.frm").unwrap(), 1);
    let table = LstTable::parse(&list.to_bytes(), LstKind::Art).unwrap();
    assert_eq!(table.entries().len(), 2);
}

#[test]
fn the_budget_fails_loudly_at_4096() {
    let base: Vec<u8> = (0..FID_INDEX_CAP - 1)
        .flat_map(|i| format!("s{i:07}.frm\n").into_bytes())
        .collect();
    let mut list = ArtList::from_base(SCENERY, &base).unwrap();
    assert_eq!(list.remaining(), 1);
    assert!(list.reserve(1).is_ok());
    assert!(matches!(
        list.reserve(2),
        Err(EmitError::FidBudget {
            category: "scenery",
            cap: 4096,
            requested: 4097
        })
    ));
    assert_eq!(list.push("last.frm").unwrap(), 4095);
    assert_eq!(list.remaining(), 0);
    match list.push("over.frm") {
        Err(EmitError::FidBudget {
            category,
            cap,
            requested,
        }) => {
            assert_eq!((category, cap, requested), ("scenery", 4096, 4097));
        }
        other => panic!("expected FidBudget, got {other:?}"),
    }
    // A name already present still resolves at the cap.
    assert_eq!(list.push("S0000007.FRM").unwrap(), 7);
    let over: Vec<u8> = (0..=FID_INDEX_CAP)
        .flat_map(|i| format!("s{i}\n").into_bytes())
        .collect();
    assert!(matches!(
        ArtList::from_base(SCENERY, &over),
        Err(EmitError::FidBudget { .. })
    ));
}

#[test]
fn bad_names_and_categories_are_refused() {
    assert!(ArtList::new(11).is_err());
    let mut list = ArtList::new(0).unwrap();
    for name in ["", "much_too_long.frm", "a b", "a,b", "x/y.frm"] {
        assert!(
            matches!(list.push(name), Err(EmitError::Lst { .. })),
            "{name:?}"
        );
    }
    assert!(list.is_empty());
}

#[test]
fn a_standalone_list_keeps_only_the_reserved_base_entries() {
    use emit::{RESERVED_TILE_ENTRIES, TILE_CATEGORY, WALL_CATEGORY, standalone_reserved};
    assert_eq!(
        standalone_reserved(TILE_CATEGORY),
        Some(RESERVED_TILE_ENTRIES)
    );
    assert_eq!(standalone_reserved(WALL_CATEGORY), Some(0));
    for category in [0, 1, 2, 5, 6, 10] {
        assert_eq!(standalone_reserved(category), None, "{category}");
        assert!(ArtList::standalone(category, b"a.frm\n").is_err());
    }
    // CRLF and a comment-like line are kept byte for byte.
    let base = b"zero.frm ; note\r\none.frm\r\ntwo.frm\nthree.frm\n";
    let mut tiles = ArtList::standalone(TILE_CATEGORY, base).unwrap();
    assert_eq!(tiles.len(), 2);
    assert_eq!(tiles.remaining(), FID_INDEX_CAP - 2);
    assert_eq!(tiles.to_bytes(), b"zero.frm ; note\r\none.frm\r\n");
    assert_eq!(tiles.find("two.frm"), None, "dropped entries are gone");
    assert_eq!(tiles.push("mine.frm").unwrap(), 2);
    let table = LstTable::parse(&tiles.to_bytes(), LstKind::Art).unwrap();
    assert_eq!(table.entries().len(), 3);
    assert_eq!(
        table.art_id(1).unwrap().name.as_deref(),
        Some(&b"one.frm"[..])
    );

    let mut walls = ArtList::standalone(WALL_CATEGORY, base).unwrap();
    assert!(walls.is_empty());
    assert_eq!(walls.remaining(), FID_INDEX_CAP);
    assert_eq!(walls.push("w.frm").unwrap(), 0);
    assert_eq!(walls.to_bytes(), b"w.frm\n");

    // A base shorter than the reserved entries is kept whole; an
    // unterminated last line gets its LF.
    let short = ArtList::standalone(TILE_CATEGORY, b"only.frm").unwrap();
    assert_eq!(short.len(), 1);
    assert_eq!(short.to_bytes(), b"only.frm\n");
    // An empty base gives an empty list.
    assert!(ArtList::standalone(TILE_CATEGORY, b"").unwrap().is_empty());
}

#[test]
fn a_kept_prefix_survives_a_split_read() {
    // A 250-byte physical line is two positions (199 + 51 bytes).
    let mut base = vec![b'a'; 250];
    base.extend_from_slice(b"\nnext.frm\n");
    let list = ArtList::from_base_prefix(SCENERY, &base, 1).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list.to_bytes(), vec![b'a'; 199]);
    let list = ArtList::from_base_prefix(SCENERY, &base, 2).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list.to_bytes(), base[..251].to_vec());
}
