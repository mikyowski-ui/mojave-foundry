//! Manifest scripting in a mod layer: `[[scripts]]`, `[[global_scripts]]`
//! and `[[hooks]]` written, parsed back by the engine's manifest parser,
//! and the checks `ModLayer::write` makes before touching the output.

use emit::int::{Words, assemble};
use emit::{EmitError, ModLayer};
use std::sync::Arc;
use vault_formats::mod_manifest::{
    GlobalScriptRegistration, HookRegistration, MANIFEST, ModManifest, ModRegistry,
    ScriptRegistration,
};
use vault_formats::vfs::LocalDirProvider;
use vault_formats::{GameProfile, LayeredVfs, Vfs};

/// A program with `start`, `map_enter_p_proc` and `ap_cost`.
fn program() -> Vec<u8> {
    let source = "
.procedure start
.procedure map_enter_p_proc
.procedure ap_cost
.entry init
init:
    push.i 0
    critical_off
    push.a start
    jmp
start:
    frame
    mod_api_version
    pop
    drop_frame
    restore_base
    ret
map_enter_p_proc:
    ret
ap_cost:
    ret
";
    assemble(source, &Words::FALLOUT2.with_mods(true)).unwrap()
}

fn layer() -> ModLayer {
    let mut layer = ModLayer::new("cheap-ap", "Cheap AP", "1.0.0").unwrap();
    layer.insert("scripts/gl_cheapap.int", program()).unwrap();
    layer.insert("scripts/mfextra.int", program()).unwrap();
    layer
}

#[test]
fn global_scripts_hooks_and_script_rows_are_written_and_read_back() {
    let mut layer = layer();
    layer.add_global_script("GL_CheapAP.int", 30, 1).unwrap();
    layer
        .add_hook("HOOK_CALCAPCOST", "gl_cheapap.int", "ap_cost")
        .unwrap();
    layer.register_script("mfextra.int", 4096, 3).unwrap();

    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("layer");
    layer.write(&dir).unwrap();
    let text = std::fs::read_to_string(dir.join(MANIFEST)).unwrap();
    let manifest = ModManifest::parse(&text).unwrap();
    assert_eq!(&manifest, layer.manifest());
    assert_eq!(manifest.id.as_deref(), Some("cheap-ap"));
    assert_eq!(manifest.api, Some(1));
    assert!(manifest.scripted());
    assert_eq!(
        manifest.global_scripts,
        [GlobalScriptRegistration {
            file: "gl_cheapap.int".into(),
            repeat: 30,
            kind: 1
        }]
    );
    assert_eq!(
        manifest.hooks,
        [HookRegistration {
            point: "HOOK_CALCAPCOST".into(),
            file: "gl_cheapap.int".into(),
            procedure: "ap_cost".into()
        }]
    );
    assert_eq!(
        manifest.scripts,
        [ScriptRegistration {
            file: "mfextra.int".into(),
            index: 4096,
            local_vars: 3
        }]
    );
    assert_eq!(layer.manifest_text().unwrap(), text);

    // Mounted, the layer registers its script row.
    let base = vault_formats::vfs::MemoryMount {
        profile: GameProfile::Fallout2,
        ..vault_formats::vfs::MemoryMount::default()
    }
    .mount()
    .unwrap();
    let mounted = vault_formats::vfs::ModLayer::new(
        "mods/cheap",
        Some(LocalDirProvider::mount(&dir).unwrap()),
        None,
        None,
    );
    let vfs = LayeredVfs::new(Arc::new(base), vec![mounted]);
    let registry = ModRegistry::from_vfs(&vfs).unwrap();
    assert_eq!(registry.scripts[&4096].file, "mfextra.int");
    assert_eq!(vfs.read("scripts/gl_cheapap.int").unwrap(), program());

    // Rewritten, byte for byte.
    let again = temp.path().join("again");
    layer.write(&again).unwrap();
    assert_eq!(
        std::fs::read(again.join(MANIFEST)).unwrap(),
        std::fs::read(dir.join(MANIFEST)).unwrap()
    );
}

