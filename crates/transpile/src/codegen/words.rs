//! The host words code generation emits, with their stack effect under the
//! Fallout 2 profile (`vault_vm::extensions::opcode_in(&WordSet::FALLOUT2,
//! ..)`, the SPEC-005 table as amended by SPEC-017). `args` is the operand
//! count; operands are pushed in call order (the first argument first), so
//! the last argument is the first popped. A test compares every row with
//! the engine's own table (`tests/codegen_words.rs`).
//!
//! Only plain Fallout 2 words: no sfall word, no manifest scripting word
//! (owner decision 6).

/// One host word: the assembler mnemonic, the operand count, whether it
/// pushes a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostWord {
    /// The mnemonic (`emit::int::Words::FALLOUT2`).
    pub name: &'static str,
    /// Operands it pops.
    pub args: u8,
    /// Whether it pushes one value.
    pub push: bool,
}

const fn w(name: &'static str, args: u8, push: bool) -> HostWord {
    HostWord { name, args, push }
}

/// `give_exp_points(points)`
pub const GIVE_EXP_POINTS: HostWord = w("give_exp_points", 1, false);
/// `get_pc_stat(stat)`
pub const GET_PC_STAT: HostWord = w("get_pc_stat", 1, true);
/// `tile_contains_pid_obj(hex, elevation, pid)`
pub const TILE_CONTAINS_PID_OBJ: HostWord = w("tile_contains_pid_obj", 3, true);
/// `has_skill(object, skill)`
pub const HAS_SKILL: HostWord = w("has_skill", 2, true);
/// `random(min, max)`
pub const RANDOM: HostWord = w("random", 2, true);
/// `move_to(object, hex, elevation)`
pub const MOVE_TO: HostWord = w("move_to", 3, true);
/// `create_object_sid(pid, hex, elevation, script)`
pub const CREATE_OBJECT_SID: HostWord = w("create_object_sid", 4, true);
/// `display_msg(text)`
pub const DISPLAY_MSG: HostWord = w("display_msg", 1, false);
/// `script_overrides`
pub const SCRIPT_OVERRIDES: HostWord = w("script_overrides", 0, false);
/// `obj_is_carrying_obj_pid(owner, pid)`
pub const OBJ_IS_CARRYING_OBJ_PID: HostWord = w("obj_is_carrying_obj_pid", 2, true);
/// `self_obj`
pub const SELF_OBJ: HostWord = w("self_obj", 0, true);
/// `source_obj`
pub const SOURCE_OBJ: HostWord = w("source_obj", 0, true);
/// `dude_obj`
pub const DUDE_OBJ: HostWord = w("dude_obj", 0, true);
/// `local_var(index)`
pub const LOCAL_VAR: HostWord = w("local_var", 1, true);
/// `set_local_var(index, value)`
pub const SET_LOCAL_VAR: HostWord = w("set_local_var", 2, false);
/// `global_var(index)`
pub const GLOBAL_VAR: HostWord = w("global_var", 1, true);
/// `set_global_var(index, value)`
pub const SET_GLOBAL_VAR: HostWord = w("set_global_var", 2, false);
/// `get_critter_stat(object, stat)`
pub const GET_CRITTER_STAT: HostWord = w("get_critter_stat", 2, true);
/// `animate_move_obj_to_tile(object, hex, flags)`
pub const ANIMATE_MOVE_OBJ_TO_TILE: HostWord = w("animate_move_obj_to_tile", 3, false);
/// `tile_distance_objs(a, b)`
pub const TILE_DISTANCE_OBJS: HostWord = w("tile_distance_objs", 2, true);
/// `tile_distance(a, b)`
pub const TILE_DISTANCE: HostWord = w("tile_distance", 2, true);
/// `tile_num(object)`
pub const TILE_NUM: HostWord = w("tile_num", 1, true);
/// `use_obj(object)`
pub const USE_OBJ: HostWord = w("use_obj", 1, false);
/// `obj_can_see_obj(observer, target)`
pub const OBJ_CAN_SEE_OBJ: HostWord = w("obj_can_see_obj", 2, true);
/// `start_gdialog(list, object, reaction, head, background)`
pub const START_GDIALOG: HostWord = w("start_gdialog", 5, false);
/// `end_dialogue`
pub const END_DIALOGUE: HostWord = w("end_dialogue", 0, false);
/// `set_obj_visibility(object, hidden)`
pub const SET_OBJ_VISIBILITY: HostWord = w("set_obj_visibility", 2, false);
/// `anim_busy(object)`
pub const ANIM_BUSY: HostWord = w("anim_busy", 1, true);
/// `critter_heal(object, amount)`
pub const CRITTER_HEAL: HostWord = w("critter_heal", 2, true);
/// `game_time`
pub const GAME_TIME: HostWord = w("game_time", 0, true);
/// `elevation(object)`
pub const ELEVATION: HostWord = w("elevation", 1, true);
/// `kill_critter(object, animation)`
pub const KILL_CRITTER: HostWord = w("kill_critter", 2, false);
/// `add_timer_event(object, ticks, parameter)`
pub const ADD_TIMER_EVENT: HostWord = w("add_timer_event", 3, false);
/// `destroy_object(object)`
pub const DESTROY_OBJECT: HostWord = w("destroy_object", 1, false);
/// `game_time_hour`
pub const GAME_TIME_HOUR: HostWord = w("game_time_hour", 0, true);
/// `fixed_param`
pub const FIXED_PARAM: HostWord = w("fixed_param", 0, true);
/// `action_being_used`
pub const ACTION_BEING_USED: HostWord = w("action_being_used", 0, true);
/// `critter_state(object)`
pub const CRITTER_STATE: HostWord = w("critter_state", 1, true);
/// `obj_pid(object)`
pub const OBJ_PID: HostWord = w("obj_pid", 1, true);
/// `cur_map_index`
pub const CUR_MAP_INDEX: HostWord = w("cur_map_index", 0, true);
/// `message_str(list, message)`
pub const MESSAGE_STR: HostWord = w("message_str", 2, true);
/// `float_msg(object, text, style)`
pub const FLOAT_MSG: HostWord = w("float_msg", 3, false);
/// `obj_carrying_pid_obj(owner, pid)`
pub const OBJ_CARRYING_PID_OBJ: HostWord = w("obj_carrying_pid_obj", 2, true);
/// `add_mult_objs_to_inven(owner, item, count)`
pub const ADD_MULT_OBJS_TO_INVEN: HostWord = w("add_mult_objs_to_inven", 3, false);
/// `rm_mult_objs_from_inven(owner, item, count)`
pub const RM_MULT_OBJS_FROM_INVEN: HostWord = w("rm_mult_objs_from_inven", 3, true);
/// `gsay_start`
pub const GSAY_START: HostWord = w("gsay_start", 0, false);
/// `gsay_end`
pub const GSAY_END: HostWord = w("gsay_end", 0, false);
/// `gsay_reply(list, message)`
pub const GSAY_REPLY: HostWord = w("gsay_reply", 2, false);
/// `gsay_option(list, message, procedure, reaction)`
pub const GSAY_OPTION: HostWord = w("gsay_option", 4, false);
/// `giq_option(iq, list, message, procedure, reaction)`
pub const GIQ_OPTION: HostWord = w("giq_option", 5, false);
/// `combat_is_initialized`
pub const COMBAT_IS_INITIALIZED: HostWord = w("combat_is_initialized", 0, true);
/// `gdialog_barter(parameter)`
pub const GDIALOG_BARTER: HostWord = w("gdialog_barter", 1, false);
/// `obj_is_locked(object)`
pub const OBJ_IS_LOCKED: HostWord = w("obj_is_locked", 1, true);
/// `obj_lock(object)`
pub const OBJ_LOCK: HostWord = w("obj_lock", 1, false);
/// `obj_unlock(object)`
pub const OBJ_UNLOCK: HostWord = w("obj_unlock", 1, false);
/// `obj_open(object)`
pub const OBJ_OPEN: HostWord = w("obj_open", 1, false);
/// `obj_close(object)`
pub const OBJ_CLOSE: HostWord = w("obj_close", 1, false);
/// `game_ui_disable`
pub const GAME_UI_DISABLE: HostWord = w("game_ui_disable", 0, false);
/// `game_ui_enable`
pub const GAME_UI_ENABLE: HostWord = w("game_ui_enable", 0, false);
/// `attack_setup(attacker, defender)`
pub const ATTACK_SETUP: HostWord = w("attack_setup", 2, false);
/// `move_obj_inven_to_obj(source, destination)`
pub const MOVE_OBJ_INVEN_TO_OBJ: HostWord = w("move_obj_inven_to_obj", 2, false);
/// `critter_add_trait(object, kind, selector, value)`
pub const CRITTER_ADD_TRAIT: HostWord = w("critter_add_trait", 4, true);
/// `anim(object, animation, direction)`
pub const ANIM: HostWord = w("anim", 3, false);
/// `get_month`
pub const GET_MONTH: HostWord = w("get_month", 0, true);
/// `get_day`
pub const GET_DAY: HostWord = w("get_day", 0, true);

