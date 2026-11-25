//! Items, loot, armed and sided people: the `map_builder::Catalogue` of one
//! map, built from the records (`docs/contracts.md` 9.6).
//!
//! **Items.** Every distinct base of: a reference the slicer placed as an
//! item; an item inside a container the slicer placed (its base's
//! inventory, leveled lists drawn); an item a placed person carries (the
//! inventory of the actor that supplies it through its templates). Each
//! becomes an `emit::rebalance` source struct filled from the records'
//! tables (`weapons`, `armors`, `ammo`, `ingestibles` and their effects,
//! `misc_items`), then a Fallout 2 spec through the run's rebalance
//! tables, then an [`ItemDef`] with the record's name and a short
//! description generated from the converted figures (never the game's own
//! text). A weapon's default ammunition is its ammunition record, or the
//! first ammunition of its form list. Weapons, armour, ammunition, aid,
//! keys and everything else (as miscellaneous items: books, notes, caps,
//! junk) are kept.
//!
//! **Weapon classes** ([`crate::rebalance::WeaponTable`]): the record's
//! animation type gives the class (0 unarmed, 1 blunt, 2 heavy melee, 3
//! pistol, 4 energy pistol, 5 rifle, 6 assault rifle, 7 energy rifle, 8
//! minigun, 9 launcher, 10 to 12 grenade, 13 thrown); a two-hand handle
//! with the energy weapons skill is an energy heavy weapon; then the
//! first matching rule replaces the class (knives, swords and axes are
//! blades; spears; shotguns; SMGs; flamers) and the first matching rule
//! with a damage kind sets it (plasma, fire, electric, pulse; launchers,
//! grenades and mines explosive). The `DNAM` automatic flag (bit 1 of its
//! first flag byte) gives a single-shot class a burst mode.
//!
//! **Leveled item lists.** In a container, drawn per container: the draw is
//! seeded by the container reference's form key and the list's, so one
//! container always holds the same things and two containers of one base
//! may differ. A list's chance of none is rolled; a "use all" list gives
//! every entry; otherwise one entry is picked among those at or below
//! [`crate::rebalance::LootRules::level`] (the lowest-level entries when
//! none is), once, or once per unit of the count when the list says
//! "each". Counts multiply down nested lists. A person's inventory is
//! resolved by the actor documents' rule instead (the first entry at the
//! lowest level; every entry of a "use all" list), so that the weapon and
//! armour people carry are the ones they are drawn with.
//!
//! **Placed references.** A placed item lies on the map in its stack's
//! count (ammunition in packs, the front pack partly full); a container
//! gets its name, its contents (at most `max_contents` stacks), a capacity
//! that holds them, and its lock; a door its name, lock and key (when the
//! lock names a key record the catalogue holds); any other upright
//! reference whose base has a real name (activators, terminals,
//! furniture) gets that name. Fallout 2 has no lock levels: a lock is
//! locked, never jammed.
//!
//! **People.** A placed person's fighter figures come from its actor
//! document and the records of the actors its templates name (level,
//! health, attributes, skills), with the worn armour (the first body
//! armour of its inventory) folded in; the right hand holds the weapon the
//! critter stage drew it with (`ActorDoc::weapons[0]` when carried), the
//! rest of its inventory is carried (at most `max_carried` stacks). Its
//! side ([`crate::rebalance::Hostility`]) comes from its AI data and
//! factions, followed through templates by the template flags (factions
//! 0x04, AI data 0x10); hostile people share the hostile team and AI
//! packet, the rest the friendly ones.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use emit::objects::{ItemState, LockState};
use emit::rebalance::{
    AidStat, SourceActor, SourceAid, SourceAmmo, SourceArmor, SourceDamage, SourceEffect,
    SourceSkill, SourceWeapon, WeaponClass,
};
use emit::{AttackMode, KeySpec, MiscSpec, ProtoText, WeaponCode};
use map_builder::{
    ArtCategory, ArtImage, Catalogue, CritterLoot, GroundArt, Held, ItemDef, ItemIcon, ItemKind,
    PlacedLoot,
};
use mir::{FormKey, Mir, Records};
use quantize::PaletteMap;
use sha2::{Digest, Sha256};

use crate::config::{ConvertError, Inputs};
use crate::rebalance::{LoadedRebalance, glob};

/// Template flags (`ACBS`): the template supplies the factions, the AI
/// data.
const USE_FACTIONS: u16 = 0x0004;
const USE_AI_DATA: u16 = 0x0010;
const USE_STATS: u16 = 0x0002;
/// The armour slot of the body (`BMDT` biped flags).
const BODY_SLOT: i64 = 0x0004;
/// The longest template or list chain followed.
const MAX_CHAIN: usize = 32;
/// The most units of one stack.
const MAX_STACK: u32 = 100_000;
/// The most hit points one aid item's effects restore in total.
const MAX_HEAL: i32 = 500;
/// Item art: the inventory icon is rendered this many times larger than
/// the ground sprite.
const ICON_SCALE: f32 = 3.0;

fn stage(e: impl std::fmt::Display) -> ConvertError {
    ConvertError::stage("map", format!("catalogue: {e}"))
}

