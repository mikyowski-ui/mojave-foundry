//! `map::attach_scripts`: script records and bindings added to a built map
//! (spec 10.4), on synthetic maps built the way the other tests build them.

use emit::map::{
    CritterPlacement, ObjectScript, ScriptAttachment, ScriptSite, attach_scripts,
    attach_scripts_to, critter_object, empty_map, plain_object,
};
use emit::objects::{ItemState, LockState, ProtoSet, Stack, container_object, item_object};
use emit::proto::{
    self, ItemStats, Material, ObjectFlags, ProtoCommon, container, critter_prototype, misc_item,
    scenery,
};
use emit::{CritterSpec, EmitError, FormatWriters, LocalWriters};
use vault_formats::map::{MapFile, MapObject, ScriptPrefix};
use vault_formats::pro::{Category, MapFlags, PackedLocation};
use vault_formats::{Fid, GameProfile};

const ROCK: u32 = 0x2000_0001;
const KEY: u32 = 0x0000_0001;
const CHEST: u32 = 0x0000_0002;
const RAIDER: u32 = 0x0100_0001;

fn common(category: Category, index: u32, art: u8, flags: ObjectFlags) -> ProtoCommon {
    ProtoCommon {
        pid: proto::pid(category, index).unwrap(),
        fid: Fid::new(art, index as u16, 0, 0, 0),
        message_base: proto::default_message_base(index).unwrap(),
        flags,
        material: Material::Wood,
    }
}

fn set() -> ProtoSet {
    let mut set = ProtoSet::new();
    let rock = scenery(common(Category::Scenery, 1, 2, ObjectFlags::SOLID)).unwrap();
    assert_eq!(rock.pid, 0x0200_0001);
    set.insert(rock);
    set.insert(
        misc_item(
            common(Category::Item, 1, 0, ObjectFlags::DECAL),
            ItemStats {
                size: 1,
                inventory_art: Some(Fid::new(7, 9, 0, 0, 0)),
                ..ItemStats::default()
            },
        )
        .unwrap(),
    );
    set.insert(
        container(
            common(Category::Item, 2, 0, ObjectFlags::SOLID),
            ItemStats::default(),
            100,
            true,
        )
        .unwrap(),
    );
    set.insert(
        critter_prototype(
            common(Category::Critter, 1, 1, ObjectFlags::SOLID),
            &CritterSpec::human(40, 1, 7),
        )
        .unwrap(),
    );
    set
}

/// Ids: rocks 10 to 10 + `rocks`, the raider 1000, the chest 1001 holding
/// the key 1002.
fn map_with(rocks: u32) -> (Vec<u8>, ProtoSet) {
    let set = set();
    let mut map = empty_map("SCRIPTED.MAP", 200, 20_100).unwrap();
    let mut objects: Vec<MapObject> = (0..rocks)
        .map(|index| {
            plain_object(
                10 + index,
                0x0200_0001,
                Fid::new(2, 1, 0, 0, 0),
                20_000 + index as i32,
                0,
                ObjectFlags::SOLID.bits(),
            )
        })
        .collect();
    let raider = set.get(RAIDER).unwrap();
    objects.push(
        critter_object(&CritterPlacement::from_prototype(raider, 1000, 30_000, 0, 2).unwrap())
            .unwrap(),
    );
    let key = item_object(&set, 0x0000_0001, 1002, None, 0, ItemState::default()).unwrap();
    objects.push(
        container_object(
            &set,
            CHEST,
            1001,
            Some(30_100),
            0,
            LockState::default(),
            vec![Stack::new(key, 1)],
        )
        .unwrap(),
    );
    map.objects.total = objects.len();
    map.objects.elevations[0] = objects;
    let _ = (ROCK, KEY);
    (LocalWriters.map(&map).unwrap(), set)
}

fn object(id: u32, kind: ObjectScript, script_index: u32, local_vars: u32) -> ScriptAttachment {
    ScriptAttachment {
        site: ScriptSite::Object { id, kind },
        script_index,
        local_vars,
    }
}

fn read(bytes: &[u8], set: &ProtoSet) -> MapFile {
    MapFile::parse_for(bytes, set, GameProfile::Fallout2).unwrap()
}

fn find(map: &MapFile, id: u32) -> &MapObject {
    fn search(object: &MapObject, id: u32) -> Option<&MapObject> {
        if object.id == id {
            return Some(object);
        }
        object
            .inventory
            .iter()
            .find_map(|entry| search(&entry.object, id))
    }
    map.objects
        .elevations
        .iter()
        .flatten()
        .find_map(|object| search(object, id))
        .unwrap()
}

