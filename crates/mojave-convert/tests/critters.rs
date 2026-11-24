//! The M4 exit check on synthetic data: the mini town's sheriff (a
//! skeleton, a skinned body part, a stand and a walk animation, all built
//! by `nif::fixture`) becomes a critter sprite set, a prototype and a
//! critter on the map.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use mojave_convert::critters::{AnimationLevel, CritterOptions, DeathSampler, Fall};
use mojave_convert::{critters, pipeline};
use vault_formats::frm::FrmArchive;

#[test]
fn an_actor_becomes_a_sprite_set_and_a_critter_on_the_map() {
    let fixture = common::fixture();
    let mut inputs = fixture.inputs("out", 2);
    // The combat level (a slice key; the run's default is the first
    // wave's minimal level).
    inputs.slice.animations = Some("combat".to_owned());
    let summary = pipeline::all(&inputs).expect("the pipeline runs");
    assert!(summary.critters.sets >= 1, "{:?}", summary.critters);
    assert_eq!(summary.critters.failed, 0);
    assert_eq!(summary.critters.rendered, summary.critters.sets);

    let index = critters::read_index(&inputs).expect("the index reads back");
    let rendered: Vec<_> = index.iter().filter(|e| e.set.is_some()).collect();
    assert_eq!(
        rendered.len() + index.iter().filter(|e| e.error.is_some()).count(),
        index.len(),
        "every actor has a set or a reason"
    );
    assert!(!rendered.is_empty());

    for entry in &rendered {
        let key = entry.set.as_deref().unwrap();
        let files = critters::read_set(&inputs, key).expect("the set reads back");
        // Every animation a fight requires is there, for the pistol the
        // sheriff carries too.
        let meta = critters::read_meta(&inputs, key).unwrap();
        let weapons: Vec<(emit::WeaponCode, [emit::AttackMode; 2])> =
            if meta.weapons.contains_key("h") {
                vec![(
                    emit::WeaponCode::Pistol,
                    [emit::AttackMode::Single, emit::AttackMode::None],
                )]
            } else {
                Vec::new()
            };
        let required: Vec<String> = emit::animation::combat_requirements(&weapons)
            .iter()
            .filter(|r| r.need == emit::Need::Required)
            .map(|r| r.suffix().unwrap())
            .collect();
        let mut suffixes = vec![critters::STAND.to_owned(), critters::WALK.to_owned()];
        suffixes.extend(required);
        for suffix in &suffixes {
            let suffix = suffix.as_str();
            let frm = FrmArchive::parse(&files[suffix]).unwrap_or_else(|e| panic!("{suffix}: {e}"));
            for direction in 0..6 {
                assert!(
                    frm.frame(direction, 0).is_some(),
                    "{suffix} direction {direction}"
                );
            }
        }
        // The walk moves along each facing's hex step; the stand does not move.
        let walk = FrmArchive::parse(&files[critters::WALK]).unwrap();
        let stand = FrmArchive::parse(&files[critters::STAND]).unwrap();
        for (direction, step) in emit::HEX_STEPS.iter().enumerate() {
            let sum = |frm: &FrmArchive| {
                let mut total = [0i32; 2];
                let mut frame = 0;
                while let Some(record) = frm.frame(direction, frame) {
                    total[0] += i32::from(record.displacement[0]);
                    total[1] += i32::from(record.displacement[1]);
                    frame += 1;
                }
                total
            };
            assert_eq!(sum(&stand), [0, 0], "the stand of direction {direction}");
            let walked = sum(&walk);
            assert_eq!(
                walked[0] * step[1],
                walked[1] * step[0],
                "direction {direction} walks along its hex step: {walked:?}"
            );
            assert!(
                walked[0] * step[0] + walked[1] * step[1] > 0,
                "direction {direction} walks forwards: {walked:?}"
            );
        }
    }

    // On the map: every placed critter is in the layer the builder verified.
    let report = std::fs::read_to_string(inputs.out.join("map-report.md")).unwrap();
    assert!(report.contains("## Characters"));
    assert_eq!(summary.map.critters, 1, "the sheriff stands outdoors");
    let files = common::tree(&inputs.mod_dir());
    assert!(
        files
            .iter()
            .any(|(path, _)| path.starts_with("art/critters/mj") && path.ends_with("aa.frm")),
        "the layer holds the stand art"
    );
    assert!(
        files
            .iter()
            .any(|(path, _)| path == "art/critters/critters.lst")
    );
    assert!(
        files
            .iter()
            .any(|(path, _)| path.starts_with("proto/critters/"))
    );

    // A second run renders nothing and changes nothing.
    let before = common::tree(&inputs.out.join("critters"));
    let again = critters::critters(&inputs).expect("the stage reruns");
    assert_eq!(again.rendered, 0);
    assert_eq!(again.reused, summary.critters.sets);
    assert_eq!(common::tree(&inputs.out.join("critters")), before);
}