/// The source damage kind a rebalance rule names.
pub fn damage_kind(name: &str) -> Option<SourceDamage> {
    use SourceDamage as D;
    Some(match name {
        "ballistic" => D::Ballistic,
        "laser" => D::Laser,
        "plasma" => D::Plasma,
        "fire" => D::Fire,
        "electric" => D::Electric,
        "pulse" => D::Pulse,
        "explosive" => D::Explosive,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Records

/// One record's raw subrecords, its signature, plugin and names.
#[derive(Debug)]
struct Raw {
    sig: String,
    plugin: String,
    editor_id: Option<String>,
    name: Option<String>,
    subrecords: Vec<(String, Vec<u8>)>,
}

impl Raw {
    fn first(&self, sig: &str) -> Option<&[u8]> {
        self.subrecords
            .iter()
            .find(|(s, _)| s == sig)
            .map(|(_, d)| d.as_slice())
    }

    fn all<'a>(&'a self, sig: &'a str) -> impl Iterator<Item = &'a [u8]> + 'a {
        self.subrecords
            .iter()
            .filter(move |(s, _)| s == sig)
            .map(|(_, d)| d.as_slice())
    }
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn i32_at(data: &[u8], at: usize) -> Option<i32> {
    u32_at(data, at).map(|v| v as i32)
}

fn f32_at(data: &[u8], at: usize) -> Option<f32> {
    u32_at(data, at).map(f32::from_bits)
}

/// Typed and raw access to `records.sqlite`, with the raw store cached.
pub(crate) struct Source<'a> {
    records: &'a Records,
    /// The plugin list, lowercase, in load order.
    plugins: Vec<String>,
    raw: RefCell<BTreeMap<FormKey, Option<Rc<Raw>>>>,
}

impl<'a> Source<'a> {
    pub(crate) fn new(records: &'a Records, plugins: &[String]) -> Self {
        Source {
            records,
            plugins: plugins.iter().map(|p| p.to_lowercase()).collect(),
            raw: RefCell::new(BTreeMap::new()),
        }
    }

    fn raw(&self, key: &FormKey) -> Result<Option<Rc<Raw>>, ConvertError> {
        if let Some(found) = self.raw.borrow().get(key) {
            return Ok(found.clone());
        }
        let found = match self.records.record(key).map_err(stage)? {
            Some(row) => {
                let subrecords = self.records.raw_subrecords(key).map_err(stage)?;
                Some(Rc::new(Raw {
                    sig: row.sig,
                    plugin: row.plugin,
                    editor_id: row.editor_id,
                    name: row.name.filter(|n| !n.trim().is_empty()),
                    subrecords,
                }))
            }
            None => None,
        };
        self.raw.borrow_mut().insert(key.clone(), found.clone());
        Ok(found)
    }

    /// A form id as stored in a record of `plugin`: the masters of a
    /// plugin are taken to be the plugins before it in the load order (the
    /// usual case, and exact for a single master).
    fn resolve(&self, plugin: &str, raw: u32) -> FormKey {
        let index = (raw >> 24) as usize;
        let own = self
            .plugins
            .iter()
            .position(|p| p == plugin)
            .unwrap_or(self.plugins.len().saturating_sub(1));
        let owner = if index < own {
            self.plugins[index].clone()
        } else {
            self.plugins
                .get(own)
                .cloned()
                .unwrap_or_else(|| plugin.to_owned())
        };
        FormKey::new(&owner, raw & 0x00FF_FFFF)
    }

    /// The record decoded by `fnv_esm`'s typed decoder.
    fn decode(&self, key: &FormKey) -> Result<Option<(Rc<Raw>, fnv_esm::Record)>, ConvertError> {
        let Some(raw) = self.raw(key)? else {
            return Ok(None);
        };
        let Ok(sig) = <[u8; 4]>::try_from(raw.sig.as_bytes()) else {
            return Ok(None);
        };
        let refs: Vec<fnv_esm::SubrecordRef<'_>> = raw
            .subrecords
            .iter()
            .filter_map(|(s, data)| {
                let sig = <[u8; 4]>::try_from(s.as_bytes()).ok()?;
                Some(fnv_esm::SubrecordRef {
                    sig: fnv_esm::Sig::new(&sig),
                    data,
                    offset: 0,
                })
            })
            .collect();
        let record = fnv_esm::decode_record(
            fnv_esm::Sig::new(&sig),
            fnv_esm::RecordMeta::default(),
            &refs,
        );
        Ok(Some((raw, record)))
    }

    /// The first entry at the lowest level of a leveled list (the actor
    /// documents' rule), or every entry of a "use all" item list.
    fn figure_entries(&self, raw: &Raw, list: &fnv_esm::LeveledList) -> Vec<(FormKey, i64)> {
        let resolve = |e: &fnv_esm::LeveledEntry| {
            (
                self.resolve(&raw.plugin, e.form.0),
                i64::from(e.count.max(1)),
            )
        };
        if list.kind == fnv_esm::LeveledKind::Item && list.uses_all() {
            return list.entries.iter().map(resolve).collect();
        }
        let Some(low) = list.entries.iter().map(|e| e.level).min() else {
            return Vec::new();
        };
        list.entries
            .iter()
            .find(|e| e.level == low)
            .map(resolve)
            .into_iter()
            .collect()
    }

    /// The NPC that supplies the data of template flag `flag` for `start`:
    /// followed through templates whose flag is set and leveled lists
    /// (their first lowest-level entry).
    fn supplier(
        &self,
        start: &FormKey,
        flag: u16,
    ) -> Result<Option<(FormKey, Rc<Raw>, fnv_esm::Npc)>, ConvertError> {
        let mut key = start.clone();
        let mut seen = BTreeSet::new();
        for _ in 0..MAX_CHAIN {
            if !seen.insert(key.clone()) {
                return Ok(None);
            }
            let Some((raw, record)) = self.decode(&key)? else {
                return Ok(None);
            };
            match record {
                fnv_esm::Record::Npc(npc) => {
                    let flags = npc.base.map_or(0, |b| b.template_flags);
                    match npc.template {
                        Some(template) if flags & flag != 0 && template.0 != 0 => {
                            key = self.resolve(&raw.plugin, template.0);
                        }
                        _ => return Ok(Some((key, raw, npc))),
                    }
                }
                fnv_esm::Record::Leveled(list) => {
                    match self.figure_entries(&raw, &list).into_iter().next() {
                        Some((next, _)) => key = next,
                        None => return Ok(None),
                    }
                }
                _ => return Ok(None),
            }
        }
        Ok(None)
    }

    /// The first form of a form list.
    fn first_of_list(&self, key: &FormKey) -> Result<Option<FormKey>, ConvertError> {
        match self.decode(key)? {
            Some((raw, fnv_esm::Record::FormList(list))) => {
                Ok(list.forms.first().map(|f| self.resolve(&raw.plugin, f.0)))
            }
            _ => Ok(None),
        }
    }

    /// A faction's relations (`XNAM`): the other faction, the modifier and
    /// the group combat reaction (0 neutral, 1 enemy, 2 ally, 3 friend; an
    /// 8-byte relation without one is enemy below 0 and friend above).
    fn relations(&self, faction: &FormKey) -> Result<Vec<(FormKey, i32, u32)>, ConvertError> {
        let Some(raw) = self.raw(faction)? else {
            return Ok(Vec::new());
        };
        Ok(raw
            .all("XNAM")
            .filter_map(|d| {
                let other = self.resolve(&raw.plugin, u32_at(d, 0)?);
                let modifier = i32_at(d, 4)?;
                let reaction = u32_at(d, 8).unwrap_or(match modifier {
                    m if m < 0 => 1,
                    m if m > 0 => 3,
                    _ => 0,
                });
                Some((other, modifier, reaction))
            })
            .collect())
    }

    /// One row of `table` for `key`, every column read as text (`NULL`
    /// as `None`).
    fn row(
        &self,
        table: &str,
        columns: &[&str],
        key: &FormKey,
    ) -> Result<Option<Row>, ConvertError> {
        let select: Vec<String> = columns
            .iter()
            .map(|c| format!("CAST({c} AS TEXT)"))
            .collect();
        let sql = format!("SELECT {} FROM {table} WHERE form = ?1", select.join(", "));
        let conn = self.records.connection();
        let mut statement = conn.prepare(&sql).map_err(stage)?;
        let mut rows = statement
            .query_map([key.to_string()], |r| {
                let mut out = Vec::with_capacity(columns.len());
                for i in 0..columns.len() {
                    out.push(r.get::<_, Option<String>>(i)?);
                }
                Ok(out)
            })
            .map_err(stage)?;
        match rows.next() {
            Some(row) => Ok(Some(Row(row.map_err(stage)?))),
            None => Ok(None),
        }
    }
}

/// A table row read as text.
struct Row(Vec<Option<String>>);

impl Row {
    fn text(&self, i: usize) -> Option<String> {
        self.0
            .get(i)
            .cloned()
            .flatten()
            .filter(|t| !t.trim().is_empty())
    }
    fn num(&self, i: usize) -> f64 {
        self.text(i)
            .and_then(|t| t.parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .unwrap_or(0.0)
    }
}

// ---------------------------------------------------------------------------
// Items

/// What the catalogue knows of one item besides its definition.
#[derive(Debug, Clone)]
struct ItemInfo {
    /// Rounds per pack, for ammunition; 1 otherwise.
    pack: u32,
    /// Size units of one (a container's capacity is checked against them).
    size: i32,
    /// Key figures, for the report and the gallery.
    summary: String,
    model: Option<String>,
    /// Body armour (the armour slot of the body).
    body: bool,
    /// A converted armour's figures, for a wearer's protection.
    armor: Option<emit::ArmorSpec>,
}

/// An item definition with its facts.
#[derive(Debug, Clone)]
struct Made {
    def: ItemDef,
    info: ItemInfo,
    /// A weapon's default ammunition record.
    ammo: Option<FormKey>,
}

/// A name of a record: its display name, else its editor id with spaces
/// before inner capitals, else its form key.
fn display_name(name: Option<&str>, editor_id: Option<&str>, key: &FormKey) -> String {
    if let Some(name) = name.filter(|n| !n.trim().is_empty()) {
        return name.trim().to_owned();
    }
    let Some(id) = editor_id.filter(|n| !n.is_empty()) else {
        return key.to_string();
    };
    let mut out = String::new();
    let mut previous_lower = false;
    for c in id.chars() {
        if c.is_ascii_uppercase() && previous_lower {
            out.push(' ');
        }
        previous_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        out.push(c);
    }
    out
}

fn round(value: f64) -> i32 {
    if value.is_finite() {
        value.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32
    } else {
        0
    }
}

/// The calibre key of an ammunition record for the rebalance table: its
/// name (then editor id), letters and digits only, with a leading `ammo`
/// and trailing `round`, `rounds`, `cartridge`, `ammo` dropped until the
/// table knows it; the plain name when it never does.
fn caliber_key(
    table: &emit::rebalance::Rebalance,
    name: Option<&str>,
    editor_id: Option<&str>,
) -> String {
    const SUFFIXES: [&str; 6] = ["rounds", "round", "cartridges", "cartridge", "ammo", "box"];
    for text in [name, editor_id].into_iter().flatten() {
        let mut key = emit::rebalance::normalize_key(text);
        if let Some(rest) = key.strip_prefix("ammo") {
            key = rest.to_owned();
        }
        loop {
            if table.caliber(&key).is_some() {
                return key;
            }
            match SUFFIXES.iter().find_map(|s| key.strip_suffix(s)) {
                Some(rest) if !rest.is_empty() => key = rest.to_owned(),
                _ => break,
            }
        }
    }
    emit::rebalance::normalize_key(name.or(editor_id).unwrap_or("unknown"))
}

/// A weapon record's class and damage kind (module notes).
pub(crate) fn classify_weapon(
    rebalance: &LoadedRebalance,
    names: &[&str],
    animation: u32,
    skill: u32,
) -> (WeaponClass, SourceDamage) {
    let table = &rebalance.convert.weapons;
    let mut class = table
        .by_animation
        .get(&animation.to_string())
        .copied()
        .unwrap_or(rebalance.table.default_class);
    if skill == table.energy_skill {
        class = match class {
            WeaponClass::Pistol => WeaponClass::EnergyPistol,
            WeaponClass::Rifle | WeaponClass::AssaultRifle => WeaponClass::EnergyRifle,
            WeaponClass::Minigun => WeaponClass::EnergyHeavy,
            other => other,
        };
    }
    let mut class_set = false;
    let mut damage = None;
    for rule in &table.rules {
        let named = rule.patterns.is_empty()
            || rule
                .patterns
                .iter()
                .any(|p| names.iter().any(|n| glob(p, n)));
        let animated = rule.animations.is_empty() || rule.animations.contains(&animation);
        let skilled = rule.skills.is_empty() || rule.skills.contains(&skill);
        if !(named && animated && skilled) {
            continue;
        }
        if let (false, Some(c)) = (class_set, rule.class) {
            class = c;
            class_set = true;
        }
        if let (None, Some(d)) = (damage, rule.damage.as_deref()) {
            damage = damage_kind(d);
        }
    }
    (class, damage.unwrap_or_default())
}

const WEAPON_COLUMNS: [&str; 15] = [
    "name",
    "editor_id",
    "model",
    "value",
    "weight",
    "damage",
    "clip_size",
    "ammo",
    "animation_type",
    "skill",
    "action_points",
    "min_spread",
    "fire_rate",
    "max_range",
    "projectiles",
];

impl Source<'_> {
    /// The ammunition record a weapon names (its first one, through a form
    /// list).
    fn weapon_ammo(&self, row: &Row) -> Result<Option<FormKey>, ConvertError> {
        let Some(key) = row.text(7).and_then(|t| t.parse::<FormKey>().ok()) else {
            return Ok(None);
        };
        match self.raw(&key)?.map(|r| r.sig.clone()).as_deref() {
            Some("AMMO") => Ok(Some(key)),
            Some("FLST") => Ok(self
                .first_of_list(&key)?
                .filter(|k| self.raw(k).ok().flatten().is_some_and(|r| r.sig == "AMMO"))),
            _ => Ok(None),
        }
    }

    /// A weapon record as the rebalance's source and its class.
    fn source_weapon(
        &self,
        key: &FormKey,
        rebalance: &LoadedRebalance,
    ) -> Result<Option<(SourceWeapon, Row, Option<FormKey>)>, ConvertError> {
        let Some(row) = self.row("weapons", &WEAPON_COLUMNS, key)? else {
            return Ok(None);
        };
        let raw = self.raw(key)?;
        let name = display_name(row.text(0).as_deref(), row.text(1).as_deref(), key);
        let animation = row.num(8).max(0.0) as u32;
        let skill = row.num(9).max(0.0) as u32;
        let editor_id = row.text(1).unwrap_or_default();
        let (class, damage_kind) = classify_weapon(
            rebalance,
            &[editor_id.as_str(), name.as_str()],
            animation,
            skill,
        );
        // Bit 1 of the DNAM's first flag byte: automatic.
        let automatic = raw
            .as_ref()
            .and_then(|r| r.first("DNAM"))
            .and_then(|d| d.get(12))
            .is_some_and(|flags| flags & 0x02 != 0)
            || animation == 6;
        let ammo = self.weapon_ammo(&row)?;
        let caliber = match &ammo {
            Some(ammo) => self
                .row("ammo", &["name", "editor_id"], ammo)?
                .map(|r| caliber_key(&rebalance.table, r.text(0).as_deref(), r.text(1).as_deref())),
            None => None,
        };
        let source = SourceWeapon {
            name,
            class: Some(class),
            damage: row.num(5) as f32,
            projectiles: row.num(14).max(0.0) as u32,
            damage_kind,
            clip: row.num(6).max(0.0) as u32,
            value: round(row.num(3)),
            weight: row.num(4) as f32,
            ap: row.num(10) as f32,
            fire_rate: row.num(12) as f32,
            automatic,
            min_spread: row.num(11) as f32,
            range: row.num(13) as f32,
            strength: 0,
            caliber,
            two_handed: matches!(animation, 2 | 5 | 6 | 7 | 8 | 9),
        };
        Ok(Some((source, row, ammo)))
    }

    /// One item definition from the records, or `None` for a form that is
    /// no item (or not in the plugin list).
    fn make_item(
        &self,
        key: &FormKey,
        rebalance: &LoadedRebalance,
    ) -> Result<Option<Made>, ConvertError> {
        let Some(raw) = self.raw(key)? else {
            return Ok(None);
        };
        let r = &rebalance.table;
        let base_name =
            |row: &Row| display_name(row.text(0).as_deref(), row.text(1).as_deref(), key);
        let made = match raw.sig.as_str() {
            "WEAP" => {
                let Some((source, row, ammo)) = self.source_weapon(key, rebalance)? else {
                    return Ok(None);
                };
                let spec = emit::rebalance::weapon_spec(&source, r);
                let name = source.name.clone();
                let mut summary = format!(
                    "{:?} {:?}, {}-{} dmg, {} AP, range {}",
                    source.class.unwrap_or(r.default_class),
                    spec.code,
                    spec.damage[0],
                    spec.damage[1],
                    spec.ap_cost[0],
                    spec.ranges[0]
                );
                let mut description = format!(
                    "{name}: {} to {} damage, {} action points an attack, range {}",
                    spec.damage[0], spec.damage[1], spec.ap_cost[0], spec.ranges[0]
                );
                if spec.capacity > 0 {
                    summary.push_str(&format!(", {} rounds", spec.capacity));
                    description.push_str(&format!(", {} rounds a load", spec.capacity));
                }
                description.push('.');
                let size = spec.stats.size;
                let mut def =
                    ItemDef::new(ItemKind::Weapon(spec), ProtoText::new(&name, &description));
                def.material = emit::proto::Material::Metal;
                Made {
                    def,
                    info: ItemInfo {
                        pack: 1,
                        size,
                        summary,
                        model: row.text(2),
                        body: false,
                        armor: None,
                    },
                    ammo,
                }
            }
            "ARMO" => {
                let Some(row) = self.row(
                    "armors",
                    &[
                        "name",
                        "editor_id",
                        "model",
                        "value",
                        "weight",
                        "armor_rating",
                        "damage_threshold",
                        "biped_flags",
                    ],
                    key,
                )?
                else {
                    return Ok(None);
                };
                let name = base_name(&row);
                let body = (row.num(7) as i64) & BODY_SLOT != 0;
                let source = SourceArmor {
                    name: name.clone(),
                    dr: (row.num(5) / 100.0) as f32,
                    dt: row.num(6) as f32,
                    value: round(row.num(3)),
                    weight: row.num(4) as f32,
                    power_armor: body
                        && [row.text(1), row.text(0)]
                            .iter()
                            .flatten()
                            .any(|n| glob("*power*armor*", n)),
                };
                let spec = emit::rebalance::armor_spec(&source, r);
                let normal = emit::DamageType::Normal.index();
                let summary = format!(
                    "AC {}, DR {}%, DT {}",
                    spec.armor_class, spec.resistances[normal], spec.thresholds[normal]
                );
                let description = format!(
                    "{name}: armor class {}, damage resistance {} percent.",
                    spec.armor_class, spec.resistances[normal]
                );
                let size = spec.stats.size;
                let mut def = ItemDef::new(
                    ItemKind::Armor(spec.clone()),
                    ProtoText::new(&name, &description),
                );
                def.material = emit::proto::Material::Leather;
                Made {
                    def,
                    info: ItemInfo {
                        pack: 1,
                        size,
                        summary,
                        model: row.text(2),
                        body,
                        armor: Some(spec),
                    },
                    ammo: None,
                }
            }
            "AMMO" => {
                let Some(row) = self.row(
                    "ammo",
                    &[
                        "name",
                        "editor_id",
                        "model",
                        "value",
                        "weight",
                        "clip_rounds",
                    ],
                    key,
                )?
                else {
                    return Ok(None);
                };
                let name = base_name(&row);
                let source = SourceAmmo {
                    name: name.clone(),
                    caliber: caliber_key(r, row.text(0).as_deref(), row.text(1).as_deref()),
                    value: row.num(3) as f32,
                    weight: row.num(4) as f32,
                    damage_mult: 0.0,
                    dr_modifier: 0,
                    ac_modifier: 0,
                    pack: row.num(5).max(0.0) as u32,
                };
                let spec = emit::rebalance::ammo_spec(&source, r);
                let pack = spec.quantity.max(1) as u32;
                let summary = format!("calibre {}, {} a pack", spec.caliber, pack);
                let description = format!("{name}: {pack} rounds a pack.");
                let size = spec.stats.size;
                let mut def =
                    ItemDef::new(ItemKind::Ammo(spec), ProtoText::new(&name, &description));
                def.material = emit::proto::Material::Metal;
                Made {
                    def,
                    info: ItemInfo {
                        pack,
                        size,
                        summary,
                        model: row.text(2),
                        body: false,
                        armor: None,
                    },
                    ammo: None,
                }
            }
            "ALCH" => {
                let Some(row) = self.row(
                    "ingestibles",
                    &["name", "editor_id", "model", "value", "weight"],
                    key,
                )?
                else {
                    return Ok(None);
                };
                let name = base_name(&row);
                let source = self.source_aid(&raw, &row, name.clone())?;
                let spec = emit::rebalance::aid_spec(&source, r);
                let summary = format!(
                    "{} hp, {} rad, {} effects",
                    source.health,
                    source.radiation,
                    source.effects.len()
                );
                let description = if source.health > 0 {
                    format!("{name}: restores hit points.")
                } else if source.radiation > 0 {
                    format!("{name}: purges radiation.")
                } else {
                    format!("{name}: something to swallow.")
                };
                let size = spec.stats.size;
                let mut def =
                    ItemDef::new(ItemKind::Drug(spec), ProtoText::new(&name, &description));
                def.material = emit::proto::Material::Glass;
                Made {
                    def,
                    info: ItemInfo {
                        pack: 1,
                        size,
                        summary,
                        model: row.text(2),
                        body: false,
                        armor: None,
                    },
                    ammo: None,
                }
            }
            "KEYM" | "MISC" | "BOOK" | "NOTE" | "CMNY" | "IMOD" | "CHIP" | "CCRD" => {
                let object = self.records.base_object(key).map_err(stage)?;
                let (value, weight, model, name) = match &object {
                    Some(o) => (
                        o.value.unwrap_or(0),
                        o.weight.unwrap_or(0.0),
                        o.model.clone(),
                        display_name(o.name.as_deref(), o.editor_id.as_deref(), key),
                    ),
                    None => (
                        0,
                        0.0,
                        None,
                        display_name(raw.name.as_deref(), raw.editor_id.as_deref(), key),
                    ),
                };
                let stats = emit::proto::ItemStats {
                    size: i32::from(weight > 0.0),
                    weight: round(f64::from(weight * r.weight_scale)).max(0),
                    cost: round(value as f64 * f64::from(r.cost_scale)).max(0),
                    ..emit::proto::ItemStats::default()
                };
                let worth = if stats.cost > 0 {
                    format!(", worth {} caps", stats.cost)
                } else {
                    String::new()
                };
                let (kind, description) = if raw.sig == "KEYM" {
                    (
                        ItemKind::Key(KeySpec { stats, code: 0 }),
                        format!("{name}: it opens a lock somewhere."),
                    )
                } else {
                    (
                        ItemKind::Misc(MiscSpec {
                            stats,
                            ..MiscSpec::default()
                        }),
                        format!("{name}{worth}."),
                    )
                };
                let summary = format!("{} caps, weight {}", stats.cost, stats.weight);
                Made {
                    def: ItemDef::new(kind, ProtoText::new(&name, &description)),
                    info: ItemInfo {
                        pack: 1,
                        size: stats.size,
                        summary,
                        model,
                        body: false,
                        armor: None,
                    },
                    ammo: None,
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(made))
    }

    /// An ingestible's effects (`EFID` and `EFIT` pairs) and addiction
    /// (`ENIT`), by the magic effects' editor ids and actor values.
    fn source_aid(&self, raw: &Raw, row: &Row, name: String) -> Result<SourceAid, ConvertError> {
        let mut aid = SourceAid {
            name,
            value: round(row.num(3)),
            weight: row.num(4) as f32,
            ..SourceAid::default()
        };
        let mut effect: Option<String> = None;
        for (sig, data) in &raw.subrecords {
            match sig.as_str() {
                "EFID" => {
                    effect = match u32_at(data, 0) {
                        Some(id) => {
                            let key = self.resolve(&raw.plugin, id);
                            self.raw(&key)?.and_then(|r| r.editor_id.clone())
                        }
                        None => None,
                    };
                }
                "EFIT" => {
                    let Some(id) = effect.take().map(|e| e.to_ascii_lowercase()) else {
                        continue;
                    };
                    let magnitude = u32_at(data, 0).unwrap_or(0).min(10_000) as i32;
                    let duration = u32_at(data, 8).unwrap_or(0).min(100_000);
                    let actor_value = i32_at(data, 16).unwrap_or(-1);
                    let total = magnitude.saturating_mul(duration.max(1) as i32);
                    if id.contains("health")
                        && ["fortify", "max", "damage", "drain", "inchealth", "increase"]
                            .iter()
                            .any(|w| id.contains(w))
                    {
                        // Raising the maximum, or harm: no Fallout 2 slot.
                    } else if id.contains("health") {
                        aid.health = aid.health.saturating_add(total.min(MAX_HEAL));
                    } else if id.contains("rad") && !id.contains("resist") {
                        if id.contains("restore") || id.contains("radaway") || id.contains("remove")
                        {
                            aid.radiation = aid.radiation.saturating_add(magnitude);
                        } else {
                            aid.radiation = aid.radiation.saturating_sub(total);
                        }
                    } else {
                        let sign = if ["damage", "drain", "weak"].iter().any(|w| id.contains(w)) {
                            -1
                        } else {
                            1
                        };
                        let stat = match actor_value {
                            5..=11 => Some(AidStat::Special((actor_value - 5) as u8)),
                            12 => Some(AidStat::ActionPoints),
                            13 => Some(AidStat::CarryWeight),
                            14 => Some(AidStat::CriticalChance),
                            15 => Some(AidStat::HealingRate),
                            17 => Some(AidStat::MeleeDamage),
                            18 => Some(AidStat::DamageResistance),
                            19 => Some(AidStat::PoisonResistance),
                            20 => Some(AidStat::RadiationResistance),
                            _ if id.contains("rad") => Some(AidStat::RadiationResistance),
                            _ => None,
                        };
                        if let Some(stat) = stat {
                            aid.effects.push(SourceEffect {
                                stat,
                                amount: sign * magnitude,
                                seconds: duration as f32,
                            });
                        }
                    }
                }
                "ENIT" => {
                    aid.addiction_chance = f32_at(data, 12).unwrap_or(0.0).clamp(0.0, 1.0);
                }
                _ => {}
            }
        }
        Ok(aid)
    }
}

// ---------------------------------------------------------------------------
// Draws

/// How a leveled item list is resolved.
#[derive(Clone, Copy)]
enum Draw<'s> {
    /// The actor documents' rule: the first entry at the lowest level.
    Figure,
    /// Drawn, seeded by a reference's form key.
    Seeded(&'s str),
}

/// What the draws did, for the report.
#[derive(Debug, Clone, Default)]
struct DrawTally {
    lists: usize,
    empty: usize,
    nested_too_deep: usize,
}

fn roll(seed: &str, list: &FormKey, salt: u64) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(seed.as_bytes());
    hasher.update([0]);
    hasher.update(list.to_string().as_bytes());
    hasher.update(salt.to_le_bytes());
    let digest = hasher.finalize();
    u64::from_le_bytes([
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
    ])
}

impl Source<'_> {
    /// `count` of `key` with every leveled item list in it resolved by
    /// `draw`, appended to `out` (counts multiplied down the lists).
    #[allow(clippy::too_many_arguments)]
    fn expand(
        &self,
        key: &FormKey,
        count: i64,
        draw: Draw<'_>,
        level: i32,
        depth: usize,
        out: &mut Vec<(FormKey, i64)>,
        tally: &mut DrawTally,
    ) -> Result<(), ConvertError> {
        if count <= 0 {
            return Ok(());
        }
        if depth > MAX_CHAIN {
            tally.nested_too_deep += 1;
            return Ok(());
        }
        let list = match self.decode(key)? {
            Some((raw, fnv_esm::Record::Leveled(list)))
                if list.kind == fnv_esm::LeveledKind::Item =>
            {
                (raw, list)
            }
            Some(_) => {
                out.push((key.clone(), count));
                return Ok(());
            }
            None => return Ok(()),
        };
        let (raw, list) = list;
        tally.lists += 1;
        let entries: Vec<(FormKey, i64)> = match draw {
            Draw::Figure => self
                .figure_entries(&raw, &list)
                .into_iter()
                .map(|(k, c)| (k, c.saturating_mul(count)))
                .collect(),
            Draw::Seeded(seed) => {
                let chance_none = u64::from(list.chance_none.unwrap_or(0).min(100));
                if chance_none > 0 && roll(seed, key, u64::MAX) % 100 < chance_none {
                    tally.empty += 1;
                    return Ok(());
                }
                let resolve = |e: &fnv_esm::LeveledEntry| {
                    (
                        self.resolve(&raw.plugin, e.form.0),
                        i64::from(e.count.max(1)),
                    )
                };
                if list.uses_all() {
                    list.entries
                        .iter()
                        .map(|e| {
                            let (k, c) = resolve(e);
                            (k, c.saturating_mul(count))
                        })
                        .collect()
                } else {
                    let mut eligible: Vec<&fnv_esm::LeveledEntry> = list
                        .entries
                        .iter()
                        .filter(|e| i32::from(e.level) <= level)
                        .collect();
                    if eligible.is_empty() {
                        let low = list.entries.iter().map(|e| e.level).min();
                        eligible = list
                            .entries
                            .iter()
                            .filter(|e| Some(e.level) == low)
                            .collect();
                    }
                    if eligible.is_empty() {
                        tally.empty += 1;
                        return Ok(());
                    }
                    let each = list.flags.unwrap_or(0) & fnv_esm::LeveledList::FLAG_EACH != 0;
                    let picks = if each { count.min(100) } else { 1 };
                    (0..picks)
                        .map(|k| {
                            let e = eligible
                                [(roll(seed, key, k as u64) % eligible.len() as u64) as usize];
                            let (form, c) = resolve(e);
                            (form, if each { c } else { c.saturating_mul(count) })
                        })
                        .collect()
                }
            }
        };
        for (entry, n) in entries {
            self.expand(&entry, n, draw, level, depth + 1, out, tally)?;
        }
        Ok(())
    }

    /// An inventory (the `inventory` table's rows of `owner`) resolved by
    /// `draw`, equal items merged in first-seen order.
    fn inventory(
        &self,
        owner: &FormKey,
        draw: Draw<'_>,
        level: i32,
        tally: &mut DrawTally,
    ) -> Result<Vec<(FormKey, i64)>, ConvertError> {
        let mut out = Vec::new();
        for (item, count) in self.records.inventory(owner).map_err(stage)? {
            if let Some(item) = item {
                self.expand(&item, i64::from(count), draw, level, 0, &mut out, tally)?;
            }
        }
        let mut merged: Vec<(FormKey, i64)> = Vec::new();
        for (key, n) in out {
            match merged.iter_mut().find(|(k, _)| *k == key) {
                Some(slot) => slot.1 = slot.1.saturating_add(n),
                None => merged.push((key, n)),
            }
        }
        Ok(merged)
    }
}

// ---------------------------------------------------------------------------
// Art

/// Ground sprites and inventory icons of items no placed reference draws:
/// the item's model rendered as the sprite stage renders a prop (cached in
/// `<out>/sprites/` by the same key scheme), and an icon rendered
/// [`ICON_SCALE`] times larger from the same model.
struct ItemArt<'a> {
    inputs: &'a Inputs,
    mir: &'a Mir,
    map: &'a PaletteMap,
    prefix: Vec<u8>,
    icon_prefix: Vec<u8>,
    renderer: Option<Box<dyn foundry_render::Renderer>>,
    icon_renderer: Option<Box<dyn foundry_render::Renderer>>,
    source: Option<Option<mir::DataSource>>,
    tally: ArtTally,
}

/// Where item art came from, for the report.
#[derive(Debug, Clone, Default)]
struct ArtTally {
    from_extract: usize,
    from_install: usize,
    cached: usize,
    rendered: usize,
    placeholders: usize,
    failures: Vec<String>,
}

/// Textures by path: the extraction's, else the install's.
struct ItemTextures<'a> {
    mir: &'a Mir,
    source: Option<&'a mir::DataSource>,
    cache: RefCell<BTreeMap<String, Option<foundry_render::Texture>>>,
}

