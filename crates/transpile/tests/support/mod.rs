//! The VM test host (spec 12.1 "Behaviour"): the engine's own `vault_vm`
//! loader, machine and stack model, with a small world implementing the
//! host words the generated programs use, a recorded dialogue transcript,
//! scripted `random`, and the dialogue loop of SPEC-006 section 3.9 as the
//! engine drives it (`gsay_end` blocks; a chosen option's procedure runs
//! as a host call; a callback that builds no option ends the loop).
//!
//! Synthetic data only.

#![allow(dead_code)]

use std::collections::{BTreeMap, VecDeque};

use transpile::ir::{FormKey, SimId, SimKind};
use transpile::lowered::LowerFacts;
use transpile::testkit::{self, forms};
use vault_vm::extensions::{self, HostCall};
use vault_vm::stack_check::{self, Table};
use vault_vm::{ExternalValue, IntProgram, RunOutcome, Value, Vm, VmError, VmHost, WordSet};

/// Parses `bytes` under the Fallout 2 word set and runs the engine's stack
/// model on it: both must accept it, with no finding.
pub fn assert_program_ok(name: &str, bytes: &[u8]) -> IntProgram {
    let program = IntProgram::parse_in(bytes, &WordSet::FALLOUT2, false)
        .unwrap_or_else(|e| panic!("{name}: the loader refuses it: {e}"));
    let findings = stack_check::check(name, &program, &Table::new(WordSet::FALLOUT2), &[]);
    assert!(
        findings.is_empty(),
        "{name}: stack model findings: {}",
        stack_check::summary(&findings)
    );
    program
}

/// One recorded event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    StartDialogue,
    Reply(String),
    Display(String),
    Float(String),
    Xp(i32),
    EndDialogue,
    Overrides,
    Attack,
    Move(i32),
    Barter,
}

/// A world object.
#[derive(Debug, Clone, Default)]
pub struct Obj {
    pub pid: i32,
    pub hex: i32,
    pub elevation: i32,
    pub hidden: bool,
    pub locked: bool,
    pub open: bool,
    pub inventory: Vec<i32>,
}

/// The test world.
#[derive(Debug, Default)]
pub struct World {
    pub globals: Vec<i32>,
    pub locals: Vec<i32>,
    /// Message list number to id to text.
    pub msgs: BTreeMap<i32, BTreeMap<i32, String>>,
    pub objects: BTreeMap<u32, Obj>,
    pub self_obj: u32,
    pub source_obj: u32,
    pub randoms: VecDeque<i32>,
    pub transcript: Vec<Event>,
    pub options: Vec<(String, u32)>,
    pub game_time: i32,
    pub map_index: i32,
    pub action: i32,
    pub fixed: i32,
    pub exports: BTreeMap<Vec<u8>, i32>,
    /// The player's skills (Fallout 2 order) and stats.
    pub skills: [i32; 18],
    pub stats: BTreeMap<i32, i32>,
    pub can_see: bool,
    pub timers: Vec<(u32, i32, i32)>,
    /// Team numbers set with `critter_add_trait(obj, 1, 6, team)`, by
    /// object.
    pub teams: BTreeMap<u32, i32>,
    in_dialogue: bool,
}

pub const DUDE: u32 = 1;

impl World {
    pub fn new(globals: usize, locals: usize) -> Self {
        let mut world = Self {
            globals: vec![0; globals],
            locals: vec![0; locals],
            map_index: 1,
            ..Self::default()
        };
        world.objects.insert(DUDE, Obj::default());
        world.stats.insert(4, 5);
        world
    }

    fn text(&self, list: i32, id: i32) -> String {
        if list == -2 && id == -2 {
            return "Done".into();
        }
        self.msgs
            .get(&list)
            .and_then(|m| m.get(&id))
            .cloned()
            .unwrap_or_else(|| format!("<missing {list}:{id}>"))
    }

    fn arg_text(&self, call: &HostCall, message: usize, list: usize) -> String {
        match call.text(message) {
            Some(text) => String::from_utf8_lossy(text).into_owned(),
            None => self.text(call.int(list), call.int(message)),
        }
    }

    fn random(&mut self, min: i32, max: i32) -> i32 {
        self.randoms.pop_front().unwrap_or(min).clamp(min, max)
    }

