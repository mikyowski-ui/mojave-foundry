//! The catalogue's build steps ([`crate::loot`] notes), called from
//! `assemble::build_layer` only when a catalogue is given.

use std::collections::{BTreeMap, BTreeSet};

use emit::icon::{INVENTORY_ICON_MAX, inventory_frm_indexed, inventory_frm_rgba};
use emit::items::{self, PowerSource};
use emit::map::CritterPlacement as CritterRecord;
use emit::objects::{self, Loadout, ProtoSet, Stack};
use emit::proto::{self, CritterSpec, ProtoCommon};
use emit::{Anim, FighterSpec, FormatWriters, LocalWriters, ProtoText, WeaponCode};
use vault_formats::map::MapObject;
use vault_formats::pro::{Category, ItemData, PrototypeData};
use vault_formats::{Fid, Prototype};

use crate::budget::{ART_INVENTORY, ART_ITEMS};
use crate::layer::{ArtKey, LayerBuilder, art_key};
use crate::loot::{
    Catalogue, CritterLoot, GroundArt, Held, ItemDef, ItemIcon, ItemKind, ItemPrototypeReport,
    LootReport, LootSkip, MapLoot, PlacedLoot,
};
use crate::plan::MapPlan;
use crate::{ArtCategory, ArtImage, BuildError, CritterPlacement, LayerInput, Placement, Role};

fn input(message: String) -> BuildError {
    BuildError::Input(message)
}

/// An `emit` refusal as an input error naming `what`.
fn refused(what: &str, error: impl std::fmt::Display) -> BuildError {
    BuildError::Input(format!("{what}: {error}"))
}

/// An item's ground art, resolved.
#[derive(Debug, Clone, Copy)]
enum Ground<'c> {
    /// A placement's art: map, elevation, art index.
    Placed(usize, usize, usize),
    Sprite(&'c ArtImage),
}

/// The prototype extras of a named, keyed or sized reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Extra {
    /// A stable label for the prototype key.
    pub label: String,
    pub text: Option<ProtoText>,
    pub capacity: Option<i32>,
    pub key: Option<i32>,
}

/// A critter prototype the catalogue asks for.
#[derive(Debug, Clone)]
pub(crate) enum CritterProto {
    Fighter(Box<FighterSpec>),
    Bare(CritterSpec),
}

impl CritterProto {
    pub(crate) fn hit_points(&self) -> i32 {
        match self {
            Self::Fighter(spec) => spec.critter.hit_points,
            Self::Bare(spec) => spec.hit_points,
        }
    }

    pub(crate) fn female(&self) -> bool {
        match self {
            Self::Fighter(spec) => spec.critter.female,
            Self::Bare(spec) => spec.female,
        }
    }

    pub(crate) fn make(&self, common: ProtoCommon) -> Result<Prototype, emit::EmitError> {
        match self {
            Self::Fighter(spec) => emit::fighter_prototype(common, spec),
            Self::Bare(spec) => emit::critter_prototype(common, spec),
        }
    }
}

/// The catalogue's state through a build.
pub(crate) struct Loot<'c> {
    pub catalogue: &'c Catalogue,
    ground: BTreeMap<&'c str, Ground<'c>>,
    icons: BTreeMap<&'c str, Vec<u8>>,
    /// Critter sprite sets armour figures show.
    pub figure_sets: BTreeSet<usize>,
    ground_fid: BTreeMap<&'c str, Fid>,
    inventory_fid: BTreeMap<&'c str, Fid>,
    pids: BTreeMap<&'c str, u32>,
    key_codes: BTreeMap<&'c str, i32>,
    /// Every prototype an object of the layer may name.
    pub set: ProtoSet,
    pub report: LootReport,
    /// References whose container contents were placed (first placement).
    filled: BTreeSet<String>,
}

/// The nearest palette colour (squared RGB distance, lowest index first).
fn nearest(palette: &[(u8, [u8; 3])], colour: [u8; 3]) -> u8 {
    let mut best = (u32::MAX, 0u8);
    let mut sorted: Vec<&(u8, [u8; 3])> = palette.iter().filter(|(i, _)| *i != 0).collect();
    sorted.sort_by_key(|(i, _)| *i);
    for (index, rgb) in sorted {
        let d: u32 = (0..3)
            .map(|k| (i32::from(rgb[k]) - i32::from(colour[k])).pow(2) as u32)
            .sum();
        if d < best.0 {
            best = (d, *index);
        }
    }
    best.1
}

