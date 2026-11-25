//! [`build_layer`]: from the input to a checked mod layer, and [`build`],
//! the single-map adapter behind [`crate::build_map`].
//!
//! The order (`DESIGN-levels.md` (2)): validate every input; plan every
//! map (phase A, `plan`); resolve stub exits, door links and stairs between
//! the plans (phase B, `links`); join islands, choose entrances, place
//! critters (phase C); check the layer's budgets; write through the
//! [`LayerBuilder`]; register the maps and ship the tables; verify.

use std::collections::{BTreeMap, BTreeSet};

use emit::map::{CritterPlacement as CritterRecord, MAP_NAME_BYTES, empty_map, plain_object};
use emit::proto::{self, DoorSpec, ItemStats, Material, ObjectFlags, ProtoCommon};
use emit::{
    CRITTER_LIST_PATH, CritterList, CritterSpec, FID_INDEX_CAP, ProtoList, critter_object,
    critter_prototype,
};
use vault_formats::map::{MAX_OBJECTS, MapObject, ObjectDetail, ObjectState, TilePair};
use vault_formats::pro::{BasicFlags, Category, LockFlags};
use vault_formats::{Fid, Prototype};

use crate::budget::{ART_CRITTERS, ART_MISC, ART_SCENERY, ART_TILES, BUILD_LISTS};
use crate::critters::{self, AI_TXT};
use crate::exits::{DoorExit, STUB_ENTRY, STUB_EXIT_RADIUS, STUB_FLOOR_RADIUS};
use crate::furniture::FurnitureReport;
use crate::islands;
use crate::layer::{BLANK_TILE, LayerBuilder, art_category, art_key, base_error, directory};
use crate::loot::MapLoot;
use crate::loot_build::Loot;
use crate::names::{map_stem, message_text, stub_stem};
use crate::plan::{self, CellIndex, CritterLevel, DoorMode, effective_blocking};
use crate::walk::{Frame, HEXES};
use crate::{
    ArtListReport, BLANK_SQUARE, BuildError, BuildInput, BuildReport, Built, BuiltLayer, BuiltMap,
    ComponentReport, CritterComponent, CritterPrototypeReport, CritterSetReport, ElevationInput,
    ElevationReport, ExitComponents, ExitReport, HiddenCritter, InteriorReport, LayerInput,
    LayerOptions, LinkReport, MapInput, ObjectRef, PlacedCritter, Role,
};

/// Records the top-level objects a placement just became (`objects[first..]`)
/// in the exact reference-to-object map, and for a hidden placement sets
/// the hidden flag on each and lists it.
fn note_objects(
    objects: &mut [MapObject],
    first: usize,
    placement: &crate::Placement,
    level: u8,
    hidden: bool,
    refs: &mut Vec<ObjectRef>,
    hidden_objects: &mut Vec<(String, u8, i32)>,
) {
    for object in &mut objects[first..] {
        if hidden {
            object.flags = BasicFlags(object.flags.0 | BasicFlags::HIDDEN.0);
        }
        let hex = object.hex.unwrap_or(placement.hex);
        refs.push(ObjectRef {
            reference: placement.reference.clone(),
            elevation: level,
            id: object.id,
            hex,
            pid: object.pid,
            hidden,
        });
        if hidden {
            hidden_objects.push((placement.reference.clone(), level, hex));
        }
    }
}

/// The exit-grid prototype every exit uses (variant 0).
pub(crate) const EXIT_GRID_PID: u32 = 0x0500_0010;
pub(crate) use crate::layer::MAPS_TXT;

fn role_category(role: Role) -> Category {
    match role {
        Role::WallStrip => Category::Wall,
        Role::Scenery | Role::Door | Role::FloorPiece => Category::Scenery,
        Role::Container | Role::Item => Category::Item,
    }
}

pub(crate) fn object_flags(role: Role, blocking: bool) -> ObjectFlags {
    match role {
        Role::FloorPiece => ObjectFlags::FLOOR,
        Role::Item => ObjectFlags {
            blocking: false,
            light_through: true,
            shoot_through: true,
            flat: true,
            no_highlight: false,
            never_cut: false,
        },
        _ if blocking => ObjectFlags::SOLID,
        _ => ObjectFlags {
            blocking: false,
            light_through: true,
            shoot_through: true,
            flat: false,
            no_highlight: false,
            never_cut: false,
        },
    }
}

/// The blocker: stops movement, lets light and shots through, and cannot
/// be highlighted.
const BLOCKER_FLAGS: ObjectFlags = ObjectFlags {
    blocking: true,
    light_through: true,
    shoot_through: true,
    flat: false,
    no_highlight: true,
    never_cut: false,
};

fn material(role: Role) -> Material {
    match role {
        Role::Door | Role::Container => Material::Wood,
        _ => Material::Stone,
    }
}

/// The AI packet for this layer's critters, and a note when it fell back.
fn critter_packet(
    base_files: &BTreeMap<String, Vec<u8>>,
    packet: Option<i32>,
) -> Result<(i32, Option<String>), BuildError> {
    let ai_txt = base_files.get(AI_TXT);
    match (packet, ai_txt) {
        (Some(packet), Some(table)) => {
            if critters::has_ai_packet(table, packet)? {
                Ok((packet, None))
            } else {
                Err(BuildError::Input(format!(
                    "critter AI packet {packet} is not in the base {AI_TXT}; the engine would \
                     use the table's first packet instead"
                )))
            }
        }
        (Some(packet), None) => Ok((packet, None)),
        (None, Some(table)) => Ok((critters::choose_ai_packet(table)?, None)),
        (None, None) => Ok((
            critters::FALLBACK_AI_PACKET,
            Some(format!(
                "no base {AI_TXT}: critters use AI packet {}",
                critters::FALLBACK_AI_PACKET
            )),
        )),
    }
}

/// Prototype message text for a critter's description: printable ASCII as
/// for names, or the name when it is blank.
fn critter_description(description: &str, name: &str) -> String {
    if description.trim().is_empty() {
        name.to_owned()
    } else {
        message_text(description, 512)
    }
}

/// The largest contributors among `labels`, at most ten.
fn largest(labels: impl IntoIterator<Item = String>) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for label in labels {
        *counts.entry(label).or_default() += 1;
    }
    let mut out: Vec<(String, usize)> = counts.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out.truncate(10);
    out
}

/// A square's floor art index: its library tile's, or the blank tile for
/// [`BLANK_SQUARE`].
fn floor_of(tile_index: &[u16], library: u16) -> u16 {
    if library == BLANK_SQUARE {
        BLANK_TILE
    } else {
        tile_index[usize::from(library)]
    }
}

fn tile_word(floor: u16) -> TilePair {
    // Roof index 1: not drawn.
    let word = 0x0001_0000 | u32::from(floor & 0x0fff);
    TilePair { stored: word, word }
}

/// An exit grid on `elevation` (its list slot) leading to `map` at hex
/// `to`, elevation `to_elevation`, facing `facing`.
#[allow(clippy::too_many_arguments)]
fn exit_object(
    id: u32,
    fid: Fid,
    hex: i32,
    elevation: u8,
    flags: BasicFlags,
    map: u32,
    to: i32,
    to_elevation: u8,
    facing: u8,
) -> MapObject {
    let mut object = plain_object(id, EXIT_GRID_PID, fid, hex, elevation, flags);
    object.state = ObjectState::Other {
        word: 0,
        detail: ObjectDetail::ExitGrid {
            map: map as i32,
            hex: to,
            elevation: i32::from(to_elevation),
            orientation: i32::from(facing),
        },
    };
    object
}

