//! Actors: NPCs (`NPC_`) and creatures (`CREA`).

use serde::{Deserialize, Serialize};

use super::{Common, ContainerItem, typed_record};
use crate::bytes::{Rd, Wr, one_f32, one_form, push, set};
use crate::formid::FormId;
use crate::strings::{decode_cp1252, encode_cp1252_lossy};
use crate::subrecord::{Out, RawSubrecord, SubrecordRef};

/// An actor's base stats (`ACBS`, 24 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ActorBase {
    /// Actor flags, as stored.
    pub flags: u32,
    /// Fatigue.
    pub fatigue: u16,
    /// Barter gold.
    pub barter_gold: u16,
    /// Level, or the level multiplier when the flags say so.
    pub level: i16,
    /// Minimum calculated level.
    pub calc_min: u16,
    /// Maximum calculated level.
    pub calc_max: u16,
    /// Speed multiplier.
    pub speed_multiplier: u16,
    /// Karma.
    pub karma: f32,
    /// Base disposition.
    pub disposition_base: i16,
    /// Which data comes from the template, as stored.
    pub template_flags: u16,
}

impl ActorBase {
    fn parse(data: &[u8]) -> Option<Self> {
        let mut rd = Rd::exact(data, 24)?;
        Some(Self {
            flags: rd.u32()?,
            fatigue: rd.u16()?,
            barter_gold: rd.u16()?,
            level: rd.i16()?,
            calc_min: rd.u16()?,
            calc_max: rd.u16()?,
            speed_multiplier: rd.u16()?,
            karma: rd.f32()?,
            disposition_base: rd.i16()?,
            template_flags: rd.u16()?,
        })
    }

    fn bytes(&self) -> Vec<u8> {
        Wr::default()
            .u32(self.flags)
            .u16(self.fatigue)
            .u16(self.barter_gold)
            .i16(self.level)
            .u16(self.calc_min)
            .u16(self.calc_max)
            .u16(self.speed_multiplier)
            .f32(self.karma)
            .i16(self.disposition_base)
            .u16(self.template_flags)
            .done()
    }
}

/// Membership of a faction (`SNAM`, 8 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FactionRank {
    /// The faction.
    pub faction: FormId,
    /// The rank in it.
    pub rank: u8,
}

impl FactionRank {
    fn parse(data: &[u8]) -> Option<Self> {
        let mut rd = Rd::exact(data, 8)?;
        Some(Self {
            faction: rd.form()?,
            rank: rd.u8()?,
        })
    }

    fn bytes(&self) -> Vec<u8> {
        Wr::default()
            .form(self.faction)
            .u8(self.rank)
            .zeros(3)
            .done()
    }
}

/// An NPC's `DATA`: 11 bytes, or 25 in an older form.
///
/// The 25-byte form puts the fourteen skill values first, then the same
/// health and attributes. Measured on the Fallout 3 master: two NPCs have
/// it, both without a `DNAM`, and their 14 leading bytes sit where a skill
/// table would (14 equal values, as a template's defaults are). Its skill
/// values go to [`Npc::skills`] (with zero offsets) when the record has no
/// `DNAM` of its own; encoding writes the 11-byte `DATA` and a `DNAM`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NpcStats {
    /// Base health.
    pub base_health: i32,
    /// The seven primary attributes, in the game's order.
    pub attributes: [u8; 7],
}

/// An NPC's `DNAM` (28 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NpcSkills {
    /// The fourteen skill values.
    pub values: [u8; 14],
    /// The fourteen skill offsets.
    pub offsets: [u8; 14],
}

/// A colour (`HCLR`: red, green, blue and one unused byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Rgb {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
}

