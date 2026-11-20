//! The skeleton and animation stores, and a character's whole chain from
//! the MIR (skeleton, animation, skinned part, pose, skin), on the
//! synthetic install of `mir::fixture::mini_town`.

use std::fs;
use std::path::PathBuf;

use mir::fixture::{
    MiniTown, SKINNED_PART, STAND_DURATION, mini_town, stand_kf, textured_skinned_body,
};
use mir::{ActorDoc, AssetStatus, ExtractSummary, Mir, MirError, extract};
use nif::fixture::{
    BODY_VERTICES, SKIN_BONES, WALK_DURATION, WALK_STRIDE, skeleton_nif, skinned_body, walk_kf,
};

struct Run {
    _install: tempfile::TempDir,
    _out: tempfile::TempDir,
    town: MiniTown,
    out: PathBuf,
    summary: ExtractSummary,
}

fn run(jobs: usize) -> Run {
    let install = tempfile::tempdir().unwrap();
    let out_dir = tempfile::tempdir().unwrap();
    let town = mini_town(install.path());
    let out = out_dir.path().join("mir");
    let summary = extract(&town.options(&out, jobs)).unwrap();
    Run {
        _install: install,
        _out: out_dir,
        town,
        out,
        summary,
    }
}

fn actor(mir: &Mir, form: &mir::FormKey) -> ActorDoc {
    mir.actor(form).unwrap().unwrap()
}

#[test]
fn skeletons_and_animations_are_stored_and_linked() {
    let r = run(2);
    let mir = Mir::open(&r.out).unwrap();
    let manifest = mir.manifest();

    // Two skeletons (the human one and the dog's), two animations.
    assert_eq!(r.summary.skeletons.referenced, 2);
    assert_eq!(r.summary.skeletons.converted, 2);
    assert_eq!(r.summary.skeletons.failed, 0);
    assert_eq!(r.summary.animations.referenced, 2);
    assert_eq!(r.summary.animations.converted, 2);
    assert_eq!(r.summary.animations.failed, 0);
    assert_eq!(manifest.skeleton_info.len(), 2);
    assert_eq!(manifest.animation_info.len(), 2);
    for hash in manifest.skeleton_info.keys() {
        assert!(
            r.out
                .join("skeletons")
                .join(format!("{hash}.json"))
                .is_file()
        );
    }
    for hash in manifest.animation_info.keys() {
        assert!(
            r.out
                .join("animations")
                .join(format!("{hash}.json"))
                .is_file()
        );
    }

    let sheriff = actor(&mir, &r.town.animated_actor);
    assert_eq!(r.town.animated_actor, r.town.sheriff);
    assert!(sheriff.notes.is_empty(), "{:?}", sheriff.notes);

    // The skeleton: the bones of the nif fixture, named by the source hash.
    let skeleton_hash = sheriff.skeleton.as_deref().unwrap();
    let skeleton = mir.skeleton(skeleton_hash).unwrap();
    let expected = nif::extract_skeleton(&nif::parse(&skeleton_nif()).unwrap()).unwrap();
    assert_eq!(skeleton, expected);
    assert_eq!(skeleton.bones.len(), 6);
    assert_eq!(
        mir::content_hash(&mir::fixture::human_skeleton()),
        skeleton_hash
    );
    let info = mir.skeleton_info(skeleton_hash).unwrap();
    assert_eq!((info.bones, info.depth, info.roots), (6, 5, 1));
    assert_eq!(info.paths, [mir::HUMAN_SKELETON]);
    assert_eq!(
        mir.skeleton_hash("Characters/_Male/Skeleton.NIF"),
        Some(skeleton_hash)
    );
    assert_eq!(
        manifest.skeletons[mir::HUMAN_SKELETON].status,
        AssetStatus::Converted
    );

    // The animations: the stand and the nif fixture's walk.
    let animations = sheriff.animations.as_ref().unwrap();
    let stand = mir.animation(&animations.stand).unwrap();
    let walk = mir.animation(&animations.walk).unwrap();
    assert_eq!(stand, nif::parse_animation(&stand_kf()).unwrap());
    assert_eq!(walk, nif::parse_animation(&walk_kf()).unwrap());
    assert_eq!(stand.duration, STAND_DURATION);
    assert_eq!(walk.duration, WALK_DURATION);
    assert_eq!(
        mir.animation_hash(mir::HUMAN_STAND),
        Some(animations.stand.as_str())
    );
    assert_eq!(
        mir.animation_hash("characters\\_male\\locomotion\\male\\mtforward.kf"),
        Some(animations.walk.as_str())
    );
    let walk_info = mir.animation_info(&animations.walk).unwrap();
    assert_eq!(walk_info.name, "Walk");
    assert_eq!(walk_info.duration, WALK_DURATION);
    assert!(walk_info.looping);
    assert_eq!(walk_info.tracks as usize, walk.tracks.len());
    assert_eq!(walk_info.text_keys, 2);
    assert_eq!(walk_info.accum_root, "Bip01");
    assert_eq!(walk_info.paths, [mir::HUMAN_WALK_MALE]);
    let stand_info = mir.animation_info(&animations.stand).unwrap();
    assert_eq!((stand_info.name.as_str(), stand_info.tracks), ("Idle", 3));

    // The deputy shares them: same skeleton, and the male walk for want of
    // a female one.
    let deputy = actor(&mir, &r.town.deputy);
    assert_eq!(deputy.skeleton.as_deref(), Some(skeleton_hash));
    assert_eq!(deputy.animations.as_ref(), Some(animations));

    // The dog: its own skeleton (a one-node file), no human animations.
    let dog = actor(&mir, &r.town.dog);
    let dog_skeleton = mir.skeleton(dog.skeleton.as_deref().unwrap()).unwrap();
    assert_eq!(dog_skeleton.bones.len(), 1);
    assert!(dog.animations.is_none());
    assert_eq!(dog.notes.len(), 1, "{:?}", dog.notes);

    // Bad hashes are errors, not panics.
    assert!(matches!(
        mir.skeleton("../manifest"),
        Err(MirError::Document {
            store: "skeletons",
            ..
        })
    ));
    assert!(matches!(
        mir.animation(&"0".repeat(64)),
        Err(MirError::Io { .. })
    ));
}