#[test]
fn registrations_the_manifest_parser_refuses_are_refused_at_once() {
    let mut layer = layer();
    // Each failed call leaves the manifest as it was.
    let before = layer.manifest().clone();
    assert!(
        layer
            .add_hook("HOOK_NOT_A_HOOK", "gl_cheapap.int", "ap_cost")
            .is_err()
    );
    assert!(
        layer
            .add_hook("HOOK_CALCAPCOST", "gl_cheapap.int", "bad name")
            .is_err()
    );
    assert!(
        layer
            .add_hook("HOOK_CALCAPCOST", "gl_cheapap.txt", "ap_cost")
            .is_err()
    );
    assert!(
        layer
            .add_hook("HOOK_CALCAPCOST", "dir/x.int", "ap_cost")
            .is_err()
    );
    assert!(layer.add_global_script("gl_cheapap.int", 0, 4).is_err());
    assert!(
        layer
            .add_global_script("gl_cheapap.int", u32::MAX, 0)
            .is_err()
    );
    assert!(layer.add_global_script("not_int.txt", 0, 0).is_err());
    assert!(
        layer
            .register_script("mfextra.int", 0x0100_0000, 0)
            .is_err()
    );
    assert!(layer.register_script("mfextra.int", 1, 1_000_001).is_err());
    assert_eq!(layer.manifest(), &before);

    // Repeats: a global script twice, a hook twice, a row or file twice.
    layer.add_global_script("gl_cheapap.int", 0, 0).unwrap();
    assert!(layer.add_global_script("gl_cheapap.int", 5, 0).is_err());
    layer
        .add_hook("HOOK_CALCAPCOST", "gl_cheapap.int", "ap_cost")
        .unwrap();
    assert!(
        layer
            .add_hook("HOOK_CALCAPCOST", "gl_cheapap.int", "start")
            .is_err()
    );
    layer.register_script("mfextra.int", 5000, 0).unwrap();
    assert!(layer.register_script("other.int", 5000, 0).is_err());
}

#[test]
fn a_layer_needs_an_id_and_api_for_scripting() {
    let manifest = ModManifest {
        name: "no api".into(),
        id: Some("x".into()),
        hooks: vec![HookRegistration {
            point: "HOOK_CALCAPCOST".into(),
            file: "a.int".into(),
            procedure: "start".into(),
        }],
        ..ModManifest::default()
    };
    // `from_manifest` writes and parses the manifest back, which needs `api`.
    assert!(matches!(
        ModLayer::from_manifest(manifest),
        Err(EmitError::Manifest(_))
    ));
}

#[test]
fn write_checks_registered_files_rows_and_hook_procedures() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("out");

    // A registered file that is not in the layer.
    let mut missing = ModLayer::new("m", "Missing", "1").unwrap();
    missing.add_global_script("gone.int", 0, 0).unwrap();
    assert!(
        matches!(missing.write(&dir), Err(EmitError::Manifest(m)) if m.contains("scripts/gone.int"))
    );
    assert!(!dir.exists());

    // A hook whose procedure the program does not have.
    let mut wrong = layer();
    wrong
        .add_hook("HOOK_CALCAPCOST", "gl_cheapap.int", "no_such_proc")
        .unwrap();
    let error = wrong.write(&dir).unwrap_err();
    assert!(error.to_string().contains("no_such_proc"), "{error}");
    // Names compare without case.
    let mut cased = layer();
    cased
        .add_hook("HOOK_CALCAPCOST", "gl_cheapap.int", "AP_COST")
        .unwrap();
    cased.write(&dir).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();

    // A program that is not one.
    let mut garbage = ModLayer::new("g", "Garbage", "1").unwrap();
    garbage.insert("scripts/junk.int", vec![1, 2, 3]).unwrap();
    garbage
        .add_hook("HOOK_CALCAPCOST", "junk.int", "start")
        .unwrap();
    assert!(garbage.write(&dir).is_err());

    // A script row that the layer's own scripts.lst already has: the
    // engine refuses it.
    let mut clash = layer();
    clash
        .insert(
            "scripts/scripts.lst",
            b"a.int\nb.int # local_vars=1\nc.int\n".to_vec(),
        )
        .unwrap();
    clash.register_script("mfextra.int", 2, 0).unwrap();
    let error = clash.write(&dir).unwrap_err();
    assert!(error.to_string().contains("scripts.lst"), "{error}");
    assert!(!dir.exists());
    let mut fine = layer();
    fine.insert("scripts/scripts.lst", b"a.int\nb.int\nc.int\n".to_vec())
        .unwrap();
    fine.register_script("mfextra.int", 3, 0).unwrap();
    fine.write(&dir).unwrap();
}