impl ItemTextures<'_> {
    fn bytes_hash(&self, path: &str) -> String {
        if let Some(hash) = self.mir.texture_hash(path) {
            return hash.to_owned();
        }
        match self.source.and_then(|s| s.read(path).ok().flatten()) {
            Some(bytes) => mir::content_hash(&bytes),
            None => "missing".to_owned(),
        }
    }
}

impl foundry_render::TextureSource for ItemTextures<'_> {
    fn texture(&self, path: &str) -> Option<std::borrow::Cow<'_, foundry_render::Texture>> {
        let mut cache = self.cache.borrow_mut();
        let slot = cache.entry(path.to_owned()).or_insert_with(|| {
            if let Some(hash) = self.mir.texture_hash(path)
                && let Ok(image) = self.mir.texture(hash)
            {
                return Some(foundry_render::Texture {
                    width: image.width,
                    height: image.height,
                    rgba: image.rgba,
                });
            }
            let bytes = self.source?.read(path).ok()??;
            let image = dds::decode(&bytes).ok()?;
            Some(foundry_render::Texture {
                width: image.width,
                height: image.height,
                rgba: image.rgba,
            })
        });
        slot.clone().map(std::borrow::Cow::Owned)
    }
}

impl<'a> ItemArt<'a> {
    fn new(
        inputs: &'a Inputs,
        mir: &'a Mir,
        map: &'a PaletteMap,
        palette_bytes: &[u8],
    ) -> Result<Self, ConvertError> {
        let backend = match inputs.backend {
            foundry_render::Backend::Auto => foundry_render::Backend::Cpu,
            other => other,
        };
        let prefix = crate::sprites::key_prefix(inputs, backend, palette_bytes)?;
        let mut icon = Sha256::new();
        icon.update(&prefix);
        icon.update(format!("inventory icon x{ICON_SCALE}").as_bytes());
        Ok(ItemArt {
            inputs,
            mir,
            map,
            prefix,
            icon_prefix: icon.finalize().to_vec(),
            renderer: None,
            icon_renderer: None,
            source: None,
            tally: ArtTally::default(),
        })
    }