/// Every word above, for the table test.
pub const ALL: &[HostWord] = &[
    GIVE_EXP_POINTS,
    GET_PC_STAT,
    TILE_CONTAINS_PID_OBJ,
    HAS_SKILL,
    RANDOM,
    MOVE_TO,
    CREATE_OBJECT_SID,
    DISPLAY_MSG,
    SCRIPT_OVERRIDES,
    OBJ_IS_CARRYING_OBJ_PID,
    SELF_OBJ,
    SOURCE_OBJ,
    DUDE_OBJ,
    LOCAL_VAR,
    SET_LOCAL_VAR,
    GLOBAL_VAR,
    SET_GLOBAL_VAR,
    GET_CRITTER_STAT,
    ANIMATE_MOVE_OBJ_TO_TILE,
    TILE_DISTANCE_OBJS,
    TILE_DISTANCE,
    TILE_NUM,
    USE_OBJ,
    OBJ_CAN_SEE_OBJ,
    START_GDIALOG,
    END_DIALOGUE,
    SET_OBJ_VISIBILITY,
    ANIM_BUSY,
    CRITTER_HEAL,
    GAME_TIME,
    ELEVATION,
    KILL_CRITTER,
    ADD_TIMER_EVENT,
    DESTROY_OBJECT,
    GAME_TIME_HOUR,
    FIXED_PARAM,
    ACTION_BEING_USED,
    CRITTER_STATE,
    OBJ_PID,
    CUR_MAP_INDEX,
    MESSAGE_STR,
    FLOAT_MSG,
    OBJ_CARRYING_PID_OBJ,
    ADD_MULT_OBJS_TO_INVEN,
    RM_MULT_OBJS_FROM_INVEN,
    GSAY_START,
    GSAY_END,
    GSAY_REPLY,
    GSAY_OPTION,
    GIQ_OPTION,
    COMBAT_IS_INITIALIZED,
    GDIALOG_BARTER,
    OBJ_IS_LOCKED,
    OBJ_LOCK,
    OBJ_UNLOCK,
    OBJ_OPEN,
    OBJ_CLOSE,
    GAME_UI_DISABLE,
    GAME_UI_ENABLE,
    ATTACK_SETUP,
    MOVE_OBJ_INVEN_TO_OBJ,
    CRITTER_ADD_TRAIT,
    ANIM,
    GET_MONTH,
    GET_DAY,
];