/// A non-player character (`NPC_`).
///
/// The AI data (`AIDT`) is not decoded and stays in `unknown`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Npc {
    /// The common fields.
    pub common: Common,
    /// Base stats (`ACBS`).
    pub base: Option<ActorBase>,
    /// Factions (`SNAM`), in file order.
    pub factions: Vec<FactionRank>,
    /// The death item (`INAM`).
    pub death_item: Option<FormId>,
    /// The voice type (`VTCK`).
    pub voice: Option<FormId>,
    /// The template actor (`TPLT`).
    pub template: Option<FormId>,
    /// The race (`RNAM`).
    pub race: Option<FormId>,
    /// The class (`CNAM`).
    pub class: Option<FormId>,
    /// Actor effects (`SPLO`), in file order.
    pub actor_effects: Vec<FormId>,
    /// Inventory (`CNTO`), in file order.
    pub inventory: Vec<ContainerItem>,
    /// AI packages (`PKID`), in file order.
    pub packages: Vec<FormId>,
    /// Health and attributes (`DATA`).
    pub stats: Option<NpcStats>,
    /// Skills (`DNAM`).
    pub skills: Option<NpcSkills>,
    /// Head parts (`PNAM`), in file order.
    pub head_parts: Vec<FormId>,
    /// The hair (`HNAM`).
    pub hair: Option<FormId>,
    /// The hair length (`LNAM`).
    pub hair_length: Option<f32>,
    /// The eyes (`ENAM`).
    pub eyes: Option<FormId>,
    /// The hair colour (`HCLR`).
    pub hair_color: Option<Rgb>,
    /// The combat style (`ZNAM`).
    pub combat_style: Option<FormId>,
    /// FaceGen symmetric geometry (`FGGS`), as stored.
    pub facegen_geometry: Option<Vec<u8>>,
    /// FaceGen asymmetric geometry (`FGGA`), as stored.
    pub facegen_asymmetry: Option<Vec<u8>>,
    /// FaceGen symmetric texture (`FGTS`), as stored.
    pub facegen_texture: Option<Vec<u8>>,
    /// Height (`NAM6`).
    pub height: Option<f32>,
    /// Weight (`NAM7`).
    pub weight: Option<f32>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl Npc {
    /// An NPC with an editor id and a display name.
    pub fn new(form_id: impl Into<FormId>, editor_id: &str, name: &str) -> Self {
        Self {
            common: Common::new(form_id)
                .with_editor_id(editor_id)
                .with_name(name),
            base: Some(ActorBase::default()),
            ..Self::default()
        }
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        // Skill values of the older 25-byte `DATA`, used when no `DNAM`
        // follows.
        // A record with both keeps the 25-byte `DATA` whole in `unknown`,
        // so that nothing is lost.
        let mut legacy_skills: Option<[u8; 14]> = None;
        let has_dnam = subs
            .iter()
            .any(|s| &s.sig.0 == b"DNAM" && s.data.len() == 28);
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"ACBS" => set(&mut self.base, ActorBase::parse(d)),
                b"SNAM" => push(&mut self.factions, FactionRank::parse(d)),
                b"INAM" => set(&mut self.death_item, one_form(d)),
                b"VTCK" => set(&mut self.voice, one_form(d)),
                b"TPLT" => set(&mut self.template, one_form(d)),
                b"RNAM" => set(&mut self.race, one_form(d)),
                b"CNAM" => set(&mut self.class, one_form(d)),
                b"SPLO" => push(&mut self.actor_effects, one_form(d)),
                b"CNTO" => push(&mut self.inventory, ContainerItem::parse(d)),
                b"PKID" => push(&mut self.packages, one_form(d)),
                b"DATA" if d.len() == 25 && !has_dnam && self.stats.is_none() => {
                    let mut rd = Rd::new(d);
                    let parsed = rd.arr::<14>().and_then(|skills| {
                        let stats = NpcStats {
                            base_health: rd.i32()?,
                            attributes: rd.arr()?,
                        };
                        Some((skills, stats))
                    });
                    match parsed {
                        Some((skills, stats)) => {
                            legacy_skills = Some(skills);
                            self.stats = Some(stats);
                            true
                        }
                        None => false,
                    }
                }
                b"DATA" => set(
                    &mut self.stats,
                    Rd::exact(d, 11).and_then(|mut rd| {
                        Some(NpcStats {
                            base_health: rd.i32()?,
                            attributes: rd.arr()?,
                        })
                    }),
                ),
                b"DNAM" => set(
                    &mut self.skills,
                    Rd::exact(d, 28).and_then(|mut rd| {
                        Some(NpcSkills {
                            values: rd.arr()?,
                            offsets: rd.arr()?,
                        })
                    }),
                ),
                b"PNAM" => push(&mut self.head_parts, one_form(d)),
                b"HNAM" => set(&mut self.hair, one_form(d)),
                b"LNAM" => set(&mut self.hair_length, one_f32(d)),
                b"ENAM" => set(&mut self.eyes, one_form(d)),
                b"HCLR" => set(
                    &mut self.hair_color,
                    Rd::exact(d, 4).and_then(|mut rd| {
                        Some(Rgb {
                            r: rd.u8()?,
                            g: rd.u8()?,
                            b: rd.u8()?,
                        })
                    }),
                ),
                b"ZNAM" => set(&mut self.combat_style, one_form(d)),
                b"FGGS" => set(&mut self.facegen_geometry, Some(d.to_vec())),
                b"FGGA" => set(&mut self.facegen_asymmetry, Some(d.to_vec())),
                b"FGTS" => set(&mut self.facegen_texture, Some(d.to_vec())),
                b"NAM6" => set(&mut self.height, one_f32(d)),
                b"NAM7" => set(&mut self.weight, one_f32(d)),
                _ => false,
            }
        });
        if let Some(values) = legacy_skills
            && self.skills.is_none()
        {
            self.skills = Some(NpcSkills {
                values,
                offsets: [0; 14],
            });
        }
    }

    fn emit_fields(&self, out: &mut Out) {
        out.opt(b"ACBS", self.base.map(|b| b.bytes()));
        for faction in &self.factions {
            out.raw(b"SNAM", faction.bytes());
        }
        out.form(b"INAM", self.death_item);
        out.form(b"VTCK", self.voice);
        out.form(b"TPLT", self.template);
        out.form(b"RNAM", self.race);
        out.form_each(b"SPLO", &self.actor_effects);
        for item in &self.inventory {
            out.raw(b"CNTO", item.bytes());
        }
        out.form_each(b"PKID", &self.packages);
        out.form(b"CNAM", self.class);
        out.opt(
            b"DATA",
            self.stats
                .map(|s| Wr::default().i32(s.base_health).bytes(&s.attributes).done()),
        );
        out.opt(
            b"DNAM",
            self.skills
                .map(|s| Wr::default().bytes(&s.values).bytes(&s.offsets).done()),
        );
        out.form_each(b"PNAM", &self.head_parts);
        out.form(b"HNAM", self.hair);
        out.f32(b"LNAM", self.hair_length);
        out.form(b"ENAM", self.eyes);
        out.opt(b"HCLR", self.hair_color.map(|c| vec![c.r, c.g, c.b, 0]));
        out.form(b"ZNAM", self.combat_style);
        out.opt(b"FGGS", self.facegen_geometry.clone());
        out.opt(b"FGGA", self.facegen_asymmetry.clone());
        out.opt(b"FGTS", self.facegen_texture.clone());
        out.f32(b"NAM6", self.height);
        out.f32(b"NAM7", self.weight);
    }
}

