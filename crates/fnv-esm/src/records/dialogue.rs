//! Dialogue: topics (`DIAL`) and their response groups (`INFO`).

use serde::{Deserialize, Serialize};

use super::{Common, Condition, ScriptBody, typed_record};
use crate::bytes::{Rd, Wr, one_f32, one_form, one_u32, push, set, zstring};
use crate::formid::FormId;
use crate::subrecord::{Out, RawSubrecord, SubrecordRef};

/// A topic's `DATA`: 2 bytes, or 1 in the older form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TopicData {
    /// The topic type, as stored: 0 topic, 1 conversation, 2 combat,
    /// 3 persuasion, 4 detection, 5 service, 6 miscellaneous, 7 radio.
    pub topic_type: u8,
    /// Flags, as stored; absent from the 1-byte form.
    pub flags: Option<u8>,
}

/// A dialogue topic (`DIAL`). Its `INFO` records follow it in a topic
/// children group; [`crate::Context::topic`] names the topic for each.
///
/// The `INFC` and `INFX` subrecords that follow a `QSTI` are not decoded
/// and stay in `unknown`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Topic {
    /// The common fields. `full_name` is the topic text.
    pub common: Common,
    /// The quests the topic belongs to (`QSTI`), in file order.
    pub quests: Vec<FormId>,
    /// Quests the topic was removed from (`QSTR`), in file order.
    pub removed_quests: Vec<FormId>,
    /// Priority (`PNAM`).
    pub priority: Option<f32>,
    /// The fallback line for a low-intelligence player (`TDUM`).
    pub dumb_response: Option<String>,
    /// Type and flags (`DATA`).
    pub data: Option<TopicData>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl Topic {
    /// A topic of type 0 belonging to one quest.
    pub fn new(
        form_id: impl Into<FormId>,
        editor_id: &str,
        text: &str,
        quest: impl Into<FormId>,
    ) -> Self {
        Self {
            common: Common::new(form_id)
                .with_editor_id(editor_id)
                .with_name(text),
            quests: vec![quest.into()],
            data: Some(TopicData {
                topic_type: 0,
                flags: Some(0),
            }),
            ..Self::default()
        }
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"QSTI" => push(&mut self.quests, one_form(d)),
                b"QSTR" => push(&mut self.removed_quests, one_form(d)),
                b"PNAM" => set(&mut self.priority, one_f32(d)),
                b"TDUM" => set(&mut self.dumb_response, zstring(d)),
                b"DATA" if matches!(d.len(), 1 | 2) => set(
                    &mut self.data,
                    Some(TopicData {
                        topic_type: d[0],
                        flags: d.get(1).copied(),
                    }),
                ),
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.form_each(b"QSTI", &self.quests);
        out.form_each(b"QSTR", &self.removed_quests);
        out.f32(b"PNAM", self.priority);
        out.zstr(b"TDUM", &self.dumb_response);
        out.opt(
            b"DATA",
            self.data.map(|d| {
                let mut data = vec![d.topic_type];
                data.extend(d.flags);
                data
            }),
        );
    }
}

typed_record!(Topic, DIAL);

/// An `INFO`'s `DATA`: 4 bytes, or 3 or 2 in the older forms.
///
/// Measured on the Fallout 3 master: 634 infos have the 3-byte form (one
/// flag byte), the rest the 4-byte one; every New Vegas info has the 4-byte
/// form. The 3-byte form's flag byte is the low byte of `flags` here, its
/// high byte zero, and `short` is set, so that the second flag byte reads
/// as "not stored" ([`InfoData::has_second_flag_byte`]); encoding writes
/// the form that was read.
///
/// # Flags
///
/// First byte: bit 0 goodbye, 1 random, 2 say once, 3 run immediately, 4
/// info refusal, 5 random end, 6 run for rumors, 7 speech challenge. Second
/// byte: bit 0 say once a day, 1 always darken. Measured: bit 7 is set on
/// exactly the infos that have a speech-challenge level (`DNAM` from 1 to 5)
/// and a `KNAM` in Fallout 3 (263 of them), and on the 3 challenge infos
/// of New Vegas. Bit 6 of the first byte is set on no info of either
/// master; in the second byte only bits 0 and 1 occur in Fallout 3, and
/// New Vegas also has bits 4 and 5 (on 28 and 20 infos), which are not
/// named here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct InfoData {
    /// The dialogue type, as stored (same codes as
    /// [`TopicData::topic_type`]).
    pub info_type: u8,
    /// Who speaks next, as stored: 0 target, 1 self, 2 either.
    pub next_speaker: u8,
    /// The two flag bytes, little-endian; absent from the 2-byte form.
    /// See [`InfoData::GOODBYE`] and the other flag constants.
    pub flags: Option<u16>,
    /// The stored `DATA` was the 3-byte form: only the first flag byte.
    pub short: bool,
}

