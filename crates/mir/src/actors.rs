//! Actor resolution: from a placed NPC or creature base to the list of
//! files its body is made of (`docs/contracts.md`, section 10.3).
//!
//! # Templates
//!
//! An actor that names a template (`TPLT`) takes some groups of its data
//! from it, as the template flags in its `ACBS` say. The bits read here,
//! from public documentation of the record format, and checked against
//! the retail master's 2,573 templated NPCs:
//!
//! | Bit | Group | What this module takes from it | Check on the master |
//! |---|---|---|---|
//! | `0x0001` | traits | race, sex, hair, hair colour | with the bit, 13 % of NPCs store a hair, 3 % head parts; without it 85 % and 43 %; the stored sex equals the template's in 127 of 127 |
//! | `0x0002` | stats | level, base health | with the bit the stored level equals the template's in 94 %, without it in 38 % |
//! | `0x0040` | model / animation | skeleton; a creature's body models | 8 of 770 NPCs with the bit store no model, none without it |
//! | `0x0080` | base data | display name | with the bit 24 % store no name, without it 0.1 % |
//! | `0x0100` | inventory | the worn outfit | with the bit 29 % store no inventory, without it 4 % |
//!
//! The other bits (`0x0004` factions, `0x0008` actor effects, `0x0010` AI
//! data, `0x0020` AI packages, `0x0200` script) carry nothing a body needs.
//! Each group is followed on its own: from the actor, while the current
//! actor has the group's bit and a template, move to the template. The
//! actor reached supplies the group.
//!
//! # Leveled lists
//!
//! A leveled list (a template, a placed base or an inventory item)
//! resolves to its first entry at the lowest level, in file order, never
//! to a random draw; the chance of none is ignored. An item list with the
//! "use all" flag gives every entry, which is what the game does and
//! involves no draw. Nested lists are followed the same way.
//!
//! Every chain is guarded: a form met twice on one chain, a chain longer
//! than [`MAX_CHAIN`], a form outside the plugin list or a record of the
//! wrong kind ends it, and a note says so in the document.

use std::collections::{BTreeMap, BTreeSet};

use fnv_esm::{BipedData, Record};

use crate::index::{Index, Key, content_hash};
use crate::model::{
    ActorDoc, ActorKind, ActorPart, ActorTint, PartRole, RefKind, Reference, SCHEMA_VERSION,
};
use crate::paths::{FormKey, creature_model_path, normalize_mesh_path, normalize_texture_path};
use crate::source::DataSource;

/// Template flag: traits (race, sex, hair, eyes, head parts, FaceGen).
pub const USE_TRAITS: u16 = 0x0001;
/// Template flag: stats (level, health, attributes, skills).
pub const USE_STATS: u16 = 0x0002;
/// Template flag: model and animation.
pub const USE_MODEL: u16 = 0x0040;
/// Template flag: base data (name, base flags).
pub const USE_BASE_DATA: u16 = 0x0080;
/// Template flag: inventory.
pub const USE_INVENTORY: u16 = 0x0100;

/// Actor flag in `ACBS`: female.
const FLAG_FEMALE: u32 = 0x0000_0001;
/// Actor flag in `ACBS`: the level is a multiple of the player's.
const FLAG_PC_LEVEL_MULT: u32 = 0x0000_0080;

/// The longest template or leveled-list chain followed.
pub const MAX_CHAIN: usize = 32;

/// The human skeleton.
pub const HUMAN_SKELETON: &str = "meshes\\characters\\_male\\skeleton.nif";
/// The human standing idle.
pub const HUMAN_STAND: &str = "meshes\\characters\\_male\\locomotion\\mtidle.kf";
/// The male forward walk.
pub const HUMAN_WALK_MALE: &str = "meshes\\characters\\_male\\locomotion\\male\\mtforward.kf";
/// The female forward walk.
pub const HUMAN_WALK_FEMALE: &str = "meshes\\characters\\_male\\locomotion\\female\\mtforward.kf";

/// Race body part indexes and the biped slot each one fills.
const BODY_SLOTS: [(u32, u32, &str); 3] = [
    (0, BipedData::SLOT_UPPER_BODY, "upper body"),
    (1, BipedData::SLOT_LEFT_HAND, "left hand"),
    (2, BipedData::SLOT_RIGHT_HAND, "right hand"),
];

