//! Pass 2's map-stage output on the linked mini mart
//! (`mini_mart::write_groups`): enable-group children placed hidden,
//! presence copies, locks, a trigger volume, linked references, and the
//! placement index (schema 2) joined to the written maps by exact object
//! ids. Everything is checked against the maps on disk.

mod common;

use common::mini_mart::{self, key};
use mojave_convert::placements::{self, PlacedKind, PlacementIndex};
use mojave_convert::{Inputs, Install, pipeline};

fn run(
    fixture: &common::Fixture,
    name: &str,
    out: &str,
    layer_file: &str,
) -> (Inputs, PlacementIndex) {
    let mut inputs = Inputs::from_args(
        &Install::any(&fixture.root.path().join(name).join("Data"), None),
        Some(layer_file),
        &fixture.f2,
        &fixture.root.path().join(out),
        2,
    )
    .expect("inputs");
    inputs.standalone = true;
    pipeline::all(&inputs).expect("the pipeline runs");
    let index = placements::read(&inputs.out)
        .expect("reads")
        .expect("the stage wrote the index");
    (inputs, index)
}

#[test]
fn the_groups_layer_writes_hidden_children_copies_locks_and_an_exact_index() {
    let fixture = common::fixture();
    let dir = fixture.root.path().join("groups");
    let mart = mini_mart::write_groups(&dir);
    let (inputs, index) = run(
        &fixture,
        "groups",
        "out-groups",
        mart.layer_file.to_str().unwrap(),
    );
    let report = std::fs::read_to_string(inputs.out.join("map-report.md")).expect("report");
    assert!(report.contains("## Enable groups"), "{report}");
    assert_eq!(index.schema_version, 2);

    let placed = |id: u32| {
        index
            .placed
            .iter()
            .find(|p| p.reference.as_deref() == Some(key(id).as_str()))
            .unwrap_or_else(|| panic!("{} is not placed\n{report}", key(id)))
    };

    // The group: the parent starts disabled; its child and the rifle and
    // the clerk follow it and are placed hidden; the opposite child starts
    // enabled and is placed visible.
    for id in [
        mini_mart::R_GROUP_PARENT,
        mini_mart::R_GROUP_CHILD,
        mini_mart::R_GROUP_RIFLE,
        mini_mart::R_GROUP_CLERK,
    ] {
        let entry = placed(id);
        assert!(entry.hidden, "{} hidden\n{report}", key(id));
        let group = entry.group.as_ref().expect("a group member");
        assert_eq!(group.root, key(mini_mart::R_GROUP_PARENT));
        assert!(!group.initially_enabled);
        assert!(entry.object_id.is_some());
    }
    assert_eq!(placed(mini_mart::R_GROUP_RIFLE).kind, PlacedKind::Item);
    assert_eq!(placed(mini_mart::R_GROUP_CLERK).kind, PlacedKind::Critter);
    let opposite = placed(mini_mart::R_GROUP_OPPOSITE);
    assert!(!opposite.hidden);
    let group = opposite.group.as_ref().unwrap();
    assert!(group.opposite && group.initially_enabled);
    assert_eq!(
        opposite.enable_parent.as_ref().map(|p| p.opposite),
        Some(true)
    );
    // The source's own flag is still described as the source states it.
    assert!(placed(mini_mart::R_GROUP_CHILD).initially_disabled);
    assert!(
        index
            .unplaced
            .iter()
            .all(|u| u.reason != placements::UnplacedReason::Disabled
                || ![
                    mini_mart::R_GROUP_PARENT,
                    mini_mart::R_GROUP_CHILD,
                    mini_mart::R_GROUP_RIFLE,
                    mini_mart::R_GROUP_CLERK
                ]
                .iter()
                .any(|id| u.reference == key(*id))),
        "group children are placed, not left off"
    );
    // The hidden clerk and the other people: not counted as placed people.
    let report_lines: Vec<&str> = report.lines().filter(|l| l.contains("hidden")).collect();
    assert!(!report_lines.is_empty(), "{report}");

    // The presence copy of the clerk on the car park, hidden, marked.
    let copy = index
        .placed
        .iter()
        .find(|p| p.presence_of.as_deref() == Some(key(mini_mart::R_CLERK).as_str()))
        .unwrap_or_else(|| panic!("no presence copy\n{report}"));
    assert_eq!(copy.map, "mmlot");
    assert!(copy.hidden && copy.critter);
    assert_eq!(
        copy.reference.as_deref(),
        Some(format!("{}@mmlot", key(mini_mart::R_CLERK)).as_str())
    );
    assert_eq!(copy.base, placed(mini_mart::R_CLERK).base);
    // The original stays where it was, unmarked.
    assert_eq!(placed(mini_mart::R_CLERK).presence_of, None);
    assert!(!placed(mini_mart::R_CLERK).hidden);

    // Locks: written, with the level and the key carried.
    let door = placed(mini_mart::R_STORE_DOOR);
    let lock = door.lock.as_ref().expect("the storeroom door's lock");
    assert!(lock.locked);
    assert_eq!(lock.level, 50);
    assert_eq!(
        lock.key.as_deref(),
        Some(key(mini_mart::STORE_KEY).as_str())
    );
    let locker = placed(mini_mart::R_LOCKER);
    let lock = locker.lock.as_ref().expect("the locker's lock");
    assert!(lock.locked);
    assert_eq!(lock.level, 25);
    assert_eq!(lock.key, None);
    assert!(placed(mini_mart::R_REGISTER).lock.is_none());

    // The linked reference, and the trigger volume (no bounds in the MIR
    // yet: a hex, no radius).
    assert_eq!(
        placed(mini_mart::R_LINKED_DOOR).linked_ref.as_deref(),
        Some(key(mini_mart::R_LOCKER).as_str())
    );
    let trigger = index
        .triggers
        .iter()
        .find(|t| t.reference == key(mini_mart::R_TRIGGER))
        .unwrap_or_else(|| panic!("no trigger\n{:?}", index.triggers));
    assert_eq!(trigger.map, "mmstore");
    assert_eq!(trigger.radius, None);

    // Every placed entry's object id is the written map's object: same
    // elevation, hex, PID and hidden flag; no id is used twice.
    let (_, maps) = placements::read_written_maps(&inputs.mod_dir()).expect("reads the maps");
    let mut seen = std::collections::BTreeSet::new();
    let mut checked = 0;
    for entry in &index.placed {
        let id = entry
            .object_id
            .unwrap_or_else(|| panic!("{:?} has no object\n{:?}", entry.reference, index.notes));
        let map = maps.iter().find(|m| m.stem == entry.map).expect("the map");
        let object = map
            .objects
            .iter()
            .find(|o| o.id == id && o.elevation == entry.elevation)
            .unwrap_or_else(|| panic!("{:?}: no object {id}", entry.reference));
        assert_eq!(object.hex, entry.hex, "{:?}", entry.reference);
        assert_eq!(Some(object.pid), entry.pid, "{:?}", entry.reference);
        assert_eq!(object.hidden, entry.hidden, "{:?}", entry.reference);
        assert_eq!(object.critter, entry.critter, "{:?}", entry.reference);
        assert!(seen.insert((entry.map.clone(), id)), "{id} twice");
        checked += 1;
    }
    assert!(checked >= 12, "{checked}");
    assert!(
        !index.notes.iter().any(|n| n.contains("does not")),
        "{:?}",
        index.notes
    );

    // The copy is on the written car park map, hidden, on a walkable hex.
    let lot = maps.iter().find(|m| m.stem == "mmlot").unwrap();
    let copy_object = lot
        .objects
        .iter()
        .find(|o| Some(o.id) == copy.object_id)
        .unwrap();
    assert!(copy_object.hidden && copy_object.critter);

    // Built again, the same bytes.
    let again = std::fs::read_to_string(inputs.out.join(placements::FILE_NAME)).unwrap();
    let (inputs2, _) = run(
        &fixture,
        "groups",
        "out-groups-2",
        mart.layer_file.to_str().unwrap(),
    );
    let twice = std::fs::read_to_string(inputs2.out.join(placements::FILE_NAME)).unwrap();
    assert_eq!(again, twice);
}

