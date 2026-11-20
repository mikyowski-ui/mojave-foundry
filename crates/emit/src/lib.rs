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
//! - [`ArtList`]: art list entries under the 4,096-entry FID budget.
//! - [`critter`]: critter animations ([`critter_frm`], [`still_frm`]);
//!   [`CritterList`] for `art/critters/critters.lst`;
//!   [`proto::critter_prototype`] and [`map::critter_object`] for the
//!   prototype and the placed record (`docs/contracts.md` section 10).
//! - [`proto`]: prototype constructors, lists, PIDs and messages.
//! - [`ModLayer`]: the mod folder and its manifest.

/// The pinned Fallout 2 format crate every adapter here targets.
pub use vault_formats;

pub mod art;
pub mod critter;
mod error;
pub mod frm;
pub mod gam;
pub mod layer;
pub mod lst;
pub mod map;
pub mod msg;
pub mod proto;
mod writers;

pub use art::{ArtList, CRITTER_LIST_PATH, CritterArt, CritterList, FID_INDEX_CAP, critter_fid};
pub use critter::{CritterFrames, HEX_STEPS, RenderedFrame, critter_frm, still_frm};
pub use error::EmitError;
pub use frm::{
    CroppedImage, FrmDirection, FrmFrameSpec, FrmSpec, PlacedFrame, crop_to_content, placed_frame,
    sprite_shift,
};
pub use gam::GamVar;
pub use layer::ModLayer;
pub use lst::LstLine;
pub use map::{CritterPlacement, critter_object};
pub use msg::{MsgEntry, MsgFile};
pub use proto::{CritterSpec, ProtoList, critter_prototype};
pub use vault_formats::GamSection;
pub use writers::{FormatWriters, LocalWriters};