impl InfoData {
    /// Flag: ends the conversation after it.
    pub const GOODBYE: u16 = 0x0001;
    /// Flag: one of a random group.
    pub const RANDOM: u16 = 0x0002;
    /// Flag: said once, never again.
    pub const SAY_ONCE: u16 = 0x0004;
    /// Flag: the result script runs right after it begins.
    pub const RUN_IMMEDIATELY: u16 = 0x0008;
    /// Flag: said, but the player stays in the same choice list.
    pub const INFO_REFUSAL: u16 = 0x0010;
    /// Flag: the last of a random group.
    pub const RANDOM_END: u16 = 0x0020;
    /// Flag: run for rumors.
    pub const RUN_FOR_RUMORS: u16 = 0x0040;
    /// Flag: a speech challenge (the first flag byte's `0x80`).
    pub const SPEECH_CHALLENGE: u16 = 0x0080;
    /// Flag: said once a day (the second flag byte's `0x01`).
    pub const SAY_ONCE_A_DAY: u16 = 0x0100;
    /// Flag: always darken the screen (the second flag byte's `0x02`).
    pub const ALWAYS_DARKEN: u16 = 0x0200;

    /// `DATA` of the 4-byte form.
    pub fn new(info_type: u8, next_speaker: u8, flags: u16) -> Self {
        Self {
            info_type,
            next_speaker,
            flags: Some(flags),
            short: false,
        }
    }

    /// `DATA` of the 3-byte form: only the first flag byte.
    pub fn short_form(info_type: u8, next_speaker: u8, flags: u8) -> Self {
        Self {
            info_type,
            next_speaker,
            flags: Some(u16::from(flags)),
            short: true,
        }
    }

    /// Whether the second flag byte (say once a day, always darken) was
    /// stored: the 4-byte form only. When it was not, those two flags read
    /// as clear.
    pub fn has_second_flag_byte(&self) -> bool {
        self.flags.is_some() && !self.short
    }

    /// Whether every bit of `mask` is set; clear when no flags are stored.
    pub fn has_flags(&self, mask: u16) -> bool {
        self.flags.is_some_and(|f| f & mask == mask)
    }

    /// The goodbye flag.
    pub fn is_goodbye(&self) -> bool {
        self.has_flags(Self::GOODBYE)
    }

    /// The random flag.
    pub fn is_random(&self) -> bool {
        self.has_flags(Self::RANDOM)
    }

    /// The say-once flag.
    pub fn is_say_once(&self) -> bool {
        self.has_flags(Self::SAY_ONCE)
    }

    /// The run-immediately flag.
    pub fn is_run_immediately(&self) -> bool {
        self.has_flags(Self::RUN_IMMEDIATELY)
    }

    /// The info-refusal flag.
    pub fn is_info_refusal(&self) -> bool {
        self.has_flags(Self::INFO_REFUSAL)
    }

    /// The random-end flag.
    pub fn is_random_end(&self) -> bool {
        self.has_flags(Self::RANDOM_END)
    }

    /// The run-for-rumors flag.
    pub fn is_run_for_rumors(&self) -> bool {
        self.has_flags(Self::RUN_FOR_RUMORS)
    }

    /// The speech-challenge flag.
    pub fn is_speech_challenge(&self) -> bool {
        self.has_flags(Self::SPEECH_CHALLENGE)
    }

    /// The say-once-a-day flag; clear in the 3-byte form.
    pub fn is_say_once_a_day(&self) -> bool {
        self.has_flags(Self::SAY_ONCE_A_DAY)
    }

    /// The always-darken flag; clear in the 3-byte form.
    pub fn is_always_darken(&self) -> bool {
        self.has_flags(Self::ALWAYS_DARKEN)
    }
}

