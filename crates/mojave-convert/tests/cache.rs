//! The map stage's render and ground caches (`mojave_convert::cache`) on
//! the linked two-storey mini mart: a warm cache gives the bytes of an
//! empty one, a damaged entry is rebuilt, `--jobs` and a builder-only key
//! need exactly the units of the base run, and a slicer key needs new ones.

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use common::mini_mart;
use mojave_convert::cache::{Key, Store};
use mojave_convert::{Inputs, Install, pipeline};

struct Mart {
    fixture: common::Fixture,
    data_dir: PathBuf,
    dir: PathBuf,
}

fn mart() -> Mart {
    let fixture = common::fixture();
    let dir = fixture.root.path().join("linked");
    let mart = mini_mart::write_linked(&dir);
    Mart {
        data_dir: mart.data_dir,
        dir,
        fixture,
    }
}

/// The fixture's layer file with `edit` applied, written as `<name>.layer.toml`.
fn layer_file(mart: &Mart, name: &str, edit: impl Fn(String) -> String) -> PathBuf {
    let text = std::fs::read_to_string(mart.dir.join("mart.layer.toml")).expect("the layer");
    let path = mart.dir.join(format!("{name}.layer.toml"));
    std::fs::write(&path, edit(text)).expect("the variant");
    path
}

fn inputs(mart: &Mart, layer: &Path, out: &str, cache: Option<&Path>, jobs: usize) -> Inputs {
    let mut inputs = Inputs::from_args(
        &Install::any(&mart.data_dir, None),
        Some(layer.to_str().expect("utf-8")),
        &mart.fixture.f2,
        &mart.fixture.root.path().join(out),
        jobs,
    )
    .expect("inputs");
    inputs.standalone = true;
    inputs.cache_dir = cache.map(Path::to_owned);
    inputs
}

/// What the map stage wrote that must not depend on the cache: the layer,
/// the placement index and the features.
fn output(inputs: &Inputs) -> Vec<(String, Vec<u8>)> {
    let mut files = common::tree(&inputs.mod_dir());
    for name in ["map-placements.json", "map-features.json"] {
        let bytes = std::fs::read(inputs.out.join(name)).unwrap_or_else(|_| panic!("{name}"));
        files.push((name.to_owned(), bytes));
    }
    files.extend(
        common::tree(&inputs.out.join("features"))
            .into_iter()
            .map(|(p, b)| (format!("features/{p}"), b)),
    );
    files
}

/// Every entry of a store: `(kind, key)`.
fn entries(dir: &Path) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    for kind in ["render", "ground"] {
        let Ok(shards) = std::fs::read_dir(dir.join(kind)) else {
            continue;
        };
        for shard in shards.flatten() {
            for file in std::fs::read_dir(shard.path()).expect("shard").flatten() {
                let name = file.file_name().to_string_lossy().into_owned();
                if let Some(key) = name.strip_suffix(".bin") {
                    out.insert((kind.to_owned(), key.to_owned()));
                }
            }
        }
    }
    out
}

fn key_of(hex: &str) -> Key {
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("hex");
    }
    key
}

/// Runs the map stage again with its stage key gone, so that only the
/// caches below it can save work.
fn rebuild(inputs: &Inputs) {
    std::fs::remove_file(inputs.out.join("cache").join("map.key")).expect("the stage key");
    let summary = mojave_convert::map::map(inputs).expect("the map stage");
    assert!(!summary.reused);
}

