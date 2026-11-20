//! The mojave-convert pipeline as a library. The binary in `main.rs` is a
//! thin argument parser over these stages, so integration tests can run
//! either.

pub mod config;
pub mod critters;
pub mod gallery;
pub mod map;
pub mod pipeline;
pub mod probe;
pub mod sprites;

pub use config::{ConvertError, Inputs, SliceSpec};
