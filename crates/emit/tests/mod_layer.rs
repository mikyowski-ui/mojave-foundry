//! The mod layer: written through `write_mod_folder`, read back, mounted
//! over an in-memory base through `LayeredVfs`, and re-run safely.

use emit::{EmitError, ModLayer};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use vault_formats::mod_manifest::{MANIFEST, ModManifest, ModRegistry, README};
use vault_formats::vfs::{ByteSource, LocalDirProvider, MemoryDir, MemoryMount};
use vault_formats::{GameProfile, LayeredVfs, Vfs};

fn layer(id: &str, art: &[u8]) -> ModLayer {
    let mut layer = ModLayer::new(id, "Synthetic test layer", "0.1.0").unwrap();
    layer
        .insert("art/scenery/scenery.lst", art.to_vec())
        .unwrap();
    layer
        .insert("ART\\Scenery\\Rock01.FRM", vec![1, 2, 3])
        .unwrap();
    layer.insert("maps/testmap.map", vec![0; 8]).unwrap();
    layer
        .insert("maps/testmap.gam", b"MAP_GLOBAL_VARS:\n".to_vec())
        .unwrap();
    layer.register_map("TESTMAP.MAP", 200).unwrap();
    layer
}

/// Every file under `dir`, by relative path with `/` separators.
fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let relative = path.strip_prefix(root).unwrap();
                let key = relative
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.insert(key, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

#[test]
fn a_written_layer_reads_back_and_mounts_over_a_base() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("mods").join("synthetic");
    let layer = layer("synthetic-test", b"rock01.frm\n");
    layer.write(&dir).unwrap();

    let files = tree(&dir);
    let names: Vec<&str> = files.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            README,
            "art/scenery/rock01.frm",
            "art/scenery/scenery.lst",
            "maps/testmap.gam",
            "maps/testmap.map",
            MANIFEST,
        ]
    );
    let manifest = ModManifest::parse(std::str::from_utf8(&files[MANIFEST]).unwrap()).unwrap();
    assert_eq!(&manifest, layer.manifest());
    assert_eq!(manifest.id.as_deref(), Some("synthetic-test"));
    assert_eq!(manifest.maps.len(), 1);
    assert_eq!(
        (manifest.maps[0].file.as_str(), manifest.maps[0].number),
        ("testmap.map", 200)
    );

    // Base data: a corpus with its own art list and one other file.
    let base_tree = MemoryDir::new([
        (
            "art/scenery/scenery.lst",
            ByteSource::new(b"base.frm\n".to_vec()).unwrap(),
        ),
        ("color.pal", ByteSource::new(vec![0; 4]).unwrap()),
    ])
    .unwrap();
    let base = MemoryMount {
        profile: GameProfile::Fallout2,
        loose: Some(base_tree),
        ..MemoryMount::default()
    }
    .mount()
    .unwrap();
    let mounted = vault_formats::vfs::ModLayer::new(
        "mods/synthetic",
        Some(LocalDirProvider::mount(&dir).unwrap()),
        None,
        None,
    );
    let vfs = LayeredVfs::new(Arc::new(base), vec![mounted]);
    let (bytes, served) = vfs
        .read_with_layer("art/scenery/rock01.frm", 1 << 20)
        .unwrap();
    assert_eq!(
        (bytes, served.as_deref()),
        (vec![1, 2, 3], Some("mods/synthetic"))
    );
    // The layer's list replaces the base list whole.
    assert_eq!(
        vfs.read("art/scenery/scenery.lst").unwrap(),
        b"rock01.frm\n"
    );
    let (_, served) = vfs.read_with_layer("color.pal", 64).unwrap();
    assert_eq!(served, None);
    let registry = ModRegistry::from_vfs(&vfs).unwrap();
    assert_eq!(registry.map_number(b"testmap.map"), Some(200));
    assert_eq!(registry.map_number(b"TESTMAP.SAV"), Some(200));
}

#[test]
fn output_is_byte_identical_across_runs() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    layer("same", b"x.frm\n").write(&a).unwrap();
    layer("same", b"x.frm\n").write(&b).unwrap();
    assert_eq!(tree(&a), tree(&b));
}

#[test]
fn a_rerun_replaces_its_own_output() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("out");
    let mut first = layer("rerun", b"one.frm\n");
    first.insert("data/stale.txt", b"old".to_vec()).unwrap();
    first.write(&dir).unwrap();
    assert!(dir.join("data/stale.txt").exists());
    let second = layer("rerun", b"two.frm\n");
    second.write(&dir).unwrap();
    assert!(!dir.join("data").exists(), "files of the old run are gone");
    assert_eq!(
        std::fs::read(dir.join("art/scenery/scenery.lst")).unwrap(),
        b"two.frm\n"
    );
    // An empty directory is fine too.
    let empty = temp.path().join("empty");
    std::fs::create_dir(&empty).unwrap();
    second.write(&empty).unwrap();
    assert_eq!(tree(&empty), tree(&dir));
}

#[test]
fn a_foreign_directory_is_refused_and_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let plain = temp.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    std::fs::write(plain.join("notes.txt"), b"keep me").unwrap();
    let layer = layer("mine", b"a.frm\n");
    assert!(matches!(
        layer.write(&plain),
        Err(EmitError::ForeignOutput { .. })
    ));
    assert_eq!(std::fs::read(plain.join("notes.txt")).unwrap(), b"keep me");
    assert_eq!(tree(&plain).len(), 1);

    let other = temp.path().join("other");
    ModLayer::new("someone-else", "Another layer", "1")
        .unwrap()
        .write(&other)
        .unwrap();
    let before = tree(&other);
    assert!(matches!(
        layer.write(&other),
        Err(EmitError::ForeignOutput { .. })
    ));
    assert_eq!(tree(&other), before);

    let broken = temp.path().join("broken");
    std::fs::create_dir(&broken).unwrap();
    std::fs::write(broken.join(MANIFEST), b"not = [valid").unwrap();
    assert!(matches!(
        layer.write(&broken),
        Err(EmitError::ForeignOutput { .. })
    ));

    let file = temp.path().join("file");
    std::fs::write(&file, b"x").unwrap();
    assert!(matches!(
        layer.write(&file),
        Err(EmitError::ForeignOutput { .. })
    ));
}

#[test]
fn invalid_layers_are_refused_before_writing() {
    assert!(ModLayer::new("Bad Id", "name", "1").is_err());
    assert!(ModLayer::new("ok", " ", "1").is_err());
    assert!(
        ModLayer::from_manifest(ModManifest {
            name: "no id".into(),
            ..ModManifest::default()
        })
        .is_err()
    );
    let mut layer = ModLayer::new("ok", "name", "1").unwrap();
    assert!(layer.insert("../escape.txt", vec![]).is_err());
    assert!(layer.insert("vault-mod.toml", vec![]).is_err());
    assert!(layer.insert("README.TXT", vec![]).is_err());
    layer.insert("data/a", vec![]).unwrap();
    assert!(layer.insert("data/a/b", vec![]).is_err());
    assert!(layer.register_map("low.map", 65).is_err());
    assert!(layer.register_map("high.map", 0x1_0000).is_err());
    assert!(layer.register_map("dir/x.map", 100).is_err());
    layer.register_map("absent.map", 100).unwrap();
    assert!(layer.register_map("absent.map", 101).is_err());
    assert!(layer.register_map("other.map", 100).is_err());
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("out");
    assert!(matches!(layer.write(&dir), Err(EmitError::Manifest(_))));
    assert!(!dir.exists());
}
