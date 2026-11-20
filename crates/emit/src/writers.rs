//! The swap point. `FormatWriters` is what the pipeline calls; when
//! `vault-formats` ships production writers (see
//! `docs/vault-engine-briefs/`), `LocalWriters` is reimplemented over them
//! here, and nothing else changes.

use crate::gam::{GamVar, write_gam};
use crate::lst::{LstLine, write_lst};
use crate::msg::{MsgEntry, write_msg};
use crate::{EmitError, FrmSpec, frm::write_frm, proto::check_prototype};
use vault_formats::{GamSection, MapFile, Prototype};

/// Serializers for every Fallout 2 file the pipeline emits.
pub trait FormatWriters {
    fn frm(&self, spec: &FrmSpec) -> Result<Vec<u8>, EmitError>;
    /// All lines but blank ones are of one list kind.
    fn lst(&self, lines: &[LstLine]) -> Result<Vec<u8>, EmitError>;
    fn msg(&self, entries: &[MsgEntry]) -> Result<Vec<u8>, EmitError>;
    fn gam(&self, section: GamSection, vars: &[GamVar]) -> Result<Vec<u8>, EmitError>;
    /// Wraps `Prototype::write`, after [`check_prototype`].
    fn pro(&self, prototype: &Prototype) -> Result<Vec<u8>, EmitError>;
    /// Wraps `MapFile::write`.
    fn map(&self, map: &MapFile) -> Result<Vec<u8>, EmitError>;
}

/// This repository's writers.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalWriters;

impl FormatWriters for LocalWriters {
    fn frm(&self, spec: &FrmSpec) -> Result<Vec<u8>, EmitError> {
        write_frm(spec)
    }
    fn lst(&self, lines: &[LstLine]) -> Result<Vec<u8>, EmitError> {
        write_lst(lines)
    }
    fn msg(&self, entries: &[MsgEntry]) -> Result<Vec<u8>, EmitError> {
        write_msg(entries)
    }
    fn gam(&self, section: GamSection, vars: &[GamVar]) -> Result<Vec<u8>, EmitError> {
        write_gam(section, vars)
    }
    fn pro(&self, prototype: &Prototype) -> Result<Vec<u8>, EmitError> {
        check_prototype(prototype)?;
        Ok(prototype.write())
    }
    fn map(&self, map: &MapFile) -> Result<Vec<u8>, EmitError> {
        Ok(map.write()?)
    }
}