/// Runs only the extract and the critter stage.
fn critter_run(
    fixture: &common::Fixture,
    out: &str,
    jobs: usize,
    options: &CritterOptions,
) -> (mojave_convert::Inputs, critters::CrittersSummary) {
    let inputs = fixture.inputs(out, jobs);
    pipeline::extract(&inputs).expect("the extract runs");
    let summary = critters::critters_with(&inputs, options).expect("the stage runs");
    (inputs, summary)
}

/// The one sheriff set and its files.
fn sheriff_set(inputs: &mojave_convert::Inputs) -> (String, BTreeMap<String, Vec<u8>>) {
    let index = critters::read_index(inputs).unwrap();
    let entry = index
        .iter()
        .find(|e| e.editor_id.as_deref() == Some("MTSheriff"))
        .expect("the sheriff is indexed");
    let key = entry
        .set
        .clone()
        .unwrap_or_else(|| panic!("{:?}", entry.error));
    let files = critters::read_set(inputs, &key).unwrap();
    (key, files)
}

fn displacements(frm: &FrmArchive, direction: usize) -> Vec<[i16; 2]> {
    (0..frm.frame_count())
        .map(|k| frm.frame(direction, k).unwrap().displacement)
        .collect()
}

#[test]
fn the_combat_level_renders_real_animations_and_reports_the_rest() {
    let fixture = common::fixture();
    let (inputs, summary) = critter_run(&fixture, "out", 2, &CritterOptions::default());
    assert_eq!(summary.failed, 0, "{summary:?}");
    let (key, files) = sheriff_set(&inputs);
    let meta = critters::read_meta(&inputs, &key).unwrap();
    assert_eq!(meta.schema, critters::SET_SCHEMA);
    assert_eq!(meta.level, Some(AnimationLevel::Combat));
    // Real from the fixture's files: run, punch, the fall back and what is
    // made of it, the pistol's stand, walk (the walk under the aim), shot,
    // point and unpoint.
    for suffix in [
        "aa", "ab", "at", "aq", "ba", "cj", "ra", "ha", "hb", "hj", "hh", "hi",
    ] {
        assert!(
            meta.real.contains_key(suffix),
            "{suffix}: {:?}",
            meta.real.keys()
        );
    }
    // Still placeholders only where the source has nothing: no dodge, no
    // hit, no fall front, no pistol equip.
    for suffix in ["an", "ao", "bb", "ch", "rb", "hc", "he"] {
        assert!(
            meta.placeholders.iter().any(|p| p == suffix),
            "{suffix}: {:?}",
            meta.placeholders
        );
    }
    for suffix in meta.real.keys().chain(&meta.placeholders) {
        assert!(files.contains_key(suffix), "{suffix}");
    }
    assert_eq!(meta.animations.len(), files.len());
    let report = std::fs::read_to_string(inputs.critters_dir().join("report.md")).unwrap();
    assert!(report.contains("MTSheriff"), "{report}");
    assert!(!summary.lines.is_empty());

    // The punch's action frame is the frame after its `Hit` key: 0.3 s in,
    // frame 3 of 9 at 10 a second.
    let punch = FrmArchive::parse(&files["aq"]).unwrap();
    assert_eq!(punch.frame_count(), 9);
    assert_eq!(punch.action_frame, 4);
    assert_eq!(meta.real["aq"].action_frame, 4);

    // The fall is baked: no offsets in any direction, and the body is
    // drawn away from where it stood (the fixture's root moves back).
    let fall = FrmArchive::parse(&files["ba"]).unwrap();
    assert_eq!(fall.frame_count(), 11);
    for direction in 0..6 {
        assert!(displacements(&fall, direction).iter().all(|d| *d == [0, 0]));
    }
    // The corpse is one still; the get-up ends standing (on its frame 0
    // the body lies as in the fall's last frame).
    let corpse = FrmArchive::parse(&files["ra"]).unwrap();
    assert_eq!(corpse.frame_count(), 1);
    let get_up = FrmArchive::parse(&files["cj"]).unwrap();
    let last = fall.frame(2, fall.frame_count() - 1).unwrap();
    let first = get_up.frame(2, 0).unwrap();
    assert_eq!(
        (last.width, last.height, last.pixels()),
        (first.width, first.height, first.pixels())
    );

    // The run walks forwards along every hex step, a cycle at a time.
    let run = FrmArchive::parse(&files["at"]).unwrap();
    for (direction, step) in emit::HEX_STEPS.iter().enumerate() {
        let total = displacements(&run, direction)
            .iter()
            .fold([0i32; 2], |t, d| {
                [t[0] + i32::from(d[0]), t[1] + i32::from(d[1])]
            });
        assert_eq!(
            total[0] * step[1],
            total[1] * step[0],
            "{direction}: {total:?}"
        );
        assert!(total[0] * step[0] + total[1] * step[1] > 0);
    }

    // The stand and the walk are the minimal level's, byte for byte.
    let fixture_minimal = common::fixture();
    let (minimal_inputs, _) = critter_run(
        &fixture_minimal,
        "out",
        1,
        &CritterOptions {
            level: AnimationLevel::Minimal,
            ..CritterOptions::default()
        },
    );
    let (minimal_key, minimal_files) = sheriff_set(&minimal_inputs);
    assert_eq!(files["aa"], minimal_files["aa"]);
    assert_ne!(key, minimal_key);
    // The walk too, with the first wave's stride (the default).
    let fixture_exact = common::fixture();
    let (exact_inputs, _) = critter_run(
        &fixture_exact,
        "out",
        1,
        &CritterOptions {
            stride: critters::Stride::Exact,
            ..CritterOptions::default()
        },
    );
    let (_, exact_files) = sheriff_set(&exact_inputs);
    assert_eq!(exact_files["ab"], minimal_files["ab"]);
    assert_eq!(exact_files["aa"], minimal_files["aa"]);
}

