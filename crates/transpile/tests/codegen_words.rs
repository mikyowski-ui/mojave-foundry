//! The host words code generation emits are the engine's: each assembles
//! under `emit::int::Words::FALLOUT2` to a word whose Fallout 2 row
//! (`vault_vm::extensions::opcode_in`) pops as many operands and pushes as
//! the codegen table says; none is an sfall or manifest scripting word.

use transpile::codegen::words::ALL;
use vault_vm::WordSet;
use vault_vm::extensions::{self, PushKind};

#[test]
fn the_codegen_word_table_is_the_engines() {
    for word in ALL {
        let number = emit::int::Words::FALLOUT2
            .word_named(word.name)
            .unwrap_or_else(|| panic!("{} does not assemble under Fallout 2", word.name));
        assert!((0x80a1..=0x8183).contains(&number), "{}", word.name);
        let spec = extensions::opcode_in(&WordSet::FALLOUT2, number)
            .unwrap_or_else(|| panic!("{} has no Fallout 2 row", word.name));
        assert!(spec.registered && !spec.variable, "{}", word.name);
        assert_eq!(spec.name, word.name);
        assert_eq!(spec.operands.len(), usize::from(word.args), "{}", word.name);
        assert_eq!(spec.push != PushKind::None, word.push, "{}", word.name);
    }
}
