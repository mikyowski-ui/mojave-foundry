//! What an actor is made of: races (`RACE`), head parts (`HDPT`), hairs
//! (`HAIR`) and eyes (`EYES`).

use serde::{Deserialize, Serialize};

use super::{Common, typed_record};
use crate::bytes::{Rd, Wr, form_array, one_u8, one_u32, set, zstring};
use crate::formid::FormId;
use crate::subrecord::{Out, RawSubrecord, SubrecordRef};

/// A race's `DATA` (36 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct RaceData {
    /// Seven skill boosts, as stored: actor value and boost.
    pub skill_boosts: [[u8; 2]; 7],
    /// Height multiplier, male then female.
    pub height: [f32; 2],
    /// Weight, male then female.
    pub weight: [f32; 2],
    /// Race flags, as stored (bit 0: playable).
    pub flags: u32,
}

impl RaceData {
    fn parse(data: &[u8]) -> Option<Self> {
        let mut rd = Rd::exact(data, 36)?;
        let mut skill_boosts = [[0u8; 2]; 7];
        for boost in &mut skill_boosts {
            *boost = rd.arr()?;
        }
        rd.skip(2)?;
        Some(Self {
            skill_boosts,
            height: [rd.f32()?, rd.f32()?],
            weight: [rd.f32()?, rd.f32()?],
            flags: rd.u32()?,
        })
    }

    fn bytes(&self) -> Vec<u8> {
        self.skill_boosts
            .iter()
            .fold(Wr::default(), |w, b| w.bytes(b))
            .zeros(2)
            .f32(self.height[0])
            .f32(self.height[1])
            .f32(self.weight[0])
            .f32(self.weight[1])
            .u32(self.flags)
            .done()
    }
}

/// One head or body part of a race: `INDX` and the `MODL` and `ICON` that
/// follow it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RacePart {
    /// The part index (`INDX`). Head parts: 0 head, 1 ears, 2 mouth,
    /// 3 lower teeth, 4 upper teeth, 5 tongue, 6 left eye, 7 right eye.
    /// Body parts: 0 upper body, 1 left hand, 2 right hand, 3 the upper
    /// body's FaceGen tint file.
    pub index: u32,
    /// The model path (`MODL`), as stored.
    pub model: Option<String>,
    /// The diffuse texture path (`ICON`), as stored.
    pub texture: Option<String>,
}

impl RacePart {
    /// A part with an index, a model and a texture.
    pub fn new(index: u32, model: Option<&str>, texture: Option<&str>) -> Self {
        Self {
            index,
            model: model.map(str::to_owned),
            texture: texture.map(str::to_owned),
        }
    }
}

/// The parts of one section (head or body), male and female.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RaceParts {
    /// The male parts, in file order.
    pub male: Vec<RacePart>,
    /// The female parts, in file order.
    pub female: Vec<RacePart>,
}

impl RaceParts {
    /// The parts of one sex.
    pub fn of(&self, female: bool) -> &[RacePart] {
        if female { &self.female } else { &self.male }
    }

    fn of_mut(&mut self, female: bool) -> &mut Vec<RacePart> {
        if female {
            &mut self.female
        } else {
            &mut self.male
        }
    }
}

/// A race (`RACE`).
///
/// # Layout of the parts
///
/// Checked against a retail master (every race there has this shape): the
/// head section starts with an empty `NAM0`, the body section with an
/// empty `NAM1`; inside each, an empty `MNAM` starts the male parts and an
/// empty `FNAM` the female ones. A part is an `INDX` followed by its
/// `MODL` (with `MODB`, `MODT`, `MODD`) and its `ICON`, in either order.
/// After the body section the hair (`HNAM`) and eyes (`ENAM`) lists
/// follow, then a second `MNAM` / `FNAM` pair that heads the FaceGen data;
/// those markers are not part markers and stay in `unknown`.
///
/// The decoder looks for the markers in that order before reading parts,
/// so a section is read only when all three of its markers are present.
/// Model data other than the path (`MODB`, `MODT`, `MODD`), the voices,
/// the older and younger races, the FaceGen data and the default hair
/// colours (`CNAM`, indexes rather than colours) stay in `unknown`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Race {
    /// The common fields.
    pub common: Common,
    /// The description (`DESC`).
    pub description: Option<String>,
    /// Skill boosts, heights, weights and flags (`DATA`).
    pub data: Option<RaceData>,
    /// The default hair, male then female (`DNAM`).
    pub default_hair: Option<[FormId; 2]>,
    /// The head section; `None` when the record has none.
    pub head: Option<RaceParts>,
    /// The body section; `None` when the record has none.
    pub body: Option<RaceParts>,
    /// The hairs this race may have (`HNAM`), in file order.
    pub hairs: Vec<FormId>,
    /// The eyes this race may have (`ENAM`), in file order.
    pub eyes: Vec<FormId>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