/// The name of a speech-challenge level (`INFO` `DNAM`): 1 very easy, 2
/// easy, 3 average, 4 hard, 5 very hard; 0 means none. Measured: every
/// challenge info of both masters has a level from 1 to 5, and the game
/// settings `iSpeechChallengeDifficulty<Name>` are the five names.
pub fn speech_difficulty_name(level: u32) -> Option<&'static str> {
    Some(match level {
        1 => "VeryEasy",
        2 => "Easy",
        3 => "Average",
        4 => "Hard",
        5 => "VeryHard",
        _ => return None,
    })
}

/// A response's `TRDT`: 24 bytes, or 20 or 16 in the older forms.
///
/// Measured on the Fallout 3 master: 2,230 responses have the 16-byte form
/// (no sound, no flags) and 1,589 the 20-byte form (no flags), besides the
/// 24-byte one. The fields a short form lacks read as a null sound and
/// zero flags; encoding always writes the 24-byte form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ResponseData {
    /// The emotion type, as stored: 0 neutral, 1 anger, 2 disgust, 3 fear,
    /// 4 sad, 5 happy, 6 surprise, 7 pained.
    pub emotion_type: u32,
    /// The emotion value, 0 to 100.
    pub emotion_value: i32,
    /// The response number, from 1.
    pub response_number: u8,
    /// The sound; null when there is none.
    pub sound: FormId,
    /// Flags, as stored; bit 0 is "use emotion animation".
    pub flags: u8,
}

/// One response of an `INFO`: a `TRDT` and the subrecords after it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Response {
    /// Emotion, number and sound (`TRDT`). Every response of a valid record
    /// starts with one; `None` marks a response the decoder had to open
    /// without it.
    pub data: Option<ResponseData>,
    /// The spoken text (`NAM1`).
    pub text: Option<String>,
    /// Notes for the voice actor (`NAM2`).
    pub script_notes: Option<String>,
    /// Edits (`NAM3`).
    pub edits: Option<String>,
    /// The speaker's animation (`SNAM`).
    pub speaker_animation: Option<FormId>,
    /// The listener's animation (`LNAM`).
    pub listener_animation: Option<FormId>,
}

impl Response {
    /// A neutral response with the given number and text.
    pub fn new(number: u8, text: &str) -> Self {
        Self {
            data: Some(ResponseData {
                response_number: number,
                ..ResponseData::default()
            }),
            text: Some(text.to_owned()),
            ..Self::default()
        }
    }

    /// Sets the emotion.
    #[must_use]
    pub fn with_emotion(mut self, emotion_type: u32, emotion_value: i32) -> Self {
        let data = self.data.get_or_insert_with(ResponseData::default);
        data.emotion_type = emotion_type;
        data.emotion_value = emotion_value;
        self
    }
}

/// A dialogue response group (`INFO`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Info {
    /// The common fields.
    pub common: Common,
    /// Type, next speaker and flags (`DATA`).
    pub data: Option<InfoData>,
    /// The quest (`QSTI`).
    pub quest: Option<FormId>,
    /// The topic (`TPIC`); usually given by the enclosing group instead.
    pub topic: Option<FormId>,
    /// The previous `INFO` (`PNAM`).
    pub previous_info: Option<FormId>,
    /// Topics this response adds (`NAME`), in file order.
    pub add_topics: Vec<FormId>,
    /// Responses, in file order.
    pub responses: Vec<Response>,
    /// Conditions (`CTDA`), in file order.
    pub conditions: Vec<Condition>,
    /// Choices offered next (`TCLT`), in file order.
    pub choices: Vec<FormId>,
    /// Topics that link here (`TCLF`), in file order.
    pub link_from: Vec<FormId>,
    /// Follow-up topics (`TCFU`), in file order.
    pub follow_up: Vec<FormId>,
    /// The result script run when the response begins: the script block
    /// before the `NEXT` marker.
    pub result_begin: ScriptBody,
    /// The result script run when the response ends: the script block
    /// after the `NEXT` marker.
    pub result_end: ScriptBody,
    /// The player's prompt (`RNAM`).
    pub prompt: Option<String>,
    /// The speaker (`ANAM`).
    pub speaker: Option<FormId>,
    /// The speech challenge level (`DNAM`), as stored; see
    /// [`speech_difficulty_name`]. Present on every challenge info and on
    /// many that are not ([`InfoData::is_speech_challenge`] tells).
    pub speech_challenge: Option<u32>,
    /// The label of a skill check (`KNAM`): the actor value or perk the
    /// option is tagged with, a form id of an `AVIF` or `PERK` record.
    pub knam: Option<FormId>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