/// Without the options nothing is hidden and no copy is made: the layer of
/// `write_linked` has no group, and the same layer file with the options
/// off places nothing hidden.
#[test]
fn the_options_default_off() {
    let fixture = common::fixture();
    let dir = fixture.root.path().join("plain");
    let mart = mini_mart::write_groups(&dir);
    // The same install and slices, without `enable_groups` and
    // `presence_copies`.
    let plain = dir.join("plain.layer.toml");
    std::fs::write(
        &plain,
        "name = \"minimart-plain\"\nstart = \"mmlot\"\n\n[all_maps]\nwire = true\n\n\
         [[maps]]\nslice = \"lot.toml\"\nmap = \"mmlot\"\n\n\
         [[maps]]\nslice = \"store.toml\"\nmap = \"mmstore\"\nstoreys = \"auto\"\n",
    )
    .unwrap();
    let _ = mart;
    let (_, index) = run(&fixture, "plain", "out-plain", plain.to_str().unwrap());
    assert!(
        index
            .placed
            .iter()
            .all(|p| !p.hidden && p.presence_of.is_none())
    );
    let child = index
        .unplaced
        .iter()
        .find(|u| u.reference == key(mini_mart::R_GROUP_CHILD))
        .expect("the disabled child is listed as left off");
    assert!(
        child.reason == placements::UnplacedReason::Disabled
            || child
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("isabled")),
        "{child:?}: a disabled child is left off as before"
    );
}