typed_record!(Npc, NPC_);

/// A creature's `DATA` (17 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CreatureData {
    /// The creature type, as stored: 0 animal, 1 mutated animal, 2 mutated
    /// insect, 3 abomination, 4 super mutant, 5 feral ghoul, 6 robot,
    /// 7 giant.
    pub creature_type: u8,
    /// Combat skill.
    pub combat_skill: u8,
    /// Magic skill.
    pub magic_skill: u8,
    /// Stealth skill.
    pub stealth_skill: u8,
    /// Health. The two bytes after it are zero in every creature of a
    /// retail master, so a 16-bit and a 32-bit reading agree.
    pub health: i16,
    /// Unarmed damage.
    pub damage: i16,
    /// The seven primary attributes, in the game's order.
    pub attributes: [u8; 7],
}

/// A creature (`CREA`).
///
/// The AI data (`AIDT`), sounds, body-part data and the other subrecords
/// not listed here are not decoded and stay in `unknown`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Creature {
    /// The common fields. `model` is the skeleton.
    pub common: Common,
    /// Base stats (`ACBS`).
    pub base: Option<ActorBase>,
    /// The body model file names (`NIFZ`), relative to the skeleton's
    /// folder.
    pub model_list: Vec<String>,
    /// The animation file names (`KFFZ`).
    pub animations: Vec<String>,
    /// Factions (`SNAM`), in file order.
    pub factions: Vec<FactionRank>,
    /// The death item (`INAM`).
    pub death_item: Option<FormId>,
    /// The voice type (`VTCK`).
    pub voice: Option<FormId>,
    /// The template actor (`TPLT`).
    pub template: Option<FormId>,
    /// Actor effects (`SPLO`), in file order.
    pub actor_effects: Vec<FormId>,
    /// Inventory (`CNTO`), in file order.
    pub inventory: Vec<ContainerItem>,
    /// AI packages (`PKID`), in file order.
    pub packages: Vec<FormId>,
    /// Type, skills, health, damage and attributes (`DATA`).
    pub data: Option<CreatureData>,
    /// The combat style (`ZNAM`).
    pub combat_style: Option<FormId>,
    /// The base scale (`BNAM`).
    pub base_scale: Option<f32>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

