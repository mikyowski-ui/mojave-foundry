//! The budget ladder.

use super::slicing::{crate_ref, house};
use super::*;
use crate::{ArtCategory, BudgetLadder, ClassKind, LadderKind, Role};

fn fence(id: u32, ground: &GroundSampler, xy: [f32; 2]) -> Reference {
    reference(
        id,
        "STAT",
        "meshes\\test\\fence.nif",
        "fence",
        ground,
        xy,
        0.0,
        0.0,
    )
}

fn street(ground: &GroundSampler) -> Vec<Reference> {
    vec![
        house(1, ground, [0.0, 0.0]),
        house(2, ground, [1500.0, 300.0]),
        fence(3, ground, [-900.0, 700.0]),
        fence(4, ground, [-900.0, -700.0]),
        crate_ref(5, ground, [700.0, 0.0], 0.0),
    ]
}

fn no_ladder() -> SlicerConfig {
    SlicerConfig {
        ladder: BudgetLadder {
            enabled: false,
            ..BudgetLadder::default()
        },
        ..SlicerConfig::default()
    }
}

#[test]
fn with_room_to_spare_the_ladder_changes_nothing() {
    let assets = TestAssets::standard();
    let ground = flat();
    let refs = street(&ground);
    let on = run(
        &assets,
        &refs,
        &ground,
        &big_budget(),
        &SlicerConfig::default(),
    )
    .unwrap();
    let off = run(&assets, &refs, &ground, &big_budget(), &no_ladder()).unwrap();
    assert!(on.report.ladder.is_empty());
    assert_eq!(on, off);
    // An exact fit is a fit.
    let walls = on.report.category(ArtCategory::Walls).used;
    let tight = Budget {
        walls,
        ..big_budget()
    };
    let exact = run(&assets, &refs, &ground, &tight, &SlicerConfig::default()).unwrap();
    assert!(exact.report.ladder.is_empty());
    assert_eq!(exact.placements, on.placements);
    assert!(!exact.report.to_string().contains("ladder"));
}

#[test]
fn the_first_step_that_fits_is_the_last_taken() {
    let assets = TestAssets::standard();
    let ground = flat();
    let refs = street(&ground);
    let plain = run(&assets, &refs, &ground, &big_budget(), &no_ladder()).unwrap();
    let longer = SlicerConfig {
        max_perimeter_run: BudgetLadder::default().perimeter_run,
        ..no_ladder()
    };
    let direct = run(&assets, &refs, &ground, &big_budget(), &longer).unwrap();
    let (w0, w1) = (
        plain.report.category(ArtCategory::Walls).used,
        direct.report.category(ArtCategory::Walls).used,
    );
    assert!(w1 < w0, "longer runs save entries: {w1} vs {w0}");
    let budget = Budget {
        walls: w1,
        ..big_budget()
    };
    // Without the ladder this is an overflow...
    assert!(matches!(
        run(&assets, &refs, &ground, &budget, &no_ladder()),
        Err(SliceError::FidBudget { .. })
    ));
    // ...with it, step 1 alone, giving what the longer runs give directly.
    let laddered = run(&assets, &refs, &ground, &budget, &SlicerConfig::default()).unwrap();
    let steps = &laddered.report.ladder;
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, LadderKind::LongerRuns);
    assert_eq!(steps[0].before[0], w0);
    assert_eq!(steps[0].after[0], w1);
    assert_eq!(steps[0].saved()[0], (w0 - w1) as i64);
    assert_eq!(laddered.art, direct.art);
    assert_eq!(laddered.placements, direct.placements);
    assert_eq!(laddered.blocked_hexes, direct.blocked_hexes);
    let text = laddered.report.to_string();
    assert!(text.contains("Budget ladder") && text.contains("longer perimeter runs"));
}

