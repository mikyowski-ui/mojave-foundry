//! The placement index's pure builder (`placements::build`): exact object
//! ids from the builder, enable groups, locks, links, presence copies,
//! triggers, and the omitted-never-null JSON.

use std::collections::BTreeSet;

use mojave_convert::placements::{
    self, EnableParent, GroupSource, MapInput, ObjectSpot, PlacedKind, PlacementIndex, SourceRef,
    TriggerSpot, UnplacedReason, WrittenObject, group_members,
};

fn source(reference: &str, base_kind: &str) -> SourceRef {
    SourceRef {
        reference: reference.to_owned(),
        editor_id: Some(format!("ed{reference}")),
        base: Some(format!("base:{reference}")),
        base_kind: Some(base_kind.to_owned()),
        critter: matches!(base_kind, "NPC_" | "CREA"),
        ..SourceRef::default()
    }
}

fn written(elevation: u8, hex: i32, id: u32) -> WrittenObject {
    WrittenObject {
        elevation,
        hex,
        id,
        pid: 0x0200_0000 + id,
        critter: false,
        door: false,
        exit_grid: false,
        holds_items: false,
        hidden: false,
        locked: false,
        team: None,
        ai_packet: None,
    }
}

fn spot(reference: &str, written: &WrittenObject) -> ObjectSpot {
    ObjectSpot {
        reference: reference.to_owned(),
        elevation: written.elevation,
        id: written.id,
        hex: written.hex,
        pid: written.pid,
    }
}

fn group_source<'a>(
    form: &'a str,
    parent: Option<(&'a str, bool)>,
    own_disabled: bool,
) -> GroupSource<'a> {
    GroupSource {
        form,
        parent,
        own_disabled,
    }
}

#[test]
fn a_child_follows_its_parent_and_an_opposite_child_follows_it_inverted() {
    let refs = [
        group_source("p", None, true),
        group_source("normal", Some(("p", false)), false),
        group_source("opposite", Some(("p", true)), true),
        group_source("deep", Some(("normal", true)), false),
        group_source("alone", None, true),
        group_source("outside", Some(("elsewhere", false)), true),
    ];
    let groups = group_members(&refs);
    // The parent starts disabled; its normal child follows, the opposite
    // child (whose own flag is set) starts enabled.
    assert!(groups["p"].starts_disabled);
    assert_eq!(groups["p"].root, "p");
    assert!(groups["normal"].starts_disabled);
    assert!(!groups["opposite"].starts_disabled);
    assert!(groups["opposite"].opposite);
    // A chain is flattened: the root, with the opposite flags multiplied.
    assert_eq!(groups["deep"].root, "p");
    assert!(groups["deep"].opposite);
    assert!(
        !groups["deep"].starts_disabled,
        "off parent, opposite of an off child"
    );
    // A reference with no parent and no child is not in a group; a parent
    // outside the references counts as enabled.
    assert!(!groups.contains_key("alone"));
    assert!(!groups["outside"].starts_disabled);
    assert_eq!(groups["outside"].root, "elsewhere");
}

#[test]
fn a_cycle_is_cut() {
    let refs = [
        group_source("a", Some(("b", false)), false),
        group_source("b", Some(("a", false)), true),
    ];
    let groups = group_members(&refs);
    assert_eq!(groups.len(), 2);
    assert!(!groups["a"].starts_disabled);
}

