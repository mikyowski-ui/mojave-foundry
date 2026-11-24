//! Which units a change makes anew, run by run, read off the timings the
//! map stage records (`mojave_convert::timings`): one map's slicer key
//! re-renders only that map, a builder-only key re-renders nothing and
//! reuses every ground. Alone in its test binary, since the timings record
//! is process-wide.

mod common;

use std::path::{Path, PathBuf};

use common::mini_mart;
use mojave_convert::{Inputs, Install, pipeline, timings};

/// Per slicer run of the last map stage, `(run label, rendered, reused)`,
/// and how many grounds were reused.
fn counts(table: &str) -> (Vec<(String, usize, usize)>, usize) {
    let mut runs = Vec::new();
    let mut label = String::new();
    for line in table.lines() {
        let text = line.get(11..).unwrap_or("").trim_start();
        if let Some(run) = text.strip_prefix("slice ") {
            label = run.to_owned();
        } else if let Some(rest) = text.strip_prefix("- renderer: ") {
            // "N renders reused, M rendered in ..."
            let words: Vec<&str> = rest.split_whitespace().collect();
            let reused = words[0].parse().expect("reused");
            let rendered = words[3].parse().expect("rendered");
            runs.push((label.clone(), rendered, reused));
        }
    }
    let grounds = table.matches("- ground reused").count();
    (runs, grounds)
}

#[test]
fn a_change_renders_again_only_what_it_touches() {
    let fixture = common::fixture();
    let dir = fixture.root.path().join("linked");
    let mart = mini_mart::write_linked(&dir);
    let variant = |name: &str, edit: &dyn Fn(String) -> String| -> PathBuf {
        let text = std::fs::read_to_string(&mart.layer_file).expect("the layer");
        let path = dir.join(format!("{name}.layer.toml"));
        std::fs::write(&path, edit(text)).expect("the variant");
        path
    };
    let run = |layer: &Path| {
        let mut inputs = Inputs::from_args(
            &Install::any(&mart.data_dir, None),
            Some(layer.to_str().expect("utf-8")),
            &fixture.f2,
            &fixture.root.path().join("out"),
            2,
        )
        .expect("inputs");
        inputs.standalone = true;
        timings::reset();
        pipeline::all(&inputs).expect("the pipeline runs");
        counts(&timings::table())
    };

    // Cold: everything rendered, no ground reused.
    let (cold, grounds) = run(&mart.layer_file);
    assert_eq!(
        cold.len(),
        3,
        "the lot and the store's two storeys: {cold:?}"
    );
    // (A render one run shares with an earlier one is reused even cold.)
    assert!(cold.iter().all(|(_, n, m)| n + m > 0), "{cold:?}");
    assert!(
        cold.iter().map(|(_, n, _)| n).sum::<usize>() > 0,
        "{cold:?}"
    );
    assert_eq!(grounds, 0);

    // The store's ceiling cut: the lot renders nothing, the store does.
    let cut = variant("cut", &|t| {
        t.replace(
            "map = \"mmstore\"\n",
            "map = \"mmstore\"\ncut_height = 150\n",
        )
    });
    let (runs, _) = run(&cut);
    for (label, rendered, reused) in &runs {
        if label.starts_with("mmlot") {
            assert_eq!(*rendered, 0, "{label} is untouched: {runs:?}");
            assert!(*reused > 0);
        }
    }
    assert!(
        runs.iter()
            .any(|(label, rendered, _)| label.starts_with("mmstore") && *rendered > 0),
        "{runs:?}"
    );

    // A builder-only key over the cut layer: nothing rendered, every
    // ground reused.
    let sight = variant("cut-sight", &|t| {
        t.replace(
            "map = \"mmstore\"\n",
            "map = \"mmstore\"\ncut_height = 150\n",
        )
        .replace("[all_maps]\n", "[all_maps]\nsight = \"visible\"\n")
    });
    let (runs, grounds) = run(&sight);
    assert!(
        runs.iter().all(|(_, rendered, _)| *rendered == 0),
        "{runs:?}"
    );
    assert_eq!(grounds, 3);
}
