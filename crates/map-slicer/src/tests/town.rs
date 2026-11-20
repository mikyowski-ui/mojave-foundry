//! The whole path on the synthetic mini town, through a real MIR directory.

use std::collections::BTreeMap;

use super::*;
use crate::{ClassKind, Role, SkipReason, estimate, slice_statics};

#[test]
fn the_mini_town_slices() {
    let install = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    let town = mir::fixture::mini_town(install.path());
    let mir_dir = out.path().join("mir");
    mir::extract(&town.options(&mir_dir, 1)).unwrap();
    let mir = mir::Mir::open(&mir_dir).unwrap();
    let worldspace = town.slice.worldspace.to_ascii_lowercase();
    let mut references = Vec::new();
    for key in mir.cells() {
        let doc = mir.cell(&key.worldspace, key.x, key.y).unwrap().unwrap();
        references.extend(doc.references);
    }
    let window = MapWindow::from_center([2048.0, 2048.0], 140.0);
    let sampler = GroundSampler::load(&mir, &worldspace, &window).unwrap();
    let projection = projection();
    let config = SlicerConfig::default();
    let sliced = slice_statics(
        &mir,
        &references,
        &window,
        &sampler,
        &projection,
        &mut renderer(),
        &palette_map(),
        &big_budget(),
        &config,
    )
    .unwrap();

    let roles: BTreeMap<String, Vec<Role>> =
        sliced
            .placements
            .iter()
            .fold(BTreeMap::new(), |mut map, p| {
                map.entry(p.reference.clone()).or_default().push(p.role);
                map
            });
    let skipped: BTreeMap<String, SkipReason> = sliced
        .skipped
        .iter()
        .map(|s| (s.reference.clone(), s.reason.clone()))
        .collect();
    let house = town.house.to_string();
    assert!(roles[&house].len() > 4 && roles[&house].iter().all(|r| *r == Role::WallStrip));
    assert_eq!(roles[&town.crate_ref.to_string()], vec![Role::Container]);
    assert_eq!(roles[&town.exterior_door.to_string()], vec![Role::Door]);
    assert_eq!(
        skipped[&town.bottle.to_string()],
        SkipReason::DisabledByParent
    );
    assert_eq!(skipped[&town.lost_crate.to_string()], SkipReason::NoMesh);
    assert!(matches!(
        skipped[&town.sheriff_ref.to_string()],
        SkipReason::Kind(_)
    ));
    // The rock is relief: baked, blocked, no art.
    assert!(!roles.contains_key(&town.scaled_rock.to_string()));
    assert!(!sliced.overlays.is_empty() && !sliced.blocked_hexes.is_empty());
    let report = &sliced.report;
    assert_eq!(report.classes[&ClassKind::GroundRelief].references, 1);
    let total: usize = report.classes.values().map(|c| c.references).sum();
    let distinct: std::collections::BTreeSet<_> = references.iter().map(|r| &r.form).collect();
    assert_eq!(total, distinct.len());
    assert!(report.fits());
    assert!(report.to_string().contains("sliced"));

    let guess = estimate(
        &mir,
        &references,
        &window,
        &sampler,
        &projection,
        &big_budget(),
        &config,
    )
    .unwrap();
    for kind in ClassKind::ALL {
        assert_eq!(
            guess.classes[&kind].references,
            report.classes[&kind].references
        );
    }
}
