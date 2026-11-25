//! Scripts: the `SCPT` record and the script blocks embedded in quest
//! stages and dialogue.

use serde::{Deserialize, Serialize};

use super::{Common, typed_record};
use crate::bytes::{Rd, Wr, one_form, one_u32, push, set, zstring, zstring_bytes};
use crate::formid::FormId;
use crate::strings::{decode_cp1252, encode_cp1252_lossy};
use crate::subrecord::{Out, RawSubrecord, SubrecordRef};

/// The script header (`SCHR`, 20 bytes; the first four are unused).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ScriptHeader {
    /// Number of references (`SCRO` / `SCRV`).
    pub ref_count: u32,
    /// Size of the compiled bytes.
    pub compiled_size: u32,
    /// Number of local variables.
    pub variable_count: u32,
    /// Script type: 0 object, 1 quest, 0x100 effect.
    pub script_type: u16,
    /// Flags; bit 0 is "enabled".
    pub flags: u16,
}

impl ScriptHeader {
    fn parse(data: &[u8]) -> Option<Self> {
        let mut rd = Rd::exact(data, 20)?;
        rd.skip(4)?;
        Some(Self {
            ref_count: rd.u32()?,
            compiled_size: rd.u32()?,
            variable_count: rd.u32()?,
            script_type: rd.u16()?,
            flags: rd.u16()?,
        })
    }

    fn bytes(&self) -> Vec<u8> {
        Wr::default()
            .zeros(4)
            .u32(self.ref_count)
            .u32(self.compiled_size)
            .u32(self.variable_count)
            .u16(self.script_type)
            .u16(self.flags)
            .done()
    }
}

/// A local variable: an `SLSD` and the `SCVR` name that follows it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LocalVariable {
    /// The variable's index.
    pub index: u32,
    /// Flags; bit 0 marks an integer variable.
    pub flags: u8,
    /// The variable's name (`SCVR`).
    pub name: Option<String>,
}

/// One entry of a script's reference list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScriptReference {
    /// A form (`SCRO`).
    Form(FormId),
    /// A local variable index (`SCRV`).
    Local(u32),
}

/// The subrecords that make up one script, wherever it is embedded.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ScriptBody {
    /// The header (`SCHR`).
    pub header: Option<ScriptHeader>,
    /// The compiled bytes (`SCDA`), as stored.
    pub compiled: Option<Vec<u8>>,
    /// The source text (`SCTX`): every byte of the subrecord, decoded as
    /// Windows-1252. It is present whenever the record has an `SCTX`.
    pub source: Option<String>,
    /// Local variables, in file order.
    pub locals: Vec<LocalVariable>,
    /// References, in file order; compiled code indexes this list from 1.
    pub references: Vec<ScriptReference>,
}

impl ScriptBody {
    /// A script with a header and source text, nothing compiled.
    pub fn from_source(source: &str) -> Self {
        Self {
            header: Some(ScriptHeader {
                flags: 1,
                ..ScriptHeader::default()
            }),
            source: Some(source.to_owned()),
            ..Self::default()
        }
    }

    /// Whether the script has no subrecords at all.
    pub fn is_empty(&self) -> bool {
        self.header.is_none()
            && self.compiled.is_none()
            && self.source.is_none()
            && self.locals.is_empty()
            && self.references.is_empty()
    }

    /// Whether the script holds nothing a compiler could use: no source,
    /// no compiled bytes, no variables and no references. A script that
    /// has only its `SCHR` header is blank, as the editor writes one for
    /// every dialogue response (measured: all 22,327 Fallout 3 infos have
    /// a begin-result header, 1,346 of them source text).
    pub fn is_blank(&self) -> bool {
        self.compiled.is_none()
            && self.source.is_none()
            && self.locals.is_empty()
            && self.references.is_empty()
    }

    /// Offers a subrecord to the script. Returns whether it was consumed.
    pub(crate) fn take(&mut self, sub: &SubrecordRef<'_>) -> bool {
        let data = sub.data;
        match &sub.sig.0 {
            b"SCHR" => set(&mut self.header, ScriptHeader::parse(data)),
            b"SCDA" => set(&mut self.compiled, Some(data.to_vec())),
            b"SCTX" => set(&mut self.source, Some(decode_cp1252(data))),
            b"SLSD" => push(&mut self.locals, parse_slsd(data)),
            b"SCVR" => match self.locals.last_mut() {
                Some(local) => set(&mut local.name, zstring(data)),
                None => false,
            },
            b"SCRO" => push(
                &mut self.references,
                one_form(data).map(ScriptReference::Form),
            ),
            b"SCRV" => push(
                &mut self.references,
                one_u32(data).map(ScriptReference::Local),
            ),
            _ => false,
        }
    }

    pub(crate) fn emit(&self, out: &mut Out) {
        out.opt(b"SCHR", self.header.map(|h| h.bytes()));
        out.opt(b"SCDA", self.compiled.clone());
        out.opt(b"SCTX", self.source.as_deref().map(encode_cp1252_lossy));
        for local in &self.locals {
            let data = Wr::default()
                .u32(local.index)
                .zeros(12)
                .u8(local.flags)
                .zeros(7)
                .done();
            out.raw(b"SLSD", data);
            out.opt(b"SCVR", local.name.as_deref().map(zstring_bytes));
        }
        for reference in &self.references {
            match *reference {
                ScriptReference::Form(form) => out.form(b"SCRO", Some(form)),
                ScriptReference::Local(index) => out.u32(b"SCRV", Some(index)),
            }
        }
    }
}

fn parse_slsd(data: &[u8]) -> Option<LocalVariable> {
    let mut rd = Rd::exact(data, 24)?;
    let index = rd.u32()?;
    rd.skip(12)?;
    Some(LocalVariable {
        index,
        flags: rd.u8()?,
        name: None,
    })
}

/// A script (`SCPT`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Script {
    /// The common fields.
    pub common: Common,
    /// The script itself.
    pub body: ScriptBody,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl Script {
    /// A script with source text.
    pub fn new(form_id: impl Into<FormId>, editor_id: &str, source: &str) -> Self {
        Self {
            common: Common::new(form_id).with_editor_id(editor_id),
            body: ScriptBody::from_source(source),
            unknown: Vec::new(),
        }
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| self.body.take(s));
    }

    fn emit_fields(&self, out: &mut Out) {
        self.body.emit(out);
    }
}

typed_record!(Script, SCPT);