impl<'c> Loot<'c> {
    /// Check the catalogue against the layer and resolve what can be
    /// resolved before anything is written: ground art, icons, figures.
    pub(crate) fn prepare(
        catalogue: &'c Catalogue,
        layer: &LayerInput,
        plans: &[MapPlan],
    ) -> Result<Self, BuildError> {
        // Every placement by reference, in map, elevation, placement order.
        let mut by_reference: BTreeMap<&str, Vec<(usize, usize, &Placement)>> = BTreeMap::new();
        for (m, plan) in plans.iter().enumerate() {
            for (e, elevation) in plan.elevations.iter().enumerate() {
                for placement in elevation.placements.iter() {
                    by_reference
                        .entry(placement.reference.as_str())
                        .or_default()
                        .push((m, e, placement));
                }
            }
        }
        let image_of = |m: usize, e: usize, art: usize| -> &ArtImage {
            &plans[m].elevations[e].input.art[art]
        };
        let items = &catalogue.items;
        let item_def = |owner: &str, key: &str| -> Result<&'c ItemDef, BuildError> {
            items.get(key).ok_or_else(|| {
                input(format!(
                    "{owner}: names item {key}, which the catalogue does not define"
                ))
            })
        };
        let mut skipped = Vec::new();

        // Placed references.
        for (reference, loot) in &catalogue.placed {
            let owner = format!("reference {reference}");
            let Some(list) = by_reference.get(reference.as_str()) else {
                skipped.push(LootSkip {
                    entry: owner,
                    reason: "no placement of the layer names it".into(),
                });
                continue;
            };
            let (wanted, kind): (&[Role], &str) = match loot {
                PlacedLoot::Item { .. } => (&[Role::Item], "an item"),
                PlacedLoot::Container { .. } => (&[Role::Container], "a container"),
                PlacedLoot::Door { .. } => (&[Role::Door], "a door"),
                PlacedLoot::Named(_) => (
                    &[
                        Role::WallStrip,
                        Role::Scenery,
                        Role::Door,
                        Role::Container,
                        Role::FloorPiece,
                    ],
                    "a name for scenery, walls, doors and containers",
                ),
            };
            if let Some((_, _, p)) = list.iter().find(|(_, _, p)| !wanted.contains(&p.role)) {
                return Err(input(format!(
                    "{owner}: its catalogue entry is {kind}, but it is placed as {:?}",
                    p.role
                )));
            }
            match loot {
                PlacedLoot::Item { item, count, .. } => {
                    item_def(&owner, item)?;
                    if *count == 0 {
                        return Err(input(format!("{owner}: places 0 of item {item}")));
                    }
                }
                PlacedLoot::Container {
                    capacity, contents, ..
                } => {
                    if capacity.is_some_and(|c| c < 0) {
                        return Err(input(format!("{owner}: capacity {capacity:?} is negative")));
                    }
                    for held in contents {
                        item_def(&owner, &held.item)?;
                        if held.quantity == 0 || held.quantity > i32::MAX as u32 {
                            return Err(input(format!(
                                "{owner}: holds {} of item {}; a stack is 1 to {}",
                                held.quantity,
                                held.item,
                                i32::MAX
                            )));
                        }
                    }
                }
                PlacedLoot::Door { key: Some(key), .. } => {
                    let kind = &item_def(&owner, key)?.kind;
                    if !matches!(kind, ItemKind::Key(_)) {
                        return Err(input(format!("{owner}: its key {key} is not a key item")));
                    }
                }
                _ => {}
            }
        }