#[test]
fn the_index_lists_each_reference_with_its_exact_object() {
    let mut parent = source("p:000001", "STAT");
    parent.initially_disabled = true;
    let mut child = source("p:000002", "STAT");
    child.enable_parent = Some(EnableParent {
        reference: "p:000001".into(),
        opposite: false,
    });
    child.initially_disabled = true;
    let mut opposite = source("p:000003", "STAT");
    opposite.enable_parent = Some(EnableParent {
        reference: "p:000001".into(),
        opposite: true,
    });
    let mut door = source("p:000004", "DOOR");
    door.lock = Some((50, Some("key:000009".into())));
    door.linked_ref = Some("p:000005".into());
    let mut chest = source("p:000006", "CONT");
    chest.lock = Some((255, None));
    let clerk = source("p:000007", "NPC_");
    let plain = source("p:000008", "STAT");

    // The builder wrote the door as object 3 and the hidden child as 1;
    // object ids need not follow the reference order.
    let mut hidden_child = written(0, 100, 1);
    hidden_child.hidden = true;
    let visible = written(0, 101, 2);
    let mut door_object = written(0, 102, 3);
    door_object.door = true;
    door_object.locked = true;
    let mut chest_object = written(0, 103, 4);
    chest_object.pid = 0x0000_0300;
    chest_object.holds_items = true;
    let mut clerk_object = written(0, 104, 5);
    clerk_object.pid = 0x0100_0007;
    clerk_object.critter = true;
    clerk_object.team = Some(5);
    clerk_object.ai_packet = Some(136);
    let mut copy_object = written(0, 105, 6);
    copy_object.pid = 0x0100_0007;
    copy_object.critter = true;
    copy_object.hidden = true;
    let blocker = written(0, 9, 7);

    let references = [parent, child, opposite, door, chest, clerk, plain];
    let all_written = [
        hidden_child.clone(),
        visible.clone(),
        door_object.clone(),
        chest_object.clone(),
        clerk_object.clone(),
        copy_object.clone(),
        blocker,
    ];
    let objects = [
        spot("p:000002", &hidden_child),
        spot("p:000003", &visible),
        spot("p:000004", &door_object),
        spot("p:000006", &chest_object),
        spot("p:000007", &clerk_object),
        spot("p:000007@town", &copy_object),
    ];
    let hostile: BTreeSet<String> = ["p:000007".to_owned()].into();
    let triggers = [TriggerSpot {
        reference: "p:000008".into(),
        elevation: 0,
        hex: 120,
        shape: Some("box".into()),
        radius: Some(3),
    }];
    let copies = [("p:000007@town".to_owned(), "p:000007".to_owned())];
    let skipped = [("p:000001".to_owned(), "slicer: a marker".to_owned())];
    let map = MapInput {
        stem: "town",
        number: 70,
        references: &references,
        objects: &objects,
        skipped: &skipped,
        written: &all_written,
        copies: &copies,
        triggers: &triggers,
        hostile: &hostile,
    };
    let index = placements::build("test", "fallout3", &[map], &BTreeSet::new());

    assert_eq!(index.schema_version, 2);
    let placed = |reference: &str| {
        index
            .placed
            .iter()
            .find(|p| p.reference.as_deref() == Some(reference))
            .unwrap_or_else(|| panic!("{reference} is not placed"))
    };
    // The hidden child: exact id 1, hidden, in group p:000001 (disabled).
    let child = placed("p:000002");
    assert_eq!(child.object_id, Some(1));
    assert!(child.hidden);
    assert!(child.initially_disabled);
    let group = child.group.as_ref().unwrap();
    assert_eq!(group.root, "p:000001");
    assert!(!group.initially_enabled);
    // The opposite child starts enabled.
    let opposite = placed("p:000003");
    assert!(opposite.group.as_ref().unwrap().opposite);
    assert!(opposite.group.as_ref().unwrap().initially_enabled);
    assert!(!opposite.hidden);
    // The locked door: lock level, key base, and what was written; its link.
    let door = placed("p:000004");
    assert_eq!(door.kind, PlacedKind::Door);
    assert_eq!(door.object_id, Some(3));
    let lock = door.lock.as_ref().unwrap();
    assert!(lock.locked);
    assert_eq!(lock.level, 50);
    assert_eq!(lock.key.as_deref(), Some("key:000009"));
    assert_eq!(door.linked_ref.as_deref(), Some("p:000005"));
    // A container whose lock was not written says so.
    let chest = placed("p:000006");
    assert_eq!(chest.kind, PlacedKind::Container);
    assert!(!chest.lock.as_ref().unwrap().locked);
    assert_eq!(chest.lock.as_ref().unwrap().level, 255);
    // The clerk and its copy.
    let clerk = placed("p:000007");
    assert!(clerk.hostile && clerk.critter);
    assert_eq!(clerk.team, Some(5));
    assert_eq!(clerk.ai_packet, Some(136));
    assert_eq!(clerk.presence_of, None);
    let copy = placed("p:000007@town");
    assert_eq!(copy.presence_of.as_deref(), Some("p:000007"));
    assert!(copy.hidden && copy.critter);
    assert_eq!(copy.base.as_deref(), Some("base:p:000007"));
    assert_eq!(copy.object_id, Some(6));
    // The parent was skipped by the slicer: listed apart with its reason;
    // plain scenery is not listed.
    assert!(
        index
            .placed
            .iter()
            .all(|p| p.reference.as_deref() != Some("p:000008"))
    );
    let parent = index
        .unplaced
        .iter()
        .find(|u| u.reference == "p:000001")
        .unwrap();
    assert_eq!(parent.reason, UnplacedReason::Skipped);
    assert_eq!(parent.group.as_ref().unwrap().root, "p:000001");
    // The linked door's target is listed even as scenery.
    assert_eq!(index.triggers.len(), 1);
    assert_eq!(index.triggers[0].radius, Some(3));
    assert!(index.notes.is_empty(), "{:?}", index.notes);
    // Sorted by map, elevation, object id.
    let ids: Vec<u32> = index.placed.iter().filter_map(|p| p.object_id).collect();
    assert_eq!(ids, [1, 2, 3, 4, 5, 6]);

    // The text round-trips, and an omitted field is not `null`.
    let text = index.to_text();
    assert!(!text.contains("null"), "{text}");
    let again = PlacementIndex::from_text(&text).unwrap();
    assert_eq!(again, index);
    assert!(
        PlacementIndex::from_text(&text.replace("\"schema_version\": 2", "\"schema_version\": 1"))
            .is_err()
    );
}