#[test]
fn a_warm_cache_gives_the_bytes_of_an_empty_one_and_damage_is_rebuilt() {
    let mart = mart();
    let layer = mart.dir.join("mart.layer.toml");
    let warm = inputs(&mart, &layer, "out-warm", None, 2);
    pipeline::all(&warm).expect("the pipeline runs");
    let cold_bytes = output(&warm);
    let store_dir = warm.store_dir();
    assert_eq!(store_dir, warm.out.join("cache"), "the default store");
    let first = entries(&store_dir);
    assert!(first.iter().any(|(k, _)| k == "render"), "{first:?}");
    assert_eq!(
        first.iter().filter(|(k, _)| k == "ground").count(),
        3,
        "one ground per elevation: the lot and the store's two storeys"
    );

    // Warm: the stage rebuilds from the caches alone, writing nothing new,
    // and its output is the cold run's, byte for byte.
    rebuild(&warm);
    assert_eq!(entries(&store_dir), first, "nothing rendered again");
    assert_eq!(output(&warm), cold_bytes);

    // Damage three entries: one cut short, one with a byte flipped, one
    // replaced by another entry's file. Each is found, rebuilt and stored
    // again; the output does not change.
    let store = Store::open(&store_dir);
    let renders: Vec<&(String, String)> = first.iter().filter(|(k, _)| k == "render").collect();
    let ground = first.iter().find(|(k, _)| k == "ground").expect("a ground");
    let (cut, flipped, swapped) = (renders[0], renders[1], ground);
    let path = |e: &(String, String)| store.path(&e.0, &key_of(&e.1));
    let bytes = std::fs::read(path(cut)).expect("entry");
    std::fs::write(path(cut), &bytes[..bytes.len() / 2]).expect("cut");
    let mut bytes = std::fs::read(path(flipped)).expect("entry");
    let last = bytes.len() - 1;
    bytes[last] ^= 0x40;
    std::fs::write(path(flipped), bytes).expect("flip");
    std::fs::copy(path(renders[2]), path(swapped)).expect("swap");
    for damaged in [cut, flipped, swapped] {
        assert!(
            store.get(&damaged.0, &key_of(&damaged.1)).is_none(),
            "{damaged:?} is refused"
        );
    }
    // (The checks above deleted them; damage them again for the stage.)
    std::fs::write(path(cut), b"MFCACHE1 truncated").expect("cut");
    std::fs::copy(path(renders[2]), path(swapped)).expect("swap");
    rebuild(&warm);
    assert_eq!(output(&warm), cold_bytes, "damage never reaches the output");
    for damaged in [cut, flipped, swapped] {
        assert!(
            store.get(&damaged.0, &key_of(&damaged.1)).is_some(),
            "{damaged:?} was rebuilt"
        );
    }
    assert_eq!(entries(&store_dir), first);

    // A fresh directory sharing the store (`--cache-dir`) renders nothing,
    // and an empty store in another gives the same bytes.
    let shared = inputs(&mart, &layer, "out-shared", Some(&store_dir), 2);
    pipeline::all(&shared).expect("the pipeline runs");
    assert_eq!(entries(&store_dir), first);
    assert_eq!(output(&shared), cold_bytes);
    assert!(
        !shared.out.join("cache").join("render").exists(),
        "the shared store holds the renders"
    );
}

#[test]
fn each_change_makes_anew_only_the_units_it_touches() {
    let mart = mart();
    let base = mart.dir.join("mart.layer.toml");
    let root = mart.fixture.root.path().to_owned();
    // One output directory (extract, sprites and critters once), a store
    // of its own per variant: each store then holds exactly the units its
    // variant needs.
    let run = |layer: &Path, store: &str, jobs: usize| {
        let store = root.join(store);
        let inputs = inputs(&mart, layer, "out", Some(&store), jobs);
        pipeline::all(&inputs).expect("the pipeline runs");
        (entries(&store), output(&inputs))
    };
    let (units, bytes) = run(&base, "store-base", 2);

    // `--jobs` changes nothing: the same units, the same bytes.
    std::fs::remove_file(root.join("out").join("cache").join("map.key")).expect("key");
    let (one_job, one_job_bytes) = run(&base, "store-jobs", 1);
    assert_eq!(one_job, units);
    assert_eq!(one_job_bytes, bytes);

    // A key only the builder reads: every render and ground is the base's.
    let sight = layer_file(&mart, "sight", |t| {
        t.replace("[all_maps]\n", "[all_maps]\nsight = \"visible\"\n")
    });
    let (sight_units, _) = run(&sight, "store-sight", 2);
    assert_eq!(sight_units, units);

    // A slicer key of one map (the store's ceiling cut): some units anew,
    // and the lot's untouched (`tests/cache_units.rs` checks that per run).
    let store_cut = layer_file(&mart, "cut", |t| {
        t.replace(
            "map = \"mmstore\"\n",
            "map = \"mmstore\"\ncut_height = 150\n",
        )
    });
    let (cut_units, _) = run(&store_cut, "store-cut", 2);
    assert!(
        cut_units.difference(&units).count() > 0,
        "the cut renders anew"
    );
    assert!(
        cut_units.intersection(&units).count() > 0,
        "the rest is reused"
    );
}
