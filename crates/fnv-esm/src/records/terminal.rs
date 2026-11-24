//! Terminals (`TERM`): a header text, then menu items, each with its own
//! result text, script and conditions.
//!
//! Measured on both retail masters (378 terminals in Fallout 3, 344 in New
//! Vegas): the header is `DESC`, `SNAM`, `PNAM` and `DNAM`; each menu item
//! starts with `ITXT`, then `RNAM`, `ANAM`, at most one of `INAM` and
//! `TNAM`, an embedded script (`SCHR` and the rest) and its `CTDA`s.

use serde::{Deserialize, Serialize};

use super::{Common, Condition, ScriptBody, typed_record};
use crate::bytes::{Wr, one_form, one_u8, set, zstring};
use crate::formid::FormId;
use crate::subrecord::{Out, RawSubrecord, SubrecordRef};

/// A terminal's `DNAM`: 4 bytes, or 3 in an older form (6 terminals of the
/// Fallout 3 master, 2 of New Vegas'). The fourth byte is unused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TerminalData {
    /// Hacking difficulty, as stored: 0 very easy to 4 very hard, 5
    /// requires a key.
    pub difficulty: u8,
    /// Flags, as stored: 0x01 leveled, 0x02 unlocked, 0x04 alternate
    /// colours, 0x08 hide the welcome text.
    pub flags: u8,
    /// The server type, as stored: which screen style is drawn.
    pub server_type: u8,
}

/// One menu item of a terminal.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TerminalMenuItem {
    /// The item's text in the menu (`ITXT`).
    pub text: Option<String>,
    /// The text shown when the item is chosen (`RNAM`).
    pub result_text: Option<String>,
    /// Flags (`ANAM`), as stored: 0x01 add a note, 0x02 force a redraw.
    pub flags: Option<u8>,
    /// The note the item shows (`INAM`).
    pub display_note: Option<FormId>,
    /// The terminal the item opens as a sub-menu (`TNAM`).
    pub sub_terminal: Option<FormId>,
    /// The script run when the item is chosen; empty when there is none.
    pub script: ScriptBody,
    /// Conditions for showing the item (`CTDA`), in file order.
    pub conditions: Vec<Condition>,
}

impl TerminalMenuItem {
    /// An item with its menu text and result text.
    pub fn new(text: &str, result_text: &str) -> Self {
        Self {
            text: Some(text.to_owned()),
            result_text: Some(result_text.to_owned()),
            flags: Some(0),
            ..Self::default()
        }
    }
}

/// A terminal (`TERM`).
///
/// `MODT` and the other model subrecords stay in `unknown`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Terminal {
    /// The common fields. `full_name` is the terminal's name.
    pub common: Common,
    /// The header text shown above the menu (`DESC`).
    pub header_text: Option<String>,
    /// The looping sound (`SNAM`).
    pub sound: Option<FormId>,
    /// The note that holds the password (`PNAM`).
    pub password_note: Option<FormId>,
    /// Difficulty, flags and server type (`DNAM`).
    pub data: Option<TerminalData>,
    /// Menu items, in file order.
    pub items: Vec<TerminalMenuItem>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

/// The item a subrecord belongs to: the last one if `is_free` says its
/// slot is still empty, otherwise a new one.
fn item_for(
    items: &mut Vec<TerminalMenuItem>,
    is_free: impl Fn(&TerminalMenuItem) -> bool,
) -> &mut TerminalMenuItem {
    if !items.last().is_some_and(is_free) {
        items.push(TerminalMenuItem::default());
    }
    let last = items.len() - 1;
    &mut items[last]
}

/// Whether an item has nothing but its text yet, so that a script still
/// belongs to it.
fn script_is_free(item: &TerminalMenuItem) -> bool {
    item.conditions.is_empty()
}

impl Terminal {
    /// A terminal with an editor id, a name and a header text.
    pub fn new(form_id: impl Into<FormId>, editor_id: &str, name: &str, header: &str) -> Self {
        Self {
            common: Common::new(form_id)
                .with_editor_id(editor_id)
                .with_name(name),
            header_text: Some(header.to_owned()),
            data: Some(TerminalData::default()),
            ..Self::default()
        }
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"DESC" if self.items.is_empty() => set(&mut self.header_text, zstring(d)),
                b"SNAM" if self.items.is_empty() => set(&mut self.sound, one_form(d)),
                b"PNAM" if self.items.is_empty() => set(&mut self.password_note, one_form(d)),
                b"DNAM" if self.items.is_empty() && matches!(d.len(), 3 | 4) => set(
                    &mut self.data,
                    Some(TerminalData {
                        difficulty: d[0],
                        flags: d[1],
                        server_type: d[2],
                    }),
                ),
                b"ITXT" => match zstring(d) {
                    Some(text) => {
                        self.items.push(TerminalMenuItem {
                            text: Some(text),
                            ..TerminalMenuItem::default()
                        });
                        true
                    }
                    None => false,
                },
                b"RNAM" => {
                    let text = zstring(d);
                    text.is_some()
                        && set(
                            &mut item_for(&mut self.items, |i| i.result_text.is_none()).result_text,
                            text,
                        )
                }
                b"ANAM" => {
                    let flags = one_u8(d);
                    flags.is_some()
                        && set(
                            &mut item_for(&mut self.items, |i| i.flags.is_none()).flags,
                            flags,
                        )
                }
                b"INAM" => {
                    let form = one_form(d);
                    form.is_some()
                        && set(
                            &mut item_for(&mut self.items, |i| i.display_note.is_none())
                                .display_note,
                            form,
                        )
                }
                b"TNAM" => {
                    let form = one_form(d);
                    form.is_some()
                        && set(
                            &mut item_for(&mut self.items, |i| i.sub_terminal.is_none())
                                .sub_terminal,
                            form,
                        )
                }
                b"SCHR" | b"SCDA" | b"SCTX" | b"SLSD" | b"SCVR" | b"SCRO" | b"SCRV" => {
                    item_for(&mut self.items, script_is_free).script.take(s)
                }
                b"CTDA" => match Condition::parse(d) {
                    Some(condition) => {
                        item_for(&mut self.items, |_| true)
                            .conditions
                            .push(condition);
                        true
                    }
                    None => false,
                },
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.zstr(b"DESC", &self.header_text);
        out.form(b"SNAM", self.sound);
        out.form(b"PNAM", self.password_note);
        out.opt(
            b"DNAM",
            self.data.map(|d| {
                Wr::default()
                    .u8(d.difficulty)
                    .u8(d.flags)
                    .u8(d.server_type)
                    .zeros(1)
                    .done()
            }),
        );
        for item in &self.items {
            out.zstr(b"ITXT", &item.text);
            out.zstr(b"RNAM", &item.result_text);
            out.u8(b"ANAM", item.flags);
            out.form(b"INAM", item.display_note);
            out.form(b"TNAM", item.sub_terminal);
            item.script.emit(out);
            for condition in &item.conditions {
                out.raw(b"CTDA", condition.bytes());
            }
        }
    }
}

typed_record!(Terminal, TERM);