#[test]
fn the_minimal_level_writes_the_first_waves_set() {
    let fixture = common::fixture();
    let options = CritterOptions {
        level: AnimationLevel::Minimal,
        ..CritterOptions::default()
    };
    let (inputs, summary) = critter_run(&fixture, "out", 2, &options);
    assert_eq!(summary.failed, 0);
    let (key, files) = sheriff_set(&inputs);
    let mut expected = vec![critters::STAND, critters::WALK];
    expected.extend(critters::PLACEHOLDERS);
    let mut got: Vec<&str> = files.keys().map(String::as_str).collect();
    got.sort_unstable();
    expected.sort_unstable();
    assert_eq!(got, expected);
    // The first wave's `set.json`: none of the new fields.
    let text = std::fs::read_to_string(inputs.critters_dir().join(&key).join("set.json")).unwrap();
    for field in [
        "schema",
        "level",
        "real",
        "placeholders",
        "weapons",
        "bytes",
    ] {
        assert!(!text.contains(&format!("\"{field}\"")), "{field} in {text}");
    }
    // No per-animation cache and no report at this level.
    assert!(!inputs.critters_dir().join("anims").exists());
    assert!(!inputs.critters_dir().join("report.md").exists());
    // The nine stills are one frame of the stand.
    let still = FrmArchive::parse(&files["ba"]).unwrap();
    assert_eq!(still.frame_count(), 1);
}