        // Critters.
        let mut critter_refs: BTreeSet<&str> = BTreeSet::new();
        for map in &layer.maps {
            for critter in map.critters {
                critter_refs.insert(&critter.reference);
            }
        }
        for (reference, loot) in &catalogue.critters {
            let owner = format!("critter {reference}");
            if !critter_refs.contains(reference.as_str()) {
                skipped.push(LootSkip {
                    entry: owner,
                    reason: "no critter of the layer has this reference".into(),
                });
                continue;
            }
            let slots = [
                ("right hand", &loot.right_hand),
                ("left hand", &loot.left_hand),
                ("worn", &loot.worn),
            ];
            for (slot, held) in slots {
                let Some(held) = held else { continue };
                let def = item_def(&owner, &held.item)?;
                if held.quantity != 1 {
                    return Err(input(format!(
                        "{owner}: holds {} of {} in its {slot}; a hand or the armour slot holds one",
                        held.quantity, held.item
                    )));
                }
                let armour = matches!(def.kind, ItemKind::Armor(_));
                if (slot == "worn") != armour {
                    return Err(input(format!(
                        "{owner}: item {} cannot go in its {slot} ({} slot)",
                        held.item,
                        if armour {
                            "armour goes in the worn"
                        } else {
                            "only armour goes in the worn"
                        }
                    )));
                }
            }
            for held in &loot.carried {
                item_def(&owner, &held.item)?;
                if held.quantity == 0 || held.quantity > i32::MAX as u32 {
                    return Err(input(format!(
                        "{owner}: carries {} of item {}; a stack is 1 to {}",
                        held.quantity,
                        held.item,
                        i32::MAX
                    )));
                }
            }
            if loot.team.is_some_and(|t| t <= 0) {
                return Err(input(format!(
                    "{owner}: team {:?} is not allowed: team 0 is the player's and teams are not \
                     negative",
                    loot.team
                )));
            }
            if loot.ai_packet.is_some_and(|p| p < 0) {
                return Err(input(format!(
                    "{owner}: AI packet {:?} is negative",
                    loot.ai_packet
                )));
            }
        }