#[test]
fn the_skinned_part_reads_back_bound_to_the_skeleton() {
    let r = run(1);
    let mir = Mir::open(&r.out).unwrap();
    let sheriff = actor(&mir, &r.town.animated_actor);
    let part = sheriff
        .parts
        .iter()
        .find(|p| p.path == SKINNED_PART)
        .unwrap();
    assert_eq!(part.role, mir::PartRole::Body);
    let hash = part.mesh.as_deref().unwrap();
    assert_eq!(mir.manifest().mesh_info[hash].skinned_primitives, 1);
    let mesh = mir.mesh(hash).unwrap();
    assert_eq!(mesh.primitives.len(), 1);
    let primitive = &mesh.primitives[0];
    let skin = primitive.skin.as_ref().unwrap();
    assert!(primitive.skinned);
    let names: Vec<&str> = skin.bones.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, SKIN_BONES);
    assert_eq!(skin.weights.len(), BODY_VERTICES.len());
    let skeleton = mir.skeleton(sheriff.skeleton.as_deref().unwrap()).unwrap();
    assert!(names.iter().all(|n| skeleton.find(n).is_some()));

    // Geometry and skin as the nif fixture has them; only a texture added.
    let original = nif::extract_mesh(&nif::parse(&skinned_body()).unwrap()).unwrap();
    assert_eq!(primitive.positions, original.primitives[0].positions);
    assert_eq!(primitive.skin, original.primitives[0].skin);
    assert_eq!(
        primitive.material.textures[0],
        "textures\\minitown\\characters\\skin.dds"
    );
    assert_eq!(
        hash,
        mir::content_hash(&textured_skinned_body(
            "textures\\minitown\\characters\\skin.dds"
        ))
    );
}

#[test]
fn a_character_poses_and_skins_from_the_mir() {
    let r = run(2);
    let mir = Mir::open(&r.out).unwrap();
    let doc = actor(&mir, &r.town.animated_actor);
    let skeleton = mir.skeleton(doc.skeleton.as_deref().unwrap()).unwrap();
    let animations = doc.animations.as_ref().unwrap();
    let walk = mir.animation(&animations.walk).unwrap();
    let stand = mir.animation(&animations.stand).unwrap();
    let part = doc.parts.iter().find(|p| p.path == SKINNED_PART).unwrap();
    let mesh = mir.mesh(part.mesh.as_deref().unwrap()).unwrap();

    for (animation, time) in [
        (&walk, 0.25 * WALK_DURATION),
        (&walk, 0.5 * WALK_DURATION),
        (&stand, 0.5 * STAND_DURATION),
    ] {
        let pose = nif::pose(&skeleton, animation, time);
        assert_eq!(pose.world.len(), skeleton.bones.len());
        let posed = nif::skin(&mesh, &skeleton, &pose);
        assert_eq!(posed.primitives.len(), 1);
        for p in &posed.primitives {
            assert!(p.skin.is_none() && !p.skinned);
            assert_eq!(p.positions.len(), BODY_VERTICES.len());
            assert!(p.positions.iter().flatten().all(|v| v.is_finite()));
            assert!(p.normals.iter().flatten().flatten().all(|v| v.is_finite()));
        }
    }
    // Half a walk cycle travels half a stride; the stand stays put.
    let half = nif::pose(&skeleton, &walk, 0.5 * WALK_DURATION);
    assert!(
        (half.travel[1] - WALK_STRIDE / 2.0).abs() < 1e-3,
        "{:?}",
        half.travel
    );
    assert!(half.travel[0].abs() < 1e-3);
    let idle = nif::pose(&skeleton, &stand, 0.5 * STAND_DURATION);
    assert_eq!(idle.travel, [0.0, 0.0]);
    // The walk bends the arm at mid-cycle: the posed arm vertices move.
    let rest = nif::skin(&mesh, &skeleton, &nif::Pose::rest(&skeleton));
    let bent = nif::skin(&mesh, &skeleton, &half);
    let moved = rest.primitives[0]
        .positions
        .iter()
        .zip(&bent.primitives[0].positions)
        .any(|(a, b)| a.iter().zip(b).any(|(x, y)| (x - y).abs() > 1.0));
    assert!(moved);
}