    fn icon_calibration(&self) -> foundry_render::Calibration {
        let mut c = self.inputs.calibration.clone();
        c.tile_metres /= ICON_SCALE;
        c.pixels_per_metre_vertical *= ICON_SCALE;
        c
    }

    fn data_source(&mut self) -> Option<&mir::DataSource> {
        if self.source.is_none() {
            self.source =
                Some(mir::DataSource::new(&self.inputs.data_dir, &self.inputs.slice.plugins).ok());
        }
        self.source.as_ref().and_then(Option::as_ref)
    }

    /// The mesh of a model: the extraction's, else read and parsed from the
    /// install; and a stable id of its bytes.
    fn mesh(&mut self, model: &str) -> Option<(nif::StaticMesh, String)> {
        if let Some(hash) = self.mir.mesh_hash(model).map(str::to_owned)
            && let Ok(mesh) = self.mir.mesh(&hash)
        {
            self.tally.from_extract += 1;
            return Some((mesh, hash));
        }
        let path = mir::normalize_mesh_path(model);
        let bytes = self.data_source()?.read(&path).ok()??;
        let mesh = nif::parse(&bytes)
            .and_then(|f| nif::extract_mesh(&f))
            .ok()?;
        self.tally.from_install += 1;
        Some((mesh, mir::content_hash(&bytes)))
    }

