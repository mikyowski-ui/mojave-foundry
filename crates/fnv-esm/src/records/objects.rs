//! Base objects: statics, doors, containers and items.

use serde::{Deserialize, Serialize};

use super::{Common, Condition, typed_record};
use crate::bytes::{Rd, Wr, one_f32, one_form, one_i32, one_u8, push, set, zstring};
use crate::formid::FormId;
use crate::subrecord::{Out, RawSubrecord, SubrecordRef};

/// A static object (`STAT`). It has only the common fields.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Static {
    /// The common fields.
    pub common: Common,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl Static {
    /// A static with an editor id and a model.
    pub fn new(form_id: impl Into<FormId>, editor_id: &str, model: &str) -> Self {
        Self {
            common: Common::new(form_id)
                .with_editor_id(editor_id)
                .with_model(model),
            unknown: Vec::new(),
        }
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |_s| false);
    }

    fn emit_fields(&self, _out: &mut Out) {}
}

typed_record!(Static, STAT);

/// A door (`DOOR`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Door {
    /// The common fields.
    pub common: Common,
    /// The opening sound (`SNAM`).
    pub open_sound: Option<FormId>,
    /// The closing sound (`ANAM`).
    pub close_sound: Option<FormId>,
    /// The looping sound (`BNAM`).
    pub loop_sound: Option<FormId>,
    /// Door flags (`FNAM`), as stored.
    pub flags: Option<u8>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl Door {
    /// A door with an editor id, a display name and a model.
    pub fn new(form_id: impl Into<FormId>, editor_id: &str, name: &str, model: &str) -> Self {
        Self {
            common: Common::new(form_id)
                .with_editor_id(editor_id)
                .with_name(name)
                .with_model(model),
            flags: Some(0),
            ..Self::default()
        }
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"SNAM" => set(&mut self.open_sound, one_form(d)),
                b"ANAM" => set(&mut self.close_sound, one_form(d)),
                b"BNAM" => set(&mut self.loop_sound, one_form(d)),
                b"FNAM" => set(&mut self.flags, one_u8(d)),
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.form(b"SNAM", self.open_sound);
        out.form(b"ANAM", self.close_sound);
        out.form(b"BNAM", self.loop_sound);
        out.u8(b"FNAM", self.flags);
    }
}

typed_record!(Door, DOOR);

/// An item and a count (`CNTO`): one line of a container's or an actor's
/// inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ContainerItem {
    /// The item.
    pub item: FormId,
    /// How many.
    pub count: i32,
}

impl ContainerItem {
    /// An inventory line.
    pub fn new(item: impl Into<FormId>, count: i32) -> Self {
        Self {
            item: item.into(),
            count,
        }
    }

    pub(crate) fn parse(data: &[u8]) -> Option<Self> {
        let mut rd = Rd::exact(data, 8)?;
        Some(Self {
            item: rd.form()?,
            count: rd.i32()?,
        })
    }

    pub(crate) fn bytes(&self) -> Vec<u8> {
        Wr::default().form(self.item).i32(self.count).done()
    }
}

/// A container's `DATA` (5 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ContainerData {
    /// Flags, as stored.
    pub flags: u8,
    /// Weight.
    pub weight: f32,
}

/// A container (`CONT`).
///
/// A `COED` (extra data of the `CNTO` before it) is not decoded and stays
/// in `unknown`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Container {
    /// The common fields.
    pub common: Common,
    /// The contents (`CNTO`), in file order.
    pub items: Vec<ContainerItem>,
    /// Flags and weight (`DATA`).
    pub data: Option<ContainerData>,
    /// The opening sound (`SNAM`).
    pub open_sound: Option<FormId>,
    /// The closing sound (`QNAM`).
    pub close_sound: Option<FormId>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl Container {
    /// A container with an editor id, a display name, a model and contents.
    pub fn new(
        form_id: impl Into<FormId>,
        editor_id: &str,
        name: &str,
        model: &str,
        items: Vec<ContainerItem>,
    ) -> Self {
        Self {
            common: Common::new(form_id)
                .with_editor_id(editor_id)
                .with_name(name)
                .with_model(model),
            items,
            data: Some(ContainerData::default()),
            ..Self::default()
        }
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"CNTO" => push(&mut self.items, ContainerItem::parse(d)),
                b"DATA" => set(
                    &mut self.data,
                    Rd::exact(d, 5).and_then(|mut rd| {
                        Some(ContainerData {
                            flags: rd.u8()?,
                            weight: rd.f32()?,
                        })
                    }),
                ),
                b"SNAM" => set(&mut self.open_sound, one_form(d)),
                b"QNAM" => set(&mut self.close_sound, one_form(d)),
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        for item in &self.items {
            out.raw(b"CNTO", item.bytes());
        }
        out.opt(
            b"DATA",
            self.data
                .map(|d| Wr::default().u8(d.flags).f32(d.weight).done()),
        );
        out.form(b"SNAM", self.open_sound);
        out.form(b"QNAM", self.close_sound);
    }
}

