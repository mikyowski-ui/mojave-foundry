//! Behaviour (spec 12.1): the testkit town's programs, lowered and run in
//! the engine's VM with the test host: dialogue picks, say-once, the
//! random group, a Fallout 3 challenge passing and failing with the outcome
//! remembered, the New Vegas "25/50" pair, quest stages and objectives,
//! enable groups with an opposite child, a lock with the key and with the
//! skill, a locked terminal with a sub-menu and Back, and a note.

mod support;

use std::sync::OnceLock;

use support::{DUDE, Event, Instance, World, load_messages, town_facts, town_objects, town_pid};
use transpile::ir::{BaseFiles, Module, SimConfig, SimId, SimKind, quest_sub};
use transpile::lower_layer;
use transpile::lowered::{LowerOutput, ProgramPlan};
use transpile::testkit::{self, forms};

struct Town {
    module: Module,
    out: LowerOutput,
}

fn town() -> &'static Town {
    static TOWN: OnceLock<Town> = OnceLock::new();
    TOWN.get_or_init(|| {
        let module = testkit::town();
        let out = lower_layer(
            &module,
            &town_facts(false),
            &BaseFiles::default(),
            &SimConfig::default(),
        )
        .expect("the town lowers");
        Town { module, out }
    })
}

fn program(kind: &str, owner: Option<u32>) -> &'static ProgramPlan {
    town()
        .out
        .plan
        .programs
        .iter()
        .find(|p| p.kind == kind && owner.is_none_or(|o| p.owner == testkit::key(o).to_string()))
        .unwrap_or_else(|| panic!("no {kind} program for {owner:?}"))
}

fn global(id: &SimId) -> usize {
    let module = &town().module;
    module
        .globals_in_allocation_order()
        .iter()
        .position(|g| &g.id == id)
        .unwrap_or_else(|| panic!("no global {id}"))
}

fn quest() -> transpile::ir::FormKey {
    testkit::key(forms::QUEST)
}

/// A world for one program, with every global at its initial value, the
/// town's objects and the program's messages.
fn world_for(plan: &ProgramPlan) -> World {
    let module = &town().module;
    let mut world = World::new(module.globals.len(), plan.local_vars as usize);
    for (i, g) in module.globals_in_allocation_order().iter().enumerate() {
        world.globals[i] = g.initial;
    }
    town_objects(&mut world, &town_facts(false));
    let path = format!("text/english/dialog/{}.msg", plan.stem);
    if let Some(bytes) = town().out.lowered.files.get(&path) {
        load_messages(&mut world, plan.row as i32 + 1, bytes);
    }
    world
}

fn load(plan: &ProgramPlan, world: &mut World) -> Instance {
    if plan.kind == "critter" {
        // E4: the map script runs first and exports the slots.
        let map = program("map", None);
        let bytes = &town().out.lowered.files[&format!("scripts/{}.int", map.stem)];
        let saved = world.self_obj;
        world.self_obj = 0;
        let mut map_script = Instance::load(&map.stem, bytes, world);
        map_script.call(world, "map_enter_p_proc");
        world.self_obj = saved;
    }
    let bytes = &town().out.lowered.files[&format!("scripts/{}.int", plan.stem)];
    Instance::load(&plan.stem, bytes, world)
}

/// The world object of a placed reference.
fn object_of(reference: u32) -> u32 {
    town_facts(false)
        .placement
        .objects
        .iter()
        .find(|o| o.reference == Some(testkit::key(reference)))
        .and_then(|o| o.object_id)
        .expect("placed")
        + 100
}

fn talk(world: &mut World, mayor: &mut Instance) {
    world.source_obj = DUDE;
    let outcome = mayor.call(world, "talk_p_proc");
    assert_eq!(
        outcome,
        vault_vm::RunOutcome::Blocked,
        "talk waits in the dialogue"
    );
}