    /// One still of `mesh` through `renderer`, cached under `key`:
    /// `(pixels, width, height, shift)`.
    fn still(
        &mut self,
        key: &str,
        mesh: &nif::StaticMesh,
        icon: bool,
    ) -> Result<(Vec<u8>, u32, u32, [i16; 2]), String> {
        let dir = self.inputs.sprites_dir();
        let frm_path = dir.join(format!("{key}.frm"));
        if let Ok(bytes) = std::fs::read(&frm_path)
            && let Ok(archive) = vault_formats::frm::FrmArchive::parse(&bytes)
            && let Some(frame) = archive.frame(0, 0)
        {
            self.tally.cached += 1;
            return Ok((
                frame.pixels().to_vec(),
                u32::from(frame.width),
                u32::from(frame.height),
                archive.shifts[0],
            ));
        }
        if mesh.primitives.is_empty() {
            return Err("the mesh has no geometry".to_owned());
        }
        let calibration = if icon {
            self.icon_calibration()
        } else {
            self.inputs.calibration.clone()
        };
        let slot = if icon {
            &mut self.icon_renderer
        } else {
            &mut self.renderer
        };
        if slot.is_none() {
            *slot = Some(
                foundry_render::renderer(self.inputs.backend, &calibration)
                    .map_err(|e| e.to_string())?,
            );
        }
        let source = self.source.as_ref().and_then(Option::as_ref);
        let textures = ItemTextures {
            mir: self.mir,
            source,
            cache: RefCell::new(BTreeMap::new()),
        };
        let scene = foundry_render::Scene {
            anchor: [0.0; 3],
            instances: vec![foundry_render::Instance::new(
                mesh,
                foundry_render::Instance::IDENTITY,
                &textures,
            )],
        };
        let renderer = slot.as_mut().expect("made above");
        let output = renderer
            .render(&scene, foundry_render::Window::Fit)
            .map_err(|e| e.to_string())?;
        let indexed = quantize::quantize(
            &output.rgba,
            output.width,
            output.height,
            self.map,
            quantize::Dither::None,
        )
        .map_err(|e| e.to_string())?;
        if indexed.iter().all(|p| *p == 0) {
            return Err("the render is empty".to_owned());
        }
        let placed = emit::placed_frame(&indexed, output.width, output.height, output.origin)
            .map_err(|e| e.to_string())?;
        let spec = emit::FrmSpec {
            fps: 0,
            action_frame: 0,
            directions: vec![emit::FrmDirection {
                shift: placed.shift,
                frames: vec![placed.frame.clone()],
            }],
        };
        let bytes =
            emit::FormatWriters::frm(&emit::LocalWriters, &spec).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(&frm_path, bytes).map_err(|e| e.to_string())?;
        self.tally.rendered += 1;
        Ok((
            placed.frame.pixels,
            u32::from(placed.frame.width),
            u32::from(placed.frame.height),
            placed.shift,
        ))
    }

    /// A ground sprite and an inventory icon for an item of `kind` drawn
    /// by `model`; a small placeholder (no icon) when the model cannot be
    /// rendered.
    fn art(
        &mut self,
        model: Option<&str>,
        kind: &str,
        label: &str,
    ) -> (ArtImage, Option<ItemIcon>) {
        let rendered = model.and_then(|model| {
            let (mesh, id) = self.mesh(model)?;
            let textures = ItemTextures {
                mir: self.mir,
                source: self.source.as_ref().and_then(Option::as_ref),
                cache: RefCell::new(BTreeMap::new()),
            };
            let key_for = |prefix: &[u8]| {
                let mut hasher = Sha256::new();
                hasher.update(prefix);
                hasher.update(id.as_bytes());
                for path in mesh.texture_paths() {
                    hasher.update(path.as_bytes());
                    hasher.update([0]);
                    hasher.update(textures.bytes_hash(&path).as_bytes());
                    hasher.update([0]);
                }
                hasher
                    .finalize()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            };
            let (sprite_key, icon_key) = (
                key_for(&self.prefix.clone()),
                key_for(&self.icon_prefix.clone()),
            );
            drop(textures);
            let ground = match self.still(&sprite_key, &mesh, false) {
                Ok(g) => g,
                Err(e) => {
                    self.tally.failures.push(format!("{label}: {e}"));
                    return None;
                }
            };
            let icon = self.still(&icon_key, &mesh, true).ok();
            Some((ground, icon))
        });
        match rendered {
            Some(((pixels, width, height, shift), icon)) => (
                ArtImage {
                    category: ArtCategory::Items,
                    width,
                    height,
                    pixels,
                    shift,
                    source: model.unwrap_or_default().to_owned(),
                },
                icon.map(|(pixels, width, height, _)| ItemIcon::Indexed {
                    width,
                    height,
                    pixels,
                }),
            ),
            None => {
                self.tally.placeholders += 1;
                (placeholder(self.map, kind, label), None)
            }
        }
    }
}

/// A small box in a colour per kind, with a dark rim, standing on its hex.
fn placeholder(map: &PaletteMap, kind: &str, label: &str) -> ArtImage {
    let (width, height) = (12u32, 8u32);
    let colour: [u8; 3] = match kind {
        "weapon" => [120, 120, 130],
        "armor" => [140, 100, 60],
        "ammo" => [180, 150, 60],
        "drug" => [200, 60, 60],
        "key" => [210, 190, 80],
        _ => [150, 140, 120],
    };
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let rim = x == 0 || y == 0 || x == width - 1 || y == height - 1;
            let c = if rim { [40, 36, 30] } else { colour };
            rgba.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    let pixels = quantize::quantize(&rgba, width, height, map, quantize::Dither::None)
        .unwrap_or_else(|_| vec![1; (width * height) as usize])
        .into_iter()
        .map(|p| p.max(1))
        .collect();
    ArtImage {
        category: ArtCategory::Items,
        width,
        height,
        pixels,
        shift: [0, 0],
        source: format!("placeholder {label}"),
    }
}

// ---------------------------------------------------------------------------
// Sides

/// A placed person's side and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Side {
    pub hostile: bool,
    pub reason: String,
    /// Whether the actor's AI data was found (else the patterns decided).
    pub from_records: bool,
}

/// The player's faction and its relations, read once.
struct PlayerFaction {
    key: Option<FormKey>,
    relations: Vec<(FormKey, i32, u32)>,
}

impl Source<'_> {
    fn player_faction(&self, editor_id: &str) -> Result<PlayerFaction, ConvertError> {
        let key = self
            .records
            .find_editor_id(editor_id)
            .map_err(stage)?
            .into_iter()
            .find(|k| self.raw(k).ok().flatten().is_some_and(|r| r.sig == "FACT"));
        let relations = match &key {
            Some(k) => self.relations(k)?,
            None => Vec::new(),
        };
        Ok(PlayerFaction { key, relations })
    }

    /// The side of an actor (module notes; [`crate::rebalance::Hostility`]).
    fn side(
        &self,
        actor: &FormKey,
        actor_editor_id: Option<&str>,
        player: &PlayerFaction,
        rules: &crate::rebalance::Hostility,
    ) -> Result<Side, ConvertError> {
        let factions: Vec<FormKey> = match self.supplier(actor, USE_FACTIONS)? {
            Some((_, raw, npc)) => npc
                .factions
                .iter()
                .map(|f| self.resolve(&raw.plugin, f.faction.0))
                .collect(),
            None => Vec::new(),
        };
        let mut names: Vec<String> = actor_editor_id.map(str::to_owned).into_iter().collect();
        for faction in &factions {
            if let Some(raw) = self.raw(faction)?
                && let Some(id) = &raw.editor_id
            {
                names.push(id.clone());
            }
        }
        if let Some(pattern) = rules
            .friendly_patterns
            .iter()
            .find(|p| names.iter().any(|n| glob(p, n)))
        {
            return Ok(Side {
                hostile: false,
                reason: format!("friendly pattern {pattern}"),
                from_records: false,
            });
        }
        let aggression = match self.supplier(actor, USE_AI_DATA)? {
            Some((_, raw, _)) => raw.first("AIDT").and_then(|d| d.first().copied()),
            None => None,
        };
        let Some(aggression) = aggression else {
            let hostile = rules
                .hostile_patterns
                .iter()
                .find(|p| names.iter().any(|n| glob(p, n)));
            return Ok(Side {
                hostile: hostile.is_some(),
                reason: match hostile {
                    Some(p) => format!("no AI data; hostile pattern {p}"),
                    None => "no AI data; no hostile pattern".to_owned(),
                },
                from_records: false,
            });
        };
        // The relation between one of the actor's factions and the
        // player's, either way round: enemy (1), or friend or ally (2, 3).
        let mut enemy = None;
        let mut friend = None;
        if let Some(player_key) = &player.key {
            for faction in &factions {
                let mut reactions: Vec<u32> = player
                    .relations
                    .iter()
                    .filter(|(other, _, _)| other == faction)
                    .map(|(_, _, r)| *r)
                    .collect();
                reactions.extend(
                    self.relations(faction)?
                        .iter()
                        .filter(|(other, _, _)| other == player_key)
                        .map(|(_, _, r)| *r),
                );
                let label = self
                    .raw(faction)?
                    .and_then(|r| r.editor_id.clone())
                    .unwrap_or_else(|| faction.to_string());
                if reactions.contains(&1) && enemy.is_none() {
                    enemy = Some(label.clone());
                }
                if reactions.iter().any(|r| *r == 2 || *r == 3) && friend.is_none() {
                    friend = Some(label);
                }
            }
        }
        let side = if aggression >= rules.hostile_aggression {
            match &friend {
                Some(f) => Side {
                    hostile: false,
                    reason: format!("aggression {aggression}, but {f} is the player's friend"),
                    from_records: true,
                },
                None => Side {
                    hostile: true,
                    reason: format!("aggression {aggression} (attacks on sight)"),
                    from_records: true,
                },
            }
        } else if aggression >= rules.enemy_aggression
            && let Some(e) = &enemy
        {
            Side {
                hostile: true,
                reason: format!("aggression {aggression}, {e} is the player's enemy"),
                from_records: true,
            }
        } else {
            Side {
                hostile: false,
                reason: format!(
                    "aggression {aggression}{}",
                    match &enemy {
                        Some(e) => format!(", {e} an enemy, but it attacks only when attacked"),
                        None => String::new(),
                    }
                ),
                from_records: true,
            }
        };
        Ok(side)
    }

    /// An actor's attributes and skills (`DATA`, `DNAM` of the NPC that
    /// supplies its stats).
    fn actor_figures(&self, actor: &FormKey) -> Result<Figures, ConvertError> {
        let Some((_, _, npc)) = self.supplier(actor, USE_STATS)? else {
            return Ok(([5; 7], Vec::new()));
        };
        let special = npc
            .stats
            .map_or([5; 7], |s| s.attributes.map(|a| i32::from(a).clamp(1, 10)));
        let mut skills = Vec::new();
        if let Some(dnam) = npc.skills {
            // The fourteen skills in actor value order, from Barter (32).
            let game_skills: [Option<SourceSkill>; 14] = [
                Some(SourceSkill::Barter),
                Some(SourceSkill::BigGuns),
                Some(SourceSkill::EnergyWeapons),
                Some(SourceSkill::Explosives),
                Some(SourceSkill::Lockpick),
                Some(SourceSkill::Medicine),
                Some(SourceSkill::MeleeWeapons),
                Some(SourceSkill::Repair),
                Some(SourceSkill::Science),
                Some(SourceSkill::SmallGuns),
                Some(SourceSkill::Sneak),
                Some(SourceSkill::Speech),
                None,
                Some(SourceSkill::Unarmed),
            ];
            for (i, skill) in game_skills.iter().enumerate() {
                if let Some(skill) = skill {
                    let value = i32::from(dnam.values[i]) + i32::from(dnam.offsets[i]);
                    skills.push((*skill, value.clamp(0, 100)));
                }
            }
        }
        Ok((special, skills))
    }
}