#[test]
fn a_budget_overflow_climbs_to_landmarks_and_fits() {
    let assets = TestAssets::standard();
    let ground = flat();
    let refs = street(&ground);
    let plain = run(&assets, &refs, &ground, &big_budget(), &no_ladder()).unwrap();
    let walls = plain.report.category(ArtCategory::Walls).used;
    // A third of the walls: coarser strips cannot get there.
    let budget = Budget {
        walls: walls / 3,
        scenery: 200,
        items: 10,
    };
    let error = run(&assets, &refs, &ground, &budget, &no_ladder()).unwrap_err();
    assert!(matches!(
        error,
        SliceError::FidBudget {
            category: ArtCategory::Walls,
            ..
        }
    ));
    let sliced = run(&assets, &refs, &ground, &budget, &SlicerConfig::default()).unwrap();
    let report = &sliced.report;
    assert!(report.fits());
    let kinds: Vec<LadderKind> = report.ladder.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        [
            LadderKind::LongerRuns,
            LadderKind::CoarserStrips,
            LadderKind::Landmarks
        ]
    );
    // Steps follow each other: each starts where the last ended.
    for pair in report.ladder.windows(2) {
        assert_eq!(pair[0].after, pair[1].before);
    }
    let landmarks = &report.ladder[2];
    assert!(
        landmarks
            .models
            .contains(&"meshes\\test\\house.nif".to_owned())
    );
    assert!(landmarks.saved()[0] > 0 && landmarks.saved()[1] < 0);
    assert!(report.classes[&ClassKind::Landmark].references >= 2);
    assert_eq!(
        report.landmarks.references,
        report.classes[&ClassKind::Landmark].references
    );
    // The houses are scenery pieces now.
    let houses: Vec<&Placement> = sliced
        .placements
        .iter()
        .filter(|p| p.reference == refs[0].form.to_string())
        .collect();
    assert!(!houses.is_empty() && houses.iter().all(|p| p.role == Role::Scenery));
    assert!(houses.len() <= SlicerConfig::default().landmark_max_pieces);
    let text = report.to_string();
    assert!(text.contains("largest sliced models as landmarks"));
    assert!(text.contains("Landmarks:"));
}

#[test]
fn the_last_step_demotes_the_smallest_and_an_unfixable_list_still_fails() {
    let assets = TestAssets::standard();
    let ground = flat();
    let refs = street(&ground);
    // No scenery at all: the crate goes to the ground.
    let budget = Budget {
        scenery: 0,
        ..big_budget()
    };
    let sliced = run(&assets, &refs, &ground, &budget, &SlicerConfig::default()).unwrap();
    let steps = &sliced.report.ladder;
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].kind, LadderKind::Demoted);
    assert_eq!(steps[0].models, ["meshes\\test\\crate.nif"]);
    assert_eq!(steps[0].references, 1);
    assert_eq!(sliced.report.category(ArtCategory::Scenery).used, 0);
    assert!(
        sliced
            .placements
            .iter()
            .all(|p| p.reference != refs[4].form.to_string())
    );
    assert!(!sliced.overlays.is_empty(), "the crate is baked");

    // Walls far over and an item list with no room: the ladder fixes the
    // walls, nothing fixes the items, and the error carries the steps.
    let mut refs = refs;
    refs.push(reference(
        6,
        "MISC",
        "meshes\\test\\can.nif",
        "crate",
        &ground,
        [300.0, 300.0],
        0.0,
        0.0,
    ));
    let budget = Budget {
        walls: 3,
        scenery: 200,
        items: 0,
    };
    let error = run(&assets, &refs, &ground, &budget, &SlicerConfig::default()).unwrap_err();
    let SliceError::FidBudget {
        category, report, ..
    } = &error
    else {
        panic!("expected a budget error, got {error}");
    };
    assert_eq!(*category, ArtCategory::Items);
    assert!(report.category(ArtCategory::Walls).used <= 3);
    assert!(!report.ladder.is_empty());
    assert!(report.to_string().contains("Budget ladder"));
}

#[test]
fn ladder_runs_are_deterministic() {
    let assets = TestAssets::standard();
    let ground = sloped();
    let refs = street(&ground);
    let budget = Budget {
        walls: 20,
        scenery: 60,
        items: 10,
    };
    let a = run(&assets, &refs, &ground, &budget, &SlicerConfig::default()).unwrap();
    let mut reversed = refs.clone();
    reversed.reverse();
    let b = run(
        &assets,
        &reversed,
        &ground,
        &budget,
        &SlicerConfig::default(),
    )
    .unwrap();
    assert!(!a.report.ladder.is_empty());
    assert_eq!(a, b);
}
