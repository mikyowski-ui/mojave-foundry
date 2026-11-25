//! A check of a real run's placement index against its written maps, by
//! hand: `T7_OUT=<out dir> cargo test -p mojave-convert --test index_check
//! -- --ignored --nocapture`. It reads `<out>/map-placements.json` and the
//! layer under `<out>/mods/mojave`, checks every placed entry's object id
//! (elevation, hex, PID, hidden flag, critter, lock bit) against the
//! written map, and prints the counts the Pass 2 report quotes. It needs
//! no game files of its own: the run it reads is the owner's.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use mojave_convert::placements::{self, PlacedKind, UnplacedReason};

#[test]
#[ignore = "reads a real run: T7_OUT=<out dir>"]
fn a_real_index_matches_its_written_maps() {
    let out = PathBuf::from(std::env::var_os("T7_OUT").expect("T7_OUT names the run's out dir"));
    let index = placements::read(&out)
        .expect("reads")
        .expect("the stage wrote the index");
    let (_, maps) = placements::read_written_maps(&out.join("mods").join("mojave")).expect("maps");
    let mut seen = BTreeSet::new();
    let mut bad = Vec::new();
    let mut matched = 0usize;
    for entry in &index.placed {
        let Some(id) = entry.object_id else {
            bad.push(format!("{:?}: no object id", entry.reference));
            continue;
        };
        let Some(map) = maps.iter().find(|m| m.stem == entry.map) else {
            bad.push(format!("{:?}: no map {}", entry.reference, entry.map));
            continue;
        };
        let Some(object) = map
            .objects
            .iter()
            .find(|o| o.id == id && o.elevation == entry.elevation)
        else {
            bad.push(format!("{:?}: no object {id}", entry.reference));
            continue;
        };
        let ok = object.hex == entry.hex
            && Some(object.pid) == entry.pid
            && object.hidden == entry.hidden
            && object.critter == entry.critter
            && entry
                .lock
                .as_ref()
                .is_none_or(|l| l.locked == object.locked)
            && seen.insert((entry.map.clone(), id));
        if ok {
            matched += 1;
        } else {
            bad.push(format!("{:?}: object {id} disagrees", entry.reference));
        }
    }
    println!(
        "placed entries: {}; matched to their written object: {matched}; disagreeing: {}",
        index.placed.len(),
        bad.len()
    );
    for line in bad.iter().take(20) {
        println!("  {line}");
    }
    let mut per_map: BTreeMap<&str, usize> = BTreeMap::new();
    for entry in &index.placed {
        *per_map.entry(&entry.map).or_default() += 1;
    }
    println!("placed per map: {per_map:?}");
    let all_objects: usize = maps.iter().map(|m| m.objects.len()).sum();
    println!("top-level objects written (all maps): {all_objects}");

    let children = |hidden: bool| {
        index
            .placed
            .iter()
            .filter(move |p| p.hidden == hidden && p.group.is_some() && p.presence_of.is_none())
    };
    let hidden: Vec<_> = children(true).collect();
    let mut by_kind: BTreeMap<&str, usize> = BTreeMap::new();
    for p in &hidden {
        *by_kind.entry(p.kind.label()).or_default() += 1;
    }
    println!(
        "hidden group members placed: {} ({by_kind:?}); visible group members placed: {}",
        hidden.len(),
        children(false).count()
    );
    let groups: BTreeSet<&str> = index
        .placed
        .iter()
        .chain(std::iter::empty())
        .filter_map(|p| p.group.as_ref().map(|g| g.root.as_str()))
        .chain(
            index
                .unplaced
                .iter()
                .filter_map(|u| u.group.as_ref().map(|g| g.root.as_str())),
        )
        .collect();
    println!("enable groups seen (distinct roots): {}", groups.len());
    let left: Vec<_> = index
        .unplaced
        .iter()
        .filter(|u| u.group.is_some() && u.reason != UnplacedReason::NotPlaced)
        .collect();
    println!(
        "group members left off with a reason: {} ({})",
        left.len(),
        left.iter()
            .map(|u| format!("{}: {:?}", u.reference, u.detail))
            .take(6)
            .collect::<Vec<_>>()
            .join("; ")
    );
    let copies: Vec<_> = index
        .placed
        .iter()
        .filter(|p| p.presence_of.is_some())
        .collect();
    println!("presence copies: {}", copies.len());
    for c in &copies {
        println!(
            "  {:?} on {} hex {} (original {:?})",
            c.reference, c.map, c.hex, c.presence_of
        );
    }
    let locked = |kind: PlacedKind| {
        index
            .placed
            .iter()
            .filter(|p| p.kind == kind && p.lock.is_some())
            .collect::<Vec<_>>()
    };
    for kind in [PlacedKind::Door, PlacedKind::Container] {
        let found = locked(kind);
        println!(
            "locked {}s in the source: {} (written locked: {}); keyed: {}",
            kind.label(),
            found.len(),
            found
                .iter()
                .filter(|p| p.lock.as_ref().is_some_and(|l| l.locked))
                .count(),
            found
                .iter()
                .filter(|p| p.lock.as_ref().is_some_and(|l| l.key.is_some()))
                .count()
        );
        for p in found {
            println!(
                "  {:?} on {} hex {}: {:?}",
                p.reference, p.map, p.hex, p.lock
            );
        }
    }
    let unplaced_locks = index.unplaced.iter().filter(|u| u.lock.is_some()).count();
    println!("locks on references not placed: {unplaced_locks}");
    println!("triggers: {}", index.triggers.len());
    for t in &index.triggers {
        println!(
            "  {} on {} elevation {} hex {} shape {:?} radius {:?}",
            t.reference, t.map, t.elevation, t.hex, t.shape, t.radius
        );
    }
    let linked = index
        .placed
        .iter()
        .filter(|p| p.linked_ref.is_some())
        .count()
        + index
            .unplaced
            .iter()
            .filter(|u| u.linked_ref.is_some())
            .count()
        + index
            .triggers
            .iter()
            .filter(|t| t.linked_ref.is_some())
            .count();
    println!("references with a linked reference listed: {linked}");
    let hostile = index.placed.iter().filter(|p| p.hostile).count();
    let packets: BTreeSet<Option<i32>> = index
        .placed
        .iter()
        .filter(|p| p.hostile)
        .map(|p| p.ai_packet)
        .collect();
    println!("hostile people: {hostile}, their packets {packets:?}");
    println!("notes: {:?}", index.notes);
    assert!(bad.is_empty(), "{bad:?}");
}
