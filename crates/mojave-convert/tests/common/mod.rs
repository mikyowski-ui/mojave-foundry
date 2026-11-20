//! Shared setup for the integration tests: a synthetic source install and a
//! synthetic Fallout 2 install, both built in a temporary directory.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use mojave_convert::{Inputs, SliceSpec};

pub struct Fixture {
    pub root: tempfile::TempDir,
    pub fnv: PathBuf,
    pub f2: PathBuf,
    pub slice: SliceSpec,
}

/// Writes the mini town and a loose-file Fallout 2 directory holding only
/// the synthetic palette.
pub fn fixture() -> Fixture {
    let root = tempfile::tempdir().expect("temp dir");
    let town = mir::fixture::mini_town(&root.path().join("source"));
    let f2 = root.path().join("fallout2");
    std::fs::create_dir_all(f2.join("data")).expect("f2 dir");
    std::fs::write(
        f2.join("data").join("color.pal"),
        quantize::fixture::synthetic_palette(),
    )
    .expect("palette");
    let slice = SliceSpec {
        // The fixture calls its slice "mini town"; a slice file's name must be
        // safe in a file name.
        name: "minitown".to_owned(),
        worldspace: town.slice.worldspace.clone(),
        plugins: town.plugins.clone(),
        x: [*town.slice.x.start(), *town.slice.x.end()],
        y: [*town.slice.y.start(), *town.slice.y.end()],
        interior_depth: town.slice.interior_depth,
        map_center: None,
    };
    Fixture {
        fnv: town.data_dir.clone(),
        f2,
        slice,
        root,
    }
}

impl Fixture {
    pub fn inputs(&self, out: &str, jobs: usize) -> Inputs {
        Inputs::new(
            &self.fnv,
            &self.f2,
            &self.root.path().join(out),
            self.slice.clone(),
            jobs,
        )
        .expect("inputs")
    }
}

/// Every file under `dir`, as sorted relative paths with their bytes.
pub fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .expect("read dir")
            .map(|e| e.expect("entry").path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let relative = path.strip_prefix(root).expect("under root");
                out.push((
                    relative.to_string_lossy().replace('\\', "/"),
                    std::fs::read(&path).expect("read file"),
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out
}