/// The response a subrecord belongs to: the last one if `is_free` says its
/// slot is still empty, otherwise a new response without a `TRDT`.
fn response_for(
    responses: &mut Vec<Response>,
    is_free: impl Fn(&Response) -> bool,
) -> Option<&mut Response> {
    if !responses.last().is_some_and(is_free) {
        responses.push(Response::default());
    }
    responses.last_mut()
}

impl Info {
    /// An `INFO` of type 0 for a quest, with responses.
    pub fn new(
        form_id: impl Into<FormId>,
        quest: impl Into<FormId>,
        responses: Vec<Response>,
    ) -> Self {
        Self {
            common: Common::new(form_id),
            data: Some(InfoData::new(0, 0, 0)),
            quest: Some(quest.into()),
            responses,
            ..Self::default()
        }
    }

    /// Sets the `DATA` flags (see [`InfoData::GOODBYE`] and the others),
    /// in the 4-byte form.
    #[must_use]
    pub fn with_flags(mut self, flags: u16) -> Self {
        let data = self.data.get_or_insert_with(|| InfoData::new(0, 0, 0));
        data.flags = Some(flags);
        data.short = false;
        self
    }

    /// Sets the type and who speaks next.
    #[must_use]
    pub fn with_type(mut self, info_type: u8, next_speaker: u8) -> Self {
        let data = self.data.get_or_insert_with(|| InfoData::new(0, 0, 0));
        data.info_type = info_type;
        data.next_speaker = next_speaker;
        self
    }

    /// Sets the skill-check label (`KNAM`).
    #[must_use]
    pub fn with_knam(mut self, label: impl Into<FormId>) -> Self {
        self.knam = Some(label.into());
        self
    }

    /// Sets the speech-challenge level (`DNAM`).
    #[must_use]
    pub fn with_speech_challenge(mut self, level: u32) -> Self {
        self.speech_challenge = Some(level);
        self
    }

    /// Sets the conditions.
    #[must_use]
    pub fn with_conditions(mut self, conditions: Vec<Condition>) -> Self {
        self.conditions = conditions;
        self
    }

    /// Sets the topic choices (`TCLT`).
    #[must_use]
    pub fn with_choices(mut self, choices: Vec<FormId>) -> Self {
        self.choices = choices;
        self
    }

    /// Sets the speaker (`ANAM`).
    #[must_use]
    pub fn with_speaker(mut self, speaker: impl Into<FormId>) -> Self {
        self.speaker = Some(speaker.into());
        self
    }

    /// Sets the result script that runs when the response begins.
    #[must_use]
    pub fn with_result_begin(mut self, script: ScriptBody) -> Self {
        self.result_begin = script;
        self
    }

