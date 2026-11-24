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
//! # Skeletons
//!
//! An NPC's skeleton is the model of the actor that supplies its model
//! group, but only when that file is a character skeleton: in the data,
//! decodable, with the humanoid root, a hip bone and the head bone. An
//! editor marker, a missing file or a skeleton without those bones gives
//! way to the race's skeleton (`skeleton.nif` beside the race's upper body
//! model for the actor's sex), then to [`HUMAN_SKELETON`], and a note says
//! so. The retail masters store the creature marker as the model of NPCs
//! whose template chain reaches a leveled NPC list, with or without the
//! model flag.
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
    ActorDoc, ActorKind, ActorPart, ActorTint, ActorWeapon, PartRole, RefKind, Reference,
    SCHEMA_VERSION,
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

/// The human skeleton, shared by both sexes: the last resort of every NPC.
pub const HUMAN_SKELETON: &str = "meshes\\characters\\_male\\skeleton.nif";

/// The file name every race's skeleton has, in the folder of the race's
/// upper body model.
const RACE_SKELETON_NAME: &str = "skeleton.nif";

/// The start of the file name of the editor's marker meshes (such as
/// `marker_creature.nif`), which are never a skeleton.
const MARKER_PREFIX: &str = "marker";

/// The humanoid root bone every character skeleton has.
const SKELETON_ROOT: &str = "Bip01";
/// The hip bones, one of which every character skeleton has.
const SKELETON_HIPS: [&str; 2] = ["Bip01 Pelvis", "Bip01 Spine"];
/// The head bone every character skeleton has; hats and hair hang on it.
const SKELETON_HEAD: &str = "Bip01 Head";
/// The human standing idle.
pub const HUMAN_STAND: &str = "meshes\\characters\\_male\\locomotion\\mtidle.kf";
/// The male forward walk.
pub const HUMAN_WALK_MALE: &str = "meshes\\characters\\_male\\locomotion\\male\\mtforward.kf";
/// The female forward walk.
pub const HUMAN_WALK_FEMALE: &str = "meshes\\characters\\_male\\locomotion\\female\\mtforward.kf";

/// The folder of every human animation.
pub const HUMAN_ANIMATIONS: &str = "meshes\\characters\\_male\\";

/// The weapon families that get armed animation slots for every human
/// whatever it carries: a pistol and a rifle, the default armed sets.
pub const DEFAULT_FAMILIES: [&str; 2] = ["1hp", "2hr"];

/// Default weapon models per family, tried in order; the first found in
/// the data is shown when an actor carries no weapon of the family. The
/// paths exist in both games (checked on the owner's installs).
pub const DEFAULT_WEAPONS: [(&str, u32, &[&str]); 2] = [
    ("1hp", 3, &["meshes\\weapons\\1handpistol\\10mmpistol.nif"]),
    (
        "2hr",
        5,
        &[
            "meshes\\weapons\\2handrifle\\huntingrifle.nif",
            "meshes\\weapons\\2handrifle\\varmintrifle.nif",
        ],
    ),
];

/// The source's animation family of a weapon animation type (the first
/// word of a weapon's `DNAM`): the prefix of its animation files. Public
/// documentation of the record and the file names agree on the codes: 0
/// hand to hand, 1 one-hand melee, 2 two-hand melee, 3 one-hand pistol,
/// 4 one-hand energy pistol, 5 two-hand rifle, 6 two-hand automatic, 7
/// two-hand energy rifle, 8 two-hand heavy ("handle"), 9 two-hand
/// launcher, 10 grenade throw, 11 land mine, 12 mine drop, 13 thrown.
pub fn weapon_family(animation_type: u32) -> Option<&'static str> {
    Some(match animation_type {
        0 => "h2h",
        1 => "1hm",
        2 => "2hm",
        3 | 4 => "1hp",
        5 | 7 => "2hr",
        6 => "2ha",
        8 => "2hh",
        9 => "2hl",
        10 | 13 => "1gt",
        11 => "1lm",
        12 => "1md",
        _ => return None,
    })
}