/// The subrecords that belong to a part.
fn is_part_sub(sig: &[u8; 4]) -> bool {
    matches!(
        sig,
        b"INDX" | b"MODL" | b"MODB" | b"MODT" | b"MODD" | b"MODS" | b"ICON" | b"MICO"
    )
}

/// Which section and sex a subrecord position belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Region {
    Head(bool),
    Body(bool),
}

/// Finds the part markers: positions of the consumed markers, and the
/// region of every subrecord that sits in a part list.
fn part_layout(subs: &[SubrecordRef<'_>]) -> (Vec<bool>, Vec<Option<Region>>) {
    let is_marker = |i: usize, sig: &[u8; 4]| {
        subs.get(i)
            .is_some_and(|s| &s.sig.0 == sig && s.data.is_empty())
    };
    // The end of the run of part subrecords that starts at `first`.
    let run = |first: usize| {
        (first..subs.len())
            .find(|&i| !is_part_sub(&subs[i].sig.0))
            .unwrap_or(subs.len())
    };
    let mut marker = vec![false; subs.len()];
    let mut region = vec![None; subs.len()];
    // A section: its opening marker, MNAM right after it, the male parts,
    // FNAM right after them, the female parts.
    let mut section = |open: &[u8; 4], from: usize, make: fn(bool) -> Region| -> Option<usize> {
        let start = (from..subs.len()).find(|&i| is_marker(i, open))?;
        let male = start + 1;
        if !is_marker(male, b"MNAM") {
            return None;
        }
        let female = run(male + 1);
        if !is_marker(female, b"FNAM") {
            return None;
        }
        let male_end = female;
        let female_end = run(female + 1);
        for i in [start, male, female] {
            marker[i] = true;
        }
        for slot in &mut region[male + 1..male_end] {
            *slot = Some(make(false));
        }
        for slot in &mut region[female + 1..female_end] {
            *slot = Some(make(true));
        }
        Some(female_end)
    };
    let after_head = section(b"NAM0", 0, Region::Head).unwrap_or(0);
    section(b"NAM1", after_head, Region::Body);
    (marker, region)
}

impl Race {
    /// A race with an editor id and a display name, and no parts.
    pub fn new(form_id: impl Into<FormId>, editor_id: &str, name: &str) -> Self {
        Self {
            common: Common::new(form_id)
                .with_editor_id(editor_id)
                .with_name(name),
            ..Self::default()
        }
    }

    /// The head parts of one sex; empty when the race has no head section.
    pub fn head_parts(&self, female: bool) -> &[RacePart] {
        self.head.as_ref().map_or(&[], |p| p.of(female))
    }

    /// The body parts of one sex; empty when the race has no body section.
    pub fn body_parts(&self, female: bool) -> &[RacePart] {
        self.body.as_ref().map_or(&[], |p| p.of(female))
    }

    /// The default hair of one sex.
    pub fn default_hair_for(&self, female: bool) -> Option<FormId> {
        self.default_hair
            .map(|h| h[usize::from(female)])
            .filter(|f| !f.is_null())
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        let (marker, region) = part_layout(subs);
        // The part the next MODL or ICON belongs to.
        let mut current: Option<(Region, usize)> = None;
        let mut position = 0usize;
        decode_loop!(self, subs, |s| {
            let i = position;
            position += 1;
            let d = s.data;
            if marker[i] {
                current = None;
                match &s.sig.0 {
                    b"NAM0" => self.head = Some(RaceParts::default()),
                    b"NAM1" => self.body = Some(RaceParts::default()),
                    _ => {}
                }
                true
            } else if let Some(r) = region[i] {
                let list = match r {
                    Region::Head(f) => self.head.get_or_insert_default().of_mut(f),
                    Region::Body(f) => self.body.get_or_insert_default().of_mut(f),
                };
                match &s.sig.0 {
                    b"INDX" => match one_u32(d) {
                        Some(index) => {
                            list.push(RacePart {
                                index,
                                ..RacePart::default()
                            });
                            current = Some((r, list.len() - 1));
                            true
                        }
                        None => {
                            current = None;
                            false
                        }
                    },
                    b"MODL" | b"ICON" => match current {
                        Some((cr, at)) if cr == r => {
                            let part = &mut list[at];
                            if &s.sig.0 == b"MODL" {
                                set(&mut part.model, zstring(d))
                            } else {
                                set(&mut part.texture, zstring(d))
                            }
                        }
                        _ => false,
                    },
                    _ => false,
                }
            } else {
                match &s.sig.0 {
                    b"DESC" => set(&mut self.description, zstring(d)),
                    b"DATA" => set(&mut self.data, RaceData::parse(d)),
                    b"DNAM" => set(
                        &mut self.default_hair,
                        Rd::exact(d, 8).and_then(|mut rd| Some([rd.form()?, rd.form()?])),
                    ),
                    b"HNAM" if self.hairs.is_empty() => match form_array(d) {
                        Some(list) if !list.is_empty() => {
                            self.hairs = list;
                            true
                        }
                        _ => false,
                    },
                    b"ENAM" if self.eyes.is_empty() => match form_array(d) {
                        Some(list) if !list.is_empty() => {
                            self.eyes = list;
                            true
                        }
                        _ => false,
                    },
                    _ => false,
                }
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.zstr(b"DESC", &self.description);
        out.opt(b"DATA", self.data.map(|d| d.bytes()));
        out.opt(
            b"DNAM",
            self.default_hair
                .map(|h| Wr::default().form(h[0]).form(h[1]).done()),
        );
        for (open, parts) in [(b"NAM0", &self.head), (b"NAM1", &self.body)] {
            let Some(parts) = parts else {
                continue;
            };
            out.raw(open, Vec::new());
            for (marker, list) in [(b"MNAM", &parts.male), (b"FNAM", &parts.female)] {
                out.raw(marker, Vec::new());
                for part in list {
                    out.u32(b"INDX", Some(part.index));
                    out.zstr(b"MODL", &part.model);
                    out.zstr(b"ICON", &part.texture);
                }
            }
        }
        out.form_array(b"HNAM", &self.hairs);
        out.form_array(b"ENAM", &self.eyes);
    }
}

typed_record!(Race, RACE);

/// A head part (`HDPT`): a mesh worn on the head, such as a beard or
/// eyebrows. Model data (`MODB`, `MODT`) and alternate textures (`MODS`)
/// stay in `unknown`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HeadPart {
    /// The common fields. `model` is the part's mesh.
    pub common: Common,
    /// Flags (`DATA`), as stored (bit 0: playable).
    pub flags: Option<u8>,
    /// Further head parts that come with this one (`HNAM`), in file order.
    pub extra_parts: Vec<FormId>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl HeadPart {
    /// A head part with an editor id and a model.
    pub fn new(form_id: impl Into<FormId>, editor_id: &str, model: &str) -> Self {
        Self {
            common: Common::new(form_id)
                .with_editor_id(editor_id)
                .with_model(model),
            ..Self::default()
        }
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"DATA" => set(&mut self.flags, one_u8(d)),
                b"HNAM" => match crate::bytes::one_form(d) {
                    Some(form) => {
                        self.extra_parts.push(form);
                        true
                    }
                    None => false,
                },
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.u8(b"DATA", self.flags);
        out.form_each(b"HNAM", &self.extra_parts);
    }
}

typed_record!(HeadPart, HDPT);

/// A hair style (`HAIR`). Model data (`MODB`, `MODT`) stays in `unknown`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Hair {
    /// The common fields. `model` is the hair mesh.
    pub common: Common,
    /// The hair texture (`ICON`), as stored.
    pub texture: Option<String>,
    /// Flags (`DATA`), as stored (bit 0: playable).
    pub flags: Option<u8>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl Hair {
    /// A hair with an editor id, a model and a texture.
    pub fn new(form_id: impl Into<FormId>, editor_id: &str, model: &str, texture: &str) -> Self {
        Self {
            common: Common::new(form_id)
                .with_editor_id(editor_id)
                .with_model(model),
            texture: Some(texture.to_owned()),
            flags: Some(1),
            ..Self::default()
        }
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"ICON" => set(&mut self.texture, zstring(d)),
                b"DATA" => set(&mut self.flags, one_u8(d)),
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.zstr(b"ICON", &self.texture);
        out.u8(b"DATA", self.flags);
    }
}

typed_record!(Hair, HAIR);

/// Eyes (`EYES`): a texture for the race's eye meshes.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Eyes {
    /// The common fields.
    pub common: Common,
    /// The eye texture (`ICON`), as stored.
    pub texture: Option<String>,
    /// Flags (`DATA`), as stored (bit 0: playable).
    pub flags: Option<u8>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl Eyes {
    /// Eyes with an editor id and a texture.
    pub fn new(form_id: impl Into<FormId>, editor_id: &str, texture: &str) -> Self {
        Self {
            common: Common::new(form_id).with_editor_id(editor_id),
            texture: Some(texture.to_owned()),
            flags: Some(1),
            ..Self::default()
        }
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"ICON" => set(&mut self.texture, zstring(d)),
                b"DATA" => set(&mut self.flags, one_u8(d)),
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.zstr(b"ICON", &self.texture);
        out.u8(b"DATA", self.flags);
    }
}

typed_record!(Eyes, EYES);