/// The actor documents of a slice, before mesh hashes are filled in.
#[derive(Debug, Default)]
pub(crate) struct ActorBuild {
    /// One per distinct placed actor base, sorted by form.
    pub docs: Vec<ActorDoc>,
    /// Every part model path, normalised.
    pub meshes: BTreeSet<String>,
    /// Every texture override path, normalised.
    pub textures: BTreeSet<String>,
}

/// A decoded record and the plugin it was read from.
type Found = (u16, Record);

struct Resolver<'i, 'a> {
    index: &'i Index<'a>,
    cache: BTreeMap<Key, Result<Found, String>>,
}

/// The notes of one actor.
#[derive(Default)]
struct Notes(Vec<String>);

impl Notes {
    fn add(&mut self, note: String) {
        if !self.0.contains(&note) {
            self.0.push(note);
        }
    }
}

/// A worn piece: its model for the actor's sex and where it came from.
struct Worn {
    path: String,
    source: Key,
}

impl<'i, 'a> Resolver<'i, 'a> {
    fn new(index: &'i Index<'a>) -> Self {
        Self {
            index,
            cache: BTreeMap::new(),
        }
    }

    /// `form (editor id)`, for notes.
    fn label(&self, key: Key) -> String {
        let form = self.index.form_key(key);
        match self.index.summary(key).and_then(|s| s.editor_id) {
            Some(id) => format!("{form} ({id})"),
            None => form.to_string(),
        }
    }

    /// The record of a form, decoded; the reason when there is none.
    fn get(&mut self, key: Key) -> Result<Found, String> {
        if let Some(found) = self.cache.get(&key) {
            return found.clone();
        }
        let found = match self.index.get(key).copied() {
            None => Err(format!(
                "{} is not in the plugin list",
                self.index.form_key(key)
            )),
            Some(entry) => match self.index.decode(&entry) {
                Ok(record) => Ok((entry.plugin, record)),
                Err(bad) => Err(format!(
                    "{} does not decode: {}",
                    self.label(key),
                    bad.error
                )),
            },
        };
        self.cache.insert(key, found.clone());
        found
    }

    /// Resolves a form id stored in a record of `plugin`.
    fn resolve(
        &self,
        plugin: u16,
        form: fnv_esm::FormId,
        what: &str,
        notes: &mut Notes,
    ) -> Option<Key> {
        if form.is_null() {
            return None;
        }
        let key = self.index.resolve(plugin, form);
        if key.is_none() {
            notes.add(format!(
                "{what} {form} names a master the plugin does not list"
            ));
        }
        key
    }

    /// The entries a leveled list gives: every one for a "use all" item
    /// list, otherwise the first at the lowest level. Forms, unresolved.
    fn pick(
        &self,
        plugin: u16,
        list: &fnv_esm::LeveledList,
        list_key: Key,
        notes: &mut Notes,
    ) -> Vec<Key> {
        let entries: Vec<&fnv_esm::LeveledEntry> =
            list.entries.iter().filter(|e| !e.form.is_null()).collect();
        let chosen: Vec<&fnv_esm::LeveledEntry> =
            if list.kind == fnv_esm::LeveledKind::Item && list.uses_all() {
                entries
            } else {
                let lowest = entries.iter().map(|e| e.level).min();
                entries
                    .into_iter()
                    .find(|e| Some(e.level) == lowest)
                    .into_iter()
                    .collect()
            };
        if chosen.is_empty() {
            notes.add(format!(
                "leveled list {} has no entry",
                self.label(list_key)
            ));
        }
        let what = format!("an entry of {}", self.label(list_key));
        chosen
            .into_iter()
            .filter_map(|e| self.resolve(plugin, e.form, &what, notes))
            .collect()
    }

    /// Follows leveled NPC and creature lists from `key` to one actor.
    /// Returns the actor and the lists passed through.
    fn through_lists(
        &mut self,
        mut key: Key,
        seen: &mut BTreeSet<Key>,
        notes: &mut Notes,
    ) -> Option<(Key, Vec<Key>)> {
        let mut via = Vec::new();
        for _ in 0..MAX_CHAIN {
            let (plugin, record) = match self.get(key) {
                Ok(found) => found,
                Err(reason) => {
                    notes.add(reason);
                    return None;
                }
            };
            match record {
                Record::Npc(_) | Record::Creature(_) => return Some((key, via)),
                Record::Leveled(list) if list.kind != fnv_esm::LeveledKind::Item => {
                    if !seen.insert(key) {
                        notes.add(format!("leveled list {} is met twice", self.label(key)));
                        return None;
                    }
                    via.push(key);
                    key = self.pick(plugin, &list, key, notes).into_iter().next()?;
                }
                other => {
                    notes.add(format!(
                        "{} is a {} record, not an actor",
                        self.label(key),
                        other.sig()
                    ));
                    return None;
                }
            }
        }
        notes.add(format!("the chain from {} is too long", self.label(key)));
        None
    }

