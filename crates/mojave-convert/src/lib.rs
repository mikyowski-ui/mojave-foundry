//! The mojave-convert pipeline as a library, for Fallout 3 and Fallout:
//! New Vegas installs alike (`source`). The binary in `main.rs` is a
//! thin argument parser over these stages, so integration tests can run
//! either.

pub mod config;
pub mod critters;
pub mod declutter;
pub mod gallery;
pub mod map;
pub mod pipeline;
pub mod probe;
pub mod replay;
pub mod route;
pub mod sequence;
pub mod shots;
pub mod source;
pub mod sprites;

pub use config::{ConvertError, Inputs, SliceSpec};
pub use mir::GameVariant;
pub use source::Install;