// ---------------------------------------------------------------------------
// The catalogue

/// An actor's attributes and skills.
type Figures = ([i32; 7], Vec<(SourceSkill, i32)>);
/// An AI packet's number and what it is.
type Packet = (i32, String);
/// A person to equip: placement, reference, actor document, inventory.
type Person<'a> = (
    &'a map_builder::CritterPlacement,
    &'a mir::Reference,
    mir::ActorDoc,
    Vec<(FormKey, i64)>,
);

/// What the map stage hands the catalogue.
pub struct CatalogueInput<'a> {
    pub inputs: &'a Inputs,
    pub mir: &'a Mir,
    pub rebalance: &'a LoadedRebalance,
    /// The window's references.
    pub references: &'a [mir::Reference],
    /// The slicer's art and placements.
    pub art: &'a [map_slicer::ArtImage],
    pub placements: &'a [map_slicer::Placement],
    /// The people the builder will place.
    pub critters: &'a [map_builder::CritterPlacement],
    /// The base game's files (`data/ai.txt`).
    pub base: &'a BTreeMap<String, Vec<u8>>,
    pub palette_map: &'a PaletteMap,
    pub palette_bytes: &'a [u8],
}

/// One person's side and kit, for the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonLoot {
    pub name: String,
    pub reference: String,
    pub side: Side,
    pub ai_packet: i32,
    pub team: i32,
    pub weapon: Option<String>,
    pub armor: Option<String>,
    pub carried: usize,
    pub dropped: usize,
}

/// One item for the gallery and the report.
#[derive(Debug, Clone)]
pub struct ItemEntry {
    pub key: String,
    pub name: String,
    pub kind: &'static str,
    pub summary: String,
    /// Where it was found: `placed`, `container`, `person`, `ammunition`.
    pub found: String,
    /// The ground art (indexed, shift) and the icon, when known.
    pub ground: Option<(Vec<u8>, u32, u32)>,
    pub icon: Option<(Vec<u8>, u32, u32)>,
}

/// What the catalogue made, beside it.
#[derive(Debug, Clone, Default)]
pub struct CatalogueReport {
    pub rebalance: String,
    pub items: Vec<ItemEntry>,
    pub placed_items: usize,
    pub containers: usize,
    pub filled_containers: usize,
    pub container_stacks: usize,
    pub truncated_containers: usize,
    pub locked: usize,
    pub doors: usize,
    pub keyed_doors: usize,
    pub named: usize,
    pub people: Vec<PersonLoot>,
    pub hostile_packet: (i32, String),
    pub friendly_packet: (i32, String),
    pub leveled_lists: usize,
    pub empty_draws: usize,
    pub art: String,
    pub notes: Vec<String>,
    pub footprints: usize,
}

impl CatalogueReport {
    /// Plain lines for `map-report.md`.
    pub fn lines(&self) -> Vec<String> {
        let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
        for item in &self.items {
            *kinds.entry(item.kind).or_default() += 1;
        }
        let kinds: Vec<String> = kinds.iter().map(|(k, n)| format!("{n} {k}")).collect();
        let hostile = self.people.iter().filter(|p| p.side.hostile).count();
        let armed = self.people.iter().filter(|p| p.weapon.is_some()).count();
        let from_records = self.people.iter().filter(|p| p.side.from_records).count();
        let mut lines = vec![
            format!("rebalance tables: {}", self.rebalance),
            format!(
                "items defined: {} ({})",
                self.items.len(),
                if kinds.is_empty() {
                    "none".to_owned()
                } else {
                    kinds.join(", ")
                }
            ),
            format!(
                "placed: {} item references on the ground; {} containers ({} filled, {} stacks, {} cut to \
                 the stack limit); {} doors ({} keyed); {} locked; {} named references",
                self.placed_items,
                self.containers,
                self.filled_containers,
                self.container_stacks,
                self.truncated_containers,
                self.doors,
                self.keyed_doors,
                self.locked,
                self.named
            ),
            format!(
                "leveled item lists drawn: {}, {} gave nothing",
                self.leveled_lists, self.empty_draws
            ),
            format!(
                "people: {} ({hostile} hostile, {} friendly; {armed} armed); sides from the \
                 records' AI data and factions for {from_records} of them",
                self.people.len(),
                self.people.len() - hostile
            ),
            format!(
                "hostile: team and AI packet {} ({}); friendly: packet {} ({})",
                self.hostile_packet.0,
                self.hostile_packet.1,
                self.friendly_packet.0,
                self.friendly_packet.1
            ),
            format!("item art: {}", self.art),
        ];
        if self.footprints > 0 {
            lines.push(format!(
                "furniture footprints handed to the builder: {}",
                self.footprints
            ));
        }
        for note in &self.notes {
            lines.push(format!("note: {note}"));
        }
        lines
    }

    /// `| name | reference | side | why | packet | team | weapon | armour | carried |`
    pub fn people_table(&self) -> String {
        let mut out = String::from(
            "| person | reference | side | why | packet | team | right hand | worn | carried |\n\
             |---|---|---|---|---:|---:|---|---|---:|\n",
        );
        for p in &self.people {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {}{} |\n",
                p.name,
                p.reference,
                if p.side.hostile {
                    "hostile"
                } else {
                    "friendly"
                },
                p.side.reason,
                p.ai_packet,
                p.team,
                p.weapon.as_deref().unwrap_or("-"),
                p.armor.as_deref().unwrap_or("-"),
                p.carried,
                if p.dropped > 0 {
                    format!(" ({} dropped)", p.dropped)
                } else {
                    String::new()
                }
            ));
        }
        out
    }
}

/// The catalogue and its report.
pub struct Built {
    pub catalogue: Catalogue,
    pub report: CatalogueReport,
}

/// The AI packets of each side, from the base `data/ai.txt` by name.
fn packets(
    input: &CatalogueInput<'_>,
    notes: &mut Vec<String>,
) -> Result<(Packet, Packet), ConvertError> {
    let rules = &input.rebalance.convert.hostility;
    let table = input
        .base
        .get(map_builder::critters::AI_TXT)
        .and_then(|bytes| {
            vault_formats::ai::Packets::parse_for(bytes, vault_formats::GameProfile::Fallout2).ok()
        });
    let by_name = |name: &str| -> Option<(i32, String)> {
        let table = table.as_ref()?;
        table
            .packets
            .iter()
            .find(|p| {
                String::from_utf8_lossy(&p.name)
                    .trim()
                    .eq_ignore_ascii_case(name.trim())
            })
            .map(|p| (p.packet_num, String::from_utf8_lossy(&p.name).into_owned()))
    };
    let default_packet = match input.base.get(map_builder::critters::AI_TXT) {
        Some(bytes) => map_builder::critters::choose_ai_packet(bytes).map_err(stage)?,
        None => map_builder::critters::FALLBACK_AI_PACKET,
    };
    let hostile = match (
        input.inputs.slice.hostile_packet,
        by_name(&rules.hostile_packet),
    ) {
        (Some(number), _) => {
            let known = table
                .as_ref()
                .is_none_or(|t| t.packets.iter().any(|p| p.packet_num == number));
            if !known {
                return Err(ConvertError::Slice {
                    name: input.inputs.slice.name.clone(),
                    reason: format!("hostile_packet = {number} is not a packet of the base ai.txt"),
                });
            }
            let named = table.as_ref().and_then(|t| {
                t.packets
                    .iter()
                    .find(|p| p.packet_num == number)
                    .map(|p| String::from_utf8_lossy(&p.name).into_owned())
            });
            (number, named.unwrap_or_else(|| format!("packet {number}")))
        }
        (None, Some(found)) => found,
        (None, None) => {
            let number = rules.hostile_packet_number;
            let known = table
                .as_ref()
                .is_none_or(|t| t.packets.iter().any(|p| p.packet_num == number));
            if known {
                if table.is_some() {
                    notes.push(format!(
                        "the base ai.txt has no packet named {:?}; hostile people take packet {number}",
                        rules.hostile_packet
                    ));
                }
                (number, format!("packet {number}"))
            } else {
                notes.push(format!(
                    "the base ai.txt has neither a packet named {:?} nor packet {number}; hostile \
                     people take the friendly default",
                    rules.hostile_packet
                ));
                (default_packet, "the builder's default".to_owned())
            }
        }
    };
    let friendly = if rules.friendly_packet.trim().is_empty() {
        (default_packet, "the builder's default".to_owned())
    } else {
        by_name(&rules.friendly_packet)
            .unwrap_or((default_packet, "the builder's default".to_owned()))
    };
    Ok((hostile, friendly))
}

/// The Fallout 2 weapon code and attack modes of every weapon an actor
/// document names, by the weapon's form key, as the run's rebalance
/// classifies it: what the critter stage draws in hand
/// (`crate::critters::CritterOptions::weapon_codes`).
pub fn weapon_codes(
    mir: &Mir,
    rebalance: &LoadedRebalance,
) -> Result<BTreeMap<String, (WeaponCode, [AttackMode; 2])>, ConvertError> {
    let records = mir.records().map_err(stage)?;
    let plugins: Vec<String> = mir
        .manifest()
        .plugins
        .iter()
        .map(|p| p.name.clone())
        .collect();
    let source = Source::new(&records, &plugins);
    let mut out = BTreeMap::new();
    for entry in mir.actors() {
        let Some(doc) = mir.actor(&entry.form).map_err(stage)? else {
            continue;
        };
        for weapon in &doc.weapons {
            let Some(form) = &weapon.form else { continue };
            if out.contains_key(&form.to_string()) {
                continue;
            }
            if let Some((weapon_source, _, _)) = source.source_weapon(form, rebalance)? {
                let spec = emit::rebalance::weapon_spec(&weapon_source, &rebalance.table);
                out.insert(form.to_string(), (spec.code, spec.modes));
            }
        }
    }
    Ok(out)
}

