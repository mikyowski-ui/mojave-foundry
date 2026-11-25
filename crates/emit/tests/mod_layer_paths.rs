//! `ModLayer::insert`'s file and folder clash rule: its speed on a large
//! layer, and that it names the same clashes as the first, scanning
//! version of the rule.

use emit::{EmitError, ModLayer};

/// The clash rule as it was first written, for comparison: the first file,
/// in path order, that is a folder of `canonical` or that `canonical` is a
/// folder of.
fn reference_clash(files: &[String], canonical: &str) -> Option<String> {
    files
        .iter()
        .find(|other| {
            other.starts_with(&format!("{canonical}/"))
                || canonical.starts_with(&format!("{other}/"))
        })
        .cloned()
}

#[test]
fn inserting_many_paths_is_fast() {
    let mut layer = ModLayer::new("many", "Many files", "1").unwrap();
    let start = std::time::Instant::now();
    for index in 0..20_000 {
        layer
            .insert(&format!("scripts/mm{index:06}.int"), vec![0; 4])
            .unwrap();
    }
    let elapsed = start.elapsed();
    eprintln!("20,000 inserts: {elapsed:?}");
    assert_eq!(layer.files().len(), 20_000);
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "20,000 inserts took {elapsed:?}"
    );
    // The folder rule still holds at this size.
    assert!(layer.insert("scripts", vec![]).is_err());
    assert!(layer.insert("scripts/mm000007.int/x", vec![]).is_err());
}

#[test]
fn the_clash_rule_matches_the_original_scan() {
    // A small alphabet makes folders and files collide often.
    let mut state = 0x2545_f491_u32;
    let mut next = move |bound: u32| {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 8) % bound
    };
    let mut clashes = 0;
    for _ in 0..40 {
        let mut layer = ModLayer::new("clash", "Clash", "1").unwrap();
        for _ in 0..60 {
            let depth = 1 + next(4);
            let path = (0..depth)
                .map(|_| ["a", "b", "ab", "A"][next(4) as usize])
                .collect::<Vec<_>>()
                .join(if next(2) == 0 { "/" } else { "\\" });
            let canonical = path.replace('\\', "/").to_ascii_lowercase();
            let keys: Vec<String> = layer.files().keys().cloned().collect();
            let expected = reference_clash(&keys, &canonical);
            match (layer.insert(&path, vec![1]), expected) {
                (Ok(()), None) => {}
                (
                    Err(EmitError::Path {
                        path: named,
                        message,
                    }),
                    Some(other),
                ) => {
                    clashes += 1;
                    assert_eq!(named, path);
                    assert_eq!(message, format!("{other} needs it as a file and a folder"));
                }
                (got, expected) => {
                    panic!("{path}: got {got:?}, expected a clash with {expected:?}")
                }
            }
        }
    }
    assert!(clashes > 100, "the generator made only {clashes} clashes");
}