    /// The actor that supplies the group of `flag`, starting at `start`,
    /// and the forms passed to reach it.
    fn supplier(
        &mut self,
        start: Key,
        flag: u16,
        group: &str,
        notes: &mut Notes,
    ) -> (Key, Vec<Key>) {
        let mut current = start;
        let mut chain = Vec::new();
        let mut seen = BTreeSet::from([start]);
        for _ in 0..MAX_CHAIN {
            let Ok((plugin, record)) = self.get(current) else {
                break;
            };
            let (base, template) = match &record {
                Record::Npc(n) => (n.base, n.template),
                Record::Creature(c) => (c.base, c.template),
                _ => break,
            };
            if base.map_or(0, |b| b.template_flags) & flag == 0 {
                break;
            }
            let Some(template) = template.filter(|t| !t.is_null()) else {
                notes.add(format!(
                    "{} takes its {group} from a template but names none",
                    self.label(current)
                ));
                break;
            };
            let what = format!("the template of {}", self.label(current));
            let Some(next) = self.resolve(plugin, template, &what, notes) else {
                break;
            };
            let Some((actor, via)) = self.through_lists(next, &mut seen, notes) else {
                notes.add(format!(
                    "the {group} template of {} does not resolve; its own {group} is used",
                    self.label(current)
                ));
                break;
            };
            if !seen.insert(actor) {
                notes.add(format!(
                    "the {group} templates of {} form a cycle",
                    self.label(start)
                ));
                break;
            }
            chain.extend(via);
            chain.push(actor);
            current = actor;
        }
        (current, chain)
    }

    /// Expands an inventory item to armour pieces, through leveled item
    /// lists.
    fn armour_pieces(
        &mut self,
        key: Key,
        seen: &mut BTreeSet<Key>,
        depth: usize,
        out: &mut Vec<Key>,
        notes: &mut Notes,
    ) {
        if depth > MAX_CHAIN {
            notes.add(format!(
                "the item chain from {} is too long",
                self.label(key)
            ));
            return;
        }
        let (plugin, record) = match self.get(key) {
            Ok(found) => found,
            Err(reason) => {
                notes.add(reason);
                return;
            }
        };
        match record {
            Record::Armor(_) => out.push(key),
            Record::Leveled(list) if list.kind == fnv_esm::LeveledKind::Item => {
                if !seen.insert(key) {
                    notes.add(format!("leveled list {} is met twice", self.label(key)));
                    return;
                }
                for next in self.pick(plugin, &list, key, notes) {
                    self.armour_pieces(next, seen, depth + 1, out, notes);
                }
                seen.remove(&key);
            }
            _ => {}
        }
    }