typed_record!(Container, CONT);

/// Value and weight: the `DATA` of a miscellaneous item (8 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ItemData {
    /// Value in caps.
    pub value: i32,
    /// Weight.
    pub weight: f32,
}

/// A miscellaneous item (`MISC`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MiscItem {
    /// The common fields.
    pub common: Common,
    /// The inventory icon path (`ICON`).
    pub icon: Option<String>,
    /// The small icon path (`MICO`).
    pub small_icon: Option<String>,
    /// Value and weight (`DATA`).
    pub data: Option<ItemData>,
    /// The pick-up sound (`YNAM`).
    pub pickup_sound: Option<FormId>,
    /// The drop sound (`ZNAM`).
    pub drop_sound: Option<FormId>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl MiscItem {
    /// An item with an editor id, a display name, a model, a value and a
    /// weight.
    pub fn new(
        form_id: impl Into<FormId>,
        editor_id: &str,
        name: &str,
        model: &str,
        value: i32,
        weight: f32,
    ) -> Self {
        Self {
            common: Common::new(form_id)
                .with_editor_id(editor_id)
                .with_name(name)
                .with_model(model),
            data: Some(ItemData { value, weight }),
            ..Self::default()
        }
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"ICON" => set(&mut self.icon, zstring(d)),
                b"MICO" => set(&mut self.small_icon, zstring(d)),
                b"DATA" => set(
                    &mut self.data,
                    Rd::exact(d, 8).and_then(|mut rd| {
                        Some(ItemData {
                            value: rd.i32()?,
                            weight: rd.f32()?,
                        })
                    }),
                ),
                b"YNAM" => set(&mut self.pickup_sound, one_form(d)),
                b"ZNAM" => set(&mut self.drop_sound, one_form(d)),
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.zstr(b"ICON", &self.icon);
        out.zstr(b"MICO", &self.small_icon);
        out.form(b"YNAM", self.pickup_sound);
        out.form(b"ZNAM", self.drop_sound);
        out.opt(
            b"DATA",
            self.data
                .map(|d| Wr::default().i32(d.value).f32(d.weight).done()),
        );
    }
}

typed_record!(MiscItem, MISC);

/// A weapon's `DATA` (15 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct WeaponData {
    /// Value in caps.
    pub value: i32,
    /// Health (condition).
    pub health: i32,
    /// Weight.
    pub weight: f32,
    /// Base damage.
    pub damage: i16,
    /// Clip size.
    pub clip_size: u8,
}

/// A weapon's critical-hit data (`CRDT`, 16 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct WeaponCritical {
    /// Critical damage.
    pub damage: u16,
    /// Critical chance multiplier.
    pub chance_multiplier: f32,
    /// Flags, as stored.
    pub flags: u8,
    /// The effect applied on a critical hit; null when there is none.
    pub effect: FormId,
}