#[test]
fn an_object_the_written_map_does_not_bear_out_is_noted() {
    let sources = [source("p:000001", "STAT"), source("p:000002", "STAT")];
    let one = written(0, 10, 1);
    let mut moved = written(0, 11, 2);
    let spots = [spot("p:000001", &one), spot("p:000002", &moved)];
    // The map file says another hex than the builder named.
    moved.hex = 12;
    let all = [one, moved];
    let hostile = BTreeSet::new();
    let referenced: BTreeSet<String> = sources.iter().map(|s| s.reference.clone()).collect();
    let map = MapInput {
        stem: "town",
        number: 70,
        references: &sources,
        objects: &spots,
        skipped: &[],
        written: &all,
        copies: &[],
        triggers: &[],
        hostile: &hostile,
    };
    let index = placements::build("test", "fallout3", &[map], &referenced);
    assert_eq!(index.placed.len(), 2);
    assert_eq!(index.notes.len(), 1, "{:?}", index.notes);
    assert!(index.notes[0].contains("does not bear out"));
}

#[test]
fn a_multi_piece_reference_lists_every_object() {
    let sources = [source("p:000001", "AMMO")];
    let first = written(0, 10, 1);
    let second = written(0, 10, 2);
    let spots = [spot("p:000001", &first), spot("p:000001", &second)];
    let all = [first, second];
    let hostile = BTreeSet::new();
    let map = MapInput {
        stem: "town",
        number: 70,
        references: &sources,
        objects: &spots,
        skipped: &[],
        written: &all,
        copies: &[],
        triggers: &[],
        hostile: &hostile,
    };
    let index = placements::build("test", "fallout3", &[map], &BTreeSet::new());
    assert_eq!(index.placed.len(), 1);
    assert_eq!(index.placed[0].pieces, 2);
    assert_eq!(index.placed[0].piece_ids, [1, 2]);
    assert_eq!(index.placed[0].object_id, Some(1));
}

#[test]
fn building_twice_gives_the_same_bytes() {
    let sources = [source("p:000001", "DOOR"), source("p:000002", "NPC_")];
    let a = written(0, 10, 1);
    let b = written(0, 11, 2);
    let spots = [spot("p:000002", &b), spot("p:000001", &a)];
    let all = [a, b];
    let hostile = BTreeSet::new();
    let make = || {
        let map = MapInput {
            stem: "town",
            number: 70,
            references: &sources,
            objects: &spots,
            skipped: &[],
            written: &all,
            copies: &[],
            triggers: &[],
            hostile: &hostile,
        };
        placements::build("test", "fallout3", &[map], &BTreeSet::new()).to_text()
    };
    assert_eq!(make(), make());
}