        // Item definitions: links, figures, ground art and icons.
        let mut figure_sets = BTreeSet::new();
        let mut ground = BTreeMap::new();
        let mut icons = BTreeMap::new();
        for (key, def) in items {
            let owner = format!("item {key}");
            if let Some(ammo) = &def.ammo {
                if !matches!(def.kind, ItemKind::Weapon(_) | ItemKind::Misc(_)) {
                    return Err(input(format!(
                        "{owner}: only a weapon or a miscellaneous item takes ammunition"
                    )));
                }
                if !matches!(item_def(&owner, ammo)?.kind, ItemKind::Ammo(_)) {
                    return Err(input(format!("{owner}: {ammo} is not ammunition")));
                }
                let pid_given = match &def.kind {
                    ItemKind::Weapon(w) => w.ammo.is_some(),
                    ItemKind::Misc(m) => m.power.is_some(),
                    _ => false,
                };
                if pid_given {
                    return Err(input(format!(
                        "{owner}: give its ammunition by key (ItemDef::ammo) or as a PID in its \
                         spec, not both"
                    )));
                }
            }
            for (slot, figure) in def.figure.iter().enumerate() {
                let Some(index) = figure else { continue };
                let ItemKind::Armor(spec) = &def.kind else {
                    return Err(input(format!("{owner}: only armour takes a figure")));
                };
                if *index >= layer.critter_art.len() {
                    return Err(input(format!(
                        "{owner}: its figure names critter art {index}; there are {}",
                        layer.critter_art.len()
                    )));
                }
                let given = if slot == 0 {
                    spec.male_art
                } else {
                    spec.female_art
                };
                if given.is_some() {
                    return Err(input(format!(
                        "{owner}: give its figure by sprite set (ItemDef::figure) or as a FID in \
                         its spec, not both"
                    )));
                }
                figure_sets.insert(*index);
            }
            let names_it = |r: &str| matches!(catalogue.placed.get(r), Some(PlacedLoot::Item { item, .. }) if item == key);
            let resolved = match &def.ground {
                GroundArt::Sprite(image) => {
                    if image.category != ArtCategory::Items {
                        return Err(input(format!(
                            "{owner}: its ground sprite is {:?} art; item art is Items",
                            image.category
                        )));
                    }
                    if (image.width as usize).checked_mul(image.height as usize)
                        != Some(image.pixels.len())
                    {
                        return Err(input(format!(
                            "{owner}: its ground sprite has {} pixel bytes for {}×{}",
                            image.pixels.len(),
                            image.width,
                            image.height
                        )));
                    }
                    Ground::Sprite(image)
                }
                GroundArt::Reference(reference) => {
                    let found = by_reference.get(reference.as_str()).and_then(|list| {
                        list.iter().find(|(_, _, p)| p.role == Role::Item).copied()
                    });
                    let Some((m, e, p)) = found else {
                        return Err(input(format!(
                            "{owner}: its ground art names reference {reference}, which has no \
                             item placement"
                        )));
                    };
                    Ground::Placed(m, e, p.art)
                }
                GroundArt::FirstPlacement => {
                    let found = by_reference
                        .iter()
                        .filter(|(r, _)| names_it(r))
                        .flat_map(|(_, list)| list.iter().copied())
                        .filter(|(_, _, p)| p.role == Role::Item)
                        .min_by_key(|(m, e, p)| {
                            // Map, elevation, placement order.
                            let index = plans[*m].elevations[*e]
                                .placements
                                .iter()
                                .position(|q| std::ptr::eq(q, *p))
                                .unwrap_or(usize::MAX);
                            (*m, *e, index)
                        });
                    let Some((m, e, p)) = found else {
                        return Err(input(format!(
                            "{owner}: no placed reference names it, so it has no ground art; \
                             give GroundArt::Sprite"
                        )));
                    };
                    Ground::Placed(m, e, p.art)
                }
            };
            let image = match resolved {
                Ground::Placed(m, e, art) => image_of(m, e, art),
                Ground::Sprite(image) => image,
            };
            let spec = match &def.icon {
                Some(ItemIcon::Indexed {
                    width,
                    height,
                    pixels,
                }) => inventory_frm_indexed(pixels, *width, *height, INVENTORY_ICON_MAX),
                Some(ItemIcon::Rgba {
                    width,
                    height,
                    rgba,
                }) => {
                    if catalogue.icon_palette.iter().all(|(i, _)| *i == 0) {
                        return Err(input(format!(
                            "{owner}: an RGBA icon needs Catalogue::icon_palette"
                        )));
                    }
                    let palette = &catalogue.icon_palette;
                    inventory_frm_rgba(rgba, *width, *height, INVENTORY_ICON_MAX, &|c| {
                        nearest(palette, c)
                    })
                }
                None => inventory_frm_indexed(
                    &image.pixels,
                    image.width,
                    image.height,
                    INVENTORY_ICON_MAX,
                ),
            }
            .map_err(|e| {
                refused(
                    &format!(
                        "{owner}: inventory icon{}",
                        if def.icon.is_none() {
                            " (from its ground art; give ItemDef::icon)"
                        } else {
                            ""
                        }
                    ),
                    e,
                )
            })?;
            let bytes = LocalWriters
                .frm(&spec)
                .map_err(|e| refused(&format!("{owner}: inventory icon"), e))?;
            ground.insert(key.as_str(), resolved);
            icons.insert(key.as_str(), bytes);
        }
        skipped.sort();
        Ok(Self {
            catalogue,
            ground,
            icons,
            figure_sets,
            ground_fid: BTreeMap::new(),
            inventory_fid: BTreeMap::new(),
            pids: BTreeMap::new(),
            key_codes: BTreeMap::new(),
            set: ProtoSet::new(),
            report: LootReport {
                skipped,
                ..LootReport::default()
            },
            filled: BTreeSet::new(),
        })
    }

    /// The new art entries the catalogue asks for, as budget labels: its
    /// own ground sprites (not already among `unique`) in the items list,
    /// its distinct icons in the inventory list.
    pub(crate) fn budget(
        &self,
        unique: &mut BTreeSet<ArtKey>,
        labels: &mut BTreeMap<u8, Vec<String>>,
    ) {
        for (key, ground) in &self.ground {
            if let Ground::Sprite(image) = ground
                && unique.insert(art_key(image))
            {
                labels
                    .entry(ART_ITEMS)
                    .or_default()
                    .push(format!("item {key}"));
            }
        }
        let mut seen: BTreeSet<&[u8]> = BTreeSet::new();
        for (key, bytes) in &self.icons {
            if seen.insert(bytes) {
                labels
                    .entry(ART_INVENTORY)
                    .or_default()
                    .push(format!("item {key} icon"));
            }
        }
    }

    /// Write the ground sprites of their own and the icons.
    pub(crate) fn write_art(
        &mut self,
        builder: &mut LayerBuilder,
        art_fid: &[Vec<Vec<Fid>>],
    ) -> Result<(), BuildError> {
        for (key, ground) in &self.ground {
            let fid = match ground {
                Ground::Placed(m, e, art) => art_fid[*m][*e][*art],
                Ground::Sprite(image) => {
                    self.report.ground_sprites += 1;
                    builder.add_sprite(image)?
                }
            };
            self.ground_fid.insert(key, fid);
        }
        let mut distinct = BTreeSet::new();
        for (key, bytes) in &self.icons {
            let fid = builder.add_frm_once(ART_INVENTORY, bytes.clone())?;
            distinct.insert(fid);
            self.inventory_fid.insert(key, fid);
        }
        self.report.inventory_art = distinct.len();
        Ok(())
    }

    /// One prototype per item definition, in sorted key order, each
    /// resolving the PIDs, calibres, figures and key codes it names.
    /// `figures` is each sprite set's stand FID.
    pub(crate) fn write_items(
        &mut self,
        builder: &mut LayerBuilder,
        figures: &BTreeMap<usize, Fid>,
    ) -> Result<(), BuildError> {
        let catalogue: &'c Catalogue = self.catalogue;
        let items = &catalogue.items;
        let first = builder.lists.protos[&Category::Item].len();
        for (i, key) in items.keys().enumerate() {
            let position = u32::try_from(first + 1 + i)
                .map_err(|_| input("the item prototype list overflows".into()))?;
            let pid = proto::pid(Category::Item, position)
                .map_err(|e| refused("proto/items/items.lst", e))?;
            self.pids.insert(key, pid);
            if let ItemKind::Key(spec) = &items[key].kind {
                let code = if spec.code != 0 {
                    spec.code
                } else {
                    (pid & proto::PID_INDEX_CAP) as i32
                };
                self.key_codes.insert(key, code);
            }
        }
        let flags = crate::assemble::object_flags(Role::Item, false);
        for (key, def) in items {
            let owner = format!("item {key}");
            let pid = self.pids[key.as_str()];
            let fid = self.ground_fid[key.as_str()];
            let inventory = Some(self.inventory_fid[key.as_str()]);
            let ammo = def.ammo.as_ref().map(|ammo| {
                let caliber = match &items[ammo].kind {
                    ItemKind::Ammo(spec) => spec.caliber,
                    _ => 0,
                };
                (self.pids[ammo.as_str()], caliber)
            });
            let kind = def.kind.clone();
            let make = |common: ProtoCommon| -> Result<Prototype, emit::EmitError> {
                match kind {
                    ItemKind::Weapon(mut spec) => {
                        spec.stats.inventory_art = inventory;
                        if let Some((pid, caliber)) = ammo {
                            spec.ammo = Some(pid);
                            if spec.caliber == 0 {
                                spec.caliber = caliber;
                            } else if spec.caliber != caliber {
                                return Err(emit::EmitError::Prototype(format!(
                                    "its calibre {} is not its ammunition's {caliber}",
                                    spec.caliber
                                )));
                            }
                        }
                        items::weapon(common, &spec)
                    }
                    ItemKind::Armor(mut spec) => {
                        spec.stats.inventory_art = inventory;
                        if let Some(set) = def.figure[0] {
                            spec.male_art = figures.get(&set).copied();
                        }
                        if let Some(set) = def.figure[1] {
                            spec.female_art = figures.get(&set).copied();
                        }
                        items::armor(common, &spec)
                    }
                    ItemKind::Ammo(mut spec) => {
                        spec.stats.inventory_art = inventory;
                        items::ammo(common, &spec)
                    }
                    ItemKind::Drug(mut spec) => {
                        spec.stats.inventory_art = inventory;
                        items::drug(common, &spec)
                    }
                    ItemKind::Key(mut spec) => {
                        spec.stats.inventory_art = inventory;
                        if spec.code == 0 {
                            spec.code = (common.pid & proto::PID_INDEX_CAP) as i32;
                        }
                        items::key(common, &spec)
                    }
                    ItemKind::Misc(mut spec) => {
                        spec.stats.inventory_art = inventory;
                        if let Some((pid, caliber)) = ammo {
                            spec.power = Some(PowerSource { ammo: pid, caliber });
                        }
                        items::misc_item_with(common, &spec)
                    }
                }
            };
            let (prototype, file) = builder
                .lists
                .add_described(
                    &mut builder.layer,
                    Category::Item,
                    |pid, message_base| ProtoCommon {
                        pid,
                        fid,
                        message_base,
                        flags,
                        material: def.material,
                    },
                    make,
                    &def.text,
                )
                .map_err(|e| match e {
                    BuildError::Emit { source, .. } => refused(&owner, source),
                    other => other,
                })?;
            if prototype.pid != pid {
                return Err(input(format!(
                    "{owner}: its prototype landed at PID {:#010x}, not {pid:#010x}",
                    prototype.pid
                )));
            }
            *self
                .report
                .item_prototypes
                .entry(def.kind.label().to_owned())
                .or_default() += 1;
            let inventory_fid = match &prototype.data {
                PrototypeData::Item(item) => item.inventory_art,
                _ => u32::MAX,
            };
            self.report.items.push(ItemPrototypeReport {
                key: key.clone(),
                pid,
                file,
                kind: def.kind.label(),
                name: String::from_utf8_lossy(&def.text.name).into_owned(),
                ground_fid: prototype.art_id,
                inventory_fid,
            });
            self.set.insert(prototype);
        }
        Ok(())
    }

    /// The prototype extras of a placement of `reference` with `role`.
    pub(crate) fn extra(&self, reference: &str, role: Role) -> Option<Extra> {
        let (text, capacity, key) = match self.catalogue.placed.get(reference)? {
            PlacedLoot::Item { .. } => return None,
            PlacedLoot::Container { text, capacity, .. } => (text.clone(), *capacity, None),
            PlacedLoot::Door { text, key, .. } => (
                text.clone(),
                None,
                key.as_ref()
                    .and_then(|k| self.key_codes.get(k.as_str()).copied()),
            ),
            PlacedLoot::Named(text) => (Some(text.clone()), None, None),
        };
        if role == Role::Item {
            return None;
        }
        Some(Extra {
            label: format!("{text:?} {capacity:?} {key:?}"),
            text,
            capacity,
            key,
        })
    }

    /// Whether `reference` is an item of the catalogue lying on the map.
    pub(crate) fn is_item(&self, reference: &str) -> bool {
        matches!(
            self.catalogue.placed.get(reference),
            Some(PlacedLoot::Item { .. })
        )
    }

    fn held_object(
        &self,
        owner: &str,
        held: &Held,
        level: u8,
        take_id: &mut dyn FnMut() -> u32,
    ) -> Result<MapObject, BuildError> {
        let pid = self.pids[held.item.as_str()];
        objects::item_object(&self.set, pid, take_id(), None, level, held.state)
            .map_err(|e| refused(&format!("{owner}: item {}", held.item), e))
    }

    /// The objects of a placement whose reference the catalogue names:
    /// `None` for one it does not (built as without a catalogue).
    /// `prototype` is the placement's own prototype (containers, doors,
    /// named scenery), `fid` its art.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn objects(
        &mut self,
        placement: &Placement,
        prototype: Option<(u32, Fid)>,
        fid: Fid,
        level: u8,
        take_id: &mut dyn FnMut() -> u32,
        counts: &mut MapLoot,
    ) -> Result<Option<Vec<MapObject>>, BuildError> {
        let catalogue: &'c Catalogue = self.catalogue;
        let reference = placement.reference.as_str();
        let Some(loot) = catalogue.placed.get(reference) else {
            return Ok(None);
        };
        let owner = format!("reference {reference}");
        let finish = |mut object: MapObject| {
            object.displacement = placement.displacement;
            object
        };
        match loot {
            PlacedLoot::Item { item, count, state } => {
                let pid = self.pids[item.as_str()];
                let mut out = Vec::new();
                for _ in 0..*count {
                    let mut object = objects::item_object(
                        &self.set,
                        pid,
                        take_id(),
                        Some(placement.hex),
                        level,
                        *state,
                    )
                    .map_err(|e| refused(&format!("{owner}: item {item}"), e))?;
                    object.art_id = fid.0;
                    out.push(finish(object));
                }
                counts.placed_items += out.len();
                Ok(Some(out))
            }
            PlacedLoot::Container { lock, contents, .. } => {
                let (pid, _) = prototype
                    .ok_or_else(|| input(format!("{owner}: a container without its prototype")))?;
                let id = take_id();
                let first = self.filled.insert(reference.to_owned());
                let mut stacks = Vec::new();
                if first {
                    for held in contents {
                        stacks.push(Stack::new(
                            self.held_object(&owner, held, level, take_id)?,
                            held.quantity,
                        ));
                    }
                    counts.containers += 1;
                    if !stacks.is_empty() {
                        counts.filled_containers += 1;
                        counts.container_stacks += stacks.len();
                    }
                    counts.locked_containers += usize::from(lock.locked);
                    counts.jammed += usize::from(lock.jammed);
                }
                let object = objects::container_object(
                    &self.set,
                    pid,
                    id,
                    Some(placement.hex),
                    level,
                    *lock,
                    stacks,
                )
                .map_err(|e| refused(&format!("container {reference}"), e))?;
                Ok(Some(vec![finish(object)]))
            }
            PlacedLoot::Door { lock, key, .. } => {
                let (pid, _) = prototype
                    .ok_or_else(|| input(format!("{owner}: a door without its prototype")))?;
                let object = objects::door_object(
                    &self.set,
                    pid,
                    take_id(),
                    placement.hex,
                    level,
                    *lock,
                    None,
                )
                .map_err(|e| refused(&format!("door {reference}"), e))?;
                counts.locked_doors += usize::from(lock.locked);
                counts.jammed += usize::from(lock.jammed);
                counts.keyed_doors += usize::from(key.is_some());
                Ok(Some(vec![finish(object)]))
            }
            PlacedLoot::Named(_) => Ok(None),
        }
    }

    /// The prototype a critter with a catalogue entry takes: a stable
    /// label and its spec; `None` for a critter the catalogue does not
    /// name. `layer` is the layer's AI packet and team; `ai_txt` the base
    /// table, when supplied.
    pub(crate) fn critter_proto(
        &self,
        critter: &CritterPlacement,
        layer: (i32, i32),
        ai_txt: Option<&[u8]>,
    ) -> Result<Option<(String, CritterProto)>, BuildError> {
        let Some(loot) = self.catalogue.critters.get(&critter.reference) else {
            return Ok(None);
        };
        let owner = format!("critter {}", critter.reference);
        let (fighter_ai, fighter_team) = loot.fighter.as_ref().map_or((None, None), |f| {
            (Some(f.critter.ai_packet), Some(f.critter.team))
        });
        let ai = loot.ai_packet.or(fighter_ai).unwrap_or(layer.0);
        let team = loot.team.or(fighter_team).unwrap_or(layer.1);
        if team <= 0 {
            return Err(input(format!(
                "{owner}: team {team} is not allowed: team 0 is the player's and teams are not \
                 negative"
            )));
        }
        if ai < 0 {
            return Err(input(format!("{owner}: AI packet {ai} is negative")));
        }
        if let Some(table) = ai_txt
            && !crate::critters::has_ai_packet(table, ai)?
        {
            return Err(input(format!(
                "{owner}: AI packet {ai} is not in the base {}; the engine would use the \
                 table's first packet instead",
                crate::critters::AI_TXT
            )));
        }
        let spec = match &loot.fighter {
            Some(fighter) => {
                let mut spec = fighter.clone();
                spec.critter.ai_packet = ai;
                spec.critter.team = team;
                CritterProto::Fighter(Box::new(spec))
            }
            None => {
                let mut spec = CritterSpec::human(critter.hit_points, ai, team);
                spec.female = critter.female;
                CritterProto::Bare(spec)
            }
        };
        let label = match &spec {
            CritterProto::Fighter(spec) => format!("fighter {spec:?}"),
            CritterProto::Bare(spec) => format!("bare {spec:?}"),
        };
        Ok(Some((label, spec)))
    }

    /// A placed critter with its kit; `None` for one the catalogue does
    /// not name. `files` is its sprite set's files by suffix.
    pub(crate) fn critter_object(
        &mut self,
        record: &CritterRecord,
        reference: &str,
        files: &BTreeMap<String, Vec<u8>>,
        take_id: &mut dyn FnMut() -> u32,
        counts: &mut MapLoot,
    ) -> Result<Option<MapObject>, BuildError> {
        let catalogue: &'c Catalogue = self.catalogue;
        let Some(loot): Option<&CritterLoot> = catalogue.critters.get(reference) else {
            return Ok(None);
        };
        let owner = format!("critter {reference}");
        let level = record.elevation;
        let mut carried = Vec::new();
        let mut right = None;
        if let Some(held) = &loot.right_hand {
            let object = self.held_object(&owner, held, level, take_id)?;
            let code = match self.set.get(object.pid).map(|p| &p.data) {
                Some(PrototypeData::Item(item)) => match &item.data {
                    ItemData::Weapon(weapon) => u8::try_from(weapon.animation)
                        .ok()
                        .and_then(WeaponCode::from_code),
                    _ => Some(WeaponCode::Unarmed),
                },
                _ => Some(WeaponCode::Unarmed),
            };
            let missing: Vec<String> = match code {
                Some(WeaponCode::Unarmed) => Vec::new(),
                Some(code) => [Anim::Stand, Anim::Walk]
                    .iter()
                    .filter_map(|anim| anim.suffix_str(code).ok())
                    .filter(|suffix| !files.contains_key(suffix))
                    .collect(),
                None => vec!["a weapon code of 0 to 10".to_owned()],
            };
            if missing.is_empty() {
                right = Some(object);
            } else {
                self.report.skipped.push(LootSkip {
                    entry: owner.clone(),
                    reason: format!(
                        "its sprite set lacks {} for the weapon {} in its right hand; the weapon \
                         is carried instead",
                        missing.join(" and "),
                        held.item
                    ),
                });
                carried.push(Stack::new(object, 1));
            }
        }
        let left = match &loot.left_hand {
            Some(held) => Some(self.held_object(&owner, held, level, take_id)?),
            None => None,
        };
        let worn = match &loot.worn {
            Some(held) => Some(self.held_object(&owner, held, level, take_id)?),
            None => None,
        };
        for held in &loot.carried {
            carried.push(Stack::new(
                self.held_object(&owner, held, level, take_id)?,
                held.quantity,
            ));
        }
        let armed = right.as_ref().is_some_and(|object| {
            matches!(
                self.set.get(object.pid).map(|p| &p.data),
                Some(PrototypeData::Item(item)) if matches!(item.data, ItemData::Weapon(_))
            )
        });
        counts.fighters += usize::from(loot.fighter.is_some());
        counts.equipped_critters += usize::from(right.is_some() || left.is_some());
        counts.armed_critters += usize::from(armed);
        let loadout = Loadout {
            right_hand: right,
            left_hand: left,
            worn,
            carried,
        };
        let object = objects::critter_with_loadout(record, &self.set, loadout)
            .map_err(|e| refused(&owner, e))?;
        counts.critter_stacks += object.inventory.len();
        Ok(Some(object))
    }
}

/// Inventory records under `objects`, at every depth.
pub(crate) fn inventory_records(objects: &[MapObject]) -> usize {
    fn count(object: &MapObject) -> usize {
        object
            .inventory
            .iter()
            .map(|entry| 1 + count(&entry.object))
            .sum()
    }
    objects.iter().map(count).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_colour_skips_the_transparent_index() {
        let palette = [(0, [0, 0, 0]), (3, [250, 0, 0]), (2, [0, 0, 10])];
        assert_eq!(nearest(&palette, [1, 1, 1]), 2);
        assert_eq!(nearest(&palette, [200, 10, 10]), 3);
    }
}