/// Builds the catalogue (module notes).
pub fn build(input: &CatalogueInput<'_>) -> Result<Built, ConvertError> {
    let records = input.mir.records().map_err(stage)?;
    let plugins: Vec<String> = input.inputs.slice.plugins.clone();
    let source = Source::new(&records, &plugins);
    let rebalance = input.rebalance;
    let loot = &rebalance.convert.loot;
    let mut report = CatalogueReport {
        rebalance: format!(
            "{} (digest {})",
            rebalance.source,
            &rebalance.digest()[..12]
        ),
        ..CatalogueReport::default()
    };
    let mut tally = DrawTally::default();
    let mut art = ItemArt::new(
        input.inputs,
        input.mir,
        input.palette_map,
        input.palette_bytes,
    )?;

    let by_key: BTreeMap<String, &mir::Reference> = input
        .references
        .iter()
        .map(|r| (r.form.to_string(), r))
        .collect();
    let mut roles: BTreeMap<&str, BTreeSet<map_slicer::Role>> = BTreeMap::new();
    let mut first_art: BTreeMap<&str, usize> = BTreeMap::new();
    for p in input.placements {
        roles
            .entry(p.reference.as_str())
            .or_default()
            .insert(p.role);
        first_art.entry(p.reference.as_str()).or_insert(p.art);
    }
    let only = |reference: &str, role: map_slicer::Role| {
        roles
            .get(reference)
            .is_some_and(|r| r.len() == 1 && r.contains(&role))
    };

    // Items wanted, in first-seen order, and where.
    let mut wanted: Vec<(FormKey, &'static str)> = Vec::new();
    let want = |key: &FormKey, found: &'static str, wanted: &mut Vec<(FormKey, &'static str)>| {
        if !wanted.iter().any(|(k, _)| k == key) {
            wanted.push((key.clone(), found));
        }
    };
    // Placed items.
    let mut placed_items: Vec<(&mir::Reference, FormKey)> = Vec::new();
    for (key, reference) in &by_key {
        if only(key, map_slicer::Role::Item) {
            placed_items.push((reference, reference.base.clone()));
            want(&reference.base, "placed", &mut wanted);
        }
    }
    // Containers.
    let mut containers: Vec<(&mir::Reference, Vec<(FormKey, i64)>)> = Vec::new();
    for (key, reference) in &by_key {
        if !only(key, map_slicer::Role::Container) {
            continue;
        }
        let contents =
            source.inventory(&reference.base, Draw::Seeded(key), loot.level, &mut tally)?;
        for (item, _) in &contents {
            want(item, "container", &mut wanted);
        }
        containers.push((reference, contents));
    }
    // People: the inventory of the actor that supplies it.
    let critter_refs: BTreeMap<&str, &map_builder::CritterPlacement> = input
        .critters
        .iter()
        .map(|c| (c.reference.as_str(), c))
        .collect();
    let mut people: Vec<Person<'_>> = Vec::new();
    for (reference_key, placement) in &critter_refs {
        let Some(reference) = by_key.get(*reference_key) else {
            continue;
        };
        let Some(doc) = input.mir.actor(&reference.base).map_err(stage)? else {
            report
                .notes
                .push(format!("{reference_key}: no actor document"));
            continue;
        };
        let supplier = source
            .supplier(&doc.form, 0x0100)?
            .map(|(k, _, _)| k)
            .unwrap_or_else(|| doc.form.clone());
        let inventory = source.inventory(&supplier, Draw::Figure, loot.level, &mut tally)?;
        for (item, _) in &inventory {
            want(item, "person", &mut wanted);
        }
        if let Some(weapon) = doc
            .weapons
            .first()
            .filter(|w| w.carried)
            .and_then(|w| w.form.clone())
        {
            want(&weapon, "person", &mut wanted);
        }
        people.push((placement, reference, doc, inventory));
    }
    report.leveled_lists = tally.lists;
    report.empty_draws = tally.empty;
    if tally.nested_too_deep > 0 {
        report.notes.push(format!(
            "{} leveled lists nested too deep were left out",
            tally.nested_too_deep
        ));
    }

    // Item definitions; ammunition a weapon names joins the list.
    let mut made: BTreeMap<String, Made> = BTreeMap::new();
    let mut found: BTreeMap<String, &'static str> = BTreeMap::new();
    let mut not_items: BTreeSet<String> = BTreeSet::new();
    let mut queue = wanted.clone();
    let mut i = 0;
    while i < queue.len() {
        let (key, how) = queue[i].clone();
        i += 1;
        let text = key.to_string();
        if made.contains_key(&text) || not_items.contains(&text) {
            continue;
        }
        match source.make_item(&key, rebalance)? {
            Some(item) => {
                if let Some(ammo) = &item.ammo
                    && !queue.iter().any(|(k, _)| k == ammo)
                {
                    queue.push((ammo.clone(), "ammunition"));
                }
                found.insert(text.clone(), how);
                made.insert(text, item);
            }
            None => {
                not_items.insert(text);
            }
        }
    }
    if !not_items.is_empty() {
        report.notes.push(format!(
            "{} forms named as items are not items the converter knows (or not in the plugin list)",
            not_items.len()
        ));
    }

    // Placed loot.
    let mut placed: BTreeMap<String, PlacedLoot> = BTreeMap::new();
    for (reference, base) in &placed_items {
        let key = base.to_string();
        let Some(item) = made.get(&key) else { continue };
        let units = reference.count.unwrap_or(1).max(1) as u32;
        let (count, state) = if matches!(item.def.kind, ItemKind::Ammo(_)) {
            let pack = item.info.pack.max(1);
            let rounds = if reference.count.is_some() {
                units
            } else {
                pack
            };
            let packs = rounds.div_ceil(pack).clamp(1, 100);
            let front = rounds - (packs - 1) * pack;
            (
                packs,
                ItemState {
                    rounds: (front < pack).then_some(front as i32),
                    ..ItemState::default()
                },
            )
        } else {
            (units.min(100), ItemState::default())
        };
        placed.insert(
            reference.form.to_string(),
            PlacedLoot::Item {
                item: key,
                count,
                state,
            },
        );
        report.placed_items += 1;
    }
    let lock_of = |reference: &mir::Reference| LockState {
        locked: reference.lock.is_some(),
        jammed: false,
    };
    let base_name = |key: &FormKey| -> Result<Option<String>, ConvertError> {
        Ok(source.raw(key)?.and_then(|r| r.name.clone()))
    };
    for (reference, contents) in &containers {
        let mut held: Vec<Held> = Vec::new();
        let mut size: i64 = 0;
        let mut kinds_cut = false;
        for (item, n) in contents {
            let Some(made) = made.get(&item.to_string()) else {
                continue;
            };
            if held.len() >= loot.max_contents {
                kinds_cut = true;
                break;
            }
            held.push(stack(&item.to_string(), *n, made));
            size +=
                i64::from(made.info.size.max(0)) * i64::from(held.last().map_or(0, |h| h.quantity));
        }
        report.truncated_containers += usize::from(kinds_cut);
        report.containers += 1;
        report.filled_containers += usize::from(!held.is_empty());
        report.container_stacks += held.len();
        let lock = lock_of(reference);
        report.locked += usize::from(lock.locked);
        let name = base_name(&reference.base)?.unwrap_or_else(|| "Container".to_owned());
        let capacity = i32::try_from(size + 1).unwrap_or(i32::MAX).max(100);
        let description = if lock.locked {
            format!("{name}. It is locked.")
        } else {
            format!("{name}.")
        };
        placed.insert(
            reference.form.to_string(),
            PlacedLoot::Container {
                text: Some(ProtoText::new(&name, &description)),
                capacity: Some(capacity),
                lock,
                contents: held,
            },
        );
    }
    for (key, reference) in &by_key {
        if placed.contains_key(key.as_str()) || !roles.contains_key(key.as_str()) {
            continue;
        }
        let name = base_name(&reference.base)?;
        if only(key, map_slicer::Role::Door) {
            let lock = lock_of(reference);
            let key_item = reference
                .lock
                .as_ref()
                .and_then(|l| l.key.as_ref())
                .map(ToString::to_string)
                .filter(|k| {
                    made.get(k)
                        .is_some_and(|m| matches!(m.def.kind, ItemKind::Key(_)))
                });
            if name.is_none() && !lock.locked {
                continue;
            }
            let name = name.unwrap_or_else(|| "Door".to_owned());
            report.doors += 1;
            report.locked += usize::from(lock.locked);
            report.keyed_doors += usize::from(key_item.is_some());
            placed.insert(
                (*key).clone(),
                PlacedLoot::Door {
                    text: Some(ProtoText::new(
                        &name,
                        &if lock.locked {
                            format!("{name}. It is locked.")
                        } else {
                            format!("{name}.")
                        },
                    )),
                    lock,
                    key: key_item,
                },
            );
        } else if let Some(name) = name
            && roles[key.as_str()]
                .iter()
                .all(|r| *r != map_slicer::Role::Item)
            && matches!(
                reference.base_kind.as_deref(),
                Some("ACTI" | "TACT" | "TERM" | "FURN" | "MSTT" | "CONT" | "DOOR")
            )
        {
            report.named += 1;
            placed.insert(
                (*key).clone(),
                PlacedLoot::Named(ProtoText::new(&name, &format!("{name}."))),
            );
        }
    }

    // People.
    let (hostile_packet, friendly_packet) = packets(input, &mut report.notes)?;
    report.hostile_packet = hostile_packet.clone();
    report.friendly_packet = friendly_packet.clone();
    let rules = &rebalance.convert.hostility;
    let friendly_team = if rules.friendly_team > 0 {
        rules.friendly_team
    } else {
        map_builder::critters::DEFAULT_CRITTER_TEAM
    };
    let player = source.player_faction(&rules.player_faction)?;
    if player.key.is_none() {
        report.notes.push(format!(
            "no faction {} in the records: sides rest on aggression alone",
            rules.player_faction
        ));
    }
    let mut critters: BTreeMap<String, CritterLoot> = BTreeMap::new();
    for (placement, reference, doc, inventory) in &people {
        let side = source.side(&doc.form, doc.editor_id.as_deref(), &player, rules)?;
        let (ai_packet, team) = if side.hostile {
            (hostile_packet.0, rules.hostile_team)
        } else {
            (friendly_packet.0, friendly_team)
        };
        let right = doc
            .weapons
            .first()
            .filter(|w| w.carried)
            .and_then(|w| w.form.as_ref())
            .map(ToString::to_string)
            .filter(|k| {
                made.get(k)
                    .is_some_and(|m| matches!(m.def.kind, ItemKind::Weapon(_)))
            });
        let worn = inventory
            .iter()
            .map(|(k, _)| k.to_string())
            .find(|k| made.get(k).is_some_and(|m| m.info.body));
        let mut carried: Vec<Held> = Vec::new();
        let mut dropped = 0;
        let mut skipped_right = false;
        let mut skipped_worn = false;
        for (item, n) in inventory {
            let text = item.to_string();
            let Some(m) = made.get(&text) else { continue };
            let mut n = *n;
            if Some(&text) == right.as_ref() && !skipped_right {
                skipped_right = true;
                n -= 1;
            }
            if Some(&text) == worn.as_ref() && !skipped_worn {
                skipped_worn = true;
                n -= 1;
            }
            if n <= 0 {
                continue;
            }
            if carried.len() >= loot.max_carried {
                dropped += 1;
                continue;
            }
            carried.push(stack(&text, n, m));
        }
        let worn_spec = worn
            .as_ref()
            .and_then(|k| made.get(k))
            .and_then(|m| m.info.armor.as_ref());
        let (special, skills) = source.actor_figures(&doc.form)?;
        let actor = SourceActor {
            name: placement.name.clone(),
            level: doc.level,
            health: doc.health.max(1),
            special,
            skills,
            action_points: 0,
            female: doc.female,
        };
        let fighter =
            emit::rebalance::fighter_spec(&actor, worn_spec, &rebalance.table, ai_packet, team);
        let name_of = |k: &Option<String>| {
            k.as_ref()
                .and_then(|k| made.get(k))
                .map(|m| String::from_utf8_lossy(&m.def.text.name).into_owned())
        };
        report.people.push(PersonLoot {
            name: placement.name.clone(),
            reference: reference.form.to_string(),
            side,
            ai_packet,
            team,
            weapon: name_of(&right),
            armor: name_of(&worn),
            carried: carried.len(),
            dropped,
        });
        critters.insert(
            placement.reference.clone(),
            CritterLoot {
                fighter: Some(fighter),
                ai_packet: Some(ai_packet),
                team: Some(team),
                right_hand: right.map(|k| Held::new(&k, 1)),
                left_hand: None,
                worn: worn.map(|k| Held::new(&k, 1)),
                carried,
            },
        );
    }
    report.people.sort_by(|a, b| {
        (a.name.as_str(), a.reference.as_str()).cmp(&(b.name.as_str(), b.reference.as_str()))
    });

    // Art, and the definitions.
    let placed_bases: BTreeSet<String> = placed_items.iter().map(|(_, b)| b.to_string()).collect();
    let mut items: BTreeMap<String, ItemDef> = BTreeMap::new();
    for (key, m) in made {
        let mut def = m.def;
        def.ammo = m
            .ammo
            .as_ref()
            .map(ToString::to_string)
            .filter(|_| matches!(def.kind, ItemKind::Weapon(_)));
        let label = String::from_utf8_lossy(&def.text.name).into_owned();
        let kind = def.kind.label();
        let (ground, icon) = if placed_bases.contains(&key) {
            def.ground = GroundArt::FirstPlacement;
            // The ground art is the slicer's sprite of its first placed
            // reference; the icon is rendered larger when the model allows.
            let first = placed_items
                .iter()
                .find(|(_, b)| b.to_string() == key)
                .and_then(|(r, _)| first_art.get(r.form.to_string().as_str()).copied())
                .and_then(|a| input.art.get(a));
            let (_, icon) = art.art(m.info.model.as_deref(), kind, &label);
            def.icon = icon.clone();
            (first.map(|a| (a.pixels.clone(), a.width, a.height)), icon)
        } else {
            let (sprite, icon) = art.art(m.info.model.as_deref(), kind, &label);
            let ground = (sprite.pixels.clone(), sprite.width, sprite.height);
            def.ground = GroundArt::Sprite(sprite);
            def.icon = icon.clone();
            (Some(ground), icon)
        };
        report.items.push(ItemEntry {
            key: key.clone(),
            name: label,
            kind,
            summary: m.info.summary.clone(),
            found: found.get(&key).copied().unwrap_or("?").to_owned(),
            ground,
            icon: icon.map(|i| match i {
                ItemIcon::Indexed {
                    width,
                    height,
                    pixels,
                } => (pixels, width, height),
                ItemIcon::Rgba {
                    width,
                    height,
                    rgba,
                } => (rgba, width, height),
            }),
        });
        items.insert(key, def);
    }
    let t = &art.tally;
    report.art = format!(
        "{} meshes from the extract, {} read from the install; {} renders, {} from the sprite \
         cache, {} placeholders",
        t.from_extract, t.from_install, t.rendered, t.cached, t.placeholders
    );
    for failure in t.failures.iter().take(8) {
        report.notes.push(format!("item art: {failure}"));
    }

    // Placed entries naming an item the catalogue lost are dropped.
    placed.retain(|_, loot| match loot {
        PlacedLoot::Item { item, .. } => items.contains_key(item),
        _ => true,
    });
    Ok(Built {
        catalogue: Catalogue {
            items,
            placed,
            critters,
            icon_palette: Vec::new(),
        },
        report,
    })
}