    /// The worn pieces of an inventory, armour addons included, with the
    /// slots they cover. The first piece in inventory order wins a slot.
    fn outfit(&mut self, supplier: Key, female: bool, notes: &mut Notes) -> (Vec<Worn>, u32) {
        let Ok((plugin, record)) = self.get(supplier) else {
            return (Vec::new(), 0);
        };
        let inventory = match &record {
            Record::Npc(n) => n.inventory.clone(),
            Record::Creature(c) => c.inventory.clone(),
            _ => Vec::new(),
        };
        let mut covered = 0u32;
        let mut worn = Vec::new();
        let owner = self.label(supplier);
        for item in inventory {
            let what = format!("an inventory item of {owner}");
            let Some(key) = self.resolve(plugin, item.item, &what, notes) else {
                continue;
            };
            let mut pieces = Vec::new();
            self.armour_pieces(key, &mut BTreeSet::new(), 0, &mut pieces, notes);
            for piece in pieces {
                let Ok((piece_plugin, Record::Armor(armour))) = self.get(piece) else {
                    continue;
                };
                let slots = armour.biped.map_or(0, |b| b.biped_flags);
                if slots & covered != 0 {
                    continue;
                }
                let Some(model) = armour.biped_model(female).map(normalize_mesh_path) else {
                    notes.add(format!("armour {} has no biped model", self.label(piece)));
                    continue;
                };
                covered |= slots;
                worn.push(Worn {
                    path: model,
                    source: piece,
                });
                let Some(list) = armour.biped_model_list else {
                    continue;
                };
                let what = format!("the biped model list of {}", self.label(piece));
                let Some(list_key) = self.resolve(piece_plugin, list, &what, notes) else {
                    continue;
                };
                let addons = match self.get(list_key) {
                    Ok((list_plugin, Record::FormList(list))) => list
                        .forms
                        .iter()
                        .filter_map(|&f| {
                            let what = format!("an entry of {}", self.label(list_key));
                            self.resolve(list_plugin, f, &what, notes)
                        })
                        .collect::<Vec<Key>>(),
                    Ok((_, other)) => {
                        notes.add(format!(
                            "the biped model list of {} is a {} record",
                            self.label(piece),
                            other.sig()
                        ));
                        Vec::new()
                    }
                    Err(reason) => {
                        notes.add(reason);
                        Vec::new()
                    }
                };
                for addon_key in addons {
                    let addon = match self.get(addon_key) {
                        Ok((_, Record::ArmorAddon(addon))) => addon,
                        Ok((_, other)) => {
                            notes.add(format!(
                                "{} in a biped model list is a {} record",
                                self.label(addon_key),
                                other.sig()
                            ));
                            continue;
                        }
                        Err(reason) => {
                            notes.add(reason);
                            continue;
                        }
                    };
                    let slots = addon.biped.map_or(0, |b| b.biped_flags);
                    if slots & covered != 0 {
                        continue;
                    }
                    let Some(model) = addon.biped_model(female).map(normalize_mesh_path) else {
                        notes.add(format!(
                            "armour addon {} has no biped model",
                            self.label(addon_key)
                        ));
                        continue;
                    };
                    covered |= slots;
                    worn.push(Worn {
                        path: model,
                        source: addon_key,
                    });
                }
            }
        }
        (worn, covered)
    }

    fn npc(&mut self, actor: Key, doc: &mut ActorDoc, notes: &mut Notes) {
        let index = self.index;
        let mut chains = BTreeMap::new();
        let mut follow = |me: &mut Self, flag: u16, group: &str, notes: &mut Notes| {
            let (supplier, chain) = me.supplier(actor, flag, group, notes);
            if !chain.is_empty() {
                chains.insert(
                    group.to_owned(),
                    chain.iter().map(|&k| index.form_key(k)).collect::<Vec<_>>(),
                );
            }
            supplier
        };
        let traits = follow(self, USE_TRAITS, "traits", notes);
        let stats = follow(self, USE_STATS, "stats", notes);
        let model = follow(self, USE_MODEL, "model", notes);
        let base_data = follow(self, USE_BASE_DATA, "base_data", notes);
        let inventory = follow(self, USE_INVENTORY, "inventory", notes);
        doc.template_chains.extend(chains);

        let npc_of = |me: &mut Self, key: Key| match me.get(key) {
            Ok((plugin, Record::Npc(n))) => Some((plugin, n)),
            _ => None,
        };
        if let Some((_, n)) = npc_of(self, stats) {
            doc.health = n.stats.map_or(0, |s| s.base_health);
            doc.level = level(n.base);
        }
        if let Some((_, n)) = npc_of(self, base_data) {
            doc.name = n.common.full_name.clone();
        }
        let skeleton = npc_of(self, model)
            .and_then(|(_, n)| n.common.model.as_deref().map(normalize_mesh_path))
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| HUMAN_SKELETON.to_owned());
        doc.skeleton_path = Some(skeleton);

        let Some((plugin, traits_npc)) = npc_of(self, traits) else {
            return;
        };
        let female = traits_npc.base.is_some_and(|b| b.flags & FLAG_FEMALE != 0);
        doc.female = female;
        doc.tint = ActorTint {
            hair: traits_npc.hair_color.map(|c| [c.r, c.g, c.b]),
        };
        let (worn, covered) = self.outfit(inventory, female, notes);
        let mut parts = Vec::new();

