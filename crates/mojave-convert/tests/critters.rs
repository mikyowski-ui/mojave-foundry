//! The M4 exit check on synthetic data: the mini town's sheriff (a
//! skeleton, a skinned body part, a stand and a walk animation, all built
//! by `nif::fixture`) becomes a critter sprite set, a prototype and a
//! critter on the map.

mod common;

use mojave_convert::{critters, pipeline};
use vault_formats::frm::FrmArchive;

#[test]
fn an_actor_becomes_a_sprite_set_and_a_critter_on_the_map() {
    let fixture = common::fixture();
    let inputs = fixture.inputs("out", 2);
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
        let mut suffixes = vec![critters::STAND, critters::WALK];
        suffixes.extend(critters::PLACEHOLDERS);
        for suffix in suffixes {
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
