//! MSG message files, as `vault_formats::msg::MsgCatalog::parse_strict`
//! reads them (SPEC-002 §2.5, §3.4 and §3.5).
//!
//! The format has no escapes: a closing brace always ends a field, line
//! feeds inside a field are dropped, and a NUL cuts the stored text. So
//! the writer refuses those bytes instead of emitting text that would load
//! as something else.

use crate::EmitError;
use std::collections::BTreeSet;
use vault_formats::MsgCatalog;

/// The most payload bytes one field may hold (SPEC-002 §3.5).
pub const MSG_FIELD_BYTES: usize = 1023;
/// The most records this writer emits. The parser counts one more scan
/// step for the line feed after the last record against its limit of one
/// million, so the writer stays one below it.
pub const MSG_MAX_RECORDS: usize = 999_999;

/// One `{id}{sound}{text}` record. Sound and text are raw bytes in the
/// game's single-byte encoding; nothing is converted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsgEntry {
    pub id: i32,
    pub sound: Vec<u8>,
    pub text: Vec<u8>,
}

impl MsgEntry {
    /// An entry with no sound.
    pub fn new(id: i32, text: impl Into<Vec<u8>>) -> Self {
        Self {
            id,
            sound: Vec::new(),
            text: text.into(),
        }
    }
}

/// Why a field cannot be carried, if it cannot.
fn check_field(field: &[u8]) -> Result<(), String> {
    if field.len() > MSG_FIELD_BYTES {
        return Err(format!(
            "{} bytes; a field holds at most {MSG_FIELD_BYTES}",
            field.len()
        ));
    }
    // `}` ends the field, LF is dropped, NUL truncates, and 0x1A ends a
    // loose file on the reference runtime (SPEC-002 §3.6). A bare CR is
    // kept by the parser and is allowed; an LF never follows it here.
    if let Some(byte) = field
        .iter()
        .find(|byte| matches!(byte, b'}' | b'\n' | 0 | 0x1a))
    {
        return Err(format!(
            "byte {byte:#04x} cannot be carried: the format has no escape for it"
        ));
    }
    Ok(())
}

fn record(out: &mut Vec<u8>, entry: &MsgEntry) -> Result<(), EmitError> {
    for (name, field) in [("sound", &entry.sound), ("text", &entry.text)] {
        check_field(field).map_err(|message| EmitError::Msg {
            id: entry.id,
            message: format!("{name}: {message}"),
        })?;
    }
    out.extend_from_slice(format!("{{{}}}{{", entry.id).as_bytes());
    out.extend_from_slice(&entry.sound);
    out.extend_from_slice(b"}{");
    out.extend_from_slice(&entry.text);
    out.extend_from_slice(b"}\n");
    Ok(())
}

/// Serialize `entries`, one record per line in ascending ID order. Two
/// entries with one ID are refused: the later would silently replace the
/// earlier on load.
pub fn write_msg(entries: &[MsgEntry]) -> Result<Vec<u8>, EmitError> {
    if entries.len() > MSG_MAX_RECORDS {
        return Err(EmitError::Msg {
            id: entries[MSG_MAX_RECORDS].id,
            message: format!("more than {MSG_MAX_RECORDS} records"),
        });
    }
    let mut sorted: Vec<&MsgEntry> = entries.iter().collect();
    sorted.sort_by_key(|entry| entry.id);
    let mut out = Vec::new();
    for (index, entry) in sorted.iter().enumerate() {
        if index > 0 && sorted[index - 1].id == entry.id {
            return Err(EmitError::Msg {
                id: entry.id,
                message: "two entries have this ID".into(),
            });
        }
        record(&mut out, entry)?;
    }
    Ok(out)
}

/// A message file that keeps an existing file's bytes and appends new
/// records after them, so the base file's own text, order and comments are
/// untouched.
#[derive(Debug, Clone, Default)]
pub struct MsgFile {
    bytes: Vec<u8>,
    ids: BTreeSet<i32>,
}

impl MsgFile {
    /// An empty file.
    pub fn new() -> Self {
        Self::default()
    }
    /// Start from `base`, which must load completely.
    pub fn from_base(base: &[u8]) -> Result<Self, EmitError> {
        let catalog = MsgCatalog::parse_strict(base)?;
        let mut bytes = base.to_vec();
        if !bytes.is_empty() && !bytes.ends_with(b"\n") {
            // Bytes outside braces are skipped, so this changes no record.
            bytes.push(b'\n');
        }
        Ok(Self {
            bytes,
            ids: catalog.entries().keys().copied().collect(),
        })
    }
    /// Whether a record with `id` exists.
    pub fn contains(&self, id: i32) -> bool {
        self.ids.contains(&id)
    }
    /// Append a record. An ID the file already holds is refused.
    pub fn push(&mut self, entry: &MsgEntry) -> Result<(), EmitError> {
        if self.ids.contains(&entry.id) {
            return Err(EmitError::Msg {
                id: entry.id,
                message: "the file already holds this ID".into(),
            });
        }
        if self.ids.len() == MSG_MAX_RECORDS {
            return Err(EmitError::Msg {
                id: entry.id,
                message: format!("more than {MSG_MAX_RECORDS} records"),
            });
        }
        record(&mut self.bytes, entry)?;
        self.ids.insert(entry.id);
        Ok(())
    }
    pub fn len(&self) -> usize {
        self.ids.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }
}