/// A weapon (`WEAP`).
///
/// The large `DNAM` block (animation, range, spread, skill and the rest) is
/// not decoded and stays in `unknown`, as do `VNAM` and the mod-slot
/// subrecords.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Weapon {
    /// The common fields.
    pub common: Common,
    /// The inventory icon path (`ICON`).
    pub icon: Option<String>,
    /// The small icon path (`MICO`).
    pub small_icon: Option<String>,
    /// The object effect (`EITM`).
    pub object_effect: Option<FormId>,
    /// The ammunition, or ammunition list (`NAM0`).
    pub ammo: Option<FormId>,
    /// The repair list (`REPL`).
    pub repair_list: Option<FormId>,
    /// The equipment type (`ETYP`), as stored.
    pub equipment_type: Option<i32>,
    /// Value, health, weight, damage and clip size (`DATA`).
    pub data: Option<WeaponData>,
    /// Critical-hit data (`CRDT`).
    pub critical: Option<WeaponCritical>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl Weapon {
    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"ICON" => set(&mut self.icon, zstring(d)),
                b"MICO" => set(&mut self.small_icon, zstring(d)),
                b"EITM" => set(&mut self.object_effect, one_form(d)),
                b"NAM0" => set(&mut self.ammo, one_form(d)),
                b"REPL" => set(&mut self.repair_list, one_form(d)),
                b"ETYP" => set(&mut self.equipment_type, one_i32(d)),
                b"DATA" => set(
                    &mut self.data,
                    Rd::exact(d, 15).and_then(|mut rd| {
                        Some(WeaponData {
                            value: rd.i32()?,
                            health: rd.i32()?,
                            weight: rd.f32()?,
                            damage: rd.i16()?,
                            clip_size: rd.u8()?,
                        })
                    }),
                ),
                b"CRDT" => set(&mut self.critical, parse_crdt(d)),
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.zstr(b"ICON", &self.icon);
        out.zstr(b"MICO", &self.small_icon);
        out.form(b"EITM", self.object_effect);
        out.form(b"NAM0", self.ammo);
        out.form(b"REPL", self.repair_list);
        out.i32(b"ETYP", self.equipment_type);
        out.opt(
            b"DATA",
            self.data.map(|d| {
                Wr::default()
                    .i32(d.value)
                    .i32(d.health)
                    .f32(d.weight)
                    .i16(d.damage)
                    .u8(d.clip_size)
                    .done()
            }),
        );
        out.opt(
            b"CRDT",
            self.critical.map(|c| {
                Wr::default()
                    .u16(c.damage)
                    .zeros(2)
                    .f32(c.chance_multiplier)
                    .u8(c.flags)
                    .zeros(3)
                    .form(c.effect)
                    .done()
            }),
        );
    }
}

typed_record!(Weapon, WEAP);

impl Weapon {
    /// The fields of the weapon's `DNAM` that [`WeaponCombat`] reads, from
    /// the first `DNAM` in `unknown`; `None` when there is none or it is
    /// too short.
    pub fn combat(&self) -> Option<WeaponCombat> {
        self.unknown
            .iter()
            .find(|s| s.sig == crate::sig::Sig::new(b"DNAM"))
            .and_then(|s| WeaponCombat::parse(&s.data))
    }
}

/// A few fields of a weapon's large `DNAM`, read in place.
///
/// The `DNAM` itself stays in [`Weapon::unknown`], whole, so nothing is
/// lost; this reads the fields a converter can use. The block is 204 bytes
/// in most New Vegas weapons and 136 in Fallout 3 (New Vegas appended
/// fields, and both masters hold some shorter ones: 120 to 200 bytes).
/// Every field here sits in the first 108 bytes, at the same offset in
/// both games; checked against both retail masters, where the skill is
/// always a skill actor value (32 to 45), the reach and spreads are small
/// non-negative numbers, and the ranges and AP costs are plausible.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct WeaponCombat {
    /// The animation type, as stored (0 is hand to hand; the codes above
    /// it name melee, pistol, rifle, launcher and thrown animations).
    pub animation_type: u32,
    /// Melee reach multiplier (offset 8).
    pub reach: f32,
    /// Minimum spread, degrees (offset 16).
    pub min_spread: f32,
    /// Spread, degrees (offset 20).
    pub spread: f32,
    /// Base chance to hit in VATS, percent (offset 40).
    pub vats_to_hit: u8,
    /// Projectiles per shot (offset 42).
    pub projectile_count: u8,
    /// Minimum range, game units (offset 44).
    pub min_range: f32,
    /// Maximum range, game units (offset 48).
    pub max_range: f32,
    /// Shots per second (offset 64).
    pub fire_rate: f32,
    /// The action point cost; 0 means "computed by the game" (offset 68).
    pub action_points: f32,
    /// Reload time, seconds (offset 92).
    pub reload_time: f32,
    /// The skill used, an actor value index: 32 barter, 33 big guns,
    /// 34 energy weapons, 35 explosives, 36 lockpick, 37 medicine, 38 melee
    /// weapons, 39 repair, 40 science, 41 guns (small guns), 42 sneak,
    /// 43 speech, 44 survival (throwing), 45 unarmed (offset 104).
    pub skill: u32,
}