#[test]
fn a_second_run_reuses_the_stores_and_jobs_do_not_matter() {
    let r = run(1);
    let again = extract(&r.town.options(&r.out, 3)).unwrap();
    assert_eq!(again.skeletons.converted, 0);
    assert_eq!(again.skeletons.reused, 2);
    assert_eq!(again.animations.converted, 0);
    assert_eq!(again.animations.reused, 2);
    let other = tempfile::tempdir().unwrap();
    let out = other.path().join("mir");
    extract(&r.town.options(&out, 4)).unwrap();
    for store in ["skeletons", "animations"] {
        let names = |dir: &std::path::Path| {
            let mut list: Vec<(String, Vec<u8>)> = fs::read_dir(dir.join(store))
                .unwrap()
                .map(|e| {
                    let e = e.unwrap();
                    (
                        e.file_name().to_string_lossy().into_owned(),
                        fs::read(e.path()).unwrap(),
                    )
                })
                .collect();
            list.sort();
            list
        };
        assert_eq!(names(&r.out), names(&out), "{store}");
    }
    assert_eq!(
        fs::read(r.out.join("manifest.json")).unwrap(),
        fs::read(out.join("manifest.json")).unwrap()
    );
}

#[test]
fn a_broken_skeleton_or_walk_is_noted_and_the_extract_goes_on() {
    let install = tempfile::tempdir().unwrap();
    let town = mini_town(install.path());
    // Loose files win over the archives.
    let male = town.data_dir.join("meshes/characters/_male");
    fs::create_dir_all(male.join("locomotion/male")).unwrap();
    fs::write(male.join("skeleton.nif"), b"not a nif file").unwrap();
    fs::write(male.join("locomotion/male/mtforward.kf"), b"not a kf file").unwrap();
    let out = install.path().join("out");
    let summary = extract(&town.options(&out, 2)).unwrap();
    assert_eq!(summary.skeletons.failed, 1);
    assert_eq!(summary.animations.failed, 1);
    assert_eq!(summary.animations.converted, 1);
    let mir = Mir::open(&out).unwrap();
    let manifest = mir.manifest();
    let skeleton = &manifest.skeletons[mir::HUMAN_SKELETON];
    assert_eq!(skeleton.status, AssetStatus::Undecodable);
    assert!(skeleton.reason.is_some() && skeleton.hash.is_some());
    assert!(mir.skeleton_hash(mir::HUMAN_SKELETON).is_none());
    let walk = &manifest.animations[mir::HUMAN_WALK_MALE];
    assert_eq!(walk.status, AssetStatus::Undecodable);
    assert!(mir.animation_hash(mir::HUMAN_STAND).is_some());

    let sheriff = actor(&mir, &town.sheriff);
    assert!(sheriff.skeleton.is_none());
    assert!(sheriff.animations.is_none());
    assert!(
        sheriff
            .notes
            .iter()
            .any(|n| n == &format!("skeleton {} is undecodable", mir::HUMAN_SKELETON)),
        "{:?}",
        sheriff.notes
    );
    assert!(
        sheriff
            .notes
            .iter()
            .any(|n| n == &format!("walk {} is undecodable", mir::HUMAN_WALK_MALE)),
        "{:?}",
        sheriff.notes
    );
    // The report lists both with their reasons.
    let text = mir.report_text().unwrap();
    assert!(
        text.contains("| skeleton | `meshes\\characters\\_male\\skeleton.nif` | undecodable |")
    );
    assert!(text.contains(
        "| animation | `meshes\\characters\\_male\\locomotion\\male\\mtforward.kf` | undecodable |"
    ));
    assert!(text.contains("NPCs with a skeleton and both a stand and a walk: 0 of 2."));
}

#[test]
fn the_report_counts_skeletons_and_animations() {
    let r = run(2);
    let text = fs::read_to_string(r.out.join("report.md")).unwrap();
    for line in [
        "### Skeletons and animations",
        "Skeletons: 2 paths, 2 converted (2 distinct files), 0 failed. Animations: 2 paths, 2 \
         converted (2 distinct files), 0 failed. NPCs with a skeleton and both a stand and a \
         walk: 2 of 2.",
        "| `meshes\\characters\\_male\\skeleton.nif` | 6 | 5 | 2 |",
        "| `meshes\\characters\\_male\\locomotion\\male\\mtforward.kf` | 1.000 | 4 | yes | 2 |",
        "| `meshes\\characters\\_male\\locomotion\\mtidle.kf` | 2.000 | 3 | yes | 2 |",
    ] {
        assert!(text.contains(line), "report lacks {line:?}\n{text}");
    }
    assert!(!text.contains("| Kind | Path | Status | Reason |"));
}