        // The race: body parts not covered, and the head.
        let race = traits_npc.race.and_then(|r| {
            self.resolve(
                plugin,
                r,
                &format!("the race of {}", self.label(traits)),
                notes,
            )
        });
        match race.map(|r| (r, self.get(r))) {
            None => notes.add(format!("{} has no race", self.label(traits))),
            Some((_, Err(reason))) => notes.add(reason),
            Some((r, Ok((_, Record::Race(race_record))))) => {
                doc.race = Some(index.form_key(r));
                let sex = if female { "female" } else { "male" };
                let body = race_record.body_parts(female);
                for (part_index, slot, name) in BODY_SLOTS {
                    if covered & slot != 0 {
                        continue;
                    }
                    let found = body
                        .iter()
                        .find(|p| p.index == part_index)
                        .and_then(|p| Some((normalize_mesh_path(p.model.as_deref()?), p)))
                        .filter(|(m, _)| !m.is_empty());
                    match found {
                        Some((path, part)) => parts.push(ActorPart {
                            role: PartRole::Body,
                            path,
                            mesh: None,
                            texture: texture(part.texture.as_deref()),
                            source: Some(index.form_key(r)),
                        }),
                        None => {
                            notes.add(format!("race {} has no {sex} {name} model", self.label(r)))
                        }
                    }
                }
                if covered & BipedData::SLOT_HEAD == 0 {
                    let head = race_record
                        .head_parts(female)
                        .iter()
                        .find(|p| p.index == 0)
                        .and_then(|p| Some((normalize_mesh_path(p.model.as_deref()?), p)))
                        .filter(|(m, _)| !m.is_empty());
                    match head {
                        Some((path, part)) => parts.push(ActorPart {
                            role: PartRole::Head,
                            path,
                            mesh: None,
                            texture: texture(part.texture.as_deref()),
                            source: Some(index.form_key(r)),
                        }),
                        None => {
                            notes.add(format!("race {} has no {sex} head model", self.label(r)))
                        }
                    }
                }
            }
            Some((r, Ok((_, other)))) => notes.add(format!(
                "the race {} is a {} record",
                self.label(r),
                other.sig()
            )),
        }

        for piece in worn {
            parts.push(ActorPart {
                role: PartRole::Outfit,
                path: piece.path,
                mesh: None,
                texture: None,
                source: Some(index.form_key(piece.source)),
            });
        }