impl WeaponCombat {
    /// Shortest `DNAM` that holds every field.
    pub const MIN_LEN: usize = 108;

    /// Reads the fields from a `DNAM` payload of at least
    /// [`Self::MIN_LEN`] bytes.
    pub fn parse(dnam: &[u8]) -> Option<Self> {
        if dnam.len() < Self::MIN_LEN {
            return None;
        }
        let u32_at = |at: usize| {
            dnam.get(at..at + 4)
                .and_then(|b| b.try_into().ok())
                .map(u32::from_le_bytes)
        };
        let f32_at = |at: usize| u32_at(at).map(f32::from_bits);
        Some(Self {
            animation_type: u32_at(0)?,
            reach: f32_at(8)?,
            min_spread: f32_at(16)?,
            spread: f32_at(20)?,
            vats_to_hit: *dnam.get(40)?,
            projectile_count: *dnam.get(42)?,
            min_range: f32_at(44)?,
            max_range: f32_at(48)?,
            fire_rate: f32_at(64)?,
            action_points: f32_at(68)?,
            reload_time: f32_at(92)?,
            skill: u32_at(104)?,
        })
    }
}

fn parse_crdt(data: &[u8]) -> Option<WeaponCritical> {
    let mut rd = Rd::exact(data, 16)?;
    let damage = rd.u16()?;
    rd.skip(2)?;
    let chance_multiplier = rd.f32()?;
    let flags = rd.u8()?;
    rd.skip(3)?;
    Some(WeaponCritical {
        damage,
        chance_multiplier,
        flags,
        effect: rd.form()?,
    })
}

/// Which body slots a piece of armour covers (`BMDT`, 8 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct BipedData {
    /// Biped slot flags, as stored. See the `SLOT_*` constants.
    pub biped_flags: u32,
    /// General flags, as stored.
    pub general_flags: u8,
}

impl BipedData {
    /// The head slot.
    pub const SLOT_HEAD: u32 = 0x0000_0001;
    /// The hair slot.
    pub const SLOT_HAIR: u32 = 0x0000_0002;
    /// The upper body slot.
    pub const SLOT_UPPER_BODY: u32 = 0x0000_0004;
    /// The left hand slot.
    pub const SLOT_LEFT_HAND: u32 = 0x0000_0008;
    /// The right hand slot.
    pub const SLOT_RIGHT_HAND: u32 = 0x0000_0010;

    fn parse(data: &[u8]) -> Option<Self> {
        let mut rd = Rd::exact(data, 8)?;
        Some(Self {
            biped_flags: rd.u32()?,
            general_flags: rd.u8()?,
        })
    }

    fn bytes(&self) -> Vec<u8> {
        Wr::default()
            .u32(self.biped_flags)
            .u8(self.general_flags)
            .zeros(3)
            .done()
    }
}

fn biped_model<'a>(
    common: &'a Common,
    female_model: &'a Option<String>,
    female: bool,
) -> Option<&'a str> {
    let nonempty = |m: &'a Option<String>| m.as_deref().filter(|m| !m.trim().is_empty());
    if female && let Some(model) = nonempty(female_model) {
        return Some(model);
    }
    nonempty(&common.model)
}

/// A piece of armour's `DATA` (12 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ArmorData {
    /// Value in caps.
    pub value: i32,
    /// Health (condition).
    pub health: i32,
    /// Weight.
    pub weight: f32,
}

