//! Names and descriptions for the prototype message files.
//!
//! A prototype's `message_base` numbers two records of its category's
//! message file (`pro_item.msg`, `pro_crit.msg`, `pro_scen.msg`, ...,
//! [`crate::proto::message_file_path`]): the name at the base and the
//! description at the base plus one (SPEC-003 §2.2). The file is read by
//! `vault_formats::MsgCatalog::parse_strict` (SPEC-002 §3.4, §3.5), which
//! has no escapes and keeps at most [`MSG_FIELD_BYTES`] bytes of a field.
//!
//! The game's text is single-byte. [`fold_text`] turns source text into
//! bytes the format carries and the game's fonts draw: printable ASCII,
//! accented Latin letters folded to their base letters, typographic quotes
//! and dashes to their ASCII forms, line breaks and tabs to spaces, and
//! anything else to `?`. The braces become parentheses, since `}` would end
//! the field. The result is cut to [`MSG_FIELD_BYTES`].
//!
//! Display limits, for choosing text (SPEC-014 §3.34): an item name is
//! clipped to 140 pixels in the hand blocks and 80 in the stats panel's
//! owner line; examine text wraps at 152 pixels and stops after 17 lines.
//! The specifications give no byte limit for a name beyond the field's.

use crate::EmitError;
use crate::msg::{MSG_FIELD_BYTES, MsgEntry, MsgFile};
use crate::proto::{NO_MESSAGE, message_entries, message_file_path};
use vault_formats::Prototype;

/// The ASCII spelling of a non-ASCII character, when it has one.
fn fold_char(c: char) -> Option<&'static str> {
    Some(match c {
        'À'..='Å' | 'Ā' | 'Ă' | 'Ą' => "A",
        'à'..='å' | 'ā' | 'ă' | 'ą' | 'ª' => "a",
        'Æ' => "AE",
        'æ' => "ae",
        'Ç' | 'Ć' | 'Ĉ' | 'Ċ' | 'Č' => "C",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'Ð' | 'Ď' | 'Đ' => "D",
        'ð' | 'ď' | 'đ' => "d",
        'È'..='Ë' | 'Ē' | 'Ĕ' | 'Ė' | 'Ę' | 'Ě' => "E",
        'è'..='ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'Ĝ' | 'Ğ' | 'Ġ' | 'Ģ' => "G",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'Ĥ' | 'Ħ' => "H",
        'ĥ' | 'ħ' => "h",
        'Ì'..='Ï' | 'Ĩ' | 'Ī' | 'Ĭ' | 'Į' | 'İ' => "I",
        'ì'..='ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'Ĵ' => "J",
        'ĵ' => "j",
        'Ķ' => "K",
        'ķ' => "k",
        'Ĺ' | 'Ļ' | 'Ľ' | 'Ŀ' | 'Ł' => "L",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'Ñ' | 'Ń' | 'Ņ' | 'Ň' => "N",
        'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'Ò'..='Ö' | 'Ø' | 'Ō' | 'Ŏ' | 'Ő' => "O",
        'ò'..='ö' | 'ø' | 'ō' | 'ŏ' | 'ő' | 'º' => "o",
        'Œ' => "OE",
        'œ' => "oe",
        'Ŕ' | 'Ŗ' | 'Ř' => "R",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'Ś' | 'Ŝ' | 'Ş' | 'Š' => "S",
        'ś' | 'ŝ' | 'ş' | 'š' => "s",
        'ß' => "ss",
        'Ţ' | 'Ť' | 'Ŧ' => "T",
        'ţ' | 'ť' | 'ŧ' => "t",
        'Þ' => "Th",
        'þ' => "th",
        'Ù'..='Ü' | 'Ũ' | 'Ū' | 'Ŭ' | 'Ů' | 'Ű' | 'Ų' => "U",
        'ù'..='ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'Ŵ' => "W",
        'ŵ' => "w",
        'Ý' | 'Ŷ' | 'Ÿ' => "Y",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'Ź' | 'Ż' | 'Ž' => "Z",
        'ź' | 'ż' | 'ž' => "z",
        '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' | '\u{2032}' | '´' | '`' => "'",
        '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' | '\u{2033}' | '«' | '»' => "\"",
        '\u{2010}'..='\u{2015}' | '\u{2212}' => "-",
        '\u{2026}' => "...",
        '\u{00a0}' | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{3000}' => " ",
        '\u{2022}' | '·' => "*",
        '×' => "x",
        '÷' => "/",
        '©' => "(c)",
        '®' => "(R)",
        '\u{2122}' => "TM",
        '°' => " deg",
        '½' => "1/2",
        '¼' => "1/4",
        '¾' => "3/4",
        '¹' => "1",
        '²' => "2",
        '³' => "3",
        '¿' => "?",
        '¡' => "!",
        _ => return None,
    })
}

