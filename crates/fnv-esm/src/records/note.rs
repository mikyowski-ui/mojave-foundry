//! Notes (`NOTE`): holotapes and paper notes with text, an image, a sound
//! or a voiced topic.
//!
//! Measured on both retail masters (840 notes in Fallout 3, 894 in New
//! Vegas): `DATA` is one byte, the note type; `TNAM` is the text, except in
//! a voice note, where it is the topic's form id; `SNAM` is the sound (or,
//! in a voice note, the speaker); `ONAM` lists quests; `XNAM` is an image's
//! texture path.

use serde::{Deserialize, Serialize};

use super::{Common, typed_record};
use crate::bytes::{one_form, one_u8, push, set, zstring};
use crate::formid::FormId;
use crate::subrecord::{Out, RawSubrecord, SubrecordRef};

/// A note (`NOTE`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Note {
    /// The common fields. `full_name` is the note's title.
    pub common: Common,
    /// The inventory icon path (`ICON`).
    pub icon: Option<String>,
    /// The small icon path (`MICO`).
    pub small_icon: Option<String>,
    /// The note type (`DATA`); see the `TYPE_*` constants.
    pub note_type: Option<u8>,
    /// Quests the note belongs to (`ONAM`), in file order.
    pub quests: Vec<FormId>,
    /// An image note's texture path (`XNAM`).
    pub texture: Option<String>,
    /// The text (`TNAM`) of any note but a voice note.
    pub text: Option<String>,
    /// A voice note's topic (`TNAM`).
    pub topic: Option<FormId>,
    /// The sound (`SNAM`); in a voice note, the speaker.
    pub sound: Option<FormId>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl Note {
    /// A note that plays a sound.
    pub const TYPE_SOUND: u8 = 0;
    /// A note of text.
    pub const TYPE_TEXT: u8 = 1;
    /// A note that shows an image.
    pub const TYPE_IMAGE: u8 = 2;
    /// A note that plays a voiced dialogue topic.
    pub const TYPE_VOICE: u8 = 3;

    /// A text note.
    pub fn text(form_id: impl Into<FormId>, editor_id: &str, title: &str, text: &str) -> Self {
        Self {
            common: Common::new(form_id)
                .with_editor_id(editor_id)
                .with_name(title),
            note_type: Some(Self::TYPE_TEXT),
            text: Some(text.to_owned()),
            ..Self::default()
        }
    }

    /// Whether `DATA` says this is a voice note.
    pub fn is_voice(&self) -> bool {
        self.note_type == Some(Self::TYPE_VOICE)
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"ICON" => set(&mut self.icon, zstring(d)),
                b"MICO" => set(&mut self.small_icon, zstring(d)),
                b"DATA" => set(&mut self.note_type, one_u8(d)),
                b"ONAM" => push(&mut self.quests, one_form(d)),
                b"XNAM" => set(&mut self.texture, zstring(d)),
                b"TNAM" if self.is_voice() => set(&mut self.topic, one_form(d)),
                b"TNAM" => set(&mut self.text, zstring(d)),
                b"SNAM" => set(&mut self.sound, one_form(d)),
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.zstr(b"ICON", &self.icon);
        out.zstr(b"MICO", &self.small_icon);
        out.u8(b"DATA", self.note_type);
        out.form_each(b"ONAM", &self.quests);
        out.zstr(b"XNAM", &self.texture);
        if self.is_voice() {
            out.form(b"TNAM", self.topic);
        } else {
            out.zstr(b"TNAM", &self.text);
        }
        out.form(b"SNAM", self.sound);
    }
}

typed_record!(Note, NOTE);