/// A piece of armour's `DNAM`: 12 bytes in this game, 4 in the older form.
/// Checked against a retail master: both sizes occur, and every 12-byte
/// one holds a damage threshold between 0 and 100.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct ArmorStats {
    /// Armour rating, as stored (hundredths).
    pub armor_rating: i16,
    /// Damage threshold; absent from the 4-byte form.
    pub damage_threshold: Option<f32>,
    /// Flags, as stored.
    pub flags: u16,
}

/// A piece of armour (`ARMO`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Armor {
    /// The common fields. `model` is the male biped model.
    pub common: Common,
    /// Body slots (`BMDT`).
    pub biped: Option<BipedData>,
    /// The male world model path (`MOD2`).
    pub male_world_model: Option<String>,
    /// The female biped model path (`MOD3`).
    pub female_model: Option<String>,
    /// The female world model path (`MOD4`).
    pub female_world_model: Option<String>,
    /// The male inventory icon path (`ICON`).
    pub icon: Option<String>,
    /// The male small icon path (`MICO`).
    pub small_icon: Option<String>,
    /// The object effect (`EITM`).
    pub object_effect: Option<FormId>,
    /// The repair list (`REPL`).
    pub repair_list: Option<FormId>,
    /// The biped model list (`BIPL`): a form list of armour addons
    /// (`ARMA`) worn with this piece, such as gloves.
    pub biped_model_list: Option<FormId>,
    /// The equipment type (`ETYP`), as stored.
    pub equipment_type: Option<i32>,
    /// Value, health and weight (`DATA`).
    pub data: Option<ArmorData>,
    /// Armour rating and damage threshold (`DNAM`).
    pub stats: Option<ArmorStats>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl Armor {
    /// The biped model for one sex: the female model when asked for and
    /// present, otherwise the male one (`MODL`).
    pub fn biped_model(&self, female: bool) -> Option<&str> {
        biped_model(&self.common, &self.female_model, female)
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"BMDT" => set(&mut self.biped, BipedData::parse(d)),
                b"MOD2" => set(&mut self.male_world_model, zstring(d)),
                b"MOD3" => set(&mut self.female_model, zstring(d)),
                b"MOD4" => set(&mut self.female_world_model, zstring(d)),
                b"ICON" => set(&mut self.icon, zstring(d)),
                b"MICO" => set(&mut self.small_icon, zstring(d)),
                b"EITM" => set(&mut self.object_effect, one_form(d)),
                b"REPL" => set(&mut self.repair_list, one_form(d)),
                b"BIPL" => set(&mut self.biped_model_list, one_form(d)),
                b"ETYP" => set(&mut self.equipment_type, one_i32(d)),
                b"DATA" => set(
                    &mut self.data,
                    Rd::exact(d, 12).and_then(|mut rd| {
                        Some(ArmorData {
                            value: rd.i32()?,
                            health: rd.i32()?,
                            weight: rd.f32()?,
                        })
                    }),
                ),
                b"DNAM" => set(&mut self.stats, parse_armor_dnam(d)),
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.form(b"EITM", self.object_effect);
        out.opt(b"BMDT", self.biped.map(|b| b.bytes()));
        out.zstr(b"MOD2", &self.male_world_model);
        out.zstr(b"MOD3", &self.female_model);
        out.zstr(b"MOD4", &self.female_world_model);
        out.zstr(b"ICON", &self.icon);
        out.zstr(b"MICO", &self.small_icon);
        out.form(b"REPL", self.repair_list);
        out.form(b"BIPL", self.biped_model_list);
        out.i32(b"ETYP", self.equipment_type);
        out.opt(
            b"DATA",
            self.data.map(|d| {
                Wr::default()
                    .i32(d.value)
                    .i32(d.health)
                    .f32(d.weight)
                    .done()
            }),
        );
        out.opt(b"DNAM", self.stats.map(armor_dnam_bytes));
    }
}

typed_record!(Armor, ARMO);