#[test]
fn sets_are_the_same_whatever_the_jobs_and_a_rerun_reuses_them() {
    let fixture = common::fixture();
    let options = CritterOptions::default();
    let (a, _) = critter_run(&fixture, "one", 1, &options);
    let (b, _) = critter_run(&fixture, "two", 3, &options);
    let tree = |inputs: &mojave_convert::Inputs| common::tree(&inputs.critters_dir());
    assert_eq!(tree(&a), tree(&b));
    // A second run renders nothing; a set removed is put back from the
    // per-animation cache with the same bytes.
    let before = tree(&a);
    let again = critters::critters_with(&a, &options).unwrap();
    assert_eq!(again.rendered, 0);
    let (key, _) = sheriff_set(&a);
    std::fs::remove_dir_all(a.critters_dir().join(&key)).unwrap();
    let third = critters::critters_with(&a, &options).unwrap();
    assert_eq!(third.rendered, 1);
    assert!(
        third.lines[0].contains("(0 rendered now)"),
        "{:?}",
        third.lines
    );
    assert_eq!(tree(&a), before);
}

/// A death sampler for tests: the skeleton's stand pose slides 30 units
/// back along −Y and sinks 20 units over five samples.
struct Slide;

impl DeathSampler for Slide {
    fn fall(
        &self,
        _skeleton: &nif::Skeleton,
        start: &nif::Pose,
        fall: Fall,
        _seed: u64,
        _sample_rate: f32,
    ) -> Result<Vec<nif::Pose>, String> {
        if fall == Fall::Front {
            return Err("no front falls here".to_owned());
        }
        Ok((0..5)
            .map(|k| {
                let mut pose = start.clone();
                for m in &mut pose.world {
                    m[1][3] -= 7.5 * k as f32;
                    m[2][3] -= 5.0 * k as f32;
                }
                pose
            })
            .collect())
    }

    fn name(&self) -> String {
        "slide".to_owned()
    }
}

#[test]
fn falls_come_from_the_death_sampler_and_are_baked_and_kept_above_ground() {
    let fixture = common::fixture();
    let options = CritterOptions {
        sampler: Arc::new(Slide),
        ..CritterOptions::default()
    };
    let (inputs, summary) = critter_run(&fixture, "out", 1, &options);
    assert_eq!(summary.failed, 0);
    let (key, files) = sheriff_set(&inputs);
    let meta = critters::read_meta(&inputs, &key).unwrap();
    assert!(
        meta.real["ba"].source.starts_with("slide"),
        "{:?}",
        meta.real["ba"]
    );
    // The front fall failed in the sampler and the source has none: a
    // still, and the reason is in the summary line.
    assert!(meta.placeholders.iter().any(|p| p == "bb"));
    assert!(
        summary.lines[0].contains("no front falls here"),
        "{:?}",
        summary.lines
    );
    let fall = FrmArchive::parse(&files["ba"]).unwrap();
    assert_eq!(fall.frame_count(), 5);
    for direction in 0..6 {
        assert!(displacements(&fall, direction).iter().all(|d| *d == [0, 0]));
        // Sinking is undone: every frame's bottom edge stays where the
        // standing frame's is, while the figure moves across the screen.
        let rows: Vec<u16> = (0..5)
            .map(|k| fall.frame(direction, k).unwrap().height)
            .collect();
        assert!(rows.iter().all(|h| *h > 0), "{direction}: {rows:?}");
    }
}