#[test]
fn the_mayor_greets_once_then_says_the_random_greeting_and_gives_a_topic() {
    let plan = program("critter", Some(forms::MAYOR));
    let mut world = world_for(plan);
    world.self_obj = object_of(forms::R_MAYOR);
    let mut mayor = load(plan, &mut world);

    talk(&mut world, &mut mayor);
    assert_eq!(world.replies(), ["Welcome to town. Mind the door."]);
    assert!(
        world.option_texts().contains(&"Got any work?".to_owned()),
        "{:?}",
        world.option_texts()
    );
    assert!(mayor.choose_text(&mut world, "Got any work?"));
    // The greeting's end block ran when its reply was left: stage 10.
    let stage = global(&SimId::quest_state(&quest(), quest_sub::STAGE));
    assert_eq!(world.globals[stage], 10);
    assert_eq!(world.replies().last().unwrap(), "Find my key.");
    assert!(mayor.choose_text(&mut world, "Where is it?"));
    assert_eq!(world.replies().last().unwrap(), "The guard has a spare.");
    // Goodbye: one Done option; choosing it ends the dialogue.
    assert_eq!(world.option_texts(), ["Done"]);
    assert!(!mayor.choose(&mut world, 0));
    assert_eq!(world.transcript.last(), Some(&Event::EndDialogue));
    let given = global(&SimId::new(SimKind::Topic, testkit::key(forms::RUMOR), 1));
    assert_eq!(world.globals[given], 1, "AddTopic ran in the end block");

    // Say once: the greeting is not said again; the random pair is.
    world.transcript.clear();
    world.randoms.push_back(1);
    talk(&mut world, &mut mayor);
    assert_eq!(world.replies(), ["Nice day."]);
    let options = world.option_texts();
    assert!(
        options.iter().any(|o| o == "Heard anything?"),
        "{options:?}"
    );
    assert!(options.iter().any(|o| o == "Goodbye."), "{options:?}");
    assert!(!mayor.choose_text(&mut world, "Goodbye."));
}

/// Talks until the hub is on offer: the first talk says the say-once
/// greeting, whose tree ends in a goodbye; later talks say the random
/// greeting, which returns to the hub.
fn to_hub(world: &mut World, mayor: &mut Instance) {
    talk(world, mayor);
    if world.option_texts().iter().any(|o| o == "Got any work?") {
        mayor.choose_text(world, "Got any work?");
        mayor.choose_text(world, "Where is it?");
        mayor.choose_text(world, "Done");
        talk(world, mayor);
    }
}

fn bribe(world: &mut World, mayor: &mut Instance) -> String {
    to_hub(world, mayor);
    let text = world
        .option_texts()
        .into_iter()
        .find(|t| t.contains("Give me the key."))
        .unwrap_or_else(|| panic!("no challenge option in {:?}", world.option_texts()));
    mayor.choose_text(world, "Give me the key.");
    text
}

#[test]
fn the_speech_challenge_passes_and_the_outcome_is_kept() {
    let plan = program("critter", Some(forms::MAYOR));
    let mut world = world_for(plan);
    world.self_obj = object_of(forms::R_MAYOR);
    world.skills[14] = 50;
    let mut mayor = load(plan, &mut world);
    world.randoms.extend([30]);
    let label = bribe(&mut world, &mut mayor);
    // round(50 * 2) - 40 (Easy) = 60.
    assert_eq!(label, "[Speech 60%] Give me the key.");
    assert_eq!(world.replies().last().unwrap(), "Fine, take the key.");
    assert!(world.transcript.contains(&Event::Xp(5)));
    // Asked again: no roll, the same branch.
    mayor.choose_text(&mut world, "Goodbye.");
    world.transcript.clear();
    world.randoms.clear();
    world.randoms.extend([1, 100]);
    bribe(&mut world, &mut mayor);
    assert_eq!(world.replies().last().unwrap(), "Fine, take the key.");
}

#[test]
fn the_speech_challenge_fails_to_the_shared_topic_and_stays_failed() {
    let plan = program("critter", Some(forms::MAYOR));
    let mut world = world_for(plan);
    world.self_obj = object_of(forms::R_MAYOR);
    world.skills[14] = 50;
    let mut mayor = load(plan, &mut world);
    world.randoms.extend([90]);
    let _ = bribe(&mut world, &mut mayor);
    assert_eq!(world.replies().last().unwrap(), "Nice try.");
    mayor.choose_text(&mut world, "Goodbye.");
    world.randoms.clear();
    world.randoms.extend([1, 1]);
    let _ = bribe(&mut world, &mut mayor);
    assert_eq!(
        world.replies().last().unwrap(),
        "Nice try.",
        "remembered forever"
    );
}