/// An armour addon (`ARMA`): a further biped model worn with a piece of
/// armour that lists it in its biped model list. Same layout as `ARMO`
/// for the fields read here.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ArmorAddon {
    /// The common fields. `model` is the male biped model.
    pub common: Common,
    /// Body slots (`BMDT`).
    pub biped: Option<BipedData>,
    /// The male world model path (`MOD2`).
    pub male_world_model: Option<String>,
    /// The female biped model path (`MOD3`).
    pub female_model: Option<String>,
    /// The female world model path (`MOD4`).
    pub female_world_model: Option<String>,
    /// The male inventory icon path (`ICON`).
    pub icon: Option<String>,
    /// The female inventory icon path (`ICO2`).
    pub female_icon: Option<String>,
    /// The equipment type (`ETYP`), as stored.
    pub equipment_type: Option<i32>,
    /// Value, health and weight (`DATA`).
    pub data: Option<ArmorData>,
    /// Armour rating and damage threshold (`DNAM`).
    pub stats: Option<ArmorStats>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl ArmorAddon {
    /// An addon with an editor id, slots and a male biped model.
    pub fn new(form_id: impl Into<FormId>, editor_id: &str, slots: u32, model: &str) -> Self {
        Self {
            common: Common::new(form_id)
                .with_editor_id(editor_id)
                .with_model(model),
            biped: Some(BipedData {
                biped_flags: slots,
                general_flags: 0,
            }),
            ..Self::default()
        }
    }

    /// The biped model for one sex, as [`Armor::biped_model`].
    pub fn biped_model(&self, female: bool) -> Option<&str> {
        biped_model(&self.common, &self.female_model, female)
    }

    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"BMDT" => set(&mut self.biped, BipedData::parse(d)),
                b"MOD2" => set(&mut self.male_world_model, zstring(d)),
                b"MOD3" => set(&mut self.female_model, zstring(d)),
                b"MOD4" => set(&mut self.female_world_model, zstring(d)),
                b"ICON" => set(&mut self.icon, zstring(d)),
                b"ICO2" => set(&mut self.female_icon, zstring(d)),
                b"ETYP" => set(&mut self.equipment_type, one_i32(d)),
                b"DATA" => set(
                    &mut self.data,
                    Rd::exact(d, 12).and_then(|mut rd| {
                        Some(ArmorData {
                            value: rd.i32()?,
                            health: rd.i32()?,
                            weight: rd.f32()?,
                        })
                    }),
                ),
                b"DNAM" => set(&mut self.stats, parse_armor_dnam(d)),
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.opt(b"BMDT", self.biped.map(|b| b.bytes()));
        out.zstr(b"MOD2", &self.male_world_model);
        out.zstr(b"MOD3", &self.female_model);
        out.zstr(b"MOD4", &self.female_world_model);
        out.zstr(b"ICON", &self.icon);
        out.zstr(b"ICO2", &self.female_icon);
        out.i32(b"ETYP", self.equipment_type);
        out.opt(
            b"DATA",
            self.data.map(|d| {
                Wr::default()
                    .i32(d.value)
                    .i32(d.health)
                    .f32(d.weight)
                    .done()
            }),
        );
        out.opt(b"DNAM", self.stats.map(armor_dnam_bytes));
    }
}

typed_record!(ArmorAddon, ARMA);

fn armor_dnam_bytes(s: ArmorStats) -> Vec<u8> {
    let w = Wr::default().i16(s.armor_rating);
    match s.damage_threshold {
        Some(dt) => w.zeros(2).f32(dt).u16(s.flags).zeros(2),
        None => w.u16(s.flags),
    }
    .done()
}

fn parse_armor_dnam(data: &[u8]) -> Option<ArmorStats> {
    let mut rd = Rd::new(data);
    match data.len() {
        4 => Some(ArmorStats {
            armor_rating: rd.i16()?,
            damage_threshold: None,
            flags: rd.u16()?,
        }),
        12 => {
            let armor_rating = rd.i16()?;
            rd.skip(2)?;
            Some(ArmorStats {
                armor_rating,
                damage_threshold: Some(rd.f32()?),
                flags: rd.u16()?,
            })
        }
        _ => None,
    }
}

/// An ingestible's `ENIT` (20 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct IngestibleData {
    /// Value in caps.
    pub value: i32,
    /// Flags, as stored.
    pub flags: u8,
    /// The withdrawal effect; null when there is none.
    pub withdrawal_effect: FormId,
    /// Chance of addiction.
    pub addiction_chance: f32,
    /// The sound played on use; null when there is none.
    pub consume_sound: FormId,
}