/// `text` as bytes the message format carries (see the module notes), at
/// most `max` bytes, with leading and trailing spaces removed. Zero-width
/// and control characters other than line breaks and tabs are dropped.
pub fn fold_text(text: &str, max: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len().min(max));
    for c in text.chars() {
        match c {
            '{' => out.push(b'('),
            '}' => out.push(b')'),
            '\n' | '\r' | '\t' => out.push(b' '),
            ' '..='~' => out.push(c as u8),
            '\u{200b}'..='\u{200d}' | '\u{feff}' | '\u{ad}' => {}
            c if c.is_control() => {}
            c => match fold_char(c) {
                Some(folded) => out.extend_from_slice(folded.as_bytes()),
                None => out.push(b'?'),
            },
        }
        if out.len() >= max {
            out.truncate(max);
            break;
        }
    }
    let end = out.iter().rposition(|b| *b != b' ').map_or(0, |i| i + 1);
    let start = out.iter().position(|b| *b != b' ').unwrap_or(end);
    out[start..end].to_vec()
}

/// A prototype's name and description, already folded by [`fold_text`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProtoText {
    pub name: Vec<u8>,
    pub description: Vec<u8>,
}

impl ProtoText {
    /// Fold `name` and `description` (each at most [`MSG_FIELD_BYTES`]).
    pub fn new(name: &str, description: &str) -> Self {
        Self {
            name: fold_text(name, MSG_FIELD_BYTES),
            description: fold_text(description, MSG_FIELD_BYTES),
        }
    }

    /// The two records for `message_base` ([`message_entries`]).
    pub fn entries(&self, message_base: i32) -> Result<[MsgEntry; 2], EmitError> {
        message_entries(message_base, &self.name, &self.description)
    }
}

/// A prototype with its message records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Described {
    pub prototype: Prototype,
    /// The name and description records; empty when the prototype has no
    /// message base.
    pub messages: Vec<MsgEntry>,
}

impl Described {
    /// The message file these records belong in, for `language`.
    pub fn message_path(&self, language: &str) -> String {
        message_file_path(self.prototype.category, language)
    }

    /// Append the records to `file` (refused when it holds either number).
    pub fn push_messages(&self, file: &mut MsgFile) -> Result<(), EmitError> {
        if let Some(entry) = self.messages.iter().find(|entry| file.contains(entry.id)) {
            return Err(EmitError::Msg {
                id: entry.id,
                message: "the file already holds this ID".into(),
            });
        }
        for entry in &self.messages {
            file.push(entry)?;
        }
        Ok(())
    }
}

/// Pair `prototype` with the records of `text` at its message base. A
/// prototype without one ([`NO_MESSAGE`]) takes no text: it is refused
/// unless both fields are empty.
pub fn describe(prototype: Prototype, text: &ProtoText) -> Result<Described, EmitError> {
    if prototype.message_base == NO_MESSAGE {
        if !text.name.is_empty() || !text.description.is_empty() {
            return Err(EmitError::Prototype(format!(
                "PID {:#010x} has no message base for its name and description",
                prototype.pid
            )));
        }
        return Ok(Described {
            prototype,
            messages: Vec::new(),
        });
    }
    let messages = text.entries(prototype.message_base)?.to_vec();
    Ok(Described {
        prototype,
        messages,
    })
}