/// Families with aim, attack and equip animations: the ones a critter
/// can be drawn holding. Thrown weapons and mines are left out.
pub const ARMED_FAMILIES: [&str; 7] = ["1hm", "2hm", "1hp", "2ha", "2hr", "2hh", "2hl"];

/// The animation slots a human document resolves beyond the stand and the
/// walk, with the files tried for each, in order, relative to
/// [`HUMAN_ANIMATIONS`]; the first one in the data wins.
///
/// Both games name these files alike, so one table serves both; what
/// differs is which files exist (surveyed on the owner's installs with
/// `cargo run -p nif --example animations`):
///
/// | Slot | First choice | Fallout 3 | New Vegas |
/// |---|---|---|---|
/// | `run` | `locomotion\<sex>\mtfastforward.kf` | yes | yes |
/// | `dodge` | `mtblockhit.kf` (a block: neither game has a dodge) | yes | yes |
/// | `hit_front` | `idleanims\mt_hittorso.kf` | yes | yes |
/// | `punch` | `h2hattackright_a.kf` | yes | yes |
/// | `kick` | `h2hattackpower.kf` (a power blow: neither game has a kick) | yes | yes |
/// | `fall_back`, `fall_front` | `idleanims\mt_knockdownfaceup.kf`, `...facedown.kf` | yes | yes |
/// | `reach_ground`, `reach_middle` | `idleanims\pickupgroundobject_0.3.kf`, `pickuptableobject_0.6.kf` | yes | yes |
/// | `<f>.stand` | `<f>aim.kf` | every family | every family |
/// | `<f>.walk` | `locomotion\<f>forward.kf` | `1hm`, `1hp` only | every family but `2hm` |
/// | `<f>.equip`, `<f>.unequip` | `<f>equip.kf`, `<f>unequip.kf` | yes | yes |
/// | `<f>.attack` | `<f>attackright.kf` | yes | yes |
/// | `<f>.loop` | `<f>attackloop.kf` (automatic fire) | `1hp`, `2ha`, `2hh` | the same |
/// | `<f>.block` | `<f>blockhit.kf` | melee families | melee families |
///
/// Neither game has a hit from behind, a human get-up, or a third-person
/// death animation of its own (deaths are physics there; `deathpose1.kf`
/// and `deathpose2.kf` are 0.17 s poses, and the per-class `<f>death.kf`
/// files 0.07 s ones): those are left to the renderer (ragdoll falls,
/// reversed falls for the get-ups).
pub fn human_animation_slots(female: bool, families: &[&str]) -> Vec<(String, Vec<String>)> {
    let sex = if female { "female" } else { "male" };
    let mut slots: Vec<(String, Vec<String>)> = vec![
        (
            "run".into(),
            vec![
                format!("locomotion\\{sex}\\mtfastforward.kf"),
                "locomotion\\male\\mtfastforward.kf".into(),
            ],
        ),
        (
            "dodge".into(),
            vec!["mtblockhit.kf".into(), "h2hblockhit.kf".into()],
        ),
        (
            "hit_front".into(),
            vec![
                "idleanims\\mt_hittorso.kf".into(),
                "idleanims\\mt_hithead.kf".into(),
                "h2hrecoil.kf".into(),
            ],
        ),
        (
            "punch".into(),
            vec![
                "h2hattackright_a.kf".into(),
                "h2hattackleft_a.kf".into(),
                "h2hattackpower.kf".into(),
            ],
        ),
        ("kick".into(), vec!["h2hattackpower.kf".into()]),
        (
            "fall_back".into(),
            vec!["idleanims\\mt_knockdownfaceup.kf".into()],
        ),
        (
            "fall_front".into(),
            vec!["idleanims\\mt_knockdownfacedown.kf".into()],
        ),
        (
            "reach_ground".into(),
            vec!["idleanims\\pickupgroundobject_0.3.kf".into()],
        ),
        (
            "reach_middle".into(),
            vec!["idleanims\\pickuptableobject_0.6.kf".into()],
        ),
    ];
    let mut seen = BTreeSet::new();
    for family in families {
        if !ARMED_FAMILIES.contains(family) || !seen.insert(*family) {
            continue;
        }
        let f = family;
        slots.extend([
            (format!("{f}.stand"), vec![format!("{f}aim.kf")]),
            (
                format!("{f}.walk"),
                vec![format!("locomotion\\{f}forward.kf")],
            ),
            (format!("{f}.equip"), vec![format!("{f}equip.kf")]),
            (format!("{f}.unequip"), vec![format!("{f}unequip.kf")]),
            (
                format!("{f}.attack"),
                vec![
                    format!("{f}attackright.kf"),
                    format!("{f}attackleft.kf"),
                    format!("{f}attack3.kf"),
                    format!("{f}attackright_a.kf"),
                    format!("{f}attackpower.kf"),
                ],
            ),
            (format!("{f}.loop"), vec![format!("{f}attackloop.kf")]),
            (format!("{f}.block"), vec![format!("{f}blockhit.kf")]),
        ]);
    }
    for (_, candidates) in &mut slots {
        for path in candidates.iter_mut() {
            *path = format!("{HUMAN_ANIMATIONS}{path}");
        }
    }
    slots
}

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
    /// Skeleton paths checked: why each is not a character skeleton, or
    /// `None` when it is one.
    skeletons: BTreeMap<String, Option<String>>,
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
            skeletons: BTreeMap::new(),
        }
    }

    /// Why `path` is not a character skeleton, if it is not
    /// ([`skeleton_problem`]); each path is read once.
    fn skeleton_problem(&mut self, path: &str, source: &DataSource) -> Option<String> {
        if let Some(problem) = self.skeletons.get(path) {
            return problem.clone();
        }
        let problem = skeleton_problem(path, source);
        self.skeletons.insert(path.to_owned(), problem.clone());
        problem
    }

    /// The skeleton of an NPC: the model of its model supplier `model`
    /// (`own`, normalised) when that is a character skeleton, otherwise
    /// the race's skeleton (`race`), otherwise [`HUMAN_SKELETON`]. The
    /// first of them that is a character skeleton is taken, and
    /// [`HUMAN_SKELETON`] when none is (a broken data set; the extract
    /// reports the file). A note says when the own model was passed over.
    ///
    /// The retail masters of both games give the editor's creature marker
    /// as the model of most NPCs whose template chain (followed whatever
    /// the template flags) reaches a leveled NPC list, 326 of 380 in one
    /// and 638 of 719 in the other, and of no other NPC: the body comes
    /// from whichever actor the list gives. An NPC without the model flag
    /// keeps that marker as its own model, so it must not be taken for a
    /// skeleton.
    fn npc_skeleton(
        &mut self,
        model: Key,
        own: Option<String>,
        race: Option<String>,
        source: &DataSource,
        notes: &mut Notes,
    ) -> String {
        let mut candidates: Vec<(String, &str)> = Vec::new();
        for (path, what) in [
            (own.clone(), "its own model"),
            (race, "the race's skeleton"),
            (Some(HUMAN_SKELETON.to_owned()), "the human skeleton"),
        ] {
            if let Some(path) = path
                && candidates.iter().all(|(p, _)| *p != path)
            {
                candidates.push((path, what));
            }
        }
        let mut own_problem = None;
        let mut chosen = None;
        for (path, what) in candidates {
            match self.skeleton_problem(&path, source) {
                None => {
                    chosen = Some((path, what));
                    break;
                }
                Some(problem) if Some(&path) == own.as_ref() => own_problem = Some(problem),
                Some(_) => {}
            }
        }
        let (path, what) = chosen.unwrap_or((HUMAN_SKELETON.to_owned(), "the human skeleton"));
        if Some(&path) != own.as_ref() {
            let label = self.label(model);
            notes.add(match (&own, own_problem) {
                (Some(model_path), Some(problem)) => format!(
                    "the model {model_path} of {label} {problem}, not a character skeleton; \
                     {what} {path} is used"
                ),
                _ => format!("{label} names no model; {what} {path} is used"),
            });
        }
        path
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

    /// Expands an inventory item to weapons, through leveled item lists.
    fn weapon_items(
        &mut self,
        key: Key,
        seen: &mut BTreeSet<Key>,
        depth: usize,
        out: &mut Vec<Key>,
        notes: &mut Notes,
    ) {
        if depth > MAX_CHAIN {
            return;
        }
        let Ok((plugin, record)) = self.get(key) else {
            return;
        };
        match record {
            Record::Weapon(_) => out.push(key),
            Record::Leveled(list) if list.kind == fnv_esm::LeveledKind::Item => {
                if !seen.insert(key) {
                    return;
                }
                for next in self.pick(plugin, &list, key, notes) {
                    self.weapon_items(next, seen, depth + 1, out, notes);
                }
                seen.remove(&key);
            }
            _ => {}
        }
    }

    /// The first weapon of an inventory, in inventory order, through
    /// leveled item lists by the same rule as the outfit: what the actor
    /// is drawn holding.
    fn carried_weapon(&mut self, supplier: Key, notes: &mut Notes) -> Option<ActorWeapon> {
        let (plugin, record) = self.get(supplier).ok()?;
        let inventory = match &record {
            Record::Npc(n) => n.inventory.clone(),
            _ => return None,
        };
        let owner = self.label(supplier);
        for item in inventory {
            let what = format!("an inventory item of {owner}");
            let Some(key) = self.resolve(plugin, item.item, &what, notes) else {
                continue;
            };
            let mut found = Vec::new();
            self.weapon_items(key, &mut BTreeSet::new(), 0, &mut found, notes);
            for weapon_key in found {
                let Ok((_, Record::Weapon(weapon))) = self.get(weapon_key) else {
                    continue;
                };
                let Some(path) = weapon
                    .common
                    .model
                    .as_deref()
                    .map(normalize_mesh_path)
                    .filter(|m| !m.is_empty())
                else {
                    notes.add(format!("weapon {} has no model", self.label(weapon_key)));
                    continue;
                };
                let animation_type = weapon.combat().map_or(0, |c| c.animation_type);
                return Some(ActorWeapon {
                    form: Some(self.index.form_key(weapon_key)),
                    editor_id: weapon.common.editor_id.clone(),
                    path,
                    mesh: None,
                    animation_type,
                    family: weapon_family(animation_type).unwrap_or("").to_owned(),
                    carried: true,
                });
            }
        }
        None
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

    fn npc(&mut self, actor: Key, source: &DataSource, doc: &mut ActorDoc, notes: &mut Notes) {
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
        let own_skeleton = npc_of(self, model)
            .and_then(|(_, n)| n.common.model.as_deref().map(normalize_mesh_path))
            .filter(|m| !m.is_empty());

        let Some((plugin, traits_npc)) = npc_of(self, traits) else {
            doc.skeleton_path = Some(self.npc_skeleton(model, own_skeleton, None, source, notes));
            return;
        };
        let female = traits_npc.base.is_some_and(|b| b.flags & FLAG_FEMALE != 0);
        doc.female = female;
        doc.tint = ActorTint {
            hair: traits_npc.hair_color.map(|c| [c.r, c.g, c.b]),
        };
        let (worn, covered) = self.outfit(inventory, female, notes);
        doc.weapons.extend(self.carried_weapon(inventory, notes));
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
        let mut race_skeleton = None;
        match race.map(|r| (r, self.get(r))) {
            None => notes.add(format!("{} has no race", self.label(traits))),
            Some((_, Err(reason))) => notes.add(reason),
            Some((r, Ok((_, Record::Race(race_record))))) => {
                doc.race = Some(index.form_key(r));
                let sex = if female { "female" } else { "male" };
                let body = race_record.body_parts(female);
                race_skeleton = body
                    .iter()
                    .find(|p| p.index == 0)
                    .and_then(|p| p.model.as_deref())
                    .and_then(|m| skeleton_beside(&normalize_mesh_path(m)));
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
        doc.skeleton_path =
            Some(self.npc_skeleton(model, own_skeleton, race_skeleton, source, notes));
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
            animation_paths: BTreeMap::new(),
            weapons: Vec::new(),
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
                    self.npc(actor, source, &mut doc, &mut notes);
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
            // Further animations: the fixed slots, the carried weapon's
            // family and the default ones. A slot the data lacks is left
            // out without a note; the renderer reports what it had.
            let mut families: Vec<&str> = Vec::new();
            for weapon in &doc.weapons {
                if let Some(f) = ARMED_FAMILIES.iter().copied().find(|f| *f == weapon.family) {
                    families.push(f);
                }
            }
            families.extend(DEFAULT_FAMILIES);
            for (slot, candidates) in human_animation_slots(doc.female, &families) {
                if let Some(path) = candidates.into_iter().find(|p| source.contains(p)) {
                    doc.animation_paths.insert(slot, path);
                }
            }
            for (family, animation_type, paths) in DEFAULT_WEAPONS {
                if !DEFAULT_FAMILIES.contains(&family) {
                    continue;
                }
                if doc.weapons.iter().any(|w| w.family == family) {
                    continue;
                }
                if let Some(path) = paths.iter().find(|p| source.contains(p)) {
                    doc.weapons.push(ActorWeapon {
                        form: None,
                        editor_id: None,
                        path: (*path).to_owned(),
                        mesh: None,
                        animation_type,
                        family: family.to_owned(),
                        carried: false,
                    });
                }
            }
        }
        doc.parts
            .sort_by(|a, b| (a.role, &a.path).cmp(&(b.role, &b.path)));
        doc.parts.dedup();
        doc.notes = notes.0;
        doc
    }
}

/// The race skeleton beside a normalised body model: `skeleton.nif` in
/// the model's folder.
fn skeleton_beside(body: &str) -> Option<String> {
    let (folder, _) = body.rsplit_once('\\')?;
    Some(format!("{folder}\\{RACE_SKELETON_NAME}"))
}

/// Why `path` (normalised) is not a character skeleton, if it is not: an
/// editor marker, a file the data lacks or that does not decode, or a
/// skeleton without the humanoid root, a hip bone or the head bone.
fn skeleton_problem(path: &str, source: &DataSource) -> Option<String> {
    let name = path.rsplit('\\').next().unwrap_or(path);
    if name.starts_with(MARKER_PREFIX) {
        return Some("is an editor marker".to_owned());
    }
    let bytes = match source.read(path) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Some("is not in the data".to_owned()),
        Err(error) => return Some(format!("does not read ({error})")),
    };
    let skeleton = match nif::parse(&bytes).and_then(|file| nif::extract_skeleton(&file)) {
        Ok(skeleton) => skeleton,
        Err(error) => return Some(format!("does not decode ({error})")),
    };
    let has = |bone: &str| skeleton.find(bone).is_some();
    if !has(SKELETON_ROOT) {
        return Some(format!("has no {SKELETON_ROOT} bone"));
    }
    if !SKELETON_HIPS.iter().any(|b| has(b)) {
        return Some(format!("has no {} bone", SKELETON_HIPS.join(" or ")));
    }
    if !has(SKELETON_HEAD) {
        return Some(format!("has no {SKELETON_HEAD} bone"));
    }
    None
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
        for weapon in &doc.weapons {
            out.meshes.insert(weapon.path.clone());
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
        Npc, Race, RacePart, RaceParts,
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
        resolve_with(plugin, base, &[])
    }

    /// [`resolve`] with loose files in the data directory, by normalised
    /// path.
    fn resolve_with(plugin: &PluginBuilder, base: u32, loose: &[(&str, Vec<u8>)]) -> ActorDoc {
        let bytes = plugin.build();
        let files = vec![PluginFile {
            name: PLUGIN.to_owned(),
            sha256: content_hash(&bytes),
            bytes,
        }];
        let index = Index::build(&files).unwrap();
        let dir = tempfile::tempdir().unwrap();
        for (path, bytes) in loose {
            let file = dir.path().join(path.replace('\\', "/"));
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, bytes).unwrap();
        }
        let source = DataSource::new(dir.path(), &[PLUGIN.to_owned()]).unwrap();
        let key = index.key(&FormKey::new(PLUGIN, base)).unwrap();
        Resolver::new(&index).actor(key, RefKind::Npc, &source)
    }

    const RACE: u32 = 0x50;
    const RACE_SKELETON: &str = "meshes\\townsfolk\\skeleton.nif";
    const CUSTOM_SKELETON: &str = "meshes\\custom\\skeleton.nif";
    const MARKER: &str = "Marker_Creature.NIF";

    /// A race whose male upper body sits beside [`RACE_SKELETON`].
    fn race() -> Race {
        let mut race = Race::new(RACE, "Townsfolk", "Townsfolk");
        race.body = Some(RaceParts {
            male: vec![RacePart::new(0, Some("Townsfolk\\UpperBody.nif"), None)],
            female: Vec::new(),
        });
        race
    }

    /// An NPC of [`race`] with a model.
    fn person(form: u32, template: Option<u32>, flags: u16, model: &str) -> Npc {
        let mut n = npc(form, template, flags);
        n.common.model = Some(model.to_owned()).filter(|m| !m.is_empty());
        n.race = Some(FormId(RACE));
        n
    }

    /// The race's, the custom and the human skeleton, all good ones.
    fn skeletons() -> Vec<(&'static str, Vec<u8>)> {
        let skeleton = nif::fixture::skeleton_nif();
        vec![
            (RACE_SKELETON, skeleton.clone()),
            (CUSTOM_SKELETON, skeleton.clone()),
            (HUMAN_SKELETON, skeleton),
        ]
    }

    fn skeleton_notes(doc: &ActorDoc) -> Vec<&String> {
        doc.notes
            .iter()
            .filter(|n| n.contains("skeleton"))
            .collect()
    }

    #[test]
    fn a_marker_model_gives_way_to_the_race_skeleton_then_the_human_one() {
        let mut plugin = PluginBuilder::new();
        plugin.add(race()).add(person(0x10, None, 0, MARKER));
        let doc = resolve_with(&plugin, 0x10, &skeletons());
        assert_eq!(doc.skeleton_path.as_deref(), Some(RACE_SKELETON));
        let notes = skeleton_notes(&doc);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].contains("meshes\\marker_creature.nif"),
            "{notes:?}"
        );
        assert!(notes[0].contains("is an editor marker"), "{notes:?}");
        assert!(notes[0].contains("the race's skeleton"), "{notes:?}");

        // Without the race's skeleton in the data: the human skeleton.
        let only_human = vec![(HUMAN_SKELETON, nif::fixture::skeleton_nif())];
        let doc = resolve_with(&plugin, 0x10, &only_human);
        assert_eq!(doc.skeleton_path.as_deref(), Some(HUMAN_SKELETON));
        assert!(
            doc.notes.iter().any(|n| n.contains("the human skeleton")),
            "{:?}",
            doc.notes
        );

        // Nothing usable at all: the human skeleton still, and the extract
        // says that the file is missing.
        let doc = resolve_with(&plugin, 0x10, &[]);
        assert_eq!(doc.skeleton_path.as_deref(), Some(HUMAN_SKELETON));
        assert!(
            doc.notes
                .iter()
                .any(|n| *n == format!("skeleton {HUMAN_SKELETON} is not in the data")),
            "{:?}",
            doc.notes
        );

        // No model at all: the race's skeleton, noted.
        let mut plugin = PluginBuilder::new();
        plugin.add(race()).add(person(0x10, None, 0, ""));
        let doc = resolve_with(&plugin, 0x10, &skeletons());
        assert_eq!(doc.skeleton_path.as_deref(), Some(RACE_SKELETON));
        assert!(
            doc.notes.iter().any(|n| n.contains("names no model")),
            "{:?}",
            doc.notes
        );
    }

    #[test]
    fn a_valid_custom_skeleton_is_kept_and_a_bad_one_is_not() {
        let mut plugin = PluginBuilder::new();
        plugin
            .add(race())
            .add(person(0x10, None, 0, "Custom\\Skeleton.NIF"))
            .add(person(0x11, None, 0, "Custom\\Lost.nif"))
            .add(person(0x12, None, 0, "Custom\\Box.nif"));
        let mut loose = skeletons();
        loose.push((
            "meshes\\custom\\box.nif",
            nif::fixture::box_mesh([1.0, 1.0, 1.0], "textures\\box.dds"),
        ));
        let kept = resolve_with(&plugin, 0x10, &loose);
        assert_eq!(kept.skeleton_path.as_deref(), Some(CUSTOM_SKELETON));
        assert!(skeleton_notes(&kept).is_empty(), "{:?}", kept.notes);

        // A missing file and a mesh without the humanoid bones.
        for (form, problem) in [(0x11, "is not in the data"), (0x12, "has no Bip01 bone")] {
            let doc = resolve_with(&plugin, form, &loose);
            assert_eq!(doc.skeleton_path.as_deref(), Some(RACE_SKELETON));
            let notes = skeleton_notes(&doc);
            assert_eq!(notes.len(), 1, "{notes:?}");
            assert!(notes[0].contains(problem), "{notes:?}");
        }
    }

    #[test]
    fn a_marker_reached_through_template_chains_gives_way_too() {
        let mut plugin = PluginBuilder::new();
        plugin
            .add(race())
            // Stats from a template that is a leveled list, whose actor has
            // a custom skeleton; the model group is the NPC's own: the
            // marker, which the race's skeleton replaces.
            .add(person(0x10, Some(0x40), USE_STATS, MARKER))
            .add(LeveledList::new(
                LeveledKind::Npc,
                0x40,
                "List",
                vec![LeveledEntry::new(1, 0x20u32, 1)],
            ))
            .add(person(0x20, None, 0, "Custom\\Skeleton.NIF"))
            // The model group through a list to an actor whose model is
            // the marker: replaced as well, and the note names that actor.
            .add(person(0x11, Some(0x41), USE_MODEL, MARKER))
            .add(LeveledList::new(
                LeveledKind::Npc,
                0x41,
                "MarkerList",
                vec![LeveledEntry::new(1, 0x21u32, 1)],
            ))
            .add(person(0x21, Some(0x40), USE_STATS, MARKER))
            // The model group through the first list: its actor's custom
            // skeleton, kept.
            .add(person(0x12, Some(0x40), USE_MODEL, MARKER));
        let loose = skeletons();

        let doc = resolve_with(&plugin, 0x10, &loose);
        assert_eq!(doc.template_chains["stats"].len(), 2);
        assert_eq!(doc.skeleton_path.as_deref(), Some(RACE_SKELETON));
        assert!(
            doc.notes
                .iter()
                .any(|n| n.contains("test.esm:000010") && n.contains("editor marker")),
            "{:?}",
            doc.notes
        );

        let doc = resolve_with(&plugin, 0x11, &loose);
        assert_eq!(
            doc.template_chains["model"],
            [FormKey::new(PLUGIN, 0x41), FormKey::new(PLUGIN, 0x21)]
        );
        assert_eq!(doc.skeleton_path.as_deref(), Some(RACE_SKELETON));
        assert!(
            doc.notes
                .iter()
                .any(|n| n.contains("test.esm:000021") && n.contains("editor marker")),
            "{:?}",
            doc.notes
        );

        let doc = resolve_with(&plugin, 0x12, &loose);
        assert_eq!(doc.skeleton_path.as_deref(), Some(CUSTOM_SKELETON));
        assert!(skeleton_notes(&doc).is_empty(), "{:?}", doc.notes);
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