#[test]
fn the_new_vegas_pair_shows_25_of_50_and_says_the_failure_line() {
    let plan = program("critter", Some(forms::GUARD));
    for (repair, label, line) in [
        (
            25,
            "[Repair 25/50] Let me fix the gate.",
            "You made it worse.",
        ),
        (60, "[Repair 50] Let me fix the gate.", "You fixed it."),
    ] {
        let mut world = world_for(plan);
        world.self_obj = object_of(forms::R_GUARD);
        world.skills[13] = repair;
        let mut guard = load(plan, &mut world);
        talk(&mut world, &mut guard);
        assert!(
            world.option_texts().contains(&label.to_owned()),
            "{:?}",
            world.option_texts()
        );
        guard.choose_text(&mut world, label);
        assert_eq!(world.replies().last().unwrap(), line);
    }
}

#[test]
fn quest_stages_run_in_the_dispatcher_and_objectives_complete() {
    let plan = program("dispatcher", None);
    let mut world = world_for(plan);
    let mut dispatcher = load(plan, &mut world);
    let q = quest();
    let stage = global(&SimId::quest_state(&q, quest_sub::STAGE));
    let pend = global(&SimId::quest_state(&q, quest_sub::PEND));
    let done = global(&SimId::quest_state(&q, quest_sub::DONE));
    let flags = global(&SimId::quest_state(&q, quest_sub::FLAGS));
    let objective = global(&SimId::objective(&q, 10));
    let count = global(&SimId::new(SimKind::QuestVar, q.clone(), 1));
    assert_eq!(world.globals[flags] & 1, 1, "start enabled");
    // Stage 10 was set elsewhere: stage global, done and pending bits
    // (stage 10 is the second stage by index).
    let (word, bit) =
        transpile::quest::stage_bit(&town().module.quests[&SimId::of(SimKind::Quest, &q)], 10)
            .unwrap();
    assert_eq!(word, 0);
    world.globals[stage] = 10;
    world.globals[done] |= 1 << bit;
    world.globals[pend] |= 1 << bit;
    dispatcher.call(&mut world, "start");
    assert_eq!(world.globals[pend], 0, "the pending bit is cleared");
    assert_eq!(
        world.globals[objective], 1,
        "the stage script displays objective 10"
    );
    assert_eq!(world.globals[count], 1);
    // The quest script completes it once qCount > 3, by game time.
    world.globals[count] = 5;
    world.game_time += 100;
    dispatcher.call(&mut world, "start");
    assert_eq!(world.globals[objective], 2, "completed");
}

#[test]
fn the_door_switches_the_group_and_the_map_script_applies_it() {
    let map = program("map", None);
    let door = program("object", Some(forms::DOOR));
    let mut world = world_for(door);
    let mut map_script = load(map, &mut world);
    map_script.call(&mut world, "map_enter_p_proc");
    let child = object_of(forms::R_CHILD);
    let opposite = object_of(forms::R_OPPOSITE);
    assert!(world.objects[&child].hidden, "the parent starts disabled");
    assert!(!world.objects[&opposite].hidden);
    world.self_obj = object_of(forms::R_DOOR);
    world.source_obj = DUDE;
    let mut door_program = load(door, &mut world);
    door_program.call(&mut world, "use_p_proc");
    assert!(!world.objects[&child].hidden, "enabled at once");
    assert!(
        world.objects[&opposite].hidden,
        "the opposite child is hidden"
    );
    assert!(
        !world.transcript.contains(&Event::Overrides),
        "Activate ran"
    );
    let group = global(&SimId::of(SimKind::Group, &testkit::key(forms::R_PARENT)));
    assert_eq!(world.globals[group], 1);
}

#[test]
fn the_lock_opens_with_the_key_or_the_skill() {
    let door = program("object", Some(forms::DOOR));
    // With the key.
    let mut world = world_for(door);
    world.self_obj = object_of(forms::R_DOOR);
    world.source_obj = 0;
    world
        .objects
        .get_mut(&DUDE)
        .unwrap()
        .inventory
        .push(town_pid(forms::KEY) as i32);
    let mut program = load(door, &mut world);
    program.call(&mut world, "use_p_proc");
    assert!(!world.objects[&world.self_obj].locked);
    assert!(
        world
            .transcript
            .contains(&Event::Display("Unlocked with the key.".into()))
    );
    // With Lockpick 50 (level 50), and without.
    for (skill, opens) in [(50, true), (40, false)] {
        let mut world = world_for(door);
        world.self_obj = object_of(forms::R_DOOR);
        world.action = 9;
        world.skills[9] = skill;
        let mut program = load(door, &mut world);
        program.call(&mut world, "use_skill_on_p_proc");
        assert_eq!(
            !world.objects[&world.self_obj].locked, opens,
            "skill {skill}"
        );
        assert!(world.transcript.contains(&Event::Overrides));
        if opens {
            assert!(world.transcript.contains(&Event::Xp(40)));
        } else {
            assert!(
                world
                    .transcript
                    .contains(&Event::Display("The lock is too hard.".into()))
            );
        }
    }
}