fn all() -> Vec<ScriptAttachment> {
    vec![
        object(10, ObjectScript::Item, 7, 2),
        object(1000, ObjectScript::Critter, 9, 0),
        object(1002, ObjectScript::Item, 11, 1),
        ScriptAttachment {
            site: ScriptSite::Spatial {
                hex: 20_500,
                elevation: 0,
                radius: 3,
            },
            script_index: 12,
            local_vars: 0,
        },
    ]
}

#[test]
fn nothing_to_attach_changes_nothing() {
    let (bytes, set) = map_with(3);
    assert_eq!(attach_scripts(&bytes, &set, &[], None).unwrap(), bytes);
}

#[test]
fn records_bindings_and_the_map_script_are_set() {
    let (bytes, set) = map_with(3);
    let out = attach_scripts(&bytes, &set, &all(), Some(5)).unwrap();
    let map = read(&out, &set);
    assert_eq!(map.header.script_reference, 6);
    assert_eq!(map.header.map_script_index(), Some(5));
    // No system record: the engine makes the map script from the header.
    assert_eq!(map.scripts[0].declared_count, 0);

    let items: Vec<_> = map.scripts[3].active().collect();
    assert_eq!(items.len(), 2, "the rock and the key");
    assert_eq!((items[0].id, items[0].file_index), (0x0300_0001, 7));
    assert_eq!(items[0].object_id, 10);
    assert_eq!(items[0].local_count, 0, "a fresh map stores no count");
    assert_eq!((items[1].id, items[1].file_index), (0x0300_0002, 11));
    assert_eq!(items[1].object_id, 1002);
    assert_eq!(items[1].local_base, -1);
    let critters: Vec<_> = map.scripts[4].active().collect();
    assert_eq!(critters.len(), 1);
    assert_eq!((critters[0].id, critters[0].file_index), (0x0400_0001, 9));
    let spatial: Vec<_> = map.scripts[1].active().collect();
    assert_eq!(spatial.len(), 1);
    assert_eq!((spatial[0].id, spatial[0].file_index), (0x0100_0001, 12));
    assert_eq!(spatial[0].object_id, -1);
    assert_eq!(
        spatial[0].prefix,
        ScriptPrefix::Spatial {
            location: PackedLocation(20_500),
            radius: 3
        }
    );
    assert_eq!(map.scripts[2].declared_count, 0);

    // Bindings, read back through the engine's own restore.
    let rock = find(&map, 10);
    assert_eq!((rock.script_id, rock.cached_script_file), (0x0300_0001, 7));
    let raider = find(&map, 1000);
    assert_eq!(raider.script_id, 0x0400_0001);
    assert_eq!(raider.cached_script_file, 9);
    assert_eq!(find(&map, 1002).script_id, 0x0300_0002);
    // Objects not named are untouched.
    for id in [11, 12, 1001] {
        let other = find(&map, id);
        assert_eq!(
            (other.script_id, other.cached_script_file),
            (-1, -1),
            "{id}"
        );
    }
    // Everything else is as it was.
    let before = read(&bytes, &set);
    assert_eq!(map.tiles, before.tiles);
    assert_eq!(map.objects.total, before.objects.total);
    assert_eq!(map.header.entry_hex, before.header.entry_hex);
}

#[test]
fn a_second_run_is_byte_identical_and_order_does_not_matter() {
    let (bytes, set) = map_with(3);
    let once = attach_scripts(&bytes, &set, &all(), Some(5)).unwrap();
    let twice = attach_scripts(&once, &set, &all(), Some(5)).unwrap();
    assert_eq!(once, twice);
    let mut reversed = all();
    reversed.reverse();
    assert_eq!(
        attach_scripts(&bytes, &set, &reversed, Some(5)).unwrap(),
        once
    );
    // Naming an object twice with the same script is the same attachment.
    let mut doubled = all();
    doubled.extend(all());
    assert_eq!(
        attach_scripts(&bytes, &set, &doubled, Some(5)).unwrap(),
        once
    );
}

#[test]
fn a_later_run_adds_to_earlier_records() {
    let (bytes, set) = map_with(3);
    let first =
        attach_scripts(&bytes, &set, &[object(11, ObjectScript::Item, 3, 0)], None).unwrap();
    let second = attach_scripts(
        &first,
        &set,
        &[
            object(11, ObjectScript::Item, 3, 0),
            object(12, ObjectScript::Item, 4, 0),
        ],
        Some(0),
    )
    .unwrap();
    let map = read(&second, &set);
    assert_eq!(map.header.script_reference, 1);
    let ids: Vec<u32> = map.scripts[3].active().map(|record| record.id).collect();
    assert_eq!(ids, [0x0300_0001, 0x0300_0002]);
    assert_eq!(find(&map, 11).script_id, 0x0300_0001);
    assert_eq!(find(&map, 12).script_id, 0x0300_0002);
    assert_eq!(find(&map, 12).cached_script_file, 4);
    // Without a map script given, the header's stays.
    let kept = attach_scripts(&second, &set, &[], None).unwrap();
    assert_eq!(read(&kept, &set).header.script_reference, 1);
}