        // The hair, unless the outfit covers it.
        if covered & BipedData::SLOT_HAIR == 0
            && let Some(hair) = traits_npc.hair
        {
            let what = format!("the hair of {}", self.label(traits));
            if let Some(h) = self.resolve(plugin, hair, &what, notes) {
                match self.get(h) {
                    Ok((_, Record::Hair(hair_record))) => {
                        match hair_record
                            .common
                            .model
                            .as_deref()
                            .map(normalize_mesh_path)
                            .filter(|m| !m.is_empty())
                        {
                            Some(path) => parts.push(ActorPart {
                                role: PartRole::Hair,
                                path,
                                mesh: None,
                                texture: None,
                                source: Some(index.form_key(h)),
                            }),
                            None => notes.add(format!("hair {} has no model", self.label(h))),
                        }
                    }
                    Ok((_, other)) => notes.add(format!(
                        "the hair {} is a {} record",
                        self.label(h),
                        other.sig()
                    )),
                    Err(reason) => notes.add(reason),
                }
            }
        }
        doc.parts = parts;
    }

    fn creature(&mut self, actor: Key, doc: &mut ActorDoc, notes: &mut Notes) {
        let index = self.index;
        let mut chains = BTreeMap::new();
        let mut follow = |me: &mut Self, flag: u16, group: &str, notes: &mut Notes| {
            let (supplier, chain) = me.supplier(actor, flag, group, notes);
            if !chain.is_empty() {
                chains.insert(
                    group.to_owned(),
                    chain.iter().map(|&k| index.form_key(k)).collect::<Vec<_>>(),
                );
            }
            supplier
        };
        let traits = follow(self, USE_TRAITS, "traits", notes);
        let stats = follow(self, USE_STATS, "stats", notes);
        let model = follow(self, USE_MODEL, "model", notes);
        let base_data = follow(self, USE_BASE_DATA, "base_data", notes);
        doc.template_chains.extend(chains);

        let creature_of = |me: &mut Self, key: Key| match me.get(key) {
            Ok((_, Record::Creature(c))) => Some(c),
            _ => None,
        };
        if let Some(c) = creature_of(self, traits) {
            doc.female = c.base.is_some_and(|b| b.flags & FLAG_FEMALE != 0);
        }
        if let Some(c) = creature_of(self, stats) {
            doc.health = c.data.map_or(0, |d| i32::from(d.health));
            doc.level = level(c.base);
        }
        if let Some(c) = creature_of(self, base_data) {
            doc.name = c.common.full_name.clone();
        }
        let Some(c) = creature_of(self, model) else {
            return;
        };
        let skeleton = c
            .common
            .model
            .as_deref()
            .map(normalize_mesh_path)
            .filter(|m| !m.is_empty());
        match &skeleton {
            Some(s) => doc.skeleton_path = Some(s.clone()),
            None => notes.add(format!("creature {} has no skeleton", self.label(model))),
        }
        let skeleton = skeleton.unwrap_or_default();
        for name in &c.model_list {
            let path = creature_model_path(&skeleton, name);
            if path.is_empty() {
                continue;
            }
            doc.parts.push(ActorPart {
                role: PartRole::Body,
                path,
                mesh: None,
                texture: None,
                source: Some(index.form_key(model)),
            });
        }
        if doc.parts.is_empty() {
            notes.add(format!("creature {} has no body model", self.label(model)));
        }
    }

    /// The document of one placed base.
    fn actor(&mut self, base: Key, kind: RefKind, source: &DataSource) -> ActorDoc {
        let index = self.index;
        let mut notes = Notes::default();
        let mut doc = ActorDoc {
            schema_version: SCHEMA_VERSION,
            form: index.form_key(base),
            editor_id: index.summary(base).and_then(|s| s.editor_id),
            name: None,
            kind: if kind == RefKind::Creature {
                ActorKind::Creature
            } else {
                ActorKind::Npc
            },
            female: false,
            race: None,
            skeleton: None,
            skeleton_path: None,
            parts: Vec::new(),
            animations: None,
            stand_path: None,
            walk_path: None,
            tint: ActorTint::default(),
            appearance: String::new(),
            health: 0,
            level: 0,
            template_chains: BTreeMap::new(),
            notes: Vec::new(),
        };
        let resolved = self.through_lists(base, &mut BTreeSet::new(), &mut notes);
        if let Some((actor, via)) = resolved {
            if !via.is_empty() {
                let mut chain: Vec<FormKey> = via.iter().map(|&k| index.form_key(k)).collect();
                chain.push(index.form_key(actor));
                doc.template_chains.insert("base".to_owned(), chain);
            }
            match self.get(actor) {
                Ok((_, Record::Npc(_))) => {
                    doc.kind = ActorKind::Npc;
                    self.npc(actor, &mut doc, &mut notes);
                }
                Ok((_, Record::Creature(_))) => {
                    doc.kind = ActorKind::Creature;
                    self.creature(actor, &mut doc, &mut notes);
                }
                _ => {}
            }
        } else {
            notes.add(format!(
                "the placed base {} does not resolve to an actor",
                self.label(base)
            ));
        }
        if doc.kind == ActorKind::Npc {
            if let Some(skeleton) = &doc.skeleton_path
                && !source.contains(skeleton)
            {
                notes.add(format!("skeleton {skeleton} is not in the data"));
            }
            if source.contains(HUMAN_STAND) {
                doc.stand_path = Some(HUMAN_STAND.to_owned());
            } else {
                notes.add(format!("standing idle {HUMAN_STAND} is not in the data"));
            }
            let walk = if doc.female && source.contains(HUMAN_WALK_FEMALE) {
                Some(HUMAN_WALK_FEMALE)
            } else if source.contains(HUMAN_WALK_MALE) {
                Some(HUMAN_WALK_MALE)
            } else {
                None
            };
            match walk {
                Some(w) => doc.walk_path = Some(w.to_owned()),
                None => notes.add(format!("walk {HUMAN_WALK_MALE} is not in the data")),
            }
        }
        doc.parts
            .sort_by(|a, b| (a.role, &a.path).cmp(&(b.role, &b.path)));
        doc.parts.dedup();
        doc.notes = notes.0;
        doc
    }
}

/// A race texture path as an override, normalised.
fn texture(raw: Option<&str>) -> Option<String> {
    raw.map(normalize_texture_path).filter(|t| !t.is_empty())
}

/// The level of an actor: the stored level, or the stored minimum when the
/// level is relative to the player's.
fn level(base: Option<fnv_esm::ActorBase>) -> i32 {
    match base {
        Some(b) if b.flags & FLAG_PC_LEVEL_MULT != 0 => i32::from(b.calc_min).max(1),
        Some(b) => i32::from(b.level),
        None => 0,
    }
}