/// One elevation's objects read by the engine's movement rule: its
/// components that hold a walkable hex, and where the entrance (`main`),
/// exits, returns and `critters` are in them.
fn component_report(
    objects: &[MapObject],
    walkable: &[bool],
    main_hex: Option<i32>,
    door_exits: &[DoorExit],
    critters: &[PlacedCritter],
) -> ComponentReport {
    let blocked = islands::engine_blocked(objects);
    let comps =
        islands::components(|h| !blocked[h as usize]).retain_holding(|h| walkable[h as usize]);
    let leaked = (0..HEXES as i32)
        .filter(|h| comps.of(*h).is_some() && !walkable[*h as usize])
        .count();
    let mut exits: Vec<ExitComponents> = door_exits
        .iter()
        .map(|exit| ExitComponents {
            door: exit.door.clone(),
            door_hex: exit.door_hex,
            grids: exit
                .grid_hexes
                .iter()
                .map(|hex| (*hex, comps.of(*hex)))
                .collect(),
            return_hex: exit.return_hex,
            return_component: comps.of(exit.return_hex),
        })
        .collect();
    exits.sort_by(|a, b| a.door.cmp(&b.door).then(a.door_hex.cmp(&b.door_hex)));
    ComponentReport {
        sizes: comps.sizes().to_vec(),
        first_hexes: (0..comps.len())
            .filter_map(|id| comps.first_hex(id))
            .collect(),
        main: match main_hex {
            Some(hex) => comps.of(hex),
            None => (!comps.is_empty()).then_some(0),
        },
        exits,
        critters: critters
            .iter()
            .map(|critter| CritterComponent {
                reference: critter.reference.clone(),
                hex: critter.hex,
                component: comps.around(critter.hex),
            })
            .collect(),
        leaked,
    }
}

fn header_name(stem: &str) -> Result<String, BuildError> {
    let name = format!("{}.MAP", stem.to_ascii_uppercase());
    if name.len() > MAP_NAME_BYTES {
        return Err(BuildError::Input(format!("map name {name} is too long")));
    }
    Ok(name)
}

/// The single-map adapter (`DESIGN-levels.md` (f)).
pub(crate) fn build(input: &BuildInput) -> Result<Built, BuildError> {
    let mut options = input.options.clone();
    let interior = options.interior.take();
    let layer_options = LayerOptions {
        standalone: options.standalone,
        write_maps_txt: options.write_maps_txt,
        critter_ai_packet: options.critter_ai_packet,
        critter_team: options.critter_team,
        isolated: interior.is_some(),
        link_doors: false,
        catalogue: options.catalogue.clone(),
        close_unreached: options.close_unreached,
    };
    let layer = LayerInput {
        layer_id: input.layer_id,
        layer_name: input.layer_name,
        base_files: input.base_files,
        critter_art: input.critter_art,
        maps: vec![MapInput {
            map_name: input.map_name,
            map_number: input.map_number,
            window: input.window,
            projection: input.projection,
            elevations: vec![ElevationInput {
                sampler: input.sampler,
                ground: input.ground,
                art: input.art,
                placements: input.placements,
                blocked_hexes: input.blocked_hexes,
                interior,
                level: options.level.take(),
            }],
            entrance_elevation: None,
            cells: input.cells,
            critters: input.critters,
            options,
        }],
        stairs: Vec::new(),
        options: layer_options,
    };
    let built = build_layer(&layer)?;
    let mut map = built
        .maps
        .into_iter()
        .next()
        .ok_or_else(|| BuildError::Input("the layer holds no map".into()))?;
    Ok(Built {
        layer: built.layer,
        walkable: map.walkable.swap_remove(0),
        report: map.report,
        objects: map.objects,
        loot: built.loot,
        furniture: map.furniture,
    })
}

/// A stub map planned for a door leading outside the build.
struct Stub {
    owner: usize,
    elevation: usize,
    door: String,
    /// Indices into the owner elevation's `door_exits`.
    exits: Vec<usize>,
    stem: String,
    number: u32,
}