/// An effect's `EFIT` (20 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct EffectData {
    /// Magnitude.
    pub magnitude: u32,
    /// Area.
    pub area: u32,
    /// Duration in seconds.
    pub duration: u32,
    /// Range type, as stored: 0 self, 1 touch, 2 target.
    pub range: u32,
    /// The actor value affected, as stored; −1 for none.
    pub actor_value: i32,
}

/// One effect of an ingestible: `EFID`, its `EFIT` and its conditions.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Effect {
    /// The base effect (`EFID`).
    pub base_effect: FormId,
    /// Magnitude, area, duration and the rest (`EFIT`).
    pub data: Option<EffectData>,
    /// The effect's conditions (`CTDA`).
    pub conditions: Vec<Condition>,
}

/// An ingestible (`ALCH`): food, drink, chems.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Ingestible {
    /// The common fields.
    pub common: Common,
    /// The inventory icon path (`ICON`).
    pub icon: Option<String>,
    /// The small icon path (`MICO`).
    pub small_icon: Option<String>,
    /// The equipment type (`ETYP`), as stored.
    pub equipment_type: Option<i32>,
    /// Weight (`DATA`).
    pub weight: Option<f32>,
    /// Value, flags and addiction data (`ENIT`).
    pub data: Option<IngestibleData>,
    /// Effects, in file order.
    pub effects: Vec<Effect>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl Ingestible {
    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"ICON" => set(&mut self.icon, zstring(d)),
                b"MICO" => set(&mut self.small_icon, zstring(d)),
                b"ETYP" => set(&mut self.equipment_type, one_i32(d)),
                b"DATA" => set(&mut self.weight, one_f32(d)),
                b"ENIT" => set(&mut self.data, parse_enit(d)),
                b"EFID" => push(
                    &mut self.effects,
                    one_form(d).map(|base_effect| Effect {
                        base_effect,
                        ..Effect::default()
                    }),
                ),
                b"EFIT" => match self.effects.last_mut() {
                    Some(effect) => set(&mut effect.data, parse_efit(d)),
                    None => false,
                },
                b"CTDA" => match self.effects.last_mut() {
                    Some(effect) => push(&mut effect.conditions, Condition::parse(d)),
                    None => false,
                },
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.zstr(b"ICON", &self.icon);
        out.zstr(b"MICO", &self.small_icon);
        out.i32(b"ETYP", self.equipment_type);
        out.f32(b"DATA", self.weight);
        out.opt(
            b"ENIT",
            self.data.map(|d| {
                Wr::default()
                    .i32(d.value)
                    .u8(d.flags)
                    .zeros(3)
                    .form(d.withdrawal_effect)
                    .f32(d.addiction_chance)
                    .form(d.consume_sound)
                    .done()
            }),
        );
        for effect in &self.effects {
            out.form(b"EFID", Some(effect.base_effect));
            out.opt(
                b"EFIT",
                effect.data.map(|d| {
                    Wr::default()
                        .u32(d.magnitude)
                        .u32(d.area)
                        .u32(d.duration)
                        .u32(d.range)
                        .i32(d.actor_value)
                        .done()
                }),
            );
            for condition in &effect.conditions {
                out.raw(b"CTDA", condition.bytes());
            }
        }
    }
}

typed_record!(Ingestible, ALCH);

fn parse_enit(data: &[u8]) -> Option<IngestibleData> {
    let mut rd = Rd::exact(data, 20)?;
    let value = rd.i32()?;
    let flags = rd.u8()?;
    rd.skip(3)?;
    Some(IngestibleData {
        value,
        flags,
        withdrawal_effect: rd.form()?,
        addiction_chance: rd.f32()?,
        consume_sound: rd.form()?,
    })
}

fn parse_efit(data: &[u8]) -> Option<EffectData> {
    let mut rd = Rd::exact(data, 20)?;
    Some(EffectData {
        magnitude: rd.u32()?,
        area: rd.u32()?,
        duration: rd.u32()?,
        range: rd.u32()?,
        actor_value: rd.i32()?,
    })
}