/// Resolves every distinct actor base the references place.
pub(crate) fn build<'r>(
    index: &Index<'_>,
    source: &DataSource,
    references: impl Iterator<Item = &'r Reference>,
) -> ActorBuild {
    let mut bases: BTreeMap<FormKey, RefKind> = BTreeMap::new();
    for r in references {
        if matches!(r.kind, RefKind::Npc | RefKind::Creature) {
            bases.entry(r.base.clone()).or_insert(r.kind);
        }
    }
    let mut resolver = Resolver::new(index);
    let mut out = ActorBuild::default();
    for (form, kind) in bases {
        let Some(key) = index.key(&form) else {
            continue;
        };
        let doc = resolver.actor(key, kind, source);
        for part in &doc.parts {
            out.meshes.insert(part.path.clone());
            out.textures.extend(part.texture.clone());
        }
        out.docs.push(doc);
    }
    out
}

/// The appearance key of a document whose part hashes are filled in.
pub(crate) fn appearance(doc: &ActorDoc, texture_hash: &dyn Fn(&str) -> Option<String>) -> String {
    let mut lines: Vec<String> = doc
        .parts
        .iter()
        .map(|p| {
            let mesh = p.mesh.clone().unwrap_or_else(|| format!("path:{}", p.path));
            let texture = match &p.texture {
                Some(t) => texture_hash(t).unwrap_or_else(|| format!("path:{t}")),
                None => "-".to_owned(),
            };
            format!("part {:?} {mesh} {texture}", p.role)
        })
        .collect();
    lines.sort();
    lines.push(format!("female {}", doc.female));
    lines.push(match doc.tint.hair {
        Some([r, g, b]) => format!("hair {r:02x}{g:02x}{b:02x}"),
        None => "hair -".to_owned(),
    });
    content_hash(lines.join("\n").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::PluginFile;
    use fnv_esm::fixture::PluginBuilder;
    use fnv_esm::{
        ActorBase, Armor, BipedData, ContainerItem, FormId, LeveledEntry, LeveledKind, LeveledList,
        Npc,
    };

    const PLUGIN: &str = "test.esm";

    fn npc(form: u32, template: Option<u32>, flags: u16) -> Npc {
        let mut n = Npc::new(form, &format!("N{form:x}"), "N");
        n.template = template.map(FormId);
        n.base = Some(ActorBase {
            template_flags: flags,
            ..ActorBase::default()
        });
        n
    }

    fn armour(form: u32, slots: u32) -> Armor {
        Armor {
            common: fnv_esm::Common::new(form)
                .with_editor_id(&format!("A{form:x}"))
                .with_model(&format!("a{form:x}.nif")),
            biped: Some(BipedData {
                biped_flags: slots,
                general_flags: 0,
            }),
            ..Armor::default()
        }
    }

    fn resolve(plugin: &PluginBuilder, base: u32) -> ActorDoc {
        let bytes = plugin.build();
        let files = vec![PluginFile {
            name: PLUGIN.to_owned(),
            sha256: content_hash(&bytes),
            bytes,
        }];
        let index = Index::build(&files).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let source = DataSource::new(dir.path(), &[PLUGIN.to_owned()]).unwrap();
        let key = index.key(&FormKey::new(PLUGIN, base)).unwrap();
        Resolver::new(&index).actor(key, RefKind::Npc, &source)
    }

    #[test]
    fn a_template_cycle_ends_with_a_note() {
        let mut plugin = PluginBuilder::new();
        plugin
            .add(npc(0x10, Some(0x11), USE_BASE_DATA))
            .add(npc(0x11, Some(0x10), USE_BASE_DATA));
        let doc = resolve(&plugin, 0x10);
        assert_eq!(
            doc.template_chains["base_data"],
            [FormKey::new(PLUGIN, 0x11)]
        );
        assert!(
            doc.notes.iter().any(|n| n.contains("cycle")),
            "{:?}",
            doc.notes
        );
    }

    #[test]
    fn a_missing_template_keeps_the_actors_own_data() {
        let mut plugin = PluginBuilder::new();
        let mut actor = npc(0x10, Some(0x99), USE_STATS);
        actor.stats = Some(fnv_esm::NpcStats {
            base_health: 7,
            attributes: [1; 7],
        });
        plugin.add(actor);
        let doc = resolve(&plugin, 0x10);
        assert_eq!(doc.health, 7);
        assert!(doc.template_chains.is_empty());
        assert!(
            doc.notes.iter().any(|n| n.contains("test.esm:000099")),
            "{:?}",
            doc.notes
        );
        // A template flag with no template is noted too.
        let mut plugin = PluginBuilder::new();
        plugin.add(npc(0x10, None, USE_TRAITS));
        let doc = resolve(&plugin, 0x10);
        assert!(doc.notes.iter().any(|n| n.contains("names none")));
    }

    #[test]
    fn leveled_item_lists_pick_the_lowest_level_or_give_all() {
        let mut plugin = PluginBuilder::new();
        let mut actor = npc(0x10, None, 0);
        actor.inventory = vec![
            ContainerItem::new(0x20, 1),
            ContainerItem::new(0x21, 1),
            ContainerItem::new(0x22, 1),
        ];
        plugin.add(actor);
        // One entry: the first at the lowest level, not the first in file.
        plugin.add(LeveledList::new(
            LeveledKind::Item,
            0x20,
            "Pick",
            vec![
                LeveledEntry::new(4, 0x30u32, 1),
                LeveledEntry::new(2, 0x31u32, 1),
                LeveledEntry::new(2, 0x32u32, 1),
            ],
        ));
        // Every entry, through a nested list.
        let mut all = LeveledList::new(
            LeveledKind::Item,
            0x21,
            "All",
            vec![
                LeveledEntry::new(9, 0x33u32, 1),
                LeveledEntry::new(1, 0x23u32, 1),
            ],
        );
        all.flags = Some(LeveledList::FLAG_USE_ALL);
        plugin.add(all);
        plugin.add(LeveledList::new(
            LeveledKind::Item,
            0x23,
            "Nested",
            vec![LeveledEntry::new(1, 0x34u32, 1)],
        ));
        // A list that contains itself.
        plugin.add(LeveledList::new(
            LeveledKind::Item,
            0x22,
            "Loop",
            vec![LeveledEntry::new(1, 0x22u32, 1)],
        ));
        for (form, slot) in [
            (0x30, 0x1000),
            (0x31, 0x2000),
            (0x32, 0x4000),
            (0x33, 0x8000),
            (0x34, 0x1_0000),
        ] {
            plugin.add(armour(form, slot));
        }
        let doc = resolve(&plugin, 0x10);
        let outfit: Vec<&str> = doc
            .parts
            .iter()
            .filter(|p| p.role == PartRole::Outfit)
            .map(|p| p.path.as_str())
            .collect();
        assert_eq!(
            outfit,
            ["meshes\\a31.nif", "meshes\\a33.nif", "meshes\\a34.nif"]
        );
        assert!(
            doc.notes.iter().any(|n| n.contains("met twice")),
            "{:?}",
            doc.notes
        );
    }

    #[test]
    fn a_placed_leveled_list_resolves_to_its_actor() {
        let mut plugin = PluginBuilder::new();
        plugin.add(npc(0x10, None, 0)).add(LeveledList::new(
            LeveledKind::Npc,
            0x40,
            "Placed",
            vec![LeveledEntry::new(1, 0x10u32, 1)],
        ));
        let doc = resolve(&plugin, 0x40);
        assert_eq!(doc.kind, ActorKind::Npc);
        assert_eq!(
            doc.template_chains["base"],
            [FormKey::new(PLUGIN, 0x40), FormKey::new(PLUGIN, 0x10)]
        );
        // No race: noted. No data source files: skeleton and animations noted.
        assert!(doc.notes.iter().any(|n| n.contains("has no race")));
        assert!(doc.notes.iter().any(|n| n.contains("skeleton")));
    }

    #[test]
    fn appearance_follows_parts_tint_and_sex() {
        let mut plugin = PluginBuilder::new();
        plugin.add(npc(0x10, None, 0));
        let a = resolve(&plugin, 0x10);
        let none = |_: &str| None;
        let base = appearance(&a, &none);
        let mut b = a.clone();
        b.editor_id = Some("Other".to_owned());
        b.health = 99;
        assert_eq!(appearance(&b, &none), base);
        b.female = true;
        assert_ne!(appearance(&b, &none), base);
        let mut c = a.clone();
        c.tint.hair = Some([1, 2, 3]);
        assert_ne!(appearance(&c, &none), base);
    }
}
