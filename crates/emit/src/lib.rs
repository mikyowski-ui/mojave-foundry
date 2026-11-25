//! Emission adapters for FRM, LST, MSG, GAM, PRO, MAP and the mod manifest.
//!
//! Every writer here targets the pinned `vault-formats` parser: what it
//! writes, that parser reads back field for field, and what that parser
//! would read differently (or not at all) is refused with an
//! [`EmitError`]. The interface shared with other crates is
//! `docs/contracts.md` section 8.3.
//!
//! - [`FormatWriters`] / [`LocalWriters`]: the file writers, behind one
//!   trait so the upstream writers can replace them in one file.
//! - [`frm`]: sprite placement ([`sprite_shift`], [`crop_to_content`]).
//! - [`ArtList`]: art list entries under the 4,096-entry FID budget;
//!   [`ArtList::standalone`] for a walls or tiles list that does not
//!   start from the base list.
//! - [`critter`]: critter animations ([`critter_frm`], [`still_frm`]);
//!   [`CritterList`] for `art/critters/critters.lst`;
//!   [`proto::critter_prototype`] and [`map::critter_object`] for the
//!   prototype and the placed record (`docs/contracts.md` section 10).
//! - [`proto`]: prototype constructors, lists, PIDs and messages.
//! - [`items`]: weapons, armour, ammunition, drugs, keys and powered
//!   items; [`transit`]: stairs, ladders, elevators; [`fighter`]:
//!   critters that fight.
//! - [`objects`]: map objects with state (containers with contents,
//!   locked doors, loaded weapons, armed critters).
//! - [`text`]: names and descriptions for the prototype message files.
//! - [`icon`]: inventory icons and the item art pair.
//! - [`int`]: the script assembler ([`int::assemble`]), from the text
//!   format of `docs/vault-engine-briefs/int-assembler.md` to `.int` bytes.
//! - [`animation`]: critter animation and weapon codes, suffixes, and the
//!   files a fighting critter needs.
//! - [`rebalance`]: source-game figures to Fallout 2 prototype values.
//! - [`append`]: appenders for the data files a simulation layer extends
//!   (messages, `vault13.gam`, `scripts.lst`, `quests.txt`, `holodisk.txt`).
//! - [`map::attach_scripts`]: script records and bindings in a built map.
//! - [`ModLayer`]: the mod folder and its manifest.

/// The pinned Fallout 2 format crate every adapter here targets.
pub use vault_formats;

pub mod animation;
pub mod append;
pub mod art;
pub mod critter;
mod error;
pub mod fighter;
pub mod frm;
pub mod gam;
pub mod icon;
pub mod int;
pub mod items;
pub mod layer;
pub mod lst;
pub mod map;
pub mod msg;
pub mod objects;
mod overlay;
pub mod proto;
pub mod rebalance;
pub mod text;
pub mod transit;
mod writers;

pub use animation::{Anim, AnimationSet, AttackMode, Need, Requirement, WeaponCode};
pub use append::{
    Appended, HolodiskRow, QuestRow, ScriptRow, append_globals, append_holodisks,
    append_message_entries, append_messages, append_quests, append_script_rows,
};
pub use art::{
    ArtList, CRITTER_LIST_PATH, CritterArt, CritterList, FID_INDEX_CAP, RESERVED_TILE_ENTRIES,
    TILE_CATEGORY, WALL_CATEGORY, critter_fid, standalone_reserved,
};
pub use critter::{
    CritterFrames, FrmOptions, HEX_STEPS, Motion, RenderedFrame, critter_frm, critter_frm_with,
    still_frm,
};
pub use error::EmitError;
pub use fighter::{FighterSpec, fighter_prototype};
pub use frm::{
    CroppedImage, FrmDirection, FrmFrameSpec, FrmSpec, PlacedFrame, crop_to_content, placed_frame,
    sprite_shift,
};
pub use gam::GamVar;
pub use icon::ItemArt;
pub use items::{AmmoSpec, ArmorSpec, DamageType, DrugSpec, KeySpec, MiscSpec, Skill, WeaponSpec};
pub use layer::ModLayer;
pub use lst::LstLine;
pub use map::{
    CritterPlacement, MapEntry, ObjectScript, ScriptAttachment, ScriptSite, attach_scripts,
    attach_scripts_to, critter_object, map_with_elevations,
};
pub use msg::{MsgEntry, MsgFile};
pub use objects::{Loadout, LockState, ProtoSet, Stack};
pub use proto::{CritterSpec, ProtoList, critter_prototype};
pub use rebalance::Rebalance;
pub use text::{Described, ProtoText, describe};
pub use vault_formats::GamSection;
pub use writers::{FormatWriters, LocalWriters};