    /// The replies said so far.
    pub fn replies(&self) -> Vec<String> {
        self.transcript
            .iter()
            .filter_map(|e| match e {
                Event::Reply(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    /// The texts of the options on offer.
    pub fn option_texts(&self) -> Vec<String> {
        self.options.iter().map(|(t, _)| t.clone()).collect()
    }
}

fn int(vm: &mut Vm, value: i32) -> Result<(), VmError> {
    vm.value_stack.push(Value::integer(value))
}

impl VmHost for World {
    fn extension(&mut self, opcode: u16, vm: &mut Vm) -> Result<(), VmError> {
        let call = extensions::decode_in(vm, &WordSet::FALLOUT2, opcode)?;
        let name = call.spec.name;
        let obj = |i: usize| call.uint(i);
        match name {
            "global_var" => {
                let v = *self
                    .globals
                    .get(call.int(0) as usize)
                    .expect("global in range");
                int(vm, v)?;
            }
            "set_global_var" => {
                let i = call.int(1) as usize;
                *self.globals.get_mut(i).expect("global in range") = call.int(0);
            }
            "local_var" => {
                let v = *self
                    .locals
                    .get(call.int(0) as usize)
                    .expect("local in range");
                int(vm, v)?;
            }
            "set_local_var" => {
                let i = call.int(1) as usize;
                *self.locals.get_mut(i).expect("local in range") = call.int(0);
            }
            "self_obj" => int(vm, self.self_obj as i32)?,
            "source_obj" => int(vm, self.source_obj as i32)?,
            "dude_obj" => int(vm, DUDE as i32)?,
            "random" => {
                let v = self.random(call.int(1), call.int(0));
                int(vm, v)?;
            }
            "has_skill" => int(vm, self.skills[call.int(0) as usize])?,
            "get_critter_stat" => int(vm, *self.stats.get(&call.int(0)).unwrap_or(&5))?,
            "get_pc_stat" => int(vm, 0)?,
            "game_time" => int(vm, self.game_time)?,
            "game_time_hour" => int(vm, (self.game_time / 36_000 % 24) * 100)?,
            "get_month" | "get_day" => int(vm, 1)?,
            "cur_map_index" => int(vm, self.map_index)?,
            "action_being_used" => int(vm, self.action)?,
            "fixed_param" => int(vm, self.fixed)?,
            "message_str" => {
                let text = self.text(call.int(1), call.int(0));
                let value = vm.intern(text.as_bytes())?;
                vm.value_stack.push(value)?;
            }
            "display_msg" => {
                let text = String::from_utf8_lossy(call.text(0).unwrap_or_default()).into_owned();
                self.transcript.push(Event::Display(text));
            }
            "float_msg" => {
                let text = String::from_utf8_lossy(call.text(1).unwrap_or_default()).into_owned();
                self.transcript.push(Event::Float(text));
            }
            "give_exp_points" => self.transcript.push(Event::Xp(call.int(0))),
            "script_overrides" => self.transcript.push(Event::Overrides),
            "start_gdialog" => {
                self.in_dialogue = true;
                self.transcript.push(Event::StartDialogue);
            }
            "gsay_start" => self.options.clear(),
            "gsay_reply" => {
                let text = self.arg_text(&call, 0, 1);
                self.options.clear();
                self.transcript.push(Event::Reply(text));
            }
            "gsay_option" | "giq_option" => {
                if name == "giq_option" && self.stats[&4] < call.int(4) {
                    return Ok(());
                }
                let text = self.arg_text(&call, 2, 3);
                self.options.push((text, call.uint(1)));
            }
            "gsay_end" => {
                if self.options.is_empty() {
                    self.options.push(("(end)".into(), 0));
                }
                vm.status.0 |= 0x20;
            }
            "end_dialogue" => {
                self.in_dialogue = false;
                self.transcript.push(Event::EndDialogue);
            }
            "gdialog_barter" => self.transcript.push(Event::Barter),
            "tile_contains_pid_obj" => {
                let (pid, elevation, hex) = (call.int(0), call.int(1), call.int(2));
                let found = self
                    .objects
                    .iter()
                    .find(|(_, o)| o.pid == pid && o.elevation == elevation && o.hex == hex)
                    .map_or(0, |(id, _)| *id as i32);
                int(vm, found)?;
            }
            "set_obj_visibility" => {
                if let Some(o) = self.objects.get_mut(&obj(1)) {
                    o.hidden = call.int(0) != 0;
                }
            }
            "obj_is_locked" => int(
                vm,
                i32::from(self.objects.get(&obj(0)).is_some_and(|o| o.locked)),
            )?,
            "obj_lock" | "obj_unlock" => {
                if let Some(o) = self.objects.get_mut(&obj(0)) {
                    o.locked = name == "obj_lock";
                }
            }
            "obj_open" | "obj_close" => {
                if let Some(o) = self.objects.get_mut(&obj(0)) {
                    o.open = name == "obj_open";
                }
            }
            "obj_is_carrying_obj_pid" => {
                let n = self.objects.get(&obj(1)).map_or(0, |o| {
                    o.inventory.iter().filter(|p| **p == call.int(0)).count()
                });
                int(vm, n as i32)?;
            }
            "obj_pid" => int(vm, self.objects.get(&obj(0)).map_or(-1, |o| o.pid))?,
            "tile_num" => int(vm, self.objects.get(&obj(0)).map_or(-1, |o| o.hex))?,
            "elevation" => int(vm, self.objects.get(&obj(0)).map_or(0, |o| o.elevation))?,
            "critter_state" => int(vm, 0)?,
            "anim_busy" => int(vm, 0)?,
            "combat_is_initialized" => int(vm, 0)?,
            "obj_can_see_obj" => int(vm, i32::from(self.can_see))?,
            "tile_distance_objs" => int(vm, 5)?,
            "animate_move_obj_to_tile" => self.transcript.push(Event::Move(call.int(1))),
            "attack_setup" => self.transcript.push(Event::Attack),
            "critter_add_trait" => {
                if (call.int(2), call.int(1)) == (1, 6) {
                    self.teams.insert(obj(3), call.int(0));
                }
                int(vm, -1)?;
            }
            "add_timer_event" => self.timers.push((obj(2), call.int(1), call.int(0))),
            "critter_heal"
            | "rm_mult_objs_from_inven"
            | "move_to"
            | "create_object_sid"
            | "obj_carrying_pid_obj" => int(vm, 0)?,
            "kill_critter"
            | "destroy_object"
            | "use_obj"
            | "game_ui_disable"
            | "game_ui_enable"
            | "add_mult_objs_to_inven"
            | "move_obj_inven_to_obj"
            | "anim" => {}
            other => return Err(VmError::Host(format!("test host: {other} not implemented"))),
        }
        Ok(())
    }

    fn export_variable(&mut self, _owner: u32, name: &[u8]) -> Result<(), VmError> {
        self.exports.entry(name.to_ascii_lowercase()).or_insert(0);
        Ok(())
    }

    fn fetch_variable(&mut self, name: &[u8]) -> Result<ExternalValue, VmError> {
        self.exports
            .get(&name.to_ascii_lowercase())
            .map(|v| ExternalValue::Value(Value::integer(*v)))
            .ok_or_else(|| VmError::Symbol(String::from_utf8_lossy(name).into()))
    }

    fn store_variable(&mut self, name: &[u8], value: ExternalValue) -> Result<(), VmError> {
        let slot = self
            .exports
            .get_mut(&name.to_ascii_lowercase())
            .ok_or_else(|| VmError::Symbol(String::from_utf8_lossy(name).into()))?;
        if let ExternalValue::Value(v) = value {
            *slot = v.payload as i32;
        }
        Ok(())
    }
}

/// One program instance.
pub struct Instance {
    pub vm: Vm,
}

fn run(vm: &mut Vm, world: &mut World) -> RunOutcome {
    for _ in 0..10_000 {
        match vm.run(world, 1000) {
            Ok(RunOutcome::Budget) => {}
            Ok(outcome) => return outcome,
            Err(error) => panic!("VM error: {error}"),
        }
    }
    panic!("the program did not finish");
}

impl Instance {
    /// Loads a program and runs its initial stream (which runs `start`).
    pub fn load(name: &str, bytes: &[u8], world: &mut World) -> Self {
        let program = assert_program_ok(name, bytes);
        let mut vm = Vm::new(program);
        let outcome = run(&mut vm, world);
        assert_eq!(outcome, RunOutcome::Ended, "{name}: the initial stream");
        Self { vm }
    }

    fn index(&self, name: &str) -> usize {
        self.vm
            .program
            .find_procedure(name.as_bytes(), true)
            .filter(|i| *i != 0)
            .unwrap_or(1)
    }

    /// Whether the program has the procedure.
    pub fn has(&self, name: &str) -> bool {
        self.vm
            .program
            .find_procedure(name.as_bytes(), true)
            .is_some()
    }

    /// A host call of a procedure by name (index 1 when it is missing).
    pub fn call(&mut self, world: &mut World, name: &str) -> RunOutcome {
        let index = self.index(name);
        self.call_index(world, index)
    }

    fn call_index(&mut self, world: &mut World, index: usize) -> RunOutcome {
        self.vm.prepare_host_call(index, true).expect("host call");
        run(&mut self.vm, world)
    }

    /// Chooses option `i` of the dialogue on offer (SPEC-006 section 3.9).
    /// Returns whether the dialogue goes on.
    pub fn choose(&mut self, world: &mut World, i: usize) -> bool {
        let (_, procedure) = world.options[i].clone();
        world.options.clear();
        if procedure != 0 {
            let outcome = self.call_index(world, procedure as usize);
            assert_eq!(outcome, RunOutcome::Yielded, "a callback returns");
            if !world.options.is_empty() {
                return true;
            }
        }
        self.vm.status.0 &= !0x20;
        let outcome = run(&mut self.vm, world);
        assert_eq!(outcome, RunOutcome::Yielded, "talk returns after the loop");
        false
    }

    /// Chooses the option whose text contains `text`.
    pub fn choose_text(&mut self, world: &mut World, text: &str) -> bool {
        let i = world
            .options
            .iter()
            .position(|(t, _)| t.contains(text))
            .unwrap_or_else(|| panic!("no option {text:?} in {:?}", world.option_texts()));
        self.choose(world, i)
    }
}

/// The town's PIDs, made up for the tests.
pub fn town_pid(base: u32) -> u32 {
    match base {
        forms::MARKER => 0x0200_0001,
        forms::DOOR => 0x0200_0002,
        forms::TERMINAL => 0x0200_0003,
        forms::NOTE => 0x0000_0004,
        forms::KEY => 0x0000_0005,
        other => 0x0100_0000 + other,
    }
}

/// The town's facts: its placement with PIDs, the key's PID, the door's
/// script.
pub fn town_facts(reversed: bool) -> LowerFacts {
    let mut facts = LowerFacts::of_slice(&testkit::slice(reversed));
    for object in &mut facts.placement.objects {
        let base = object.base.as_ref().map(local_id).unwrap_or(0);
        object.pid = Some(town_pid(base));
    }
    facts
        .pids
        .insert(testkit::key(forms::KEY), town_pid(forms::KEY));
    facts.base_scripts.insert(
        testkit::key(forms::DOOR),
        SimId::of(SimKind::Script, &testkit::key(forms::DOOR_SCRIPT)),
    );
    facts
}

/// The local id of a town form key.
pub fn local_id(key: &FormKey) -> u32 {
    key.as_str()
        .rsplit_once(':')
        .and_then(|(_, id)| u32::from_str_radix(id, 16).ok())
        .unwrap_or(0)
}

/// The world objects of the town's placement.
pub fn town_objects(world: &mut World, facts: &LowerFacts) {
    for object in &facts.placement.objects {
        world.objects.insert(
            object.object_id.unwrap_or(0) + 100,
            Obj {
                pid: object.pid.unwrap_or(0) as i32,
                hex: object.hex as i32,
                elevation: i32::from(object.elevation),
                hidden: object.hidden,
                locked: object.lock.is_some(),
                ..Obj::default()
            },
        );
    }
}

/// Reads a generated message file into the world as list `list`.
pub fn load_messages(world: &mut World, list: i32, bytes: &[u8]) {
    let catalog = vault_formats::MsgCatalog::parse_strict(bytes).expect("message file parses");
    let mut out = BTreeMap::new();
    for (id, message) in catalog.entries() {
        out.insert(*id, String::from_utf8_lossy(&message.text).into_owned());
    }
    world.msgs.insert(list, out);
}