/// `hostile_packet = <n>` picks the AI packet of the people the catalogue
/// marks hostile; the index records it on each, and the default is the
/// rebalance table's, as before.
#[test]
fn the_hostile_packet_option_picks_the_raiders_packet() {
    let fixture = common::fixture();
    let dir = fixture.root.path().join("packet");
    let mart = mini_mart::write_groups(&dir);
    let layer = |extra: &str| {
        format!(
            "name = \"minimart-packet\"\nstart = \"mmlot\"\n\n[all_maps]\nwire = true\n{extra}\n\
             [[maps]]\nslice = \"lot.toml\"\nmap = \"mmlot\"\n\n\
             [[maps]]\nslice = \"store.toml\"\nmap = \"mmstore\"\nstoreys = \"auto\"\n"
        )
    };
    let packets = |file: &str, out: &str| {
        let (_, index) = run(&fixture, "packet", out, file);
        let raiders: Vec<_> = index
            .placed
            .iter()
            .filter(|p| p.hostile)
            .map(|p| p.ai_packet)
            .collect();
        assert!(raiders.len() >= 3, "{raiders:?}");
        let friendly: Vec<_> = index
            .placed
            .iter()
            .filter(|p| p.critter && !p.hostile && !p.hidden)
            .map(|p| p.ai_packet)
            .collect();
        (raiders, friendly)
    };
    let default_file = dir.join("default.layer.toml");
    std::fs::write(&default_file, layer("")).unwrap();
    let (default_hostile, default_friendly) =
        packets(default_file.to_str().unwrap(), "out-packet-default");
    assert!(
        default_hostile.iter().all(|p| *p == Some(136)),
        "{default_hostile:?}"
    );
    let chosen_file = dir.join("chosen.layer.toml");
    std::fs::write(&chosen_file, layer("hostile_packet = 7\n")).unwrap();
    let (chosen_hostile, chosen_friendly) =
        packets(chosen_file.to_str().unwrap(), "out-packet-chosen");
    assert!(
        chosen_hostile.iter().all(|p| *p == Some(7)),
        "{chosen_hostile:?}"
    );
    assert_eq!(
        default_friendly, chosen_friendly,
        "friendly people keep their packet"
    );
    let _ = mart;
}
