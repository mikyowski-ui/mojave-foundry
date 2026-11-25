//! The assembler against the engine's own loader, VM and stack model
//! (`vault-vm`, the pinned revision). A dev-dependency of `emit`, tests only.

use emit::int::{Words, assemble};
use vault_vm::stack_check::{self, Table};
use vault_vm::{BasicHost, IntProgram, RunOutcome, Value, Vm, VmStatus, WordSet, extensions};

const VECTOR_A: &str = include_str!("int_vector_a.asm");

fn vm_words(words: &Words) -> WordSet {
    let base = match words.game {
        emit::int::Game::Fallout1 => WordSet::FALLOUT1,
        emit::int::Game::Fallout2 => WordSet::FALLOUT2,
    };
    base.with_mods(words.mods)
}

fn run_to_end(vm: &mut Vm) {
    let mut host = BasicHost::default();
    for _ in 0..1000 {
        match vm.update(&mut host).unwrap() {
            RunOutcome::Budget => {}
            RunOutcome::Ended => return,
            other => panic!("{other:?}"),
        }
    }
    panic!("the program did not end");
}

#[test]
fn vector_a_loads_and_runs_to_the_fixtures_final_state() {
    let bytes = assemble(VECTOR_A, &Words::FALLOUT2).unwrap();
    assert_eq!(bytes.len(), 282);
    let program = IntProgram::parse_in(&bytes, &WordSet::FALLOUT2, false).unwrap();
    assert_eq!(program.image(), bytes);
    assert_eq!(
        (
            program.identifier_origin,
            program.literal_origin,
            program.code_start
        ),
        (0x5e, 0x76, 0x84)
    );
    let procedures = program.procedures();
    assert_eq!(procedures.len(), 2);
    assert_eq!(procedures[0].name, b"main");
    assert_eq!(procedures[1].name, b"sumxy");
    assert_eq!(
        (
            procedures[0].body,
            procedures[1].body,
            procedures[1].arguments
        ),
        (0x9c, 0xfc, 2)
    );
    assert_eq!(program.identifier(6), Some(b"main".as_slice()));
    assert_eq!(program.identifier(14), Some(b"sumxy".as_slice()));
    assert_eq!(program.literal(6), Some(b"hi".as_slice()));
    assert!(program.boundary(0x84) && program.boundary(0x11a - 2));

    let mut vm = Vm::new(program);
    run_to_end(&mut vm);
    assert_eq!(
        (vm.pc, vm.f, vm.g, vm.return_stack.byte_len()),
        (20, -1, 0, 0)
    );
    assert_eq!(vm.value_stack.bytes(), [0, 0, 0, 0x63, 0xc0, 0x01]);
    assert_eq!(vm.value_stack.top().unwrap(), Value::integer(99));
    assert!(!vm.status.contains(VmStatus::ERROR));

    let findings = stack_check::check(
        "a.int",
        &IntProgram::parse_in(&bytes, &WordSet::FALLOUT2, false).unwrap(),
        &Table::new(WordSet::FALLOUT2),
        &[],
    );
    assert!(findings.is_empty(), "{findings:?}");
}

#[test]
fn vector_b_runs_to_the_end_instruction_with_empty_stacks() {
    let source = "
.procedure start
.entry init
init:
    push.i 0
    critical_off
    push.a start
    jmp
start:
    frame
    drop_frame
    restore_base
    ret
";
    let bytes = assemble(source, &Words::FALLOUT2).unwrap();
    let program = IntProgram::parse_in(&bytes, &WordSet::FALLOUT2, false).unwrap();
    assert_eq!(program.code_start, 0x5a);
    let mut vm = Vm::new(program);
    run_to_end(&mut vm);
    assert_eq!(vm.pc, 20);
    assert_eq!(vm.value_stack.byte_len(), 0);
    assert_eq!(vm.return_stack.byte_len(), 0);
}