#[test]
fn many_records_span_blocks_and_read_back() {
    let (bytes, set) = map_with(100);
    let attachments: Vec<_> = (0..100)
        .map(|index| object(10 + index, ObjectScript::Item, 100 + index, 0))
        .collect();
    let out = attach_scripts(&bytes, &set, &attachments, None).unwrap();
    let map = read(&out, &set);
    assert_eq!(map.scripts[3].declared_count, 100);
    assert_eq!(map.scripts[3].blocks.len(), 7);
    assert_eq!(map.scripts[3].active().count(), 100);
    for index in 0..100 {
        let rock = find(&map, 10 + index);
        assert_eq!(rock.script_id, 0x0300_0000 | (index as i32 + 1));
        assert_eq!(rock.cached_script_file, 100 + index as i32);
    }
    assert_eq!(attach_scripts(&out, &set, &attachments, None).unwrap(), out);
}

#[test]
fn what_cannot_be_attached_is_refused_and_named() {
    let (bytes, set) = map_with(3);
    let refuse = |attachments: &[ScriptAttachment], map_script| match attach_scripts(
        &bytes,
        &set,
        attachments,
        map_script,
    ) {
        Err(EmitError::Map(message)) => message,
        other => panic!("expected a map error, got {other:?}"),
    };
    let message = refuse(&[object(999, ObjectScript::Item, 1, 0)], None);
    assert!(message.contains("object id 999"), "{message}");
    // A critter script on a rock, an item script on a critter.
    assert!(refuse(&[object(10, ObjectScript::Critter, 1, 0)], None).contains("object 10"));
    assert!(refuse(&[object(1000, ObjectScript::Item, 1, 0)], None).contains("object 1000"));
    assert!(refuse(&[object(10, ObjectScript::Item, 0x0100_0000, 0)], None).contains("outside"));
    assert!(refuse(&[object(10, ObjectScript::Item, 1, 1_000_001)], None).contains("local"));
    assert!(refuse(&[], Some(0x0100_0000)).contains("map script"));
    for (hex, elevation, radius) in [(40_000, 0, 1), (-1, 0, 1), (5, 3, 1), (5, 0, -1)] {
        let spatial = ScriptAttachment {
            site: ScriptSite::Spatial {
                hex,
                elevation,
                radius,
            },
            script_index: 1,
            local_vars: 0,
        };
        assert!(refuse(&[spatial], None).contains("spatial"));
    }
    // One object, two different scripts.
    let message = refuse(
        &[
            object(10, ObjectScript::Item, 1, 0),
            object(10, ObjectScript::Item, 2, 0),
        ],
        None,
    );
    assert!(
        message.contains("object 10 already has script"),
        "{message}"
    );
    // The same, against an earlier run.
    let attached =
        attach_scripts(&bytes, &set, &[object(10, ObjectScript::Item, 1, 0)], None).unwrap();
    assert!(matches!(
        attach_scripts(
            &attached,
            &set,
            &[object(10, ObjectScript::Item, 2, 0)],
            None
        ),
        Err(EmitError::Map(_))
    ));
    // A map that does not parse is a format error, not a panic.
    assert!(attach_scripts(&bytes[..40], &set, &[], None).is_err());
}

#[test]
fn a_refused_attachment_leaves_the_map_as_it_was() {
    let (bytes, set) = map_with(3);
    let mut map = read(&bytes, &set);
    let result = attach_scripts_to(
        &mut map,
        &[
            object(10, ObjectScript::Item, 7, 0),
            object(999, ObjectScript::Item, 8, 0),
        ],
        Some(3),
    );
    assert!(result.is_err());
    assert_eq!(map.header.script_reference, 0);
    assert_eq!(find(&map, 10).script_id, -1);
    assert_eq!(map.scripts[3].declared_count, 0);
    assert_eq!(LocalWriters.map(&map).unwrap(), bytes);
}

#[test]
fn a_saved_map_keeps_the_local_count() {
    let (bytes, set) = map_with(2);
    let mut map = read(&bytes, &set);
    map.header.flags = MapFlags(map.header.flags.0 | MapFlags::SAVED.0);
    let saved = LocalWriters.map(&map).unwrap();
    let out = attach_scripts(&saved, &set, &[object(10, ObjectScript::Item, 7, 2)], None).unwrap();
    let back = read(&out, &set);
    assert_eq!(back.scripts[3].active().next().unwrap().local_count, 2);
    assert_eq!(
        attach_scripts(&out, &set, &[object(10, ObjectScript::Item, 7, 2)], None).unwrap(),
        out
    );
}
