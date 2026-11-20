//! Real-data check of the actor stores in an extracted MIR directory:
//! skeletons and animations converted, the human skeleton's bone count,
//! the stand and walk durations, how many NPC documents link all three,
//! and the posed height of one exterior townsperson. Prints tallies only
//! (counts, durations, extents); writes nothing.
//!
//! ```text
//! MIR_OUT=out/m4-mir cargo run --release -p mir --example characters
//! ```

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use mir::{ActorDoc, ActorKind, AssetStatus, Mir, RefKind};

fn extent(meshes: &[nif::StaticMesh]) -> Option<([f32; 3], [f32; 3])> {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in meshes
        .iter()
        .flat_map(|m| &m.primitives)
        .flat_map(|p| &p.positions)
    {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    lo[0].is_finite().then_some((lo, hi))
}

fn check(mir: &Mir) -> Result<(), mir::MirError> {
    let manifest = mir.manifest();
    let count = |entries: &std::collections::BTreeMap<String, mir::AssetEntry>| {
        let ok = entries
            .values()
            .filter(|e| e.status == AssetStatus::Converted)
            .count();
        (entries.len(), ok, entries.len() - ok)
    };
    let (paths, ok, failed) = count(&manifest.skeletons);
    println!(
        "skeletons: {paths} paths, {ok} converted, {failed} failed, {} distinct files",
        manifest.skeleton_info.len()
    );
    let (paths, ok, failed) = count(&manifest.animations);
    println!(
        "animations: {paths} paths, {ok} converted, {failed} failed, {} distinct files",
        manifest.animation_info.len()
    );
    for entry in manifest
        .skeletons
        .values()
        .chain(manifest.animations.values())
    {
        if entry.status != AssetStatus::Converted {
            println!("  failure: {:?}", entry.status);
        }
    }
    if let Some(hash) = mir.skeleton_hash(mir::HUMAN_SKELETON) {
        let skeleton = mir.skeleton(hash)?;
        println!(
            "human skeleton: {} bones, depth {}",
            skeleton.bones.len(),
            skeleton.depth()
        );
    } else {
        println!("human skeleton: not converted");
    }
    for (label, path) in [
        ("stand", mir::HUMAN_STAND),
        ("male walk", mir::HUMAN_WALK_MALE),
        ("female walk", mir::HUMAN_WALK_FEMALE),
    ] {
        match mir.animation_hash(path) {
            Some(hash) => {
                let a = mir.animation(hash)?;
                println!(
                    "{label}: {:.3} s, {} tracks, looping {}",
                    a.duration,
                    a.tracks.len(),
                    a.looping
                );
            }
            None => println!("{label}: not converted"),
        }
    }

    let mut docs: Vec<ActorDoc> = Vec::new();
    for entry in mir.actors() {
        if let Some(doc) = mir.actor(&entry.form)? {
            docs.push(doc);
        }
    }
    let npcs: Vec<&ActorDoc> = docs.iter().filter(|d| d.kind == ActorKind::Npc).collect();
    let linked = |d: &ActorDoc| d.skeleton.is_some() && d.animations.is_some();
    println!(
        "NPC documents with skeleton, stand and walk hashes: {} of {}",
        npcs.iter().filter(|d| linked(d)).count(),
        npcs.len()
    );
    let creatures = docs.len() - npcs.len();
    println!(
        "creature documents with a skeleton hash: {} of {creatures}",
        docs.iter()
            .filter(|d| d.kind == ActorKind::Creature && d.skeleton.is_some())
            .count()
    );

    // One townsperson: the first enabled exterior NPC reference whose
    // document links everything.
    let mut chosen = None;
    'cells: for key in mir.cells() {
        let Some(cell) = mir.cell(&key.worldspace, key.x, key.y)? else {
            continue;
        };
        for r in &cell.references {
            if r.kind != RefKind::Npc || r.flags.initially_disabled {
                continue;
            }
            if let Some(doc) = npcs.iter().find(|d| d.form == r.base && linked(d)) {
                chosen = Some((*doc).clone());
                break 'cells;
            }
        }
    }
    let Some(doc) = chosen else {
        println!("no enabled exterior NPC with every hash");
        return Ok(());
    };
    let skeleton = mir.skeleton(doc.skeleton.as_deref().unwrap_or_default())?;
    let Some(animations) = &doc.animations else {
        return Ok(());
    };
    let walk = mir.animation(&animations.walk)?;
    let stand = mir.animation(&animations.stand)?;
    let mut parts = Vec::new();
    for part in &doc.parts {
        if let Some(hash) = &part.mesh {
            parts.push((part.role, mir.mesh(hash)?));
        }
    }
    let skinned = parts
        .iter()
        .filter(|(_, m)| m.primitives.iter().any(|p| p.skin.is_some()))
        .count();
    println!(
        "townsperson: {} parts listed, {} converted, {skinned} skinned",
        doc.parts.len(),
        parts.len()
    );
    for (label, animation, times) in [
        ("walk", &walk, [0.0, 0.25, 0.5, 0.75]),
        ("stand", &stand, [0.0, 0.25, 0.5, 0.75]),
    ] {
        for f in times {
            let time = f * animation.duration;
            let pose = nif::pose(&skeleton, animation, time);
            let posed: Vec<nif::StaticMesh> = parts
                .iter()
                .filter(|(_, m)| m.primitives.iter().any(|p| p.skin.is_some()))
                .map(|(_, m)| nif::skin(m, &skeleton, &pose))
                .collect();
            let finite = posed
                .iter()
                .flat_map(|m| &m.primitives)
                .flat_map(|p| &p.positions)
                .all(|p| p.iter().all(|v| v.is_finite()));
            let left = posed
                .iter()
                .flat_map(|m| &m.primitives)
                .filter(|p| p.skin.is_some())
                .count();
            match extent(&posed) {
                Some((lo, hi)) => println!(
                    "  {label} at {time:.2} s: height {:.1} (z {:.1}..{:.1}), width {:.1}, \
                     depth {:.1}, travel ({:.1}, {:.1}), finite {finite}, skins left {left}",
                    hi[2] - lo[2],
                    lo[2],
                    hi[2],
                    hi[0] - lo[0],
                    hi[1] - lo[1],
                    pose.travel[0],
                    pose.travel[1]
                ),
                None => println!("  {label} at {time:.2} s: no geometry"),
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let Some(out) = env::var_os("MIR_OUT").map(PathBuf::from) else {
        eprintln!("MIR_OUT is not set");
        return ExitCode::FAILURE;
    };
    let result = Mir::open(&out).and_then(|mir| check(&mir));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("check failed: {error}");
            ExitCode::FAILURE
        }
    }
}
