//! The synthetic vectors of `docs/vault-engine-briefs/`, checked against
//! this crate's writers and the pinned parsers, so the briefs ask for
//! bytes the local adapter already produces.

use emit::gam::write_gam;
use emit::lst::write_lst;
use emit::msg::write_msg;
use emit::{
    ArtList, FrmDirection, FrmFrameSpec, FrmSpec, GamSection, GamVar, LstLine, MsgEntry, MsgFile,
};
use vault_formats::{FrmArchive, GamFile, LstKind, LstTable, MsgCatalog};

fn frame(pixels: &[u8], displacement: [i16; 2]) -> FrmFrameSpec {
    FrmFrameSpec {
        width: 2,
        height: 1,
        displacement,
        pixels: pixels.to_vec(),
    }
}

#[test]
fn frm_vectors() {
    let a = FrmSpec {
        fps: 10,
        action_frame: 0,
        directions: vec![FrmDirection {
            shift: [0, 0],
            frames: vec![frame(&[1, 2], [0, 0])],
        }],
    };
    let mut expected = vec![0, 0, 0, 4, 0, 0x0a, 0, 0, 0, 1];
    expected.extend([0; 48]);
    expected.extend([0, 0, 0, 0x0e]);
    expected.extend([0, 2, 0, 1, 0, 0, 0, 2, 0, 0, 0, 0, 1, 2]);
    assert_eq!(emit::frm::write_frm(&a).unwrap(), expected);
    assert_eq!(expected.len(), 76);

    let b = FrmSpec {
        directions: vec![FrmDirection {
            shift: [-3, 20],
            frames: vec![frame(&[1, 2], [-1, 1])],
        }],
        ..a.clone()
    };
    let archive = FrmArchive::parse(&emit::frm::write_frm(&b).unwrap()).unwrap();
    assert_eq!(archive.shifts, [[-3, 20]; 6]);
    let rect = archive
        .frame(0, 0)
        .unwrap()
        .world_rect([100, 50], [-3, 20], [0, 0]);
    assert_eq!(
        (rect.left, rect.top, rect.right, rect.bottom),
        (96, 70, 97, 70)
    );

    let distinct = |seed: u8| vec![frame(&[seed, seed], [0, 0])];
    let c = FrmSpec {
        directions: [1, 1, 2, 3, 4, 5]
            .into_iter()
            .map(|seed| FrmDirection {
                shift: [0, 0],
                frames: distinct(seed),
            })
            .collect(),
        ..a
    };
    let archive = FrmArchive::parse(&emit::frm::write_frm(&c).unwrap()).unwrap();
    assert_eq!(archive.offsets, [0, 0, 14, 28, 42, 56]);
    assert_eq!(archive.frame_area_len, 70);
    assert_eq!(archive.stored_sequences(), 5);
}

#[test]
fn lst_vectors() {
    let art = write_lst(&[
        LstLine::art("rock.frm"),
        LstLine::Blank,
        LstLine::Art {
            name: "crit".into(),
            numbers: vec![3],
        },
        LstLine::Art {
            name: "head".into(),
            numbers: vec![1, 2, 3],
        },
    ])
    .unwrap();
    assert_eq!(art, b"rock.frm\n\ncrit,3\nhead,1,2,3\n");
    let table = LstTable::parse(&art, LstKind::Art).unwrap();
    assert_eq!(table.entries()[2].alias, Some(3));
    assert_eq!(table.entries()[3].head_columns, Some([1, 2, 3]));
    let script = write_lst(&[LstLine::Script {
        stem: "obj_a".into(),
        local_vars: Some(12),
        comment: Some("test".into()),
    }])
    .unwrap();
    assert_eq!(script, b"obj_a.int ; test # local_vars=12\n");
    let proto = write_lst(&[
        LstLine::prototype("00000001.pro"),
        LstLine::Prototype {
            file: "00000002.pro".into(),
            comment: Some("x".into()),
        },
    ])
    .unwrap();
    assert_eq!(proto, b"00000001.pro\n00000002.pro x\n");
    let mut list = ArtList::from_base(2, b"a.frm").unwrap();
    assert_eq!(list.push("b.frm").unwrap(), 1);
    assert_eq!(list.to_bytes(), b"a.frm\nb.frm\n");
    let mut full = ArtList::from_base(2, &[b'a'; 199]).unwrap();
    assert_eq!(full.push("b.frm").unwrap(), 1);
    assert_eq!(full.to_bytes(), [&[b'a'; 199][..], b"b.frm\n"].concat());
}

#[test]
fn msg_vectors() {
    let entry = |id, sound: &[u8], text: &[u8]| MsgEntry {
        id,
        sound: sound.to_vec(),
        text: text.to_vec(),
    };
    assert_eq!(
        write_msg(&[
            entry(3, b"", b"c"),
            entry(-1, b"", b"a"),
            entry(2, b"", b"b")
        ])
        .unwrap(),
        b"{-1}{}{a}\n{2}{}{b}\n{3}{}{c}\n"
    );
    let braces = write_msg(&[entry(100, b"snd01", b"x { y")]).unwrap();
    assert_eq!(braces, b"{100}{snd01}{x { y}\n");
    assert_eq!(
        MsgCatalog::parse_strict(&braces)
            .unwrap()
            .get(100)
            .unwrap()
            .text,
        b"x { y"
    );
    let cr = write_msg(&[entry(5, b"", b"a\rb")]).unwrap();
    assert_eq!(
        cr,
        [
            0x7b, 0x35, 0x7d, 0x7b, 0x7d, 0x7b, 0x61, 0x0d, 0x62, 0x7d, 0x0a
        ]
    );
    let mut file = MsgFile::from_base(b"{1}{}{One}").unwrap();
    file.push(&entry(2, b"", b"Two")).unwrap();
    assert_eq!(file.to_bytes(), b"{1}{}{One}\n{2}{}{Two}\n");
    assert!(file.push(&entry(1, b"", b"x")).is_err());
}

#[test]
fn gam_vectors() {
    let vars = [
        GamVar::new("VAR_0000", 0),
        GamVar::new("VAR_0001", -17),
        GamVar::new("VAR_0002", i32::MAX),
    ];
    let map = write_gam(GamSection::Map, &vars).unwrap();
    assert_eq!(
        map,
        b"MAP_GLOBAL_VARS:\nVAR_0000 = 0;\nVAR_0001 = -17;\nVAR_0002 = 2147483647;\n"
    );
    let parsed = GamFile::parse(&map, GamSection::Map).unwrap();
    assert!(parsed.found);
    assert_eq!(parsed.values, [0, -17, i32::MAX]);
    let campaign = GamFile::parse(&map, GamSection::Campaign).unwrap();
    assert!(!campaign.found && campaign.values.is_empty());
    assert_eq!(
        write_gam(GamSection::Campaign, &[GamVar::new("A", 5)]).unwrap(),
        b"GAME_GLOBAL_VARS:\nA = 5;\n"
    );
    for name in ["a;b", "a=b", "//c", "MAP_GLOBAL_VARS:x"] {
        assert!(write_gam(GamSection::Map, &[GamVar::new(name, 1)]).is_err());
    }
    assert!(write_gam(GamSection::Map, &[GamVar::new("n".repeat(250), i32::MIN)]).is_err());
}