#[test]
fn what_the_assembler_makes_loads_in_every_word_set_it_names() {
    let source = "
.procedure start
.procedure timed_one flags=timed deadline=5 args=1
.procedure cond_one flags=conditional condition=check
.import far args=2
.name shared
.literal l \"one\"
.literal m \"two words\"
.entry init
init:
    push.i 0
    critical_off
    push.a start
    jmp
start:
    frame
    push.s l
    push.s m
    push.n shared
    export_var
    pop
    pop
    drop_frame
    restore_base
    ret
timed_one:
    ret
cond_one:
    ret
check:
    push.i 1
    ret
";
    for words in [
        Words::FALLOUT1,
        Words::FALLOUT2,
        Words::FALLOUT2.with_mods(true),
    ] {
        let bytes = assemble(source, &words).unwrap();
        let program = IntProgram::parse_in(&bytes, &vm_words(&words), false).unwrap();
        let procedures = program.procedures();
        assert_eq!(procedures.len(), 4);
        assert_eq!(procedures[1].flags, 1);
        assert_eq!(procedures[1].deadline, 5);
        assert_eq!(procedures[2].flags, 2);
        assert!(program.boundary(procedures[2].condition));
        assert_eq!(procedures[3].flags, 4);
        assert_eq!(procedures[3].body, 0);
        assert_eq!(program.image(), bytes);
    }
}

#[test]
fn the_mnemonic_tables_are_the_engines_names() {
    for words in [
        Words::FALLOUT1,
        Words::FALLOUT2,
        Words::FALLOUT2.with_mods(true),
    ] {
        let set = vm_words(&words);
        let mut checked = 0;
        for word in 0x804c..=0x8309u16 {
            if !words.accepts(word) {
                assert!(!set.accepts(word), "{word:#06x}");
                continue;
            }
            assert!(set.accepts(word), "{word:#06x}");
            let ours = words.name_of(word);
            let theirs = extensions::opcode_in(&set, word)
                .filter(|spec| spec.registered)
                .map(|spec| spec.name)
                .filter(|name| *name != "unnamed")
                .filter(|name| {
                    !name.is_empty()
                        && name.bytes().all(|byte| {
                            byte.is_ascii_lowercase() || byte == b'_' || byte.is_ascii_digit()
                        })
                });
            assert_eq!(ours, theirs, "{word:#06x} under {words:?}");
            checked += usize::from(ours.is_some());
        }
        assert!(checked > 200);
    }
}

#[test]
fn whatever_the_assembler_accepts_the_engine_loads() {
    // Deterministic random bodies after a fixed header: any source the
    // assembler accepts must parse under the loader of its word set.
    let lines = [
        "push.i 5",
        "push.i -1",
        "push.f 2.5",
        "push.s l",
        "push.n n",
        "push.a a",
        "push.a z",
        "push.p b",
        "push.raw 0xC801 3",
        "op 0x8010",
        "op 0x8156",
        "op 0x8200",
        "nop",
        "frame",
        "jmp",
        "call",
        "ret",
        "export_var",
        "sayquit",
        "metarule3",
        "mod_log",
        "bogus",
        "z:",
        "z: nop",
        "end",
    ];
    let mut state = 0x7f4a_7c15_u32;
    let mut next = move |bound: usize| {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 8) as usize % bound
    };
    let header = ".procedure a\n.procedure b args=1\n.name n\n.literal l \"x\"\n.entry a\na:\nb:\n";
    let (mut accepted, mut refused) = (0, 0);
    for _ in 0..20_000 {
        let mut source = header.to_owned();
        for _ in 0..next(14) {
            source += lines[next(lines.len())];
            source.push('\n');
        }
        for words in [
            Words::FALLOUT1,
            Words::FALLOUT2,
            Words::FALLOUT2.with_mods(true),
        ] {
            match assemble(&source, &words) {
                Ok(bytes) => {
                    accepted += 1;
                    let program = IntProgram::parse_in(&bytes, &vm_words(&words), false)
                        .unwrap_or_else(|error| panic!("{error}: {source:?}"));
                    assert_eq!(program.image(), bytes);
                }
                Err(_) => refused += 1,
            }
        }
    }
    assert!(
        accepted > 1000 && refused > 1000,
        "{accepted} accepted, {refused} refused"
    );
}