/// Reads a list of NUL-terminated strings that ends with an empty one.
/// An empty list gives `None`, so that its subrecord is kept as it is.
fn string_list(data: &[u8]) -> Option<Vec<String>> {
    let list: Vec<String> = data
        .split(|&b| b == 0)
        .filter(|part| !part.is_empty())
        .map(decode_cp1252)
        .collect();
    (!list.is_empty()).then_some(list)
}

fn string_list_bytes(list: &[String]) -> Option<Vec<u8>> {
    if list.is_empty() {
        return None;
    }
    let mut data = Vec::new();
    for name in list {
        data.extend(encode_cp1252_lossy(name));
        data.push(0);
    }
    data.push(0);
    Some(data)
}

fn fill(slot: &mut Vec<String>, value: Option<Vec<String>>) -> bool {
    match value {
        Some(list) if slot.is_empty() => {
            *slot = list;
            true
        }
        _ => false,
    }
}

impl Creature {
    /// A creature with an editor id, a display name and a skeleton model.
    pub fn new(form_id: impl Into<FormId>, editor_id: &str, name: &str, model: &str) -> Self {
        Self {
            common: Common::new(form_id)
                .with_editor_id(editor_id)
                .with_name(name)
                .with_model(model),
            base: Some(ActorBase::default()),
            ..Self::default()
        }
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"ACBS" => set(&mut self.base, ActorBase::parse(d)),
                b"NIFZ" => fill(&mut self.model_list, string_list(d)),
                b"KFFZ" => fill(&mut self.animations, string_list(d)),
                b"SNAM" => push(&mut self.factions, FactionRank::parse(d)),
                b"INAM" => set(&mut self.death_item, one_form(d)),
                b"VTCK" => set(&mut self.voice, one_form(d)),
                b"TPLT" => set(&mut self.template, one_form(d)),
                b"SPLO" => push(&mut self.actor_effects, one_form(d)),
                b"CNTO" => push(&mut self.inventory, ContainerItem::parse(d)),
                b"PKID" => push(&mut self.packages, one_form(d)),
                b"DATA" => set(&mut self.data, parse_creature_data(d)),
                b"ZNAM" => set(&mut self.combat_style, one_form(d)),
                b"BNAM" => set(&mut self.base_scale, one_f32(d)),
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.form_each(b"SPLO", &self.actor_effects);
        out.opt(b"NIFZ", string_list_bytes(&self.model_list));
        out.opt(b"ACBS", self.base.map(|b| b.bytes()));
        for faction in &self.factions {
            out.raw(b"SNAM", faction.bytes());
        }
        out.form(b"INAM", self.death_item);
        out.form(b"VTCK", self.voice);
        out.form(b"TPLT", self.template);
        for item in &self.inventory {
            out.raw(b"CNTO", item.bytes());
        }
        out.form_each(b"PKID", &self.packages);
        out.opt(b"KFFZ", string_list_bytes(&self.animations));
        out.opt(
            b"DATA",
            self.data.map(|d| {
                Wr::default()
                    .u8(d.creature_type)
                    .u8(d.combat_skill)
                    .u8(d.magic_skill)
                    .u8(d.stealth_skill)
                    .i16(d.health)
                    .zeros(2)
                    .i16(d.damage)
                    .bytes(&d.attributes)
                    .done()
            }),
        );
        out.form(b"ZNAM", self.combat_style);
        out.f32(b"BNAM", self.base_scale);
    }
}

typed_record!(Creature, CREA);

fn parse_creature_data(data: &[u8]) -> Option<CreatureData> {
    let mut rd = Rd::exact(data, 17)?;
    let creature_type = rd.u8()?;
    let combat_skill = rd.u8()?;
    let magic_skill = rd.u8()?;
    let stealth_skill = rd.u8()?;
    let health = rd.i16()?;
    rd.skip(2)?;
    Some(CreatureData {
        creature_type,
        combat_skill,
        magic_skill,
        stealth_skill,
        health,
        damage: rd.i16()?,
        attributes: rd.arr()?,
    })
}
