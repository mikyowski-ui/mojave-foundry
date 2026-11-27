//! The middle of the compiler on the testkit's town (spec 12.1 "Front
//! end"): the module as a JSON snapshot of synthetic data, stable under
//! reordering of the input records and across builds.

use transpile::testkit;

const SNAPSHOT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/front_town/town.module.json"
);

#[test]
fn module_snapshot() {
    let json = testkit::town().to_json();
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(std::path::Path::new(SNAPSHOT).parent().unwrap()).unwrap();
        std::fs::write(SNAPSHOT, &json).unwrap();
    }
    let expected =
        std::fs::read_to_string(SNAPSHOT).expect("snapshot; run with UPDATE_SNAPSHOTS=1");
    assert_eq!(json, expected);
}

#[test]
fn stable_under_reordering_and_repeated_builds() {
    let first = testkit::town().to_json();
    assert_eq!(testkit::town().to_json(), first, "two builds differ");
    assert_eq!(
        testkit::town_with(true).to_json(),
        first,
        "reordered input changes the module"
    );
}