/// Ammunition's `DATA` (13 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct AmmoData {
    /// Projectile speed.
    pub speed: f32,
    /// Flags, as stored.
    pub flags: u8,
    /// Value in caps.
    pub value: i32,
    /// Rounds per clip.
    pub clip_rounds: u8,
}

/// Ammunition's `DAT2`, which this game added: 20 bytes, or 12 in a short
/// form that stops after the weight. Checked against a retail master: both
/// sizes occur, the projectile field names projectiles and the consumed
/// field names miscellaneous items.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct AmmoData2 {
    /// Projectiles per shot.
    pub projectiles_per_shot: u32,
    /// The projectile; null when the weapon decides.
    pub projectile: FormId,
    /// Weight of one round.
    pub weight: f32,
    /// What is left after firing (a casing); null when nothing is, and in
    /// the 12-byte form.
    pub consumed_ammo: FormId,
    /// Chance of leaving it, in percent (0 to 100); 0 in the 12-byte form.
    pub consumed_percentage: f32,
}

/// Ammunition (`AMMO`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Ammo {
    /// The common fields.
    pub common: Common,
    /// The inventory icon path (`ICON`).
    pub icon: Option<String>,
    /// The small icon path (`MICO`).
    pub small_icon: Option<String>,
    /// Speed, flags, value and clip rounds (`DATA`).
    pub data: Option<AmmoData>,
    /// Projectile, weight and casing (`DAT2`).
    pub data2: Option<AmmoData2>,
    /// The short name (`ONAM`).
    pub short_name: Option<String>,
    /// The abbreviation (`QNAM`).
    pub abbreviation: Option<String>,
    /// Subrecords the decoder did not consume.
    pub unknown: Vec<RawSubrecord>,
}

impl Ammo {
    fn decode_fields(&mut self, subs: &[SubrecordRef<'_>]) {
        decode_loop!(self, subs, |s| {
            let d = s.data;
            match &s.sig.0 {
                b"ICON" => set(&mut self.icon, zstring(d)),
                b"MICO" => set(&mut self.small_icon, zstring(d)),
                b"DATA" => set(&mut self.data, parse_ammo_data(d)),
                b"DAT2" => set(&mut self.data2, parse_ammo_dat2(d)),
                b"ONAM" => set(&mut self.short_name, zstring(d)),
                b"QNAM" => set(&mut self.abbreviation, zstring(d)),
                _ => false,
            }
        });
    }

    fn emit_fields(&self, out: &mut Out) {
        out.zstr(b"ICON", &self.icon);
        out.zstr(b"MICO", &self.small_icon);
        out.opt(
            b"DATA",
            self.data.map(|d| {
                Wr::default()
                    .f32(d.speed)
                    .u8(d.flags)
                    .zeros(3)
                    .i32(d.value)
                    .u8(d.clip_rounds)
                    .done()
            }),
        );
        out.opt(
            b"DAT2",
            self.data2.map(|d| {
                Wr::default()
                    .u32(d.projectiles_per_shot)
                    .form(d.projectile)
                    .f32(d.weight)
                    .form(d.consumed_ammo)
                    .f32(d.consumed_percentage)
                    .done()
            }),
        );
        out.zstr(b"ONAM", &self.short_name);
        out.zstr(b"QNAM", &self.abbreviation);
    }
}

typed_record!(Ammo, AMMO);

fn parse_ammo_dat2(data: &[u8]) -> Option<AmmoData2> {
    if !matches!(data.len(), 12 | 20) {
        return None;
    }
    let mut rd = Rd::new(data);
    Some(AmmoData2 {
        projectiles_per_shot: rd.u32()?,
        projectile: rd.form()?,
        weight: rd.f32()?,
        consumed_ammo: rd.form().unwrap_or(FormId::NULL),
        consumed_percentage: rd.f32().unwrap_or(0.0),
    })
}

fn parse_ammo_data(data: &[u8]) -> Option<AmmoData> {
    let mut rd = Rd::exact(data, 13)?;
    let speed = rd.f32()?;
    let flags = rd.u8()?;
    rd.skip(3)?;
    Some(AmmoData {
        speed,
        flags,
        value: rd.i32()?,
        clip_rounds: rd.u8()?,
    })
}
