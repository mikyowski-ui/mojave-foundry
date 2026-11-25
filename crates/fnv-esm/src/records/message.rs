//! Messages (`MESG`): the texts of message boxes and notices, with their
//! buttons.
//!
//! Measured on both retail masters (518 messages in Fallout 3, 1,144 in
//! New Vegas): `DESC` is the text, `FULL` the title (on about a third), `INAM`
//! an icon form id (null in every message), `DNAM` the flags, `TNAM` the
//! display time in seconds of a notice, then for a message box one `ITXT`
//! per button, each followed by the `CTDA` conditions that show it. A few
//! messages (20 and 19) have `NAM0` to `NAM9` instead of `DNAM`; those stay
//! in `unknown`.

use serde::{Deserialize, Serialize};

use super::{Common, Condition, typed_record};
use crate::bytes::{one_form, one_u32, push, set, zstring};
use crate::formid::FormId;
use crate::subrecord::{Out, RawSubrecord, SubrecordRef};

/// One button of a message box: its text and the conditions that show it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MessageButton {
    /// The button's text (`ITXT`).
    pub text: Option<String>,
    /// Conditions (`CTDA`), in file order.
    pub conditions: Vec<Condition>,
}

/// A message (`MESG`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Message {
    /// The common fields; `full_name` is the title.
    pub common: Common,
    /// The text (`DESC`).
    pub text: Option<String>,
    /// The icon (`INAM`); null in every message of the masters.
    pub icon: Option<FormId>,
    /// Flags (`DNAM`), as stored: see [`Message::MESSAGE_BOX`] and
    /// [`Message::AUTO_DISPLAY`].
    pub flags: Option<u32>,
    /// The display time in seconds (`TNAM`), for a notice.
    pub display_time: Option<u32>,
    /// The buttons, in file order.
    pub buttons: Vec<MessageButton>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl Message {
    /// Flag: shown as a message box, which waits for a button; else a
    /// notice that times out.
    pub const MESSAGE_BOX: u32 = 0x0000_0001;
    /// Flag: shown without being asked for.
    pub const AUTO_DISPLAY: u32 = 0x0000_0002;

    /// A notice: a title and text, no buttons.
    pub fn new(form_id: impl Into<FormId>, editor_id: &str, title: &str, text: &str) -> Self {
        Self {
            common: Common::new(form_id)
                .with_editor_id(editor_id)
                .with_name(title),
            text: Some(text.to_owned()),
            icon: Some(FormId::NULL),
            flags: Some(0),
            ..Self::default()
        }
    }

    /// Sets the flags.
    #[must_use]
    pub fn with_flags(mut self, flags: u32) -> Self {
        self.flags = Some(flags);
        self
    }

    /// Sets the display time.
    #[must_use]
    pub fn with_display_time(mut self, seconds: u32) -> Self {
        self.display_time = Some(seconds);
        self
    }

    /// Adds a button without conditions.
    #[must_use]
    pub fn with_button(mut self, text: &str) -> Self {
        self.buttons.push(MessageButton {
            text: Some(text.to_owned()),
            conditions: Vec::new(),
        });
        self
    }

    /// Adds a button shown only when its conditions pass.
    #[must_use]
    pub fn with_conditional_button(mut self, text: &str, conditions: Vec<Condition>) -> Self {
        self.buttons.push(MessageButton {
            text: Some(text.to_owned()),
            conditions,
        });
        self
    }

    /// The title (`FULL`).
    pub fn title(&self) -> Option<&str> {
        self.common.full_name.as_deref()
    }

    /// Whether the message is a message box.
    pub fn is_message_box(&self) -> bool {
        self.flags.is_some_and(|f| f & Self::MESSAGE_BOX != 0)
    }

    /// Whether the message is shown without being asked for.
    pub fn is_auto_display(&self) -> bool {
        self.flags.is_some_and(|f| f & Self::AUTO_DISPLAY != 0)
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"DESC" => set(&mut self.text, zstring(d)),
                b"INAM" => set(&mut self.icon, one_form(d)),
                b"DNAM" => set(&mut self.flags, one_u32(d)),
                b"TNAM" => set(&mut self.display_time, one_u32(d)),
                b"ITXT" => match zstring(d) {
                    Some(text) => {
                        self.buttons.push(MessageButton {
                            text: Some(text),
                            conditions: Vec::new(),
                        });
                        true
                    }
                    None => false,
                },
                b"CTDA" => match self.buttons.last_mut() {
                    Some(button) => push(&mut button.conditions, Condition::parse(d)),
                    None => false,
                },
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.zstr(b"DESC", &self.text);
        out.form(b"INAM", self.icon);
        out.u32(b"DNAM", self.flags);
        out.u32(b"TNAM", self.display_time);
        for button in &self.buttons {
            out.zstr(b"ITXT", &button.text);
            for condition in &button.conditions {
                out.raw(b"CTDA", condition.bytes());
            }
        }
    }
}

typed_record!(Message, MESG);