    /// Sets the result script that runs when the response ends.
    #[must_use]
    pub fn with_result_end(mut self, script: ScriptBody) -> Self {
        self.result_end = script;
        self
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        let mut seen_next = false;
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"DATA" => set(&mut self.data, parse_data(d)),
                b"QSTI" => set(&mut self.quest, one_form(d)),
                b"TPIC" => set(&mut self.topic, one_form(d)),
                b"PNAM" => set(&mut self.previous_info, one_form(d)),
                b"NAME" => push(&mut self.add_topics, one_form(d)),
                b"TRDT" => push(
                    &mut self.responses,
                    parse_trdt(d).map(|data| Response {
                        data: Some(data),
                        ..Response::default()
                    }),
                ),
                b"NAM1" => match response_for(&mut self.responses, |r| r.text.is_none()) {
                    Some(response) => set(&mut response.text, zstring(d)),
                    None => false,
                },
                b"NAM2" => match response_for(&mut self.responses, |r| r.script_notes.is_none()) {
                    Some(response) => set(&mut response.script_notes, zstring(d)),
                    None => false,
                },
                b"NAM3" => match response_for(&mut self.responses, |r| r.edits.is_none()) {
                    Some(response) => set(&mut response.edits, zstring(d)),
                    None => false,
                },
                b"SNAM" => match one_form(d) {
                    Some(form) => {
                        match response_for(&mut self.responses, |r| r.speaker_animation.is_none()) {
                            Some(response) => set(&mut response.speaker_animation, Some(form)),
                            None => false,
                        }
                    }
                    None => false,
                },
                b"LNAM" => match one_form(d) {
                    Some(form) => {
                        match response_for(&mut self.responses, |r| r.listener_animation.is_none())
                        {
                            Some(response) => set(&mut response.listener_animation, Some(form)),
                            None => false,
                        }
                    }
                    None => false,
                },
                b"CTDA" => push(&mut self.conditions, Condition::parse(d)),
                b"TCLT" => push(&mut self.choices, one_form(d)),
                b"TCLF" => push(&mut self.link_from, one_form(d)),
                b"TCFU" => push(&mut self.follow_up, one_form(d)),
                b"NEXT" if !seen_next && d.is_empty() => {
                    seen_next = true;
                    true
                }
                b"SCHR" | b"SCDA" | b"SCTX" | b"SLSD" | b"SCVR" | b"SCRO" | b"SCRV" => {
                    if seen_next {
                        self.result_end.take(s)
                    } else {
                        self.result_begin.take(s)
                    }
                }
                b"RNAM" => set(&mut self.prompt, zstring(d)),
                b"ANAM" => set(&mut self.speaker, one_form(d)),
                b"DNAM" => set(&mut self.speech_challenge, one_u32(d)),
                b"KNAM" => set(&mut self.knam, one_form(d)),
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.opt(
            b"DATA",
            self.data.map(|d| {
                let w = Wr::default().u8(d.info_type).u8(d.next_speaker);
                match d.flags {
                    Some(flags) if d.short => w.u8(flags as u8),
                    Some(flags) => w.u16(flags),
                    None => w,
                }
                .done()
            }),
        );
        out.form(b"QSTI", self.quest);
        out.form(b"TPIC", self.topic);
        out.form(b"PNAM", self.previous_info);
        out.form_each(b"NAME", &self.add_topics);
        for response in &self.responses {
            out.opt(
                b"TRDT",
                response.data.map(|d| {
                    Wr::default()
                        .u32(d.emotion_type)
                        .i32(d.emotion_value)
                        .zeros(4)
                        .u8(d.response_number)
                        .zeros(3)
                        .form(d.sound)
                        .u8(d.flags)
                        .zeros(3)
                        .done()
                }),
            );
            out.zstr(b"NAM1", &response.text);
            out.zstr(b"NAM2", &response.script_notes);
            out.zstr(b"NAM3", &response.edits);
            out.form(b"SNAM", response.speaker_animation);
            out.form(b"LNAM", response.listener_animation);
        }
        for condition in &self.conditions {
            out.raw(b"CTDA", condition.bytes());
        }
        out.form_each(b"TCLT", &self.choices);
        out.form_each(b"TCLF", &self.link_from);
        out.form_each(b"TCFU", &self.follow_up);
        if !self.result_begin.is_empty() || !self.result_end.is_empty() {
            self.result_begin.emit(out);
            out.raw(b"NEXT", Vec::new());
            self.result_end.emit(out);
        }
        out.zstr(b"RNAM", &self.prompt);
        out.form(b"ANAM", self.speaker);
        out.u32(b"DNAM", self.speech_challenge);
        out.form(b"KNAM", self.knam);
    }
}

typed_record!(Info, INFO);

fn parse_data(data: &[u8]) -> Option<InfoData> {
    if !matches!(data.len(), 2..=4) {
        return None;
    }
    let mut rd = Rd::new(data);
    let info_type = rd.u8()?;
    let next_speaker = rd.u8()?;
    let flags = match data.len() {
        4 => rd.u16(),
        3 => rd.u8().map(u16::from),
        _ => None,
    };
    Some(InfoData {
        info_type,
        next_speaker,
        flags,
        short: data.len() == 3,
    })
}

fn parse_trdt(data: &[u8]) -> Option<ResponseData> {
    if !matches!(data.len(), 16 | 20 | 24) {
        return None;
    }
    let mut rd = Rd::new(data);
    let emotion_type = rd.u32()?;
    let emotion_value = rd.i32()?;
    rd.skip(4)?;
    let response_number = rd.u8()?;
    rd.skip(3)?;
    let sound = if data.len() >= 20 {
        rd.form()?
    } else {
        FormId::NULL
    };
    let flags = if data.len() >= 24 { rd.u8()? } else { 0 };
    Some(ResponseData {
        emotion_type,
        emotion_value,
        response_number,
        sound,
        flags,
    })
}
