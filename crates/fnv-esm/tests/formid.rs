//! Form id resolution against a plugin's master list.

use fnv_esm::fixture::PluginBuilder;
use fnv_esm::*;

fn plugin_with_masters(masters: &[&str]) -> Vec<u8> {
    let mut builder = PluginBuilder::new();
    for master in masters {
        builder.master(master);
    }
    builder.build()
}

#[test]
fn parts_of_a_form_id() {
    let id = FormId(0x02AB_CDEF);
    assert_eq!(id.master_index(), 2);
    assert_eq!(id.local_id(), 0x00AB_CDEF);
    assert!(!id.is_null());
    assert!(FormId::NULL.is_null());
    assert_eq!(id.to_string(), "02ABCDEF");
    assert_eq!(format!("{id:?}"), "FormId(02ABCDEF)");
    assert_eq!(FormId::from(7u32), FormId(7));
}

#[test]
fn zero_masters_everything_at_index_zero_is_the_plugin() {
    let bytes = plugin_with_masters(&[]);
    let plugin = Plugin::parse(&bytes).expect("plugin");
    let list = plugin.master_list("Standalone.ESM");
    assert_eq!(list.master_count(), 0);
    assert!(list.masters().is_empty());
    assert_eq!(list.plugin_name(), "standalone.esm");

    let resolved = list.resolve(FormId(0x0001_2345)).expect("own form");
    assert_eq!(resolved.plugin, "standalone.esm");
    assert_eq!(resolved.local_id, 0x01_2345);
    assert_eq!(resolved.to_string(), "standalone.esm:012345");

    assert_eq!(
        list.resolve(FormId(0x0100_0001)),
        Err(EsmError::MasterIndexOutOfRange {
            form_id: FormId(0x0100_0001),
            index: 1,
            master_count: 0,
        })
    );
}

#[test]
fn one_master_index_zero_is_the_master_and_one_the_plugin() {
    let bytes = plugin_with_masters(&["BaseGame.esm"]);
    let plugin = Plugin::parse(&bytes).expect("plugin");
    let list = plugin.master_list("TownMod.esp");
    assert_eq!(list.master_count(), 1);
    assert_eq!(list.masters(), ["basegame.esm"]);

    let from_master = list.resolve(FormId(0x0000_0ABC)).expect("master form");
    assert_eq!(
        (from_master.plugin, from_master.local_id),
        ("basegame.esm", 0xABC)
    );
    let own = list.resolve(FormId(0x0100_0ABC)).expect("own form");
    assert_eq!((own.plugin, own.local_id), ("townmod.esp", 0xABC));
    // Same local id, different owners: different forms.
    assert_ne!(from_master, own);

    let error = list
        .resolve(FormId(0x0200_0ABC))
        .expect_err("index 2 of 1 master");
    assert_eq!(
        error,
        EsmError::MasterIndexOutOfRange {
            form_id: FormId(0x0200_0ABC),
            index: 2,
            master_count: 1,
        }
    );
    assert_eq!(error.offset(), None);
    assert_eq!(error.sig(), None);
    assert!(error.to_string().contains("02000ABC"));
}

#[test]
fn two_masters_resolve_in_file_order() {
    let bytes = plugin_with_masters(&["BaseGame.esm", "Expansion One.ESM"]);
    let plugin = Plugin::parse(&bytes).expect("plugin");
    assert_eq!(
        plugin.header().masters,
        ["BaseGame.esm", "Expansion One.ESM"]
    );
    let list = plugin.master_list("Patch.esp");
    assert_eq!(list.masters(), ["basegame.esm", "expansion one.esm"]);

    let cases = [
        (0x0012_3456u32, "basegame.esm"),
        (0x0112_3456, "expansion one.esm"),
        (0x0212_3456, "patch.esp"),
    ];
    for (id, plugin_name) in cases {
        let resolved = list.resolve(FormId(id)).expect("in range");
        assert_eq!(resolved.plugin, plugin_name);
        assert_eq!(resolved.local_id, 0x12_3456);
    }
    for id in [0x0312_3456u32, 0x7F00_0001, 0xFF00_0000, 0xFFFF_FFFF] {
        assert!(matches!(
            list.resolve(FormId(id)),
            Err(EsmError::MasterIndexOutOfRange {
                master_count: 2,
                ..
            })
        ));
    }
    // The null id is not special to the resolver.
    let null = list.resolve(FormId::NULL).expect("index 0");
    assert_eq!((null.plugin, null.local_id), ("basegame.esm", 0));
}

#[test]
fn a_master_list_can_be_built_without_a_file() {
    let list = MasterList::new("Mod.esp", &["A.esm", "B.esm"]);
    assert_eq!(list.master_count(), 2);
    assert_eq!(
        list.resolve(FormId(0x0100_0001)).map(|r| r.plugin),
        Ok("b.esm")
    );
    let none: [&str; 0] = [];
    assert_eq!(MasterList::new("Mod.esp", &none).master_count(), 0);
}