/// `n` units of an item as held: ammunition in packs (the front pack
/// partly full), anything else as it is, at most [`MAX_STACK`].
fn stack(key: &str, n: i64, made: &Made) -> Held {
    let n = n.clamp(1, i64::from(MAX_STACK)) as u32;
    if matches!(made.def.kind, ItemKind::Ammo(_)) {
        let pack = made.info.pack.max(1);
        let packs = n.div_ceil(pack).max(1);
        let front = n - (packs - 1) * pack;
        Held {
            item: key.to_owned(),
            quantity: packs,
            state: ItemState {
                rounds: (front < pack).then_some(front as i32),
                ..ItemState::default()
            },
        }
    } else {
        Held::new(key, n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibre_keys_come_from_ammunition_names() {
        let table = emit::rebalance::Rebalance::fallout3_default();
        for (name, key) in [
            ("5.56mm Round", "556mm"),
            (".32 Caliber Round", "32caliber"),
            ("10mm Round", "10mm"),
            ("Energy Cell", "energycell"),
            ("Microfusion Cell", "microfusioncell"),
            ("Mini Nuke", "mininuke"),
            ("Shotgun Shell", "shotgunshell"),
            (".44 Round", "44"),
        ] {
            assert_eq!(caliber_key(&table, Some(name), None), key, "{name}");
            assert!(table.caliber(key).is_some(), "{key}");
        }
        assert_eq!(caliber_key(&table, Some("Odd Pellets"), None), "oddpellets");
        assert_eq!(caliber_key(&table, None, Some("Ammo10mm")), "10mm");
    }

    #[test]
    fn weapon_classes_follow_the_table() {
        for game in [
            mir::GameVariant::Fallout3,
            mir::GameVariant::FalloutNewVegas,
        ] {
            let r = LoadedRebalance::built_in(game);
            let class = |names: &[&str], anim, skill| classify_weapon(&r, names, anim, skill);
            assert_eq!(
                class(&["Weap32CaliberPistol"], 3, 41).0,
                WeaponClass::Pistol
            );
            assert_eq!(
                class(&["WeapLaserPistol"], 4, 34).0,
                WeaponClass::EnergyPistol
            );
            assert_eq!(class(&["WeapHuntingRifle"], 5, 41).0, WeaponClass::Rifle);
            assert_eq!(class(&["WeapCombatShotgun"], 5, 41).0, WeaponClass::Shotgun);
            assert_eq!(class(&["Weap10mmSMG"], 6, 41).0, WeaponClass::Smg);
            assert_eq!(
                class(&["WeapAssaultRifle"], 6, 41).0,
                WeaponClass::AssaultRifle
            );
            assert_eq!(class(&["WeapMinigun"], 8, 33).0, WeaponClass::Minigun);
            assert_eq!(
                class(&["WeapGatlingLaser"], 8, 34).0,
                WeaponClass::EnergyHeavy
            );
            assert_eq!(
                class(&["WeapFlamer"], 8, 33),
                (WeaponClass::Flamer, SourceDamage::Fire)
            );
            assert_eq!(
                class(&["WeapPlasmaRifle"], 7, 34),
                (WeaponClass::EnergyRifle, SourceDamage::Plasma)
            );
            assert_eq!(class(&["WeapCombatKnife"], 1, 38).0, WeaponClass::Blade);
            assert_eq!(class(&["WeapPoolCue"], 1, 38).0, WeaponClass::Blunt);
            assert_eq!(
                class(&["WeapBaseballBat"], 2, 38).0,
                WeaponClass::HeavyMelee
            );
            assert_eq!(class(&["WeapBrassKnuckles"], 0, 45).0, WeaponClass::Unarmed);
            assert_eq!(
                class(&["WeapMissileLauncher"], 9, 33),
                (WeaponClass::Launcher, SourceDamage::Explosive)
            );
            assert_eq!(
                class(&["WeapFragGrenade"], 10, 35),
                (WeaponClass::Grenade, SourceDamage::Explosive)
            );
            // Searchlights and arcades are not electric.
            assert_eq!(
                class(&["WeapSearchlightRifle"], 5, 41).1,
                SourceDamage::Ballistic
            );
        }
    }

    #[test]
    fn names_fall_back_to_spaced_editor_ids() {
        let key = FormKey::new("a.esm", 1);
        assert_eq!(display_name(Some(" Stimpak "), Some("x"), &key), "Stimpak");
        assert_eq!(display_name(None, Some("MTTinCan"), &key), "MTTin Can");
        assert_eq!(display_name(Some(""), None, &key), "a.esm:000001");
    }

    #[test]
    fn draws_depend_only_on_their_seed() {
        let list = FormKey::new("a.esm", 0x77);
        let a: Vec<u64> = (0..8).map(|k| roll("a.esm:000100", &list, k)).collect();
        let b: Vec<u64> = (0..8).map(|k| roll("a.esm:000100", &list, k)).collect();
        let c: Vec<u64> = (0..8).map(|k| roll("a.esm:000101", &list, k)).collect();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