/// Real data, run by hand: the critter stage on an extract of the owner's
/// install, at the level and look given, and the gallery's character
/// pages. Writes only under `MOJAVE_OUT`.
///
/// ```text
/// MOJAVE_OUT=out/characters/fo3-megaton  MOJAVE_SLICE=slices/fo3/megaton.toml \
/// MOJAVE_GAME=fo3 MOJAVE_GAME_DIR=<install>/Data  MOJAVE_F2=<fallout 2 dir> \
/// MOJAVE_ONLY=MegatonSettler01,LucasSimms  MOJAVE_LEVEL=combat  MOJAVE_LOOK=default \
///   cargo test -p mojave-convert --release --test critters real_characters -- --ignored --nocapture
/// ```
#[test]
#[ignore = "needs the owner's installs"]
fn real_characters() {
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"));
    let out = std::path::PathBuf::from(var("MOJAVE_OUT"));
    let game_dir = std::path::PathBuf::from(var("MOJAVE_GAME_DIR"));
    let install = match var("MOJAVE_GAME").as_str() {
        "fo3" => mojave_convert::Install::fallout3(&game_dir),
        _ => mojave_convert::Install::new_vegas(&game_dir),
    };
    let slice = std::env::var("MOJAVE_SLICE").ok().filter(|s| !s.is_empty());
    let jobs = std::env::var("MOJAVE_JOBS")
        .ok()
        .and_then(|j| j.parse().ok())
        .unwrap_or(2);
    let inputs = mojave_convert::Inputs::from_args(
        &install,
        slice.as_deref(),
        std::path::Path::new(&var("MOJAVE_F2")),
        &out,
        jobs,
    )
    .expect("inputs");
    let only: Vec<String> = std::env::var("MOJAVE_ONLY")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let level = std::env::var("MOJAVE_LEVEL")
        .unwrap_or_else(|_| "combat".to_owned())
        .parse()
        .unwrap();
    let look = match std::env::var("MOJAVE_LOOK").as_deref() {
        Ok("readable") => critters::FigureLook::readable(),
        _ => critters::FigureLook::default(),
    };
    let stride = match std::env::var("MOJAVE_STRIDE").as_deref() {
        Ok("whole") => critters::Stride::WholeHexes,
        Ok("matched") => critters::Stride::Matched,
        _ => critters::Stride::Exact,
    };
    let deaths = match std::env::var("MOJAVE_DEATHS").as_deref() {
        Ok("game") => critters::DeathSource::Game,
        _ => critters::DeathSource::Ragdoll,
    };
    let options = CritterOptions {
        level,
        look,
        stride,
        deaths,
        only: only.clone(),
        ..CritterOptions::default()
    };
    let started = std::time::Instant::now();
    let summary = critters::critters_with(&inputs, &options).expect("the stage runs");
    println!(
        "critters in {:.1} s: {summary:#?}",
        started.elapsed().as_secs_f32()
    );
    let pages = mojave_convert::gallery::character_pages(
        &inputs,
        &mojave_convert::gallery::CharacterPages {
            only,
            zoom: std::env::var("MOJAVE_ZOOM")
                .ok()
                .and_then(|z| z.parse().ok())
                .unwrap_or(2),
            suffixes: std::env::var("MOJAVE_ROWS")
                .unwrap_or_default()
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect(),
            tag: std::env::var("MOJAVE_TAG").unwrap_or_default(),
            direction: std::env::var("MOJAVE_DIRECTION")
                .ok()
                .and_then(|d| d.parse().ok())
                .unwrap_or(2),
            ..mojave_convert::gallery::CharacterPages::default()
        },
    )
    .expect("the pages are written");
    for page in pages {
        println!("page: {}", page.display());
    }
}
