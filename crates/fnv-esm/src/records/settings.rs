//! Global variables (`GLOB`) and game settings (`GMST`).
//!
//! Measured on both retail masters (155 globals and 530 settings in
//! Fallout 3, 218 and 648 in New Vegas): a global is `FNAM`, one byte
//! naming its type, and `FLTV`, its value as an `f32` whatever the type; a
//! setting is an `EDID` whose first letter gives the type of the `DATA`
//! that follows (`f` float, `i` integer, `s` string, both 4 bytes except
//! the string).

use serde::{Deserialize, Serialize};

use super::{Common, typed_record};
use crate::bytes::{Rd, Wr, one_f32, one_u8, set, zstring, zstring_bytes};
use crate::formid::FormId;
use crate::subrecord::{Out, RawSubrecord, SubrecordRef};

/// The type of a global variable (`FNAM`): `s` short, `l` long, `f`
/// float. Every value is stored as an `f32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GlobalType {
    /// `s`: a 16-bit integer.
    Short,
    /// `l`: a 32-bit integer.
    Long,
    /// `f`: a float.
    Float,
    /// Any other byte, as stored.
    Other(u8),
}

impl GlobalType {
    /// The type of a stored `FNAM` byte.
    pub fn from_code(code: u8) -> Self {
        match code {
            b's' => Self::Short,
            b'l' => Self::Long,
            b'f' => Self::Float,
            other => Self::Other(other),
        }
    }

    /// The stored `FNAM` byte.
    pub fn code(self) -> u8 {
        match self {
            Self::Short => b's',
            Self::Long => b'l',
            Self::Float => b'f',
            Self::Other(code) => code,
        }
    }

    /// `"short"`, `"long"`, `"float"` or `"other"`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Short => "short",
            Self::Long => "long",
            Self::Float => "float",
            Self::Other(_) => "other",
        }
    }

    /// Whether the value is an integer type.
    pub fn is_integer(self) -> bool {
        matches!(self, Self::Short | Self::Long)
    }
}

/// A global variable (`GLOB`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Global {
    /// The common fields.
    pub common: Common,
    /// The type (`FNAM`).
    pub value_type: Option<GlobalType>,
    /// The initial value (`FLTV`).
    pub value: Option<f32>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl Global {
    /// A global of a type with an initial value.
    pub fn new(
        form_id: impl Into<FormId>,
        editor_id: &str,
        value_type: GlobalType,
        value: f32,
    ) -> Self {
        Self {
            common: Common::new(form_id).with_editor_id(editor_id),
            value_type: Some(value_type),
            value: Some(value),
            ..Self::default()
        }
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"FNAM" => set(&mut self.value_type, one_u8(d).map(GlobalType::from_code)),
                b"FLTV" => set(&mut self.value, one_f32(d)),
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.u8(b"FNAM", self.value_type.map(GlobalType::code));
        out.f32(b"FLTV", self.value);
    }
}

typed_record!(Global, GLOB);

/// The value of a game setting, typed by the first letter of its editor
/// id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GameSettingValue {
    /// `i`: a signed integer.
    Int(i32),
    /// `u`: an unsigned integer.
    Unsigned(u32),
    /// `f`: a float.
    Float(f32),
    /// `s`: a string (Windows-1252).
    String(String),
    /// `b`: a boolean, stored as an integer.
    Bool(bool),
}

impl GameSettingValue {
    /// The value as a float, for the numeric kinds.
    pub fn as_f32(&self) -> Option<f32> {
        match *self {
            Self::Int(v) => Some(v as f32),
            Self::Unsigned(v) => Some(v as f32),
            Self::Float(v) => Some(v),
            Self::Bool(v) => Some(f32::from(u8::from(v))),
            Self::String(_) => None,
        }
    }

    /// The value as an integer, for the integer kinds.
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Self::Int(v) => Some(i64::from(v)),
            Self::Unsigned(v) => Some(i64::from(v)),
            Self::Bool(v) => Some(i64::from(v)),
            Self::Float(_) | Self::String(_) => None,
        }
    }

    /// `"int"`, `"unsigned"`, `"float"`, `"string"` or `"bool"`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Int(_) => "int",
            Self::Unsigned(_) => "unsigned",
            Self::Float(_) => "float",
            Self::String(_) => "string",
            Self::Bool(_) => "bool",
        }
    }
}

/// A game setting (`GMST`). The editor id is the setting's name, and its
/// first letter says how `DATA` reads; a setting with another first letter
/// keeps its `DATA` in `unknown`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct GameSetting {
    /// The common fields; `editor_id` is the name.
    pub common: Common,
    /// The value (`DATA`).
    pub value: Option<GameSettingValue>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl GameSetting {
    /// A setting with a value.
    pub fn new(form_id: impl Into<FormId>, name: &str, value: GameSettingValue) -> Self {
        Self {
            common: Common::new(form_id).with_editor_id(name),
            value: Some(value),
            ..Self::default()
        }
    }

    /// The setting's name.
    pub fn name(&self) -> Option<&str> {
        self.common.editor_id.as_deref()
    }

    /// The type letter a name starts with, if it is one this decoder
    /// reads: `i`, `u`, `f`, `s` or `b`.
    pub fn type_letter(name: &str) -> Option<char> {
        name.chars()
            .next()
            .filter(|c| matches!(c, 'i' | 'u' | 'f' | 's' | 'b'))
    }

    fn parse_value(letter: char, data: &[u8]) -> Option<GameSettingValue> {
        match letter {
            's' => Some(GameSettingValue::String(zstring(data)?)),
            'f' => Some(GameSettingValue::Float(Rd::exact(data, 4)?.f32()?)),
            'i' => Some(GameSettingValue::Int(Rd::exact(data, 4)?.i32()?)),
            'u' => Some(GameSettingValue::Unsigned(Rd::exact(data, 4)?.u32()?)),
            'b' => Some(GameSettingValue::Bool(Rd::exact(data, 4)?.u32()? != 0)),
            _ => None,
        }
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        let letter = subs
            .iter()
            .find(|s| &s.sig.0 == b"EDID")
            .and_then(|s| zstring(s.data))
            .and_then(|name| Self::type_letter(&name));
        decode_loop!(self, subs, |s| match (&s.sig.0, letter) {
            (b"DATA", Some(letter)) => set(&mut self.value, Self::parse_value(letter, s.data)),
            _ => false,
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        let data = self.value.as_ref().map(|value| match value {
            GameSettingValue::String(text) => zstring_bytes(text),
            GameSettingValue::Float(v) => Wr::default().f32(*v).done(),
            GameSettingValue::Int(v) => Wr::default().i32(*v).done(),
            GameSettingValue::Unsigned(v) => Wr::default().u32(*v).done(),
            GameSettingValue::Bool(v) => Wr::default().u32(u32::from(*v)).done(),
        });
        out.opt(b"DATA", data);
    }
}

typed_record!(GameSetting, GMST);