#[test]
fn the_terminal_hacks_open_and_its_sub_menu_goes_back() {
    let plan = program("terminal", Some(forms::TERMINAL));
    let mut world = world_for(plan);
    world.self_obj = object_of(forms::R_TERMINAL);
    world.skills[12] = 30;
    let mut terminal = load(plan, &mut world);
    assert_eq!(
        terminal.call(&mut world, "use_p_proc"),
        vault_vm::RunOutcome::Blocked
    );
    assert_eq!(world.replies(), ["WELCOME, CITIZEN"]);
    assert_eq!(world.option_texts(), ["[Science 25] Hack", "Exit"]);
    assert!(terminal.choose_text(&mut world, "Hack"));
    assert!(world.transcript.contains(&Event::Xp(30)));
    assert_eq!(
        world.option_texts(),
        ["Read the log", "Maintenance", "Exit"]
    );
    assert!(terminal.choose_text(&mut world, "Maintenance"));
    assert_eq!(world.replies().last().unwrap(), "MAINTENANCE");
    assert_eq!(world.option_texts(), ["Run diagnostics", "Back"]);
    assert!(terminal.choose_text(&mut world, "Run diagnostics"));
    assert_eq!(world.replies().last().unwrap(), "All systems nominal.");
    assert!(terminal.choose_text(&mut world, "Back"));
    assert_eq!(world.replies().last().unwrap(), "WELCOME, CITIZEN");
    assert!(!terminal.choose_text(&mut world, "Exit"));
    assert!(world.transcript.contains(&Event::EndDialogue));
    // Opened once, it stays open.
    world.transcript.clear();
    terminal.call(&mut world, "use_p_proc");
    assert!(world.option_texts().contains(&"Maintenance".to_owned()));
}

#[test]
fn the_terminal_offers_the_password_to_the_holder_of_the_note() {
    let plan = program("terminal", Some(forms::TERMINAL));
    let mut world = world_for(plan);
    world.self_obj = object_of(forms::R_TERMINAL);
    let note = global(&SimId::of(SimKind::Note, &testkit::key(forms::NOTE)));
    world.globals[note] = 1;
    let mut terminal = load(plan, &mut world);
    terminal.call(&mut world, "use_p_proc");
    assert_eq!(world.option_texts(), ["[Password]", "Exit"]);
    assert!(terminal.choose_text(&mut world, "[Password]"));
    assert!(world.option_texts().contains(&"Read the log".to_owned()));
}

#[test]
fn picking_up_the_note_adds_it() {
    let plan = program("note", Some(forms::NOTE));
    let mut world = world_for(plan);
    world.self_obj = object_of(forms::R_NOTE);
    let mut note = load(plan, &mut world);
    note.call(&mut world, "pickup_p_proc");
    let g = global(&SimId::of(SimKind::Note, &testkit::key(forms::NOTE)));
    assert_eq!(world.globals[g], 1);
    assert!(
        world
            .transcript
            .contains(&Event::Display("Note added: Mayor's Log".into()))
    );
}

#[test]
fn the_raider_attacks_on_sight_and_the_mayor_walks_her_package() {
    let plan = program("critter", Some(forms::RAIDER));
    let mut world = world_for(plan);
    world.self_obj = object_of(forms::R_RAIDER);
    let mut raider = load(plan, &mut world);
    world.can_see = true;
    raider.call(&mut world, "critter_p_proc");
    assert!(world.transcript.contains(&Event::Attack));

    let plan = program("critter", Some(forms::MAYOR));
    let mut world = world_for(plan);
    world.self_obj = object_of(forms::R_MAYOR);
    world.game_time = 9 * 36_000;
    let mut mayor = load(plan, &mut world);
    world.randoms.extend([200, 1]);
    mayor.call(&mut world, "critter_p_proc");
    assert!(
        world.transcript.iter().any(|e| matches!(e, Event::Move(_))),
        "{:?}",
        world.transcript
    );
}