pub(crate) fn build_layer(input: &LayerInput) -> Result<BuiltLayer, BuildError> {
    if input.maps.is_empty() {
        return Err(BuildError::Input("a layer needs at least one map".into()));
    }
    let single = input.maps.len() == 1 && input.maps[0].elevations.len() == 1;
    let several_maps = input.maps.len() > 1;
    let options = &input.options;
    plan::validate_base(input)?;
    for map in &input.maps {
        plan::validate_map(map, input, !single)?;
    }
    crate::links::validate_stairs(&input.stairs, &input.maps)?;
    if options.isolated && !input.stairs.is_empty() {
        return Err(BuildError::Input(
            "an isolated layer holds no exit grid, so it takes no stairs".into(),
        ));
    }
    let mut stems = Vec::new();
    for map in &input.maps {
        let stem = map_stem(map.map_name)?;
        if let Some(other) = stems.iter().position(|s| *s == stem) {
            return Err(BuildError::Input(format!(
                "maps {} and {} share the file stem {stem}",
                input.maps[other].map_name, map.map_name
            )));
        }
        stems.push(stem);
    }
    let mut builder = LayerBuilder::new(
        input.layer_id,
        input.layer_name,
        input.base_files,
        options.standalone,
        options.write_maps_txt,
    )?;

    // Map numbers: given ones honoured, the rest the next free ones.
    let first_number = builder.first_number;
    let mut used: BTreeSet<u32> = BTreeSet::new();
    for map in &input.maps {
        if let Some(number) = map.map_number {
            if number < first_number {
                return Err(BuildError::Input(format!(
                    "map number {number} is below {first_number}, the first one a layer may \
                     register here"
                )));
            }
            if !used.insert(number) {
                return Err(BuildError::Input(format!(
                    "map number {number} is given to two maps"
                )));
            }
        }
    }
    let mut next = first_number;
    let mut numbers = Vec::new();
    for map in &input.maps {
        numbers.push(match map.map_number {
            Some(number) => number,
            None => {
                while used.contains(&next) {
                    next = next
                        .checked_add(1)
                        .ok_or_else(|| BuildError::Input("map numbers overflow".into()))?;
                }
                used.insert(next);
                next
            }
        });
    }

    // Phase A.
    let mode = if options.isolated {
        DoorMode::Isolated
    } else if options.link_doors {
        DoorMode::Linked
    } else {
        DoorMode::Stubs
    };
    let index = CellIndex::new(&input.maps);
    let mut references: BTreeMap<String, mir::Reference> = BTreeMap::new();
    if mode == DoorMode::Linked {
        for map in &input.maps {
            for cell in map.cells {
                for reference in &cell.references {
                    references
                        .entry(reference.form.to_string())
                        .or_insert_with(|| reference.clone());
                }
            }
        }
    }
    let mut plans = Vec::new();
    for (m, map) in input.maps.iter().enumerate() {
        plans.push(plan::plan_map(
            map,
            stems[m].clone(),
            numbers[m],
            mode,
            &index,
            &references,
            single,
        )?);
    }
    // Phase B.
    crate::links::resolve(&mut plans, input, &index)?;
    // Phase C: islands and entrances, then the sprite sets, then critters.
    for plan in &mut plans {
        plan::reconnect(plan).map_err(|e| match e {
            BuildError::NoWalkableHex if !single => BuildError::Input(format!(
                "map {}: no walkable hex is left for the entrance",
                plan.input.map_name
            )),
            other => other,
        })?;
    }
    // Islands, diagnosed before anything is closed (crate notes, "Reach").
    let diagnoses: Vec<Vec<Vec<crate::reach::IslandDiagnosis>>> = plans
        .iter()
        .map(|plan| {
            (0..plan.elevations.len())
                .map(|e| {
                    let main_hex = if e == plan.entrance_elevation {
                        plan.entrance_hex()
                    } else {
                        plan.elevations[e].arrivals.first().map(|(hex, _)| *hex)
                    };
                    crate::reach::diagnose(plan, e, main_hex)
                })
                .collect()
        })
        .collect();
    // What the start cannot reach.
    if options.close_unreached {
        crate::reach::close_unreached(&mut plans);
    }
    // Nothing to pick up on a link's hexes (crate notes, "Link clearance").
    for plan in &mut plans {
        crate::clearance::move_off_links(plan);
        crate::clearance::move_hidden_off_links(plan);
    }
    // The catalogue, checked against every plan before anything is
    // written.
    let mut loot = match &options.catalogue {
        Some(catalogue) => Some(Loot::prepare(catalogue, input, &plans)?),
        None => None,
    };
    let figure_sets: BTreeSet<usize> = loot
        .as_ref()
        .map_or_else(BTreeSet::new, |l| l.figure_sets.clone());
    let mut critter_files: BTreeMap<usize, BTreeMap<String, Vec<u8>>> = BTreeMap::new();
    let wanted_sets = input
        .maps
        .iter()
        .flat_map(|map| map.critters.iter().map(|c| c.art))
        .chain(figure_sets.iter().copied());
    for art in wanted_sets {
        if let std::collections::btree_map::Entry::Vacant(slot) = critter_files.entry(art) {
            slot.insert(critters::checked_art(art, &input.critter_art[art])?);
        }
    }
    for plan in &mut plans {
        plan::place_critters(plan);
        plan::place_hidden_critters(plan);
    }
    let any_spot = plans.iter().any(|p| {
        p.elevations
            .iter()
            .any(|e| !e.spots.is_empty() || !e.hidden_spots.is_empty())
    });
    let used_sets: BTreeSet<usize> = plans
        .iter()
        .flat_map(|p| {
            p.elevations.iter().flat_map(|e| {
                e.spots
                    .iter()
                    .chain(e.hidden_spots.iter())
                    .map(|s| p.input.critters[s.index].art)
            })
        })
        .chain(figure_sets.iter().copied())
        .collect();
    let mut critter_list = None;
    let mut critter_ai = (0, 0);
    let mut packet_note = None;
    if any_spot || !figure_sets.is_empty() {
        let list = match input.base_files.get(CRITTER_LIST_PATH) {
            Some(bytes) => {
                CritterList::from_base(bytes).map_err(|e| base_error(CRITTER_LIST_PATH, e))?
            }
            None => CritterList::new(),
        };
        let requested = list.len() + used_sets.len();
        if requested > FID_INDEX_CAP {
            return Err(BuildError::FidBudget {
                category: "critters",
                cap: FID_INDEX_CAP,
                base: list.len(),
                requested,
                largest: largest(
                    used_sets
                        .iter()
                        .map(|art| input.critter_art[*art].label.clone()),
                ),
            });
        }
        critter_list = Some(list);
        builder.lists.load_critters(input.base_files)?;
        let (packet, note) = critter_packet(input.base_files, options.critter_ai_packet)?;
        packet_note = note;
        critter_ai = (packet, options.critter_team);
    }

    // The FID budget of the whole layer, checked before anything is
    // written.
    let mut unique: BTreeSet<crate::layer::ArtKey> = BTreeSet::new();
    let mut new_labels: BTreeMap<u8, Vec<String>> = BTreeMap::new();
    let mut duplicates = vec![0usize; plans.len()];
    for (m, plan) in plans.iter().enumerate() {
        for elevation in &plan.elevations {
            for image in elevation.input.art {
                if unique.insert(art_key(image)) {
                    new_labels
                        .entry(art_category(image.category))
                        .or_default()
                        .push(if several_maps {
                            plan.stem.clone()
                        } else {
                            image.source.clone()
                        });
                } else {
                    duplicates[m] += 1;
                }
            }
        }
    }
    let tile_pad = builder.tile_pad();
    // An interior's all-transparent library tiles are the blank tile.
    let blank_tiles: Vec<Vec<Vec<bool>>> = plans
        .iter()
        .map(|plan| {
            plan.elevations
                .iter()
                .map(|elevation| {
                    elevation
                        .input
                        .ground
                        .tiles
                        .iter()
                        .map(|tile| {
                            elevation.input.interior.is_some() && tile.iter().all(|p| *p == 0)
                        })
                        .collect()
                })
                .collect()
        })
        .collect();
    let mut seen_tiles: BTreeSet<&[u8]> = BTreeSet::new();
    let mut tile_labels = Vec::new();
    for (m, plan) in plans.iter().enumerate() {
        for (e, elevation) in plan.elevations.iter().enumerate() {
            let mut group = Vec::new();
            for (tile, blank) in elevation.input.ground.tiles.iter().zip(&blank_tiles[m][e]) {
                if *blank {
                    continue;
                }
                if !seen_tiles.contains(tile.as_slice()) {
                    tile_labels.push(if several_maps {
                        format!("{} ground tiles", plan.stem)
                    } else {
                        "ground tiles".to_owned()
                    });
                }
                group.push(tile.as_slice());
            }
            seen_tiles.extend(group);
        }
    }
    tile_labels.extend(std::iter::repeat_n("ground tiles".to_owned(), tile_pad));
    new_labels.entry(ART_TILES).or_default().extend(tile_labels);
    new_labels
        .entry(ART_SCENERY)
        .or_default()
        .push("invisible blocker".into());
    let any_grid = plans.iter().any(|p| {
        p.elevations
            .iter()
            .any(|e| !e.door_exits.is_empty() || !e.links.is_empty())
    });
    if any_grid {
        new_labels
            .entry(ART_MISC)
            .or_default()
            .push("exit grid marker".into());
    }
    if let Some(loot) = &loot {
        builder.lists.load_inventory(input.base_files)?;
        loot.budget(&mut unique, &mut new_labels);
    }
    for (category, labels) in &new_labels {
        let base = builder.lists.art_base[category];
        let requested = base + labels.len();
        if requested > FID_INDEX_CAP {
            return Err(BuildError::FidBudget {
                category: directory(*category),
                cap: FID_INDEX_CAP,
                base,
                requested,
                largest: largest(labels.iter().cloned()),
            });
        }
    }

    // Floor tiles: the reserved positions of an empty list, then each
    // elevation's library in order.
    builder.pad_tiles()?;
    let mut tile_index: Vec<Vec<Vec<u16>>> = Vec::new();
    for (m, plan) in plans.iter().enumerate() {
        let mut per = Vec::new();
        for (e, elevation) in plan.elevations.iter().enumerate() {
            per.push(builder.add_tiles(elevation.input.ground.tiles, &blank_tiles[m][e])?);
        }
        tile_index.push(per);
    }
    // Sprites, one FRM per distinct image of the layer.
    let mut art_fid: Vec<Vec<Vec<Fid>>> = Vec::new();
    for plan in &plans {
        let mut per = Vec::new();
        for elevation in &plan.elevations {
            let mut fids = Vec::with_capacity(elevation.input.art.len());
            for image in elevation.input.art {
                fids.push(builder.add_sprite(image)?);
            }
            per.push(fids);
        }
        art_fid.push(per);
    }
    let blocker_fid = builder.add_marker(ART_SCENERY)?;
    let exit_fid = if any_grid {
        Some(builder.add_marker(ART_MISC)?)
    } else {
        None
    };
    if let Some(loot) = loot.as_mut() {
        loot.write_art(&mut builder, &art_fid)?;
    }
    let mut art_entries: BTreeMap<String, usize> = BTreeMap::new();
    let mut art_lists: BTreeMap<String, ArtListReport> = BTreeMap::new();
    {
        let lists = &builder.lists;
        for category in &lists.touched_art {
            let added = lists.art[category].len() - lists.art_base[category];
            art_entries.insert(directory(*category).into(), added);
        }
        for category in BUILD_LISTS {
            let kept = lists.art_base[&category];
            let len = lists.art[&category].len();
            art_lists.insert(
                directory(category).into(),
                ArtListReport {
                    replaced: crate::budget::replaced(category, options.standalone),
                    base_entries: lists.art_base_entries[&category],
                    kept,
                    added: len - kept,
                    free_before: FID_INDEX_CAP - kept,
                    free_after: FID_INDEX_CAP - len,
                    shipped: lists.touched_art.contains(&category),
                },
            );
        }
    }

    // Critter sprite sets: one list row each, in ascending art index.
    let mut set_art: BTreeMap<usize, (String, Fid)> = BTreeMap::new();
    let mut critter_sets = Vec::new();
    if let Some(list) = critter_list.as_mut() {
        let mut next_stem = 0;
        for art in &used_sets {
            let stem = critters::next_stem(list, &mut next_stem)?;
            let row = list
                .append(&stem)
                .map_err(|e| BuildError::emit(CRITTER_LIST_PATH, e))?;
            let mut files = Vec::new();
            for (suffix, bytes) in &critter_files[art] {
                let path = format!("art/critters/{stem}{suffix}.frm");
                builder
                    .layer
                    .insert(&path, bytes.clone())
                    .map_err(|e| BuildError::emit(path.clone(), e))?;
                files.push(path);
            }
            // The stand and walk FIDs must name the files just written.
            let mut stand = None;
            for animation in [0, 1] {
                let resolved = list
                    .art(row, animation, 0)
                    .map_err(|e| BuildError::emit(CRITTER_LIST_PATH, e))?;
                if !files.contains(&resolved.path) {
                    return Err(BuildError::Input(format!(
                        "critter row {row} animation {animation} resolves to {}, which the \
                         set does not hold",
                        resolved.path
                    )));
                }
                stand.get_or_insert(resolved.fid);
            }
            let stand = stand.ok_or_else(|| BuildError::Input("no stand FID".into()))?;
            set_art.insert(*art, (stem.clone(), stand));
            critter_sets.push(CritterSetReport {
                art: *art,
                label: input.critter_art[*art].label.clone(),
                stem,
                row,
                files,
            });
        }
        builder
            .layer
            .insert(CRITTER_LIST_PATH, list.to_bytes())
            .map_err(|e| BuildError::emit(CRITTER_LIST_PATH, e))?;
        art_entries.insert(directory(ART_CRITTERS).into(), used_sets.len());
        let kept = list.len() - used_sets.len();
        art_lists.insert(
            directory(ART_CRITTERS).into(),
            ArtListReport {
                replaced: false,
                base_entries: kept,
                kept,
                added: used_sets.len(),
                free_before: FID_INDEX_CAP - kept,
                free_after: list.remaining(),
                shipped: true,
            },
        );
    }

    // Catalogue items: one prototype per definition, after the sprite sets
    // (armour figures) and before every other item prototype.
    if let Some(loot) = loot.as_mut() {
        let figures: BTreeMap<usize, Fid> = set_art
            .iter()
            .map(|(art, (_, stand))| (*art, *stand))
            .collect();
        loot.write_items(&mut builder, &figures)?;
    }

    // Prototypes: per map, one per distinct (elevation, art, role,
    // blocking), and with a catalogue per distinct extra (a reference's
    // name, a container's capacity, a door's key); catalogue items lying
    // on the map take their definition's prototype instead.
    type ProtoKey = (usize, usize, Role, bool, Option<String>);
    let mut prototypes: Vec<BTreeMap<ProtoKey, (u32, BasicFlags)>> = Vec::new();
    for (m, plan) in plans.iter().enumerate() {
        let mut map_protos = BTreeMap::new();
        for (e, elevation) in plan.elevations.iter().enumerate() {
            for placement in elevation
                .placements
                .iter()
                .chain(elevation.hidden_placements.iter())
            {
                if placement.role == Role::Item
                    && loot
                        .as_ref()
                        .is_some_and(|l| l.is_item(&placement.reference))
                {
                    continue;
                }
                let blocking = effective_blocking(placement.role, placement.blocking);
                let extra = loot
                    .as_ref()
                    .and_then(|l| l.extra(&placement.reference, placement.role));
                let key = (
                    e,
                    placement.art,
                    placement.role,
                    blocking,
                    extra.as_ref().map(|x| x.label.clone()),
                );
                if map_protos.contains_key(&key) {
                    continue;
                }
                let role = placement.role;
                let fid = art_fid[m][e][placement.art];
                let flags = object_flags(role, blocking);
                let source = message_text(&elevation.input.art[placement.art].source, 48);
                let name = format!("{} ({source})", role.label());
                let description = format!("Converted from {source}.");
                let common = |pid, message_base| ProtoCommon {
                    pid,
                    fid,
                    message_base,
                    flags,
                    material: material(role),
                };
                let capacity = extra
                    .as_ref()
                    .and_then(|x| x.capacity)
                    .unwrap_or(plan.input.options.container_capacity);
                let door = DoorSpec {
                    key: extra.as_ref().and_then(|x| x.key).unwrap_or(-1),
                    ..DoorSpec::default()
                };
                let make = |common| match role {
                    Role::WallStrip => proto::wall(common),
                    Role::Scenery | Role::FloorPiece => proto::scenery(common),
                    Role::Door => proto::door(common, door),
                    Role::Container => {
                        proto::container(common, ItemStats::default(), capacity, false)
                    }
                    Role::Item => proto::misc_item(common, ItemStats::default()),
                };
                let prototype = match extra.as_ref().and_then(|x| x.text.as_ref()) {
                    Some(text) => {
                        builder
                            .lists
                            .add_described(
                                &mut builder.layer,
                                role_category(role),
                                common,
                                make,
                                text,
                            )?
                            .0
                    }
                    None => builder.lists.add_prototype(
                        &mut builder.layer,
                        role_category(role),
                        common,
                        make,
                        &name,
                        &description,
                    )?,
                };
                map_protos.insert(key, (prototype.pid, prototype.flags));
                if let Some(loot) = loot.as_mut() {
                    loot.report.named_prototypes += usize::from(extra.is_some());
                    loot.set.insert(prototype);
                }
            }
        }
        prototypes.push(map_protos);
    }
    let blocker = builder.lists.add_prototype(
        &mut builder.layer,
        Category::Scenery,
        |pid, message_base| ProtoCommon {
            pid,
            fid: blocker_fid,
            message_base,
            flags: BLOCKER_FLAGS,
            material: Material::Stone,
        },
        proto::scenery,
        "Impassable ground",
        "You cannot go that way.",
    )?;
    let exit_flags = ObjectFlags::DECAL.bits();
    if let Some(exit_fid) = exit_fid {
        // Exit grids are misc PIDs 16 to 23 by number: a base list that
        // already reaches position 16 keeps its own; a shorter one is
        // padded with placeholder misc prototypes up to it.
        let misc_len = builder.lists.protos[&Category::Misc].len();
        if misc_len < 16 {
            let grid = proto::exit_grid(0, exit_fid, proto::NO_MESSAGE)
                .map_err(|e| BuildError::emit("proto/misc", e))?;
            for _ in misc_len..15 {
                let template = grid.clone();
                builder
                    .lists
                    .add_raw_prototype(&mut builder.layer, Category::Misc, |pid| {
                        Ok(Prototype { pid, ..template })
                    })?;
            }
            let placed =
                builder
                    .lists
                    .add_raw_prototype(&mut builder.layer, Category::Misc, |_| Ok(grid))?;
            if placed.pid != EXIT_GRID_PID {
                return Err(BuildError::Input(format!(
                    "the exit-grid prototype landed at PID {:#010x}",
                    placed.pid
                )));
            }
        }
    }

    // Critter prototypes: one per distinct character, in placement order
    // (maps, elevations, spots); with a catalogue, per distinct fighter
    // spec, AI packet and team too.
    type CritterKey = (usize, String, String, i32, bool, Option<String>);
    let mut critter_protos: BTreeMap<CritterKey, Prototype> = BTreeMap::new();
    let mut critter_prototypes = Vec::new();
    let mut spot_keys: Vec<Vec<Vec<CritterKey>>> = Vec::new();
    let ai_table = input.base_files.get(AI_TXT).map(Vec::as_slice);
    for plan in &plans {
        let mut per = Vec::new();
        for elevation in &plan.elevations {
            let mut keys = Vec::with_capacity(elevation.spots.len());
            for spot in elevation.spots.iter().chain(elevation.hidden_spots.iter()) {
                let critter = &plan.input.critters[spot.index];
                let name = message_text(&critter.name, 128);
                let description = critter_description(&critter.description, &name);
                let kit = match &loot {
                    Some(loot) if !critter.hidden => {
                        loot.critter_proto(critter, critter_ai, ai_table)?
                    }
                    _ => None,
                };
                let key = (
                    critter.art,
                    name.clone(),
                    description.clone(),
                    critter.hit_points,
                    critter.female,
                    kit.as_ref().map(|(label, _)| label.clone()),
                );
                keys.push(key.clone());
                if critter_protos.contains_key(&key) {
                    continue;
                }
                let (stem, stand) = set_art[&critter.art].clone();
                let mut spec = CritterSpec::human(critter.hit_points, critter_ai.0, critter_ai.1);
                spec.female = critter.female;
                let (prototype, file) = builder.lists.add_prototype_file(
                    &mut builder.layer,
                    Category::Critter,
                    |pid, message_base| ProtoCommon {
                        pid,
                        fid: stand,
                        message_base,
                        flags: ObjectFlags::SOLID,
                        material: Material::default(),
                    },
                    |common| match &kit {
                        Some((_, kit)) => kit.make(common),
                        None => critter_prototype(common, &spec),
                    },
                    &name,
                    &description,
                )?;
                critter_prototypes.push(CritterPrototypeReport {
                    pid: prototype.pid,
                    file,
                    stem,
                    name,
                    description,
                    hit_points: kit
                        .as_ref()
                        .map_or(critter.hit_points, |(_, kit)| kit.hit_points()),
                    female: kit.as_ref().map_or(critter.female, |(_, kit)| kit.female()),
                });
                if let Some(loot) = loot.as_mut() {
                    loot.report.critter_prototypes += usize::from(kit.is_some());
                    loot.set.insert(prototype.clone());
                }
                critter_protos.insert(key, prototype);
            }
            per.push(keys);
        }
        spot_keys.push(per);
    }

    // Stub maps: per owning map, one per door (sorted by form key),
    // numbered after every real map.
    let mut stubs: Vec<Stub> = Vec::new();
    let mut stub_number = numbers.iter().copied().max().unwrap_or(first_number);
    for (m, plan) in plans.iter().enumerate() {
        let mut doors: BTreeMap<String, (usize, Vec<usize>)> = BTreeMap::new();
        for (e, elevation) in plan.elevations.iter().enumerate() {
            for (i, exit) in elevation.door_exits.iter().enumerate() {
                let entry = doors.entry(exit.door.clone()).or_insert((e, Vec::new()));
                if entry.0 == e {
                    entry.1.push(i);
                }
            }
        }
        for (index, (door, (e, exits))) in doors.into_iter().enumerate() {
            stub_number = stub_number
                .checked_add(1)
                .ok_or_else(|| BuildError::Input("map numbers overflow".into()))?;
            stubs.push(Stub {
                owner: m,
                elevation: e,
                door,
                exits,
                stem: stub_stem(&plan.stem, index)?,
                number: stub_number,
            });
        }
    }
    {
        let mut all: BTreeMap<&str, &str> = BTreeMap::new();
        for plan in &plans {
            all.insert(&plan.stem, plan.input.map_name);
        }
        for stub in &stubs {
            let owner = plans[stub.owner].input.map_name;
            if let Some(other) = all.insert(&stub.stem, owner) {
                return Err(BuildError::Input(format!(
                    "the stub map {} of map {owner} collides with a map of {other}; rename one \
                     of them (stub stems are a map's first four letters)",
                    stub.stem
                )));
            }
        }
    }
    let stub_of = |m: usize, door: &str| {
        stubs
            .iter()
            .find(|s| s.owner == m && s.door == door)
            .map(|s| s.number)
    };

    // The maps' objects and files.
    let reach = crate::reach::planned_reach(&plans);
    let mut built_maps: Vec<BuiltMap> = Vec::new();
    for (m, plan) in plans.iter().enumerate() {
        let mut report = BuildReport {
            interactive: plan.interactive,
            art_duplicates: duplicates[m],
            notes: plan.notes.clone(),
            first_map_number: first_number,
            ..BuildReport::default()
        };
        let placed_any = plan
            .elevations
            .iter()
            .any(|e| !e.spots.is_empty() || !e.hidden_spots.is_empty());
        if placed_any {
            report.notes.extend(packet_note.clone());
            report.critter_ai_packet = Some(critter_ai.0);
            report.critter_team = Some(critter_ai.1);
        }
        let mut next_id = 1u32;
        let mut take_id = || {
            let id = next_id;
            next_id += 1;
            id
        };
        let mut counts = MapLoot {
            file: format!("maps/{}.map", plan.stem),
            ..MapLoot::default()
        };
        let mut per_objects: Vec<Vec<MapObject>> = Vec::new();
        let mut object_refs: Vec<ObjectRef> = Vec::new();
        let mut hidden_objects: Vec<(String, u8, i32)> = Vec::new();
        let mut hidden_critters: Vec<HiddenCritter> = Vec::new();
        let mut placed_critters: Vec<(u8, PlacedCritter)> = Vec::new();
        let mut elevation_critters: Vec<Vec<PlacedCritter>> = Vec::new();
        for (e, elevation) in plan.elevations.iter().enumerate() {
            let level = e as u8;
            let mut objects: Vec<MapObject> = Vec::new();
            // The visible placements, then the hidden ones.
            for (placement, hidden) in elevation
                .placements
                .iter()
                .map(|p| (p, false))
                .chain(elevation.hidden_placements.iter().map(|p| (p, true)))
            {
                let fid = art_fid[m][e][placement.art];
                let first = objects.len();
                if let Some(loot) = loot.as_mut()
                    && placement.role == Role::Item
                    && loot.is_item(&placement.reference)
                {
                    let placed =
                        loot.objects(placement, None, fid, level, &mut take_id, &mut counts)?;
                    objects.extend(placed.unwrap_or_default());
                    note_objects(
                        &mut objects,
                        first,
                        placement,
                        level,
                        hidden,
                        &mut object_refs,
                        &mut hidden_objects,
                    );
                    continue;
                }
                let blocking = effective_blocking(placement.role, placement.blocking);
                let extra = loot
                    .as_ref()
                    .and_then(|l| l.extra(&placement.reference, placement.role));
                let (pid, flags) = prototypes[m][&(
                    e,
                    placement.art,
                    placement.role,
                    blocking,
                    extra.as_ref().map(|x| x.label.clone()),
                )];
                if let Some(loot) = loot.as_mut() {
                    counts.named_placements +=
                        usize::from(extra.as_ref().is_some_and(|x| x.text.is_some()));
                    if let Some(placed) = loot.objects(
                        placement,
                        Some((pid, fid)),
                        fid,
                        level,
                        &mut take_id,
                        &mut counts,
                    )? {
                        objects.extend(placed);
                        note_objects(
                            &mut objects,
                            first,
                            placement,
                            level,
                            hidden,
                            &mut object_refs,
                            &mut hidden_objects,
                        );
                        continue;
                    }
                }
                let mut object = plain_object(
                    take_id(),
                    pid,
                    art_fid[m][e][placement.art],
                    placement.hex,
                    level,
                    flags,
                );
                object.displacement = placement.displacement;
                object.state = ObjectState::Other {
                    word: 0,
                    detail: match placement.role {
                        Role::Door => ObjectDetail::Door(LockFlags(0)),
                        Role::Item => ObjectDetail::Charges(0),
                        _ => ObjectDetail::Plain,
                    },
                };
                objects.push(object);
                note_objects(
                    &mut objects,
                    first,
                    placement,
                    level,
                    hidden,
                    &mut object_refs,
                    &mut hidden_objects,
                );
            }
            for hex in &elevation.border {
                objects.push(plain_object(
                    take_id(),
                    blocker.pid,
                    blocker_fid,
                    *hex,
                    level,
                    blocker.flags,
                ));
            }
            let mut placed_grids = BTreeSet::new();
            if let Some(exit_fid) = exit_fid {
                for exit in &elevation.door_exits {
                    let number = stub_of(m, &exit.door)
                        .ok_or_else(|| BuildError::Input("an exit without its stub".into()))?;
                    for hex in &exit.grid_hexes {
                        if placed_grids.insert(*hex) {
                            objects.push(exit_object(
                                take_id(),
                                exit_fid,
                                *hex,
                                level,
                                exit_flags,
                                number,
                                STUB_ENTRY,
                                0,
                                0,
                            ));
                        }
                    }
                }
                for link in &elevation.links {
                    let (dm, de, to, facing) = link.to;
                    for hex in &link.grid {
                        if placed_grids.insert(*hex) {
                            objects.push(exit_object(
                                take_id(),
                                exit_fid,
                                *hex,
                                level,
                                exit_flags,
                                plans[dm].number,
                                to,
                                de,
                                facing,
                            ));
                        }
                    }
                }
            }
            let mut here = Vec::new();
            for (spot, key) in elevation
                .spots
                .iter()
                .chain(elevation.hidden_spots.iter())
                .zip(&spot_keys[m][e])
            {
                let critter = &plan.input.critters[spot.index];
                let prototype = &critter_protos[key];
                let record = CritterRecord::from_prototype(
                    prototype,
                    take_id(),
                    spot.hex,
                    level,
                    spot.orientation,
                )
                .map_err(|e| BuildError::emit(format!("critter {}", critter.reference), e))?;
                let kitted = match loot.as_mut() {
                    Some(loot) if !critter.hidden => loot.critter_object(
                        &record,
                        &critter.reference,
                        &critter_files[&critter.art],
                        &mut take_id,
                        &mut counts,
                    )?,
                    _ => None,
                };
                let mut object = match kitted {
                    Some(object) => object,
                    None => critter_object(&record).map_err(|e| {
                        BuildError::emit(format!("critter {}", critter.reference), e)
                    })?,
                };
                if critter.hidden {
                    object.flags = BasicFlags(object.flags.0 | BasicFlags::HIDDEN.0);
                }
                object_refs.push(ObjectRef {
                    reference: critter.reference.clone(),
                    elevation: level,
                    id: object.id,
                    hex: spot.hex,
                    pid: object.pid,
                    hidden: critter.hidden,
                });
                objects.push(object);
                let placed = PlacedCritter {
                    reference: critter.reference.clone(),
                    index: spot.index,
                    hex: spot.hex,
                    orientation: spot.orientation,
                    steps: spot.steps,
                    moved: spot.moved,
                    pid: prototype.pid,
                    stem: set_art[&critter.art].0.clone(),
                };
                if critter.hidden {
                    hidden_critters.push(HiddenCritter {
                        elevation: level,
                        placed,
                        presence_of: critter.presence_of.clone(),
                    });
                    continue;
                }
                here.push(placed.clone());
                placed_critters.push((level, placed));
            }
            elevation_critters.push(here);
            per_objects.push(objects);
        }
        let total: usize = per_objects.iter().map(Vec::len).sum();
        if total > MAX_OBJECTS {
            return Err(BuildError::Input(format!(
                "the map needs {total} objects; the format holds {MAX_OBJECTS}"
            )));
        }
        if let Some(loot) = loot.as_mut() {
            let records: usize = per_objects
                .iter()
                .map(|objects| crate::loot_build::inventory_records(objects))
                .sum();
            if total + records > MAX_OBJECTS {
                return Err(BuildError::Input(format!(
                    "map {}: {total} objects and {records} inventory records; the format reads \
                     at most {MAX_OBJECTS} records in all",
                    plan.input.map_name
                )));
            }
            counts.inventory_records = records;
            loot.report.maps.push(counts);
        }
        report.objects = total;
        placed_critters.sort_by_key(|(_, c)| c.index);
        report.critters = placed_critters.iter().map(|(_, c)| c.clone()).collect();
        hidden_critters.sort_by_key(|c| c.placed.index);
        report.hidden_critters = hidden_critters;
        report.hidden_objects = hidden_objects;
        let critter_elevations: Vec<u8> = placed_critters.iter().map(|(e, _)| *e).collect();
        let mut skipped_critters: Vec<_> = plan
            .elevations
            .iter()
            .flat_map(|e| e.skipped_critters.iter().cloned())
            .collect();
        skipped_critters.sort_by_key(|s| s.index);
        report.skipped_critters = skipped_critters;
        report.skipped_teleports = plan
            .elevations
            .iter()
            .flat_map(|e| e.skipped.iter().cloned())
            .collect();
        let entry = plan.entrance_hex().ok_or(BuildError::NoWalkableHex)?;
        let mut elevations = Vec::new();
        for (e, elevation) in plan.elevations.iter().enumerate() {
            let main_hex = if e == plan.entrance_elevation {
                Some(entry)
            } else {
                elevation.arrivals.first().map(|(hex, _)| *hex)
            };
            let components = component_report(
                &per_objects[e],
                &elevation.walkable,
                main_hex,
                &elevation.door_exits,
                &elevation_critters[e],
            );
            if components.leaked > 0 {
                report.notes.push(format!(
                    "{}{} hexes that are not walkable can be walked onto from walkable ones: the \
                     sealing border leaks",
                    if plan.elevations.len() > 1 && e > 0 {
                        format!("elevation {e}: ")
                    } else {
                        String::new()
                    },
                    components.leaked
                ));
            }
            if elevation.moved_off_links > 0 {
                report.notes.push(format!(
                    "{}moved off link hexes: {}",
                    if plan.elevations.len() > 1 && e > 0 {
                        format!("elevation {e}: ")
                    } else {
                        String::new()
                    },
                    elevation.moved_off_links
                ));
            }
            let readability = crate::readability::measure(
                &elevation.verdicts,
                &elevation.walkable,
                &elevation.placement_blocks,
                &elevation.cover,
                elevation.options.entrance_box,
                elevation.options.readability_sight,
            );
            let blocked = islands::engine_blocked(&per_objects[e]);
            let comps = islands::components(|h| !blocked[h as usize])
                .retain_holding(|h| elevation.walkable[h as usize]);
            elevations.push(ElevationReport {
                elevation: e as u8,
                band: elevation.options.interior.as_ref().map(|i| i.band()),
                walk: elevation.stats.clone(),
                entrance: elevation.entrance,
                components,
                passages: elevation.passages.clone(),
                islands_left: elevation.islands_left.clone(),
                opened_hexes: elevation.opened.len(),
                blockers: elevation.border.len(),
                objects: per_objects[e].len(),
                readability,
                arrivals: elevation
                    .arrivals
                    .iter()
                    .map(|(hex, what)| (what.clone(), *hex, comps.of(*hex)))
                    .collect(),
                critters: elevation_critters[e]
                    .iter()
                    .map(|c| c.reference.clone())
                    .collect(),
                islands: {
                    let mut islands = diagnoses[m][e].clone();
                    if let Some(r) = &reach {
                        for island in &mut islands {
                            island.reached = Some(r[m][e][island.first_hex as usize]);
                        }
                    }
                    islands
                },
                reached: reach.as_ref().map(|r| {
                    (
                        (0..HEXES)
                            .filter(|h| elevation.walkable[*h] && r[m][e][*h])
                            .count(),
                        elevation.walkable.iter().filter(|w| **w).count(),
                    )
                }),
                closed: elevation.closed.clone(),
                perches: elevation.perches.clone(),
                lost_to_closing: elevation.placed_before_closing.as_ref().map(|placed| {
                    let mut lost: Vec<String> = placed
                        .iter()
                        .filter(|i| elevation.spots.iter().all(|s| s.index != **i))
                        .map(|i| {
                            let c = &plan.input.critters[*i];
                            if c.name.is_empty() {
                                c.reference.clone()
                            } else {
                                c.name.clone()
                            }
                        })
                        .collect();
                    lost.sort();
                    lost
                }),
                moved_off_links: elevation.moved_off_links,
            });
        }
        // Furniture: the footprints used, and upright art walkers can stand
        // inside.
        let footprints = &plan.input.options.footprints;
        let mut furniture = FurnitureReport {
            footprints_given: footprints.len(),
            ..FurnitureReport::default()
        };
        let mut used: BTreeSet<&str> = BTreeSet::new();
        let mut empty: BTreeSet<&str> = BTreeSet::new();
        for (e, elevation) in plan.elevations.iter().enumerate() {
            used.extend(elevation.footprints.used.iter().map(String::as_str));
            empty.extend(elevation.footprints.empty.iter().map(String::as_str));
            furniture.blocked_hexes += elevation.footprint_blocked;
            furniture.suspects.extend(crate::furniture::suspects(
                e as u8,
                elevation.input.art,
                &elevation.placements,
                &elevation.walkable,
                &elevation.cover,
                footprints,
            ));
        }
        furniture.footprints_used = used.len();
        furniture.empty_footprints = empty.into_iter().map(str::to_owned).collect();
        let main = &elevations[plan.entrance_elevation];
        report.entrance = main.entrance.unwrap_or_default();
        report.entry_hex = entry;
        report.walk = main.walk.clone();
        report.components = main.components.clone();
        report.passages = main.passages.clone();
        report.islands_left = main.islands_left.clone();
        report.reach = main.reached;
        report.closed = main.closed.clone();
        report.perches = main.perches.clone();
        report.lost_to_closing = main.lost_to_closing.clone();
        report.opened_hexes = main.opened_hexes;
        report.blockers = main.blockers;
        report.readability = main.readability;

        // The map file.
        let map_file = format!("{}.map", plan.stem);
        let mut map = empty_map(&header_name(&plan.stem)?, plan.number as i32, entry)
            .map_err(|e| BuildError::emit(map_file.clone(), e))?;
        map.header.entry_elevation = plan.entrance_elevation as u8;
        for (e, elevation) in plan.elevations.iter().enumerate() {
            map.tiles[e] = Some(
                elevation
                    .input
                    .ground
                    .squares
                    .iter()
                    .map(|library| tile_word(floor_of(&tile_index[m][e], *library)))
                    .collect(),
            );
        }
        map.objects.total = total;
        for (e, objects) in per_objects.into_iter().enumerate() {
            map.objects.elevations[e] = objects;
        }
        builder.add_map(
            &map_file,
            &map,
            plan.number,
            plan.entrance_elevation as u8,
            entry,
        )?;
        report.map_file = format!("maps/{map_file}");
        report.map_number = plan.number;
        built_maps.push(BuiltMap {
            file: format!("maps/{map_file}"),
            number: plan.number,
            entrance_elevation: plan.entrance_elevation as u8,
            walkable: plan.elevations.iter().map(|e| e.walkable.clone()).collect(),
            elevations,
            links: Vec::new(),
            critter_elevations,
            objects: object_refs,
            report,
            furniture,
        });
    }

    // Stub maps.
    for stub in &stubs {
        let plan = &plans[stub.owner];
        let elevation = &plan.elevations[stub.elevation];
        let first: &DoorExit = &elevation.door_exits[stub.exits[0]];
        let exit_fid =
            exit_fid.ok_or_else(|| BuildError::Input("an exit without exit-grid art".into()))?;
        let floor = Frame::square_of_hex(first.door_hex)
            .and_then(|square| elevation.input.ground.squares.get(square as usize))
            .map_or(1, |library| {
                floor_of(&tile_index[stub.owner][stub.elevation], *library)
            });
        let mut tiles = vec![TilePair::BLANK; 10_000];
        for hex in crate::exits::hexes_around(STUB_ENTRY, STUB_FLOOR_RADIUS, false) {
            if let Some(square) = Frame::square_of_hex(hex) {
                tiles[square as usize] = tile_word(floor);
            }
        }
        let mut objects = Vec::new();
        for (id, hex) in crate::exits::hexes_around(STUB_ENTRY, STUB_EXIT_RADIUS, true)
            .into_iter()
            .enumerate()
        {
            objects.push(exit_object(
                id as u32 + 1,
                exit_fid,
                hex,
                0,
                exit_flags,
                plan.number,
                first.return_hex,
                stub.elevation as u8,
                first.return_orientation,
            ));
        }
        let file = format!("{}.map", stub.stem);
        let mut map = empty_map(&header_name(&stub.stem)?, stub.number as i32, STUB_ENTRY)
            .map_err(|e| BuildError::emit(file.clone(), e))?;
        map.tiles[0] = Some(tiles);
        map.objects.total = objects.len();
        map.objects.elevations[0] = objects;
        builder.add_map(&file, &map, stub.number, 0, STUB_ENTRY)?;
        let report = &mut built_maps[stub.owner].report;
        for index in &stub.exits {
            let exit = &elevation.door_exits[*index];
            report.exits.push(ExitReport {
                door: exit.door.clone(),
                door_hex: exit.door_hex,
                interior: exit.interior.clone(),
                interior_editor_id: exit.interior_editor_id.clone(),
                stub_file: file.clone(),
                stub_number: stub.number,
                grid_hexes: exit.grid_hexes.clone(),
                grid_steps: exit.grid_steps,
                return_hex: exit.return_hex,
                return_orientation: exit.return_orientation,
                return_from_teleport: exit.return_from_teleport,
            });
        }
    }
    for map in &mut built_maps {
        map.report
            .exits
            .sort_by(|a, b| a.door.cmp(&b.door).then(a.door_hex.cmp(&b.door_hex)));
    }

    // Links, with whether each has a return.
    let mut expected_one_way: BTreeSet<String> = BTreeSet::new();
    for (m, plan) in plans.iter().enumerate() {
        for (e, elevation) in plan.elevations.iter().enumerate() {
            for link in &elevation.links {
                let (dm, de, to, facing) = link.to;
                let returns = plans[dm].elevations[usize::from(de)]
                    .links
                    .iter()
                    .any(|back| back.to.0 == m && usize::from(back.to.1) == e);
                if !returns {
                    expected_one_way.insert(crate::verify::pair_label(
                        &format!("{}.map", plan.stem),
                        e as u8,
                        &format!("{}.map", plans[dm].stem),
                        de,
                    ));
                    built_maps[m].report.notes.push(format!(
                        "{} {} leads to {} elevation {de}, but nothing there leads back",
                        match link.kind {
                            crate::LinkKind::Door => "door",
                            crate::LinkKind::Stairs => "stairs",
                        },
                        link.from,
                        plans[dm].stem
                    ));
                }
                built_maps[m].links.push(LinkReport {
                    kind: link.kind,
                    from: link.from.clone(),
                    elevation: e as u8,
                    door_hex: link.door_hex,
                    grid_hexes: link.grid.clone(),
                    grid_steps: link.steps,
                    to_file: format!("maps/{}.map", plans[dm].stem),
                    to_number: plans[dm].number,
                    to_elevation: de,
                    to_hex: to,
                    to_orientation: facing,
                    returns,
                });
            }
        }
    }

    // Registration, and the map table Fallout 2 transitions read.
    let (maps_txt, maps_note) = builder.register_maps()?;
    let maps = builder.maps();
    builder.lists.finish(&mut builder.layer)?;
    let mut prototype_counts: BTreeMap<String, usize> = BTreeMap::new();
    for category in &builder.lists.touched_protos {
        let base = input
            .base_files
            .get(&proto::prototype_list_path(*category))
            .map_or(Ok(0), |bytes| {
                ProtoList::from_base(*category, bytes).map(|l| l.len())
            })
            .map_err(|e| base_error(&proto::prototype_list_path(*category), e))?;
        prototype_counts.insert(
            category.directory().into(),
            builder.lists.protos[category].len() - base,
        );
    }

    let verify = crate::verify_layer(&builder.layer, input.base_files)?;
    if options.isolated {
        let wanted = input.maps.len();
        let grids: usize = verify.maps.iter().map(|map| map.exit_grids).sum();
        if verify.maps.len() != wanted || grids > 0 {
            let file = built_maps[0].report.map_file.clone();
            return Err(BuildError::verify(
                file,
                if single {
                    format!(
                        "an interior demo is one map without exit grids; the layer holds {} \
                         maps and {grids} exit grids",
                        verify.maps.len()
                    )
                } else {
                    format!(
                        "an isolated layer holds its {wanted} maps and no exit grid; it holds {} \
                         maps and {grids} exit grids",
                        verify.maps.len()
                    )
                },
            ));
        }
    }
    let links = crate::verify_links(&builder.layer, input.base_files)?;
    crate::verify_clearance(&builder.layer, input.base_files, &built_maps)?;
    if let Some(loot) = loot.as_mut() {
        let check = crate::verify_loot(&builder.layer, input.base_files)?;
        if let Some(item) = loot
            .report
            .items
            .iter()
            .find(|item| check.no_inventory_art.contains(&item.pid))
        {
            return Err(BuildError::verify(
                item.file.clone(),
                format!("item {} has no inventory art", item.key),
            ));
        }
        loot.report.skipped.sort();
        loot.report.check = check;
    }
    let found: BTreeSet<String> = links.one_way.iter().cloned().collect();
    if found != expected_one_way {
        return Err(BuildError::verify(
            "links",
            format!(
                "one-way links written {:?}, planned {:?}",
                found, expected_one_way
            ),
        ));
    }

    // Layer-wide fields, and each interior map's own account.
    for (m, built) in built_maps.iter_mut().enumerate() {
        let plan = &plans[m];
        let report = &mut built.report;
        report.maps = maps.clone();
        report.maps_txt = maps_txt;
        report.notes.extend(maps_note.clone());
        report.art_entries = art_entries.clone();
        report.art_lists = art_lists.clone();
        report.prototypes = prototype_counts.clone();
        report.critter_sets = critter_sets.clone();
        report.critter_prototypes = critter_prototypes.clone();
        report.verify = verify.clone();
        let Some(interior) = &plan.elevations[plan.entrance_elevation].options.interior else {
            continue;
        };
        let (near, given) = plan.entrance_near.unwrap_or((plan.window_centre, false));
        let mut account = InteriorReport {
            cells: plan
                .input
                .cells
                .iter()
                .filter(|cell| cell.is_interior())
                .map(|cell| (cell.form.to_string(), cell.editor_id.clone()))
                .collect(),
            floor_z: interior.floor_z,
            band: interior.band(),
            entrance_near: [near[0] as f32, near[1] as f32],
            entrance_near_given: given,
            doors: plan.listed_doors.clone(),
            ..InteriorReport::default()
        };
        let other = |index: usize| match plan.levels.get(index) {
            Some(CritterLevel::Other(z)) => Some(*z),
            _ => None,
        };
        for critter in &report.critters {
            if let Some(z) = other(critter.index) {
                account.clamped.push((critter.reference.clone(), z));
            }
        }
        account.other_level = report
            .skipped_critters
            .iter()
            .filter(|skipped| {
                other(skipped.index).is_some() && skipped.reason.starts_with("on another level")
            })
            .cloned()
            .collect();
        account.unknown_level = plan
            .input
            .critters
            .iter()
            .zip(&plan.levels)
            .filter(|(_, level)| matches!(level, CritterLevel::Unknown))
            .map(|(critter, _)| critter.reference.clone())
            .collect();
        if !account.unknown_level.is_empty() {
            report.notes.push(format!(
                "{} critters name references the interior cells do not hold; their level is \
                 unknown and they were placed as on the floor level",
                account.unknown_level.len()
            ));
        }
        report.interior = Some(account);
    }
    let stub_list = stubs
        .iter()
        .map(|s| (format!("{}.map", s.stem), s.number))
        .collect();
    Ok(BuiltLayer {
        layer: builder.layer,
        maps: built_maps,
        stubs: stub_list,
        verify,
        links,
        loot: loot.map(|l| l.report),
    })
}
